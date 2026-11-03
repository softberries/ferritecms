//! `WebhookService` (M6-6, design 18): CRUD and capability refusals, the write-only
//! secret, the audit (host only), test deliveries, redelivery and the attempt state
//! machine (success, retries with unique job keys, `410`, auto-disable, paused copies,
//! unreadable secrets), on both backends with a fake transport.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use ferrite_core::{
    Actor, CoreError, Scopes,
    auth::SiteKey,
    webhooks::{
        Delivered, OutgoingWebhook, SendOutcome, UrlPolicy, WebhookInput, WebhookPatch,
        WebhookService, WebhookState, WebhookTransport, sign,
    },
};
use ferrite_domain::{
    AuditQuery, DeliveryQuery, DeliveryStatus, DisabledReason, JobQuery, NewUser, Role, Secret,
    Timestamp, UserId, UserStatus, WebhookId,
};
use ferrite_store::{SCHEMA_META_INSTALL_ID, Store};
use ferrite_store_tests::{TestDb, backend_tests};
use url::Url;

fn any_ip(_: IpAddr) -> bool {
    true
}

fn key() -> SiteKey {
    SiteKey::from_hex(&"ab".repeat(32)).unwrap()
}

fn service(store: Arc<dyn Store>) -> WebhookService {
    WebhookService::new(
        store,
        key(),
        UrlPolicy {
            local_targets: false,
            allow_ip: any_ip,
        },
    )
}

fn site() -> Url {
    Url::parse("https://example.com/").unwrap()
}

async fn actor(store: &dyn Store, role: Role) -> Actor {
    let id = UserId::new();
    store
        .read()
        .users()
        .insert(&NewUser {
            id,
            login: format!("user-{id}"),
            email: format!("{id}@example.com"),
            display_name: "Test".into(),
            password_hash: None,
            role,
            status: UserStatus::Active,
            must_reset: false,
        })
        .await
        .unwrap();
    Actor::new(id, role)
}

fn input(name: &str) -> WebhookInput {
    WebhookInput {
        name: name.into(),
        url: "https://hooks.example.com/deploy?token=s3cr3t-token".into(),
        events: vec!["content.*".into()],
        ..WebhookInput::default()
    }
}

/// Answers each request with the next queued outcome (200 when none is left) and
/// records the requests.
#[derive(Default)]
struct Fake {
    answers: Mutex<Vec<SendOutcome>>,
    seen: Mutex<Vec<OutgoingWebhook>>,
}

fn answered(status: u16) -> SendOutcome {
    SendOutcome::Answered {
        status,
        retry_after: None,
        excerpt: format!("answer {status}"),
        elapsed: Duration::from_millis(7),
    }
}

impl Fake {
    fn with(answers: Vec<SendOutcome>) -> Self {
        let mut answers = answers;
        answers.reverse();
        Self {
            answers: Mutex::new(answers),
            seen: Mutex::default(),
        }
    }
    fn count(&self) -> usize {
        self.seen.lock().unwrap().len()
    }
}

#[async_trait]
impl WebhookTransport for Fake {
    async fn send(&self, request: OutgoingWebhook) -> SendOutcome {
        self.seen.lock().unwrap().push(request);
        self.answers.lock().unwrap().pop().unwrap_or(answered(200))
    }
}

/// Queues a test delivery and returns its id (the simplest way to get a delivery).
async fn ping(svc: &WebhookService, admin: &Actor, id: WebhookId) -> ferrite_domain::DeliveryId {
    svc.test(admin, id).await.unwrap()
}

/// A content event delivery for `id`, as emit would write it.
async fn content_delivery(store: &dyn Store, id: WebhookId) -> ferrite_domain::DeliveryId {
    use ferrite_domain::{Delivery, WebhookEventId, WebhookEventRow};
    let now = Timestamp::now();
    let w = store.read().webhooks();
    let ev = WebhookEventRow {
        id: WebhookEventId::new(),
        kind: "content.published".into(),
        data: serde_json::json!({"id": "x", "path": "/hello/", "public": true}),
        created_at: now,
    };
    w.insert_event(&ev).await.unwrap();
    let d = Delivery {
        id: ferrite_domain::DeliveryId::new(),
        endpoint_id: id,
        event_id: ev.id,
        status: DeliveryStatus::Pending,
        attempts: 0,
        next_attempt_at: Some(now),
        last_status_code: None,
        last_error: None,
        redelivery_of: None,
        created_at: now,
        finished_at: None,
    };
    w.insert_delivery(&d).await.unwrap();
    d.id
}

