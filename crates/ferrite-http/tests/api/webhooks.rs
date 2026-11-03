//! M6-6 task 6: `/api/v1/webhooks` (design 18 §8): CRUD with a write-only secret, the
//! field errors, the 20-endpoint cap, capability and scope refusals, rotation, test
//! deliveries, the delivery log, redelivery, the event catalogue and CSRF.

use async_trait::async_trait;
use axum::http::{Method, StatusCode};
use ferrite_core::Scopes;
use ferrite_core::webhooks::{OutgoingWebhook, SendOutcome, WebhookTransport};
use ferrite_domain::{DeliveryId, Role};
use serde_json::{Value, json};

use crate::{
    api_tests,
    harness::{Api, Bearer, Client},
};

api_tests!(
    crud_round_trip,
    validation_fields,
    twenty_one_is_422,
    non_admin_and_scopes_refused,
    rotate_returns_new_secret_and_window,
    test_delivery_log_and_redelivery,
    events_catalogue_lists_all_types,
    csrf_required_for_session_writes,
);

const HOOKS: &str = "/api/v1/webhooks";

fn input(name: &str) -> Value {
    json!({
        "name": name,
        "url": "https://hooks.example.com/build?token=t0ken",
        "events": ["content.*"],
    })
}

async fn admin(api: &Api) -> Bearer {
    let ada = api.user("ada", Role::Admin, false).await;
    api.bearer(&ada, &["*"]).await
}

async fn create(api: &Api, b: &Bearer, body: &Value) -> Value {
    let r = b.send(api, Method::POST, HOOKS, Some(body), None).await;
    assert_eq!(r.status, StatusCode::CREATED, "{r:?}");
    r.json()
}

