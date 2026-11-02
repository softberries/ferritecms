//! Webhooks on a real node (M6-6 task 5, design 18 §4): a publish is delivered through the
//! runner to a local receiver (`[webhooks] local_targets`) with a valid signature, a
//! failing receiver is retried on the (shortened) schedule, `410` disables the endpoint
//! and mails the administrators; at handler level a full node refunds the attempt
//! (`Busy`) and the prune deletes old events.

use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;

use ferrite::jobs::{WebhookDeliverHandler, WebhooksPruneHandler};
use ferrite_core::{
    mail::MemoryMailer,
    webhooks::{
        OutgoingWebhook, SendOutcome, UrlPolicy, WebhookInput, WebhookService, WebhookTransport,
        secret, sign,
    },
};
use ferrite_domain::{DeliveryQuery, DeliveryStatus, DisabledReason, WebhookId};
use ferrite_http::webhook_send::webhook_ip_rule;
use tokio::net::TcpListener;
use tokio::sync::Notify;

use super::*;

/// One request the receiver read: headers (lowercase names) and body.
#[derive(Debug, Clone)]
pub(crate) struct Hit {
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Hit {
    pub fn header(&self, name: &str) -> &str {
        self.headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
            .unwrap_or_default()
    }
}

/// A plain-HTTP receiver answering the statuses in `answers` in order (then 200).
pub(crate) struct Receiver {
    pub addr: SocketAddr,
    pub hits: Arc<Mutex<Vec<Hit>>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Receiver {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Receiver {
    pub async fn start(answers: Vec<u16>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let hits = Arc::new(Mutex::new(Vec::new()));
        let answers = Arc::new(Mutex::new(answers.into_iter().rev().collect::<Vec<_>>()));
        let h = hits.clone();
        let task = tokio::spawn(async move {
            loop {
                let Ok((mut tcp, _)) = listener.accept().await else {
                    return;
                };
                let (h, answers) = (h.clone(), answers.clone());
                tokio::spawn(async move {
                    let Some(hit) = read(&mut tcp).await else {
                        return;
                    };
                    h.lock().unwrap().push(hit);
                    let status = answers.lock().unwrap().pop().unwrap_or(200);
                    let out = format!(
                        "HTTP/1.1 {status} X\r\nConnection: close\r\nContent-Length: 2\r\n\r\nok"
                    );
                    let _ = tcp.write_all(out.as_bytes()).await;
                });
            }
        });
        Self { addr, hits, task }
    }

    pub fn url(&self) -> String {
        format!("http://{}/hook", self.addr)
    }

    pub fn count(&self) -> usize {
        self.hits.lock().unwrap().len()
    }
}

async fn read(io: &mut tokio::net::TcpStream) -> Option<Hit> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let end = loop {
        let n = io.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i;
        }
    };
    let head = String::from_utf8_lossy(&buf[..end]).into_owned();
    let headers: Vec<(String, String)> = head
        .split("\r\n")
        .skip(1)
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_owned()))
        .collect();
    let len: usize = headers
        .iter()
        .find(|(k, _)| k == "content-length")
        .and_then(|(_, v)| v.parse().ok())
        .unwrap_or(0);
    let mut body = buf[end + 4..].to_vec();
    while body.len() < len {
        let n = io.read(&mut chunk).await.ok()?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    Some(Hit { headers, body })
}

/// The site's config with `[webhooks] local_targets` on.
pub(crate) fn local_config(site: &Site) -> Config {
    let env = vec![
        ("FERRITE_AUTH__SECRET_KEY".to_owned(), "ab".repeat(32)),
        (
            "FERRITE_WEBHOOKS__LOCAL_TARGETS".to_owned(),
            "true".to_owned(),
        ),
    ];
    ferrite_config::load(site.data(), env).unwrap()
}

/// The webhook service of the node's database and site key (as the API's will be).
pub(crate) fn service(store: Arc<dyn Store>) -> WebhookService {
    WebhookService::new(
        store,
        SiteKey::from_hex(&"ab".repeat(32)).unwrap(),
        UrlPolicy {
            local_targets: true,
            allow_ip: webhook_ip_rule(true),
        },
    )
}