async fn deliver_jobs(store: &dyn Store) -> Vec<ferrite_domain::Job> {
    store
        .read()
        .jobs()
        .list(&JobQuery {
            kind: Some("webhook.deliver".into()),
            limit: 200,
            ..JobQuery::default()
        })
        .await
        .unwrap()
        .items
}

async fn create_returns_secret_once_and_get_never(db: &TestDb) {
    let store = db.shared_store();
    let svc = service(store.clone());
    let admin = actor(store.as_ref(), Role::Admin).await;
    let (view, secret) = svc.create(&admin, input("Deploy")).await.unwrap();
    assert!(secret.expose().starts_with("whsec_"));
    assert_eq!(view.state, WebhookState::Active);
    let e = &view.endpoint;
    assert!(!e.secret_sealed.contains(secret.expose()));
    let got = svc.get(&admin, e.id).await.unwrap();
    assert!(!format!("{got:?}").contains(secret.expose()));
    assert!(!format!("{got:?}").contains(&e.secret_sealed));
    assert_eq!(svc.list(&admin).await.unwrap().len(), 1);
    // Bring your own.
    let own = "whsec_MfKQ9r8GKYqrTwjUPD8ILPZIo2LaLaSw";
    let mut i = input("Own");
    i.secret = Some(Secret::new(own));
    let (_, s) = svc.create(&admin, i).await.unwrap();
    assert_eq!(s.expose(), own);
    let mut i = input("Bad");
    i.secret = Some(Secret::new("whsec_short"));
    assert!(matches!(
        svc.create(&admin, i).await,
        Err(CoreError::InvalidField {
            field: "secret",
            ..
        })
    ));
    // Field checks.
    for (field, mut i) in [
        ("name", input("")),
        ("url", input("x")),
        ("events", input("x")),
        ("content_types", input("x")),
    ] {
        match field {
            "url" => i.url = "http://hooks.example.com/".into(),
            "events" => i.events = vec!["content.nope".into()],
            "content_types" => i.content_types = Some(vec!["nope".into()]),
            _ => {}
        }
        match svc.create(&admin, i).await {
            Err(CoreError::InvalidField { field: f, .. }) => assert_eq!(f, field),
            other => panic!("{field}: {other:?}"),
        }
    }
    let mut i = input("Posts");
    i.content_types = Some(vec!["post".into()]);
    svc.create(&admin, i).await.unwrap();
}

async fn twentieth_plus_one_is_refused(db: &TestDb) {
    let store = db.shared_store();
    let svc = service(store.clone());
    let admin = actor(store.as_ref(), Role::Admin).await;
    for n in 0..20 {
        svc.create(&admin, input(&format!("hook {n}")))
            .await
            .unwrap();
    }
    match svc.create(&admin, input("one too many")).await {
        Err(CoreError::Invalid(m)) => assert!(m.contains("at most 20"), "{m}"),
        other => panic!("{other:?}"),
    }
}