async fn crud_round_trip(api: Api) -> Api {
    let b = admin(&api).await;
    let r = b
        .send(&api, Method::POST, HOOKS, Some(&input("Deploy")), None)
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{r:?}");
    let body = r.json();
    let id = body["webhook"]["id"].as_str().unwrap().to_owned();
    assert_eq!(r.header("location"), Some(format!("{HOOKS}/{id}").as_str()));
    let secret = body["secret"].as_str().unwrap().to_owned();
    assert!(secret.starts_with("whsec_"));
    let hook = &body["webhook"];
    assert_eq!(hook["state"], "active");
    assert_eq!(hook["enabled"], true);
    assert_eq!(hook["content_types"], Value::Null);
    assert_eq!(hook["counts_24h"]["delivered"], 0);
    assert!(hook.get("secret").is_none());
    // Get and list never show the secret.
    let got = b.get(&api, &format!("{HOOKS}/{id}")).await;
    assert_eq!(got.status, StatusCode::OK);
    let text = got.body.clone();
    assert!(
        !text.contains(&secret) && !text.contains("whsec_"),
        "{text}"
    );
    assert_eq!(
        got.json()["url"],
        "https://hooks.example.com/build?token=t0ken"
    );
    let list = b.get(&api, HOOKS).await;
    assert!(!list.body.contains("whsec_"));
    assert_eq!(list.json()["items"].as_array().unwrap().len(), 1);
    // Patch: rename, narrow, disable; then clear the filter and enable.
    let r = b
        .send(
            &api,
            Method::PATCH,
            &format!("{HOOKS}/{id}"),
            Some(&json!({"name": "Build", "content_types": ["post"], "enabled": false})),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    let v = r.json();
    assert_eq!(
        (
            v["name"].clone(),
            v["state"].clone(),
            v["disabled_reason"].clone()
        ),
        (json!("Build"), json!("disabled"), json!("manual"))
    );
    assert_eq!(v["content_types"], json!(["post"]));
    let v = b
        .send(
            &api,
            Method::PATCH,
            &format!("{HOOKS}/{id}"),
            Some(&json!({"content_types": null, "enabled": true})),
            None,
        )
        .await
        .json();
    assert_eq!(v["content_types"], Value::Null);
    assert_eq!(v["state"], "active");
    // `null` is refused where it means nothing.
    b.send(
        &api,
        Method::PATCH,
        &format!("{HOOKS}/{id}"),
        Some(&json!({"name": null})),
        None,
    )
    .await
    .assert_problem(400, "malformed-json");
    // Delete, then 404.
    let r = b
        .send(&api, Method::DELETE, &format!("{HOOKS}/{id}"), None, None)
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    b.get(&api, &format!("{HOOKS}/{id}"))
        .await
        .assert_problem(404, "not-found");
    b.send(&api, Method::DELETE, &format!("{HOOKS}/{id}"), None, None)
        .await
        .assert_problem(404, "not-found");
    // The audit holds the host, not the URL.
    let audit = b.get(&api, "/api/v1/system/audit?limit=50").await;
    let text = audit.body.clone();
    assert!(text.contains("webhook.create") && text.contains("hooks.example.com"));
    assert!(!text.contains("t0ken") && !text.contains(&secret), "{text}");
    api
}

async fn validation_fields(api: Api) -> Api {
    let b = admin(&api).await;
    for (field, body) in [
        (
            "name",
            json!({"name": "", "url": "https://h.example/", "events": ["*"]}),
        ),
        (
            "url",
            json!({"name": "a", "url": "http://h.example/", "events": ["*"]}),
        ),
        (
            "url",
            json!({"name": "a", "url": "https://u:p@h.example/", "events": ["*"]}),
        ),
        (
            "events",
            json!({"name": "a", "url": "https://h.example/", "events": []}),
        ),
        (
            "events",
            json!({"name": "a", "url": "https://h.example/", "events": ["webhook.ping"]}),
        ),
        (
            "content_types",
            json!({"name": "a", "url": "https://h.example/", "events": ["*"], "content_types": ["nope"]}),
        ),
        (
            "secret",
            json!({"name": "a", "url": "https://h.example/", "events": ["*"], "secret": "whsec_x"}),
        ),
    ] {
        let p = b
            .send(&api, Method::POST, HOOKS, Some(&body), None)
            .await
            .assert_problem(422, "validation");
        assert_eq!(p["errors"][0]["field"], field, "{body}: {p}");
        // The secret is never echoed.
        assert!(!p.to_string().contains("whsec_x"), "{p}");
    }
    api
}

async fn twenty_one_is_422(api: Api) -> Api {
    let b = admin(&api).await;
    for n in 0..20 {
        create(&api, &b, &input(&format!("hook {n:02}"))).await;
    }
    let p = b
        .send(&api, Method::POST, HOOKS, Some(&input("one more")), None)
        .await
        .assert_problem(422, "validation");
    assert!(p["detail"].as_str().unwrap().contains("at most 20"), "{p}");
    let names: Vec<String> = b.get(&api, HOOKS).await.json()["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["name"].as_str().unwrap().to_owned())
        .collect();
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(names, sorted, "by name");
    api
}

async fn non_admin_and_scopes_refused(api: Api) -> Api {
    let b = admin(&api).await;
    let id = create(&api, &b, &input("Deploy")).await["webhook"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let ed = api.user("ed", Role::Editor, false).await;
    let editor = api.bearer(&ed, &["*"]).await;
    let p = editor
        .get(&api, HOOKS)
        .await
        .assert_problem(403, "forbidden");
    assert!(
        p["detail"].as_str().unwrap().contains("manage_webhooks"),
        "{p}"
    );
    editor
        .send(&api, Method::POST, HOOKS, Some(&input("x")), None)
        .await
        .assert_problem(403, "forbidden");
    let ada = api.user("ada2", Role::Admin, false).await;
    for scopes in [
        &["system:write"][..],
        &["settings:write"],
        &["content:write"],
    ] {
        let t = api.bearer(&ada, scopes).await;
        let p = t.get(&api, HOOKS).await.assert_problem(403, "forbidden");
        assert!(
            p["detail"].as_str().unwrap().contains("webhooks:read"),
            "{p}"
        );
        let p = t
            .send(
                &api,
                Method::POST,
                &format!("{HOOKS}/{id}/test"),
                None,
                None,
            )
            .await
            .assert_problem(403, "forbidden");
        assert!(
            p["detail"].as_str().unwrap().contains("webhooks:write"),
            "{p}"
        );
    }
    // Every scope an OAuth client can be granted is not enough (and OAuth access tokens
    // are refused on /api/v1 anyway, `oauth.rs`).
    let names: Vec<String> = Scopes::OAUTH.to_strings();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let t = api.bearer(&ada, &refs).await;
    t.get(&api, HOOKS).await.assert_problem(403, "forbidden");
    // A read token reads, and writes nothing.
    let reader = api.bearer(&ada, &["webhooks:read"]).await;
    assert_eq!(reader.get(&api, HOOKS).await.status, StatusCode::OK);
    reader
        .send(&api, Method::DELETE, &format!("{HOOKS}/{id}"), None, None)
        .await
        .assert_problem(403, "forbidden");
    api
}

async fn rotate_returns_new_secret_and_window(api: Api) -> Api {
    let b = admin(&api).await;
    let created = create(&api, &b, &input("Deploy")).await;
    let id = created["webhook"]["id"].as_str().unwrap();
    let r = b
        .send(
            &api,
            Method::POST,
            &format!("{HOOKS}/{id}/secret"),
            None,
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    let body = r.json();
    let new = body["secret"].as_str().unwrap();
    assert!(new.starts_with("whsec_"));
    assert_ne!(new, created["secret"].as_str().unwrap());
    let until = body["previous_valid_until"].as_str().unwrap().to_owned();
    let v = b.get(&api, &format!("{HOOKS}/{id}")).await.json();
    assert_eq!(v["secret_rotating_until"], until);
    api
}

/// Answers 500 to every request.
struct Failing;

#[async_trait]
impl WebhookTransport for Failing {
    async fn send(&self, _request: OutgoingWebhook) -> SendOutcome {
        SendOutcome::Answered {
            status: 500,
            retry_after: None,
            excerpt: "<b>broken</b>".into(),
            elapsed: std::time::Duration::from_millis(3),
        }
    }
}

async fn test_delivery_log_and_redelivery(api: Api) -> Api {
    let b = admin(&api).await;
    let id = create(&api, &b, &input("Deploy")).await["webhook"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let r = b
        .send(
            &api,
            Method::POST,
            &format!("{HOOKS}/{id}/test"),
            None,
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::ACCEPTED, "{r:?}");
    let delivery = r.json()["delivery"].as_str().unwrap().to_owned();
    let list = b
        .get(&api, &format!("{HOOKS}/{id}/deliveries"))
        .await
        .json();
    assert_eq!(list["items"][0]["id"], delivery.as_str());
    assert_eq!(list["items"][0]["status"], "pending");
    assert_eq!(list["items"][0]["event_type"], "webhook.ping");
    // The node's job would send it; here the service does, through a failing receiver.
    let site = url::Url::parse("https://example.com/").unwrap();
    api.api
        .webhooks
        .deliver(delivery.parse::<DeliveryId>().unwrap(), 1, &site, &Failing)
        .await
        .unwrap();
    let detail = b
        .get(&api, &format!("{HOOKS}/{id}/deliveries/{delivery}"))
        .await
        .json();
    assert_eq!(detail["delivery"]["status"], "failed", "{detail}");
    assert_eq!(detail["event"]["type"], "webhook.ping");
    assert_eq!(detail["event"]["data"]["endpoint"], id.as_str());
    let attempt = &detail["attempts"][0];
    assert_eq!(attempt["status_code"], 500);
    assert_eq!(attempt["error"], "status 500");
    assert_eq!(attempt["response_excerpt"], "<b>broken</b>");
    // Filter and paging.
    for _ in 0..2 {
        b.send(
            &api,
            Method::POST,
            &format!("{HOOKS}/{id}/test"),
            None,
            None,
        )
        .await;
    }
    let failed = b
        .get(&api, &format!("{HOOKS}/{id}/deliveries?status=failed"))
        .await
        .json();
    assert_eq!(failed["items"].as_array().unwrap().len(), 1);
    let page = b
        .get(&api, &format!("{HOOKS}/{id}/deliveries?limit=2"))
        .await
        .json();
    assert_eq!(page["items"].as_array().unwrap().len(), 2);
    let cursor = page["next_cursor"].as_str().unwrap();
    let rest = b
        .get(
            &api,
            &format!("{HOOKS}/{id}/deliveries?limit=2&cursor={cursor}"),
        )
        .await
        .json();
    assert_eq!(rest["items"][0]["id"], delivery.as_str());
    b.get(&api, &format!("{HOOKS}/{id}/deliveries?cursor=nope"))
        .await
        .assert_problem(400, "validation");
    b.get(&api, &format!("{HOOKS}/{id}/deliveries?status=lost"))
        .await
        .assert_problem(400, "malformed-query");
    // Redeliver one, then since a time.
    let r = b
        .send(
            &api,
            Method::POST,
            &format!("{HOOKS}/{id}/deliveries/{delivery}/redeliver"),
            None,
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::ACCEPTED, "{r:?}");
    let again = r.json()["delivery"].as_str().unwrap().to_owned();
    let d = b
        .get(&api, &format!("{HOOKS}/{id}/deliveries/{again}"))
        .await
        .json();
    assert_eq!(d["delivery"]["redelivery_of"], delivery.as_str());
    assert_eq!(d["event"]["id"], detail["event"]["id"]);
    let r = b
        .send(
            &api,
            Method::POST,
            &format!("{HOOKS}/{id}/redeliver"),
            Some(&json!({"since": "2000-01-01T00:00:00Z"})),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::ACCEPTED, "{r:?}");
    assert_eq!(r.json()["count"], 1, "one failed delivery");
    let p = b
        .send(
            &api,
            Method::POST,
            &format!("{HOOKS}/{id}/redeliver"),
            Some(&json!({"since": "2000-01-01T00:00:00Z", "statuses": ["pending"]})),
            None,
        )
        .await
        .assert_problem(422, "validation");
    assert_eq!(p["errors"][0]["field"], "statuses");
    // Another webhook's delivery is not found here.
    let other = create(&api, &b, &input("Other")).await["webhook"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    b.get(&api, &format!("{HOOKS}/{other}/deliveries/{delivery}"))
        .await
        .assert_problem(404, "not-found");
    api
}

async fn events_catalogue_lists_all_types(api: Api) -> Api {
    let b = admin(&api).await;
    let r = b.get(&api, &format!("{HOOKS}/events")).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    let body = r.json();
    assert_eq!(body["version"], 1);
    let types: Vec<&str> = body["types"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["type"].as_str().unwrap())
        .collect();
    assert_eq!(types.len(), ferrite_core::webhooks::EVENT_TYPES.len());
    for t in [
        "content.published",
        "media.uploaded",
        "comment.moderated",
        "user.deleted",
        "import.finished",
    ] {
        assert!(types.contains(&t), "{t}");
    }
    assert!(!types.contains(&"webhook.ping"));
    assert_eq!(body["types"][0]["group"], "content");
    api
}

async fn csrf_required_for_session_writes(api: Api) -> Api {
    api.user("ada", Role::Admin, false).await;
    let mut c = api.logged_in("ada").await;
    let r = c.post(&api, HOOKS, input("Deploy")).await;
    assert_eq!(r.status, StatusCode::CREATED, "{r:?}");
    let mut no_csrf = Client::new();
    no_csrf.cookie = c.cookie.clone();
    no_csrf
        .post(&api, HOOKS, input("Other"))
        .await
        .assert_problem(403, "csrf");
    // A test delivery limit: 10 per user and minute, then 429 with Retry-After.
    let id = r.json()["webhook"]["id"].as_str().unwrap().to_owned();
    let session = (c.cookie.clone(), c.csrf.clone());
    api.in_one_window(std::time::Duration::from_secs(10), |_| {
        let (api, id) = (&api, id.clone());
        let mut c = Client::new();
        c.cookie = session.0.clone();
        c.csrf = session.1.clone();
        async move {
            let path = format!("{HOOKS}/{id}/test");
            let mut last = None;
            for _ in 0..11 {
                last = Some(c.post_empty(api, &path).await);
            }
            let r = last.unwrap();
            assert_eq!(r.status, StatusCode::TOO_MANY_REQUESTS, "{r:?}");
            assert!(r.header("retry-after").is_some());
        }
    })
    .await;
    api
}