pub(crate) async fn hook(svc: &WebhookService, admin: &Actor, url: &str) -> (WebhookId, Secret) {
    let (view, secret) = svc
        .create(
            admin,
            WebhookInput {
                name: "receiver".into(),
                url: url.into(),
                events: vec!["content.*".into()],
                ..WebhookInput::default()
            },
        )
        .await
        .unwrap();
    (view.endpoint.id, secret)
}

pub(crate) async fn publish(store: &Arc<dyn Store>, actor: &Actor, slug: &str) {
    let svc = ContentService::new(store.clone(), Arc::new(PermalinkRules::default()));
    let item = svc
        .create(
            actor,
            ContentDraft {
                kind: "post".into(),
                title: format!("Post {slug}"),
                slug: Some(slug.into()),
                parent_id: None,
                excerpt: None,
                body: json!({"type": "doc", "content": []}),
                fields: json!({}),
                menu_order: 0,
                terms: Vec::new(),
                seo: None,
            },
        )
        .await
        .unwrap();
    svc.publish(actor, item.id, item.version).await.unwrap();
}

use ferrite_domain::Secret;

fn verify(hit: &Hit, secret: &Secret) {
    let key = secret::parse(secret.expose()).unwrap();
    let ts: i64 = hit.header("webhook-timestamp").parse().unwrap();
    let want = sign::signature(&key, hit.header("webhook-id"), ts, &hit.body);
    assert!(
        hit.header("webhook-signature")
            .split(' ')
            .any(|s| s == want),
        "{hit:?}"
    );
}