async fn editor_and_scoped_tokens_refused(db: &TestDb) {
    let store = db.shared_store();
    let svc = service(store.clone());
    let admin = actor(store.as_ref(), Role::Admin).await;
    let (view, _) = svc.create(&admin, input("Deploy")).await.unwrap();
    let id = view.endpoint.id;
    let editor = actor(store.as_ref(), Role::Editor).await;
    let scoped = |names: &[&str]| {
        admin.with_scopes(
            Scopes::from_strs(&names.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>()).unwrap(),
        )
    };
    let cases = [
        (editor, "manage_webhooks", "manage_webhooks"),
        (scoped(&["system:write"]), "webhooks:read", "webhooks:write"),
        (
            scoped(&["settings:write"]),
            "webhooks:read",
            "webhooks:write",
        ),
        (
            admin.with_scopes(Scopes::OAUTH),
            "webhooks:read",
            "webhooks:write",
        ),
    ];
    for (who, read, write) in cases {
        let is = |r: Result<(), CoreError>, want: &str| match r {
            Err(CoreError::Forbidden(n)) => assert_eq!(n, want, "{who:?}"),
            other => panic!("{who:?}: {other:?}"),
        };
        is(svc.list(&who).await.map(drop), read);
        is(svc.get(&who, id).await.map(drop), read);
        is(svc.event_types(&who).map(drop), read);
        is(
            svc.deliveries(&who, id, &DeliveryQuery::default())
                .await
                .map(drop),
            read,
        );
        is(svc.create(&who, input("x")).await.map(drop), write);
        is(svc.delete(&who, id).await, write);
        is(svc.rotate_secret(&who, id).await.map(drop), write);
        is(svc.test(&who, id).await.map(drop), write);
    }
    let reader = scoped(&["webhooks:read"]);
    svc.list(&reader).await.unwrap();
    assert!(matches!(
        svc.create(&reader, input("x")).await,
        Err(CoreError::Forbidden("webhooks:write"))
    ));
}

async fn audit_has_host_not_url_or_secret(db: &TestDb) {
    let store = db.shared_store();
    let svc = service(store.clone());
    let admin = actor(store.as_ref(), Role::Admin).await;
    let (view, secret) = svc.create(&admin, input("Deploy")).await.unwrap();
    let id = view.endpoint.id;
    svc.update(
        &admin,
        id,
        WebhookPatch {
            url: Some("https://other.example.com/x?token=another-token".into()),
            enabled: Some(false),
            ..WebhookPatch::default()
        },
    )
    .await
    .unwrap();
    let (rotated, _) = svc.rotate_secret(&admin, id).await.unwrap();
    svc.test(&admin, id).await.unwrap();
    svc.delete(&admin, id).await.unwrap();
    let rows = store
        .read()
        .audit()
        .list(&AuditQuery::first(50))
        .await
        .unwrap()
        .items;
    let actions: Vec<&str> = rows.iter().map(|r| r.action.as_str()).collect();
    for a in [
        "webhook.create",
        "webhook.update",
        "webhook.disable",
        "webhook.rotate_secret",
        "webhook.test",
        "webhook.delete",
    ] {
        assert!(actions.contains(&a), "{a}: {actions:?}");
    }
    let all = format!("{rows:?}");
    for leaked in [
        secret.expose(),
        rotated.expose(),
        "s3cr3t-token",
        "another-token",
        "/deploy",
    ] {
        assert!(!all.contains(leaked), "{leaked} in {all}");
    }
    assert!(all.contains("other.example.com"));
    let update = rows.iter().find(|r| r.action == "webhook.update").unwrap();
    assert_eq!(update.details["fields"], serde_json::json!(["url"]));
}

async fn deliver_success_records_attempt(db: &TestDb) {
    let store = db.shared_store();
    let svc = service(store.clone());
    let admin = actor(store.as_ref(), Role::Admin).await;
    let (view, secret) = svc.create(&admin, input("Deploy")).await.unwrap();
    let id = view.endpoint.id;
    let d = content_delivery(store.as_ref(), id).await;
    let fake = Fake::default();
    let out = svc.deliver(d, 1, &site(), &fake).await.unwrap();
    assert_eq!(out, Delivered::Sent { status: Some(200) });
    let req = fake.seen.lock().unwrap().pop().unwrap();
    let header = |n: &str| {
        req.headers
            .iter()
            .find(|(k, _)| *k == n)
            .map(|(_, v)| v.clone())
            .unwrap()
    };
    // The signature verifies with the secret shown at create.
    let key = ferrite_core::webhooks::secret::parse(secret.expose()).unwrap();
    let ts: i64 = header("webhook-timestamp").parse().unwrap();
    assert_eq!(
        header("webhook-signature"),
        sign::signature(&key, &header("webhook-id"), ts, &req.body)
    );
    let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
    assert_eq!(body["type"], "content.published");
    assert_eq!(body["site"]["url"], "https://example.com");
    assert_eq!(body["data"]["url"], "https://example.com/hello/");
    assert!(body["data"].get("public").is_none());
    assert_eq!(body["id"], header("webhook-id"));
    let detail = svc.delivery(&admin, id, d).await.unwrap();
    assert_eq!(detail.delivery.status, DeliveryStatus::Delivered);
    assert_eq!(detail.attempts.len(), 1);
    assert_eq!(detail.attempts[0].status_code, Some(200));
    let e = svc.get(&admin, id).await.unwrap();
    assert!(e.endpoint.last_success_at.is_some());
    assert_eq!(e.counts_24h.delivered, 1);
}

async fn deliver_failure_schedules_next_job_with_unique_key(db: &TestDb) {
    let store = db.shared_store();
    let svc = service(store.clone());
    let admin = actor(store.as_ref(), Role::Admin).await;
    let (view, _) = svc.create(&admin, input("Deploy")).await.unwrap();
    let id = view.endpoint.id;
    let d = content_delivery(store.as_ref(), id).await;
    let fake = Fake::with(vec![answered(503), answered(302)]);
    assert_eq!(
        svc.deliver(d, 1, &site(), &fake).await.unwrap(),
        Delivered::Sent { status: Some(503) }
    );
    let jobs = deliver_jobs(store.as_ref()).await;
    let keys: Vec<_> = jobs.iter().filter_map(|j| j.unique_key.clone()).collect();
    assert_eq!(keys, vec![format!("webhook.deliver:{d}:2")]);
    let job = &jobs[0];
    assert_eq!(job.payload["attempt"], 2);
    assert_eq!(job.max_attempts, 3);
    let delay = job.run_at.as_micros() - Timestamp::now().as_micros();
    assert!(delay > 3_000_000 && delay <= 6_000_000, "{delay}");
    let detail = svc.delivery(&admin, id, d).await.unwrap();
    assert_eq!(detail.delivery.status, DeliveryStatus::Pending);
    assert_eq!(detail.delivery.last_error.as_deref(), Some("status 503"));
    let e = svc.get(&admin, id).await.unwrap();
    assert_eq!(e.state, WebhookState::Failing);
    assert_eq!(e.endpoint.consecutive_failures, 1);
    // A redirect is a failure.
    svc.deliver(d, 2, &site(), &fake).await.unwrap();
    let detail = svc.delivery(&admin, id, d).await.unwrap();
    assert_eq!(
        detail.delivery.last_error.as_deref(),
        Some("redirect not followed (status 302)")
    );
    // The last attempt fails the delivery.
    let short = service(store.clone());
    let d2 = content_delivery(store.as_ref(), id).await;
    let failing = Fake::with((0..8).map(|_| answered(500)).collect());
    for n in 1..=8 {
        short.deliver(d2, n, &site(), &failing).await.unwrap();
    }
    let detail = svc.delivery(&admin, id, d2).await.unwrap();
    assert_eq!(detail.delivery.status, DeliveryStatus::Failed);
    assert_eq!(detail.attempts.len(), 8);
    assert!(detail.delivery.finished_at.is_some());
    assert!(
        !deliver_jobs(store.as_ref())
            .await
            .iter()
            .any(|j| j.unique_key.as_deref() == Some(&format!("webhook.deliver:{d2}:9")))
    );
}

async fn same_attempt_twice_sends_once(db: &TestDb) {
    let store = db.shared_store();
    let svc = service(store.clone());
    let admin = actor(store.as_ref(), Role::Admin).await;
    let (view, _) = svc.create(&admin, input("Deploy")).await.unwrap();
    let d = content_delivery(store.as_ref(), view.endpoint.id).await;
    let fake = Fake::default();
    svc.deliver(d, 1, &site(), &fake).await.unwrap();
    assert_eq!(
        svc.deliver(d, 1, &site(), &fake).await.unwrap(),
        Delivered::Nothing
    );
    assert_eq!(
        svc.deliver(d, 3, &site(), &fake).await.unwrap(),
        Delivered::Nothing
    );
    assert_eq!(fake.count(), 1);
    let detail = svc.delivery(&admin, view.endpoint.id, d).await.unwrap();
    assert_eq!(detail.attempts.len(), 1);
}