async fn publish_is_delivered_with_valid_signature(db: Arc<TestDb>) {
    let site = Site::new(&db);
    let config = local_config(&site);
    let node = start(&config, db.shared_store(), NodeOptions::default())
        .await
        .unwrap();
    let store = db.shared_store();
    let user = fixture_user(store.as_ref()).await;
    let admin = Actor::new(user.id, Role::Admin);
    let receiver = Receiver::start(vec![]).await;
    let svc = service(store.clone());
    let (id, secret) = hook(&svc, &admin, &receiver.url()).await;
    publish(&store, &admin, "hello").await;
    // created + published.
    eventually(SETTLE, "two deliveries", || async { receiver.count() >= 2 }).await;
    let hits = receiver.hits.lock().unwrap().clone();
    for hit in &hits {
        verify(hit, &secret);
        assert_eq!(hit.header("content-type"), "application/json");
        assert!(hit.header("user-agent").ends_with(" webhooks"));
    }
    let types: Vec<String> = hits
        .iter()
        .map(|h| {
            serde_json::from_slice::<Value>(&h.body).unwrap()["type"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect();
    assert!(types.contains(&"content.published".to_owned()), "{types:?}");
    let published = hits
        .iter()
        .map(|h| serde_json::from_slice::<Value>(&h.body).unwrap())
        .find(|b| b["type"] == "content.published")
        .unwrap();
    assert_eq!(published["site"]["url"], SITE_URL);
    assert_eq!(
        published["data"]["url"],
        format!("{SITE_URL}{}", published["data"]["path"].as_str().unwrap())
    );
    eventually(SETTLE, "deliveries recorded", || async {
        svc.deliveries(&admin, id, &DeliveryQuery::default())
            .await
            .unwrap()
            .items
            .iter()
            .all(|l| l.delivery.status == DeliveryStatus::Delivered)
    })
    .await;
    node.shutdown().await.unwrap();
}

async fn receiver_down_retries_on_schedule(db: Arc<TestDb>) {
    let site = Site::new(&db);
    let config = local_config(&site);
    let opts = NodeOptions {
        webhook_schedule: Some(vec![Duration::ZERO; 8]),
        ..NodeOptions::default()
    };
    let node = start(&config, db.shared_store(), opts).await.unwrap();
    let store = db.shared_store();
    let user = fixture_user(store.as_ref()).await;
    let admin = Actor::new(user.id, Role::Admin);
    let receiver = Receiver::start(vec![500, 503]).await;
    let svc = service(store.clone());
    let (view, _) = svc
        .create(
            &admin,
            WebhookInput {
                name: "only published".into(),
                url: receiver.url(),
                events: vec!["content.published".into()],
                ..WebhookInput::default()
            },
        )
        .await
        .unwrap();
    let id = view.endpoint.id;
    publish(&store, &admin, "retried").await;
    eventually(SETTLE, "delivered after two failures", || async {
        svc.deliveries(&admin, id, &DeliveryQuery::default())
            .await
            .unwrap()
            .items
            .first()
            .is_some_and(|l| l.delivery.status == DeliveryStatus::Delivered)
    })
    .await;
    let list = svc
        .deliveries(&admin, id, &DeliveryQuery::default())
        .await
        .unwrap();
    let d = svc
        .delivery(&admin, id, list.items[0].delivery.id)
        .await
        .unwrap();
    let codes: Vec<Option<i32>> = d.attempts.iter().map(|a| a.status_code).collect();
    assert_eq!(codes, [Some(500), Some(503), Some(200)]);
    assert_eq!(receiver.count(), 3);
    // Every attempt carries the same webhook-id (the event id).
    let ids: Vec<String> = receiver
        .hits
        .lock()
        .unwrap()
        .iter()
        .map(|h| h.header("webhook-id").to_owned())
        .collect();
    assert!(ids.iter().all(|i| *i == d.event.id.to_string()), "{ids:?}");
    node.shutdown().await.unwrap();
}

async fn gone_disables_and_mails_admins(db: Arc<TestDb>) {
    let site = Site::new(&db);
    let config = local_config(&site);
    let mailer = Arc::new(MemoryMailer::new());
    let opts = NodeOptions {
        webhook_mailer: Some(mailer.clone() as Arc<dyn ferrite_core::mail::Mailer>),
        ..NodeOptions::default()
    };
    let node = start(&config, db.shared_store(), opts).await.unwrap();
    let store = db.shared_store();
    let user = fixture_user(store.as_ref()).await;
    let admin = Actor::new(user.id, Role::Admin);
    let receiver = Receiver::start(vec![410]).await;
    let svc = service(store.clone());
    let (id, _) = hook(&svc, &admin, &receiver.url()).await;
    publish(&store, &admin, "gone").await;
    let sent = mailer.wait_for(1, SETTLE).await;
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert!(sent[0].subject.contains("Webhook disabled: receiver"));
    assert_eq!(sent[0].bcc, vec![user.email.clone()]);
    assert!(sent[0].text.contains("127.0.0.1"));
    let e = svc.get(&admin, id).await.unwrap();
    assert_eq!(e.endpoint.disabled_reason, Some(DisabledReason::Gone));
    // Both events (created, published) may have been in flight at once; each answer
    // was a 410, and the endpoint was disabled (and mailed about) once.
    eventually(SETTLE, "both finished", || async {
        svc.deliveries(&admin, id, &DeliveryQuery::default())
            .await
            .unwrap()
            .items
            .iter()
            .all(|l| l.delivery.status != DeliveryStatus::Pending)
    })
    .await;
    // (The publish may also have come after the disable: then it has no delivery.)
    let sent_before = receiver.count();
    assert!((1..=2).contains(&sent_before), "{sent_before}");
    assert_eq!(mailer.sent().len(), 1);
    let count = || async {
        svc.deliveries(&admin, id, &DeliveryQuery::default())
            .await
            .unwrap()
            .items
            .len()
    };
    let before = count().await;
    // A disabled endpoint gets no new events.
    publish(&store, &admin, "after").await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(receiver.count(), sent_before);
    assert_eq!(count().await, before, "no delivery for the later events");
    node.shutdown().await.unwrap();
}

/// Holds every send until released.
struct Gate {
    entered: AtomicUsize,
    release: Notify,
}

#[async_trait]
impl WebhookTransport for Gate {
    async fn send(&self, _request: OutgoingWebhook) -> SendOutcome {
        self.entered
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.release.notified().await;
        SendOutcome::Answered {
            status: 200,
            retry_after: None,
            excerpt: String::new(),
            elapsed: Duration::ZERO,
        }
    }
}

async fn busy_slots_refund_attempts(db: Arc<TestDb>) {
    let store = db.shared_store();
    let user = fixture_user(store.as_ref()).await;
    let admin = Actor::new(user.id, Role::Admin);
    let svc = Arc::new(service(store.clone()));
    let (_id, _) = hook(&svc, &admin, "https://hooks.example.com/x").await;
    publish(&store, &admin, "busy").await;
    let jobs = store
        .read()
        .jobs()
        .list(&ferrite_domain::JobQuery {
            kind: Some("webhook.deliver".into()),
            limit: 10,
            ..Default::default()
        })
        .await
        .unwrap()
        .items;
    assert_eq!(jobs.len(), 2, "created + published");
    let gate = Arc::new(Gate {
        entered: AtomicUsize::new(0),
        release: Notify::new(),
    });
    let handler = Arc::new(WebhookDeliverHandler::new(
        svc.clone(),
        gate.clone(),
        url::Url::parse(SITE_URL).unwrap(),
        1,
        None,
    ));
    let first = {
        let (h, job) = (handler.clone(), jobs[0].clone());
        tokio::spawn(async move { h.run(&job).await })
    };
    eventually(SETTLE, "first send in flight", || async {
        gate.entered.load(std::sync::atomic::Ordering::SeqCst) == 1
    })
    .await;
    assert!(matches!(
        handler.run(&jobs[1]).await,
        Err(JobError::Busy(_))
    ));
    gate.release.notify_one();
    first.await.unwrap().unwrap();
    // A bad payload is permanent.
    let mut bad = jobs[1].clone();
    bad.payload = json!({"v": 1});
    assert!(matches!(
        handler.run(&bad).await,
        Err(JobError::Permanent(_))
    ));
}

async fn prune_deletes_old_events(db: Arc<TestDb>) {
    let store = db.shared_store();
    let w = store.read().webhooks();
    let user = fixture_user(store.as_ref()).await;
    let admin = Actor::new(user.id, Role::Admin);
    let svc = Arc::new(service(store.clone()));
    hook(&svc, &admin, "https://hooks.example.com/x").await;
    let old = Timestamp::from_micros(Timestamp::now().as_micros() - 31 * 86_400_000_000).unwrap();
    let ev = ferrite_domain::WebhookEventRow {
        id: ferrite_domain::WebhookEventId::new(),
        kind: "content.created".into(),
        data: json!({}),
        created_at: old,
    };
    w.insert_event(&ev).await.unwrap();
    let fresh = ferrite_domain::WebhookEventRow {
        id: ferrite_domain::WebhookEventId::new(),
        created_at: Timestamp::now(),
        ..ev.clone()
    };
    w.insert_event(&fresh).await.unwrap();
    let handler = WebhooksPruneHandler { service: svc };
    let job = Job {
        id: ferrite_domain::JobId::new(),
        kind: "webhooks.prune".into(),
        payload: json!({"v": 1}),
        unique_key: None,
        run_at: Timestamp::now(),
        priority: 0,
        status: JobStatus::Running,
        attempts: 1,
        max_attempts: 5,
        locked_by: None,
        locked_until: None,
        last_error: None,
        created_at: Timestamp::now(),
        finished_at: None,
    };
    handler.run(&job).await.unwrap();
    assert_eq!(w.get_event(ev.id).await.unwrap(), None);
    assert!(w.get_event(fresh.id).await.unwrap().is_some());
}

backend_tests_spawning!(
    publish_is_delivered_with_valid_signature,
    receiver_down_retries_on_schedule,
    gone_disables_and_mails_admins,
    busy_slots_refund_attempts,
    prune_deletes_old_events,
);