async fn gone_disables_at_once(db: &TestDb) {
    let store = db.shared_store();
    let svc = service(store.clone());
    let admin = actor(store.as_ref(), Role::Admin).await;
    let (view, _) = svc.create(&admin, input("Deploy")).await.unwrap();
    let id = view.endpoint.id;
    let d = content_delivery(store.as_ref(), id).await;
    let out = svc
        .deliver(d, 1, &site(), &Fake::with(vec![answered(410)]))
        .await
        .unwrap();
    let Delivered::AutoDisabled { reason, .. } = out else {
        panic!("{out:?}")
    };
    assert_eq!(reason, DisabledReason::Gone);
    let e = svc.get(&admin, id).await.unwrap();
    assert_eq!(e.state, WebhookState::Disabled);
    assert_eq!(e.endpoint.disabled_reason, Some(DisabledReason::Gone));
    assert_eq!(
        svc.delivery(&admin, id, d).await.unwrap().delivery.status,
        DeliveryStatus::Failed
    );
    assert!(deliver_jobs(store.as_ref()).await.is_empty());
    let audit = store
        .read()
        .audit()
        .list(&AuditQuery::first(10))
        .await
        .unwrap()
        .items;
    let row = audit
        .iter()
        .find(|r| r.action == "webhook.auto_disabled")
        .unwrap();
    assert_eq!(row.actor_id, None);
    assert_eq!(row.details["reason"], "gone");
    assert_eq!(row.details["host"], "hooks.example.com");
}

async fn set_failing_since(store: &dyn Store, id: WebhookId, days: i64, failures: i32) {
    let w = store.read().webhooks();
    let mut e = w.get_endpoint(id).await.unwrap().unwrap();
    e.failing_since = Some(
        Timestamp::from_micros(Timestamp::now().as_micros() - days * 24 * 3600 * 1_000_000)
            .unwrap(),
    );
    e.consecutive_failures = failures;
    w.update_endpoint(&e).await.unwrap();
}

async fn five_days_and_ten_failures_auto_disable(db: &TestDb) {
    let store = db.shared_store();
    let svc = service(store.clone());
    let admin = actor(store.as_ref(), Role::Admin).await;
    let (view, _) = svc.create(&admin, input("Deploy")).await.unwrap();
    let id = view.endpoint.id;
    let fail = || Fake::with(vec![answered(500)]);
    // Six days of failing but only 9 failures after this attempt: stays.
    set_failing_since(store.as_ref(), id, 6, 8).await;
    let d = content_delivery(store.as_ref(), id).await;
    svc.deliver(d, 1, &site(), &fail()).await.unwrap();
    assert!(svc.get(&admin, id).await.unwrap().endpoint.enabled);
    // Many failures but only two days: stays.
    set_failing_since(store.as_ref(), id, 2, 40).await;
    let d = content_delivery(store.as_ref(), id).await;
    svc.deliver(d, 1, &site(), &fail()).await.unwrap();
    assert!(svc.get(&admin, id).await.unwrap().endpoint.enabled);
    // Both: disabled as failing.
    set_failing_since(store.as_ref(), id, 6, 9).await;
    let d = content_delivery(store.as_ref(), id).await;
    let out = svc.deliver(d, 1, &site(), &fail()).await.unwrap();
    assert!(matches!(
        out,
        Delivered::AutoDisabled {
            reason: DisabledReason::Failing,
            ..
        }
    ));
    let e = svc.get(&admin, id).await.unwrap();
    assert_eq!(e.endpoint.disabled_reason, Some(DisabledReason::Failing));
    // Re-enabling clears the reason and the health.
    let e = svc
        .update(
            &admin,
            id,
            WebhookPatch {
                enabled: Some(true),
                ..WebhookPatch::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(e.state, WebhookState::Active);
    assert_eq!(e.endpoint.consecutive_failures, 0);
    assert_eq!(e.endpoint.failing_since, None);
    assert_eq!(e.endpoint.disabled_reason, None);
}

async fn two_xx_resets_health(db: &TestDb) {
    let store = db.shared_store();
    let svc = service(store.clone());
    let admin = actor(store.as_ref(), Role::Admin).await;
    let (view, _) = svc.create(&admin, input("Deploy")).await.unwrap();
    let id = view.endpoint.id;
    set_failing_since(store.as_ref(), id, 1, 5).await;
    let d = content_delivery(store.as_ref(), id).await;
    svc.deliver(d, 1, &site(), &Fake::default()).await.unwrap();
    let e = svc.get(&admin, id).await.unwrap();
    assert_eq!(e.state, WebhookState::Active);
    assert_eq!(
        (e.endpoint.consecutive_failures, e.endpoint.failing_since),
        (0, None)
    );
}

async fn disabled_endpoint_skips(db: &TestDb) {
    let store = db.shared_store();
    let svc = service(store.clone());
    let admin = actor(store.as_ref(), Role::Admin).await;
    let (view, _) = svc.create(&admin, input("Deploy")).await.unwrap();
    let id = view.endpoint.id;
    let d = content_delivery(store.as_ref(), id).await;
    svc.update(
        &admin,
        id,
        WebhookPatch {
            enabled: Some(false),
            ..WebhookPatch::default()
        },
    )
    .await
    .unwrap();
    let fake = Fake::default();
    assert_eq!(
        svc.deliver(d, 1, &site(), &fake).await.unwrap(),
        Delivered::Skipped("endpoint disabled")
    );
    assert_eq!(fake.count(), 0);
    let detail = svc.delivery(&admin, id, d).await.unwrap();
    assert_eq!(detail.delivery.status, DeliveryStatus::Skipped);
    // A disabled endpoint cannot be redelivered to.
    assert!(matches!(
        svc.redeliver(&admin, id, d).await,
        Err(CoreError::Invalid(_))
    ));
}

async fn paused_copy_skips(db: &TestDb) {
    let store = db.shared_store();
    let svc = service(store.clone());
    let admin = actor(store.as_ref(), Role::Admin).await;
    let (view, _) = svc.create(&admin, input("Deploy")).await.unwrap();
    let id = view.endpoint.id;
    let d = content_delivery(store.as_ref(), id).await;
    store
        .read()
        .schema_meta()
        .set(SCHEMA_META_INSTALL_ID, "a-copy")
        .await
        .unwrap();
    assert_eq!(
        svc.get(&admin, id).await.unwrap().state,
        WebhookState::Paused
    );
    let fake = Fake::default();
    assert_eq!(
        svc.deliver(d, 1, &site(), &fake).await.unwrap(),
        Delivered::Skipped("paused: copied database")
    );
    assert_eq!(fake.count(), 0);
    // Enabling stamps this install id.
    let e = svc
        .update(
            &admin,
            id,
            WebhookPatch {
                enabled: Some(true),
                ..WebhookPatch::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(e.state, WebhookState::Active);
    assert_eq!(e.endpoint.install_id, "a-copy");
}

async fn unreadable_secret_skips(db: &TestDb) {
    let store = db.shared_store();
    let svc = service(store.clone());
    let admin = actor(store.as_ref(), Role::Admin).await;
    let (view, _) = svc.create(&admin, input("Deploy")).await.unwrap();
    let id = view.endpoint.id;
    let d = content_delivery(store.as_ref(), id).await;
    // The same database under another site key.
    let other = WebhookService::new(
        store.clone(),
        SiteKey::from_hex(&"cd".repeat(32)).unwrap(),
        svc.policy(),
    );
    let fake = Fake::default();
    assert_eq!(
        other.deliver(d, 1, &site(), &fake).await.unwrap(),
        Delivered::Skipped("secret cannot be read: rotate it")
    );
    assert_eq!(fake.count(), 0);
}

async fn rotation_signs_with_both(db: &TestDb) {
    let store = db.shared_store();
    let svc = service(store.clone());
    let admin = actor(store.as_ref(), Role::Admin).await;
    let (view, old) = svc.create(&admin, input("Deploy")).await.unwrap();
    let id = view.endpoint.id;
    let (new, until) = svc.rotate_secret(&admin, id).await.unwrap();
    assert_ne!(old.expose(), new.expose());
    let in_a_day = Timestamp::now().as_micros() + 24 * 3600 * 1_000_000;
    assert!((until.as_micros() - in_a_day).abs() < 5_000_000);
    let v = svc.get(&admin, id).await.unwrap();
    assert_eq!(v.secret_rotating_until, Some(until));
    let d = content_delivery(store.as_ref(), id).await;
    let fake = Fake::default();
    svc.deliver(d, 1, &site(), &fake).await.unwrap();
    let req = fake.seen.lock().unwrap().pop().unwrap();
    let get = |n: &str| req.headers.iter().find(|(k, _)| *k == n).unwrap().1.clone();
    let ts: i64 = get("webhook-timestamp").parse().unwrap();
    let parse = |s: &Secret| ferrite_core::webhooks::secret::parse(s.expose()).unwrap();
    let want = format!(
        "{} {}",
        sign::signature(&parse(&new), &get("webhook-id"), ts, &req.body),
        sign::signature(&parse(&old), &get("webhook-id"), ts, &req.body)
    );
    assert_eq!(get("webhook-signature"), want);
}

async fn test_on_disabled_works_on_paused_refused(db: &TestDb) {
    let store = db.shared_store();
    let svc = service(store.clone());
    let admin = actor(store.as_ref(), Role::Admin).await;
    let mut i = input("Deploy");
    i.enabled = Some(false);
    let (view, _) = svc.create(&admin, i).await.unwrap();
    let id = view.endpoint.id;
    assert_eq!(view.state, WebhookState::Disabled);
    let d = ping(&svc, &admin, id).await;
    let jobs = deliver_jobs(store.as_ref()).await;
    assert_eq!(jobs[0].priority, 10);
    let fake = Fake::with(vec![answered(500)]);
    assert_eq!(
        svc.deliver(d, 1, &site(), &fake).await.unwrap(),
        Delivered::Sent { status: Some(500) }
    );
    // A test gets one attempt.
    let detail = svc.delivery(&admin, id, d).await.unwrap();
    assert_eq!(detail.delivery.status, DeliveryStatus::Failed);
    assert_eq!(detail.event.kind, "webhook.ping");
    let body: serde_json::Value =
        serde_json::from_slice(&fake.seen.lock().unwrap()[0].body).unwrap();
    assert_eq!(body["data"]["endpoint"], id.to_string());
    store
        .read()
        .schema_meta()
        .set(SCHEMA_META_INSTALL_ID, "a-copy")
        .await
        .unwrap();
    assert!(matches!(
        svc.test(&admin, id).await,
        Err(CoreError::Invalid(_))
    ));
}

async fn redeliver_and_since_cap(db: &TestDb) {
    let store = db.shared_store();
    let svc = service(store.clone());
    let admin = actor(store.as_ref(), Role::Admin).await;
    let (view, _) = svc.create(&admin, input("Deploy")).await.unwrap();
    let id = view.endpoint.id;
    let start = Timestamp::from_micros(Timestamp::now().as_micros() - 1_000_000).unwrap();
    let w = store.read().webhooks();
    let mut originals = Vec::new();
    for _ in 0..3 {
        let d = content_delivery(store.as_ref(), id).await;
        w.set_delivery_status(
            d,
            DeliveryStatus::Failed,
            Some("status 500"),
            Timestamp::now(),
        )
        .await
        .unwrap();
        originals.push(d);
    }
    let again = svc.redeliver(&admin, id, originals[0]).await.unwrap();
    let detail = svc.delivery(&admin, id, again).await.unwrap();
    assert_eq!(detail.delivery.redelivery_of, Some(originals[0]));
    let orig = svc.delivery(&admin, id, originals[0]).await.unwrap();
    assert_eq!(
        detail.event.id, orig.event.id,
        "same event, same webhook-id"
    );
    assert_eq!(
        svc.redeliver_since(&admin, id, start, None).await.unwrap(),
        3
    );
    assert!(matches!(
        svc.redeliver_since(&admin, id, start, Some(vec![DeliveryStatus::Pending]))
            .await,
        Err(CoreError::InvalidField {
            field: "statuses",
            ..
        })
    ));
    // Unknown delivery of another endpoint is not found.
    let (other, _) = svc.create(&admin, input("Other")).await.unwrap();
    assert!(matches!(
        svc.redeliver(&admin, other.endpoint.id, originals[0]).await,
        Err(CoreError::NotFound)
    ));
    // The cap: 1 000 per call.
    let tx = store.begin().await.unwrap();
    for _ in 0..1005 {
        let now = Timestamp::now();
        let ev = ferrite_domain::WebhookEventRow {
            id: ferrite_domain::WebhookEventId::new(),
            kind: "content.updated".into(),
            data: serde_json::json!({}),
            created_at: now,
        };
        tx.webhooks().insert_event(&ev).await.unwrap();
        tx.webhooks()
            .insert_delivery(&ferrite_domain::Delivery {
                id: ferrite_domain::DeliveryId::new(),
                endpoint_id: other.endpoint.id,
                event_id: ev.id,
                status: DeliveryStatus::Skipped,
                attempts: 0,
                next_attempt_at: None,
                last_status_code: None,
                last_error: None,
                redelivery_of: None,
                created_at: now,
                finished_at: Some(now),
            })
            .await
            .unwrap();
    }
    tx.commit().await.unwrap();
    assert_eq!(
        svc.redeliver_since(&admin, other.endpoint.id, start, None)
            .await
            .unwrap(),
        1000
    );
}

async fn test_limit_10_per_minute(db: &TestDb) {
    let store = db.shared_store();
    let svc = service(store.clone());
    let admin = actor(store.as_ref(), Role::Admin).await;
    let (view, _) = svc.create(&admin, input("Deploy")).await.unwrap();
    for _ in 0..10 {
        svc.test(&admin, view.endpoint.id).await.unwrap();
    }
    assert!(matches!(
        svc.test(&admin, view.endpoint.id).await,
        Err(CoreError::RateLimited { .. })
    ));
}

async fn prune_and_deleted_endpoint(db: &TestDb) {
    let store = db.shared_store();
    let svc = service(store.clone());
    let admin = actor(store.as_ref(), Role::Admin).await;
    let (view, _) = svc.create(&admin, input("Deploy")).await.unwrap();
    let id = view.endpoint.id;
    let d = content_delivery(store.as_ref(), id).await;
    svc.delete(&admin, id).await.unwrap();
    // A job for a deleted endpoint's delivery finds nothing.
    assert_eq!(
        svc.deliver(d, 1, &site(), &Fake::default()).await.unwrap(),
        Delivered::Nothing
    );
    let later = Timestamp::from_micros(Timestamp::now().as_micros() + 1_000_000).unwrap();
    assert_eq!(svc.prune(later, 100).await.unwrap(), 1);
    assert!(matches!(
        svc.delete(&admin, id).await,
        Err(CoreError::NotFound)
    ));
}

backend_tests!(
    create_returns_secret_once_and_get_never,
    twentieth_plus_one_is_refused,
    editor_and_scoped_tokens_refused,
    audit_has_host_not_url_or_secret,
    deliver_success_records_attempt,
    deliver_failure_schedules_next_job_with_unique_key,
    same_attempt_twice_sends_once,
    gone_disables_at_once,
    five_days_and_ten_failures_auto_disable,
    two_xx_resets_health,
    disabled_endpoint_skips,
    paused_copy_skips,
    unreadable_secret_skips,
    rotation_signs_with_both,
    test_on_disabled_works_on_paused_refused,
    redeliver_and_since_cap,
    test_limit_10_per_minute,
    prune_and_deleted_endpoint,
);
