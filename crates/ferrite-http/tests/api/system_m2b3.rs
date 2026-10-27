//! M2b-3 Task 3: audit filters, the mail facts and the test mail (P49), typed node
//! `meta`, `me.mail_configured`, and the site title read at use time (P51).

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::http::{Method, StatusCode};
use ferrite_domain::{NewAudit, NewNode, NodeId, Role};
use serde_json::{Value, json};

use crate::{
    api_tests,
    harness::{Api, Client, MAIL_FROM, SITE_TITLE, SITE_URL, json_body},
};

api_tests!(
    status_reports_mail_without_secrets,
    me_reports_mail_configured_by_role,
    mail_test_sends_to_self_and_is_limited,
    nodes_meta_is_typed,
    totp_issuer_follows_site_title_option,
    audit_filters_over_the_api,
);

const STATUS: &str = "/api/v1/system/status";
const NODES: &str = "/api/v1/system/nodes";
const AUDIT: &str = "/api/v1/system/audit";
const MAIL_TEST: &str = "/api/v1/system/mail/test";
const ME: &str = "/api/v1/auth/me";
const SETTINGS: &str = "/api/v1/settings";
const TOTP_SETUP: &str = "/api/v1/auth/totp/setup";
const MAIL_WAIT: Duration = Duration::from_secs(5);

async fn admin(api: &Api, login: &str) -> Client {
    api.user(login, Role::Admin, false).await;
    api.logged_in(login).await
}

async fn set_title(api: &Api, c: &mut Client, title: Value) {
    let r = c
        .send(
            api,
            Method::PATCH,
            SETTINGS,
            Some(&json!({ "site.title": title })),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
}

/// `mail: {configured, from}`; the SMTP URL is never in the body (it holds credentials).
async fn status_reports_mail_without_secrets(api: Api) -> Api {
    let mut c = admin(&api, "admin").await;
    let r = c.get(&api, STATUS).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    let s = r.json();
    assert_eq!(
        s["mail"],
        json!({"configured": true, "from": MAIL_FROM}),
        "{s}"
    );
    for secret in ["smtp", "smtps", "password"] {
        assert!(!r.body.to_lowercase().contains(secret), "{secret}: {s}");
    }

    let router = api.router_without_mail();
    let req = c
        .builder(Method::GET, STATUS)
        .body(axum::body::Body::empty())
        .unwrap();
    let r = c.send_on(&router, req).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert_eq!(
        r.json()["mail"],
        json!({"configured": false, "from": null}),
        "{r:?}"
    );
    api
}

/// A boolean for roles holding `manage_users` or `manage_system` (whatever the token's
/// scopes), `null` for everyone else.
async fn me_reports_mail_configured_by_role(api: Api) -> Api {
    let mut root = admin(&api, "root").await;
    assert_eq!(root.get(&api, ME).await.json()["mail_configured"], true);
    let root_user = api
        .store
        .read()
        .users()
        .get_by_login("root")
        .await
        .unwrap()
        .unwrap();
    let narrow = api.bearer(&root_user, &["content:read"]).await;
    assert_eq!(narrow.get(&api, ME).await.json()["mail_configured"], true);

    for (login, role) in [("ed", Role::Editor), ("al", Role::Author)] {
        api.user(login, role, false).await;
        let mut c = api.logged_in(login).await;
        let me = c.get(&api, ME).await.json();
        assert!(me["mail_configured"].is_null(), "{login}: {me}");
    }

    let router = api.router_without_mail();
    let req = root
        .builder(Method::GET, ME)
        .body(axum::body::Body::empty())
        .unwrap();
    let me = root.send_on(&router, req).await.json();
    assert_eq!(me["mail_configured"], false, "{me}");
    api
}

/// Waits until at least 30 s are left in the current 600 s window, so the four calls
/// below share one window.
async fn align_to_mail_window() {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let left = 600 - now % 600;
    if left < 30 {
        tokio::time::sleep(Duration::from_secs(left + 1)).await;
    }
}

/// `202 {to}` to the actor's own address through the queue, audited; the fourth call in
/// 10 minutes is `429`; editors and tokens without `system:write` are refused; a site
/// without `[mail]` is `422` (field `mail`).
async fn mail_test_sends_to_self_and_is_limited(api: Api) -> Api {
    let mut c = admin(&api, "admin").await;

    // Refusals first: none of them sends or counts.
    api.user("ed", Role::Editor, false).await;
    let mut ed = api.logged_in("ed").await;
    let p = ed
        .post_empty(&api, MAIL_TEST)
        .await
        .assert_problem(403, "forbidden");
    assert!(
        p["detail"].as_str().unwrap().contains("manage_system"),
        "{p}"
    );
    let admin_user = api
        .store
        .read()
        .users()
        .get_by_login("admin")
        .await
        .unwrap()
        .unwrap();
    let reader = api.bearer(&admin_user, &["system:read"]).await;
    let p = reader
        .send(&api, Method::POST, MAIL_TEST, None, None)
        .await
        .assert_problem(403, "forbidden");
    assert!(
        p["detail"].as_str().unwrap().contains("system:write"),
        "{p}"
    );
    let router = api.router_without_mail();
    let req = c
        .builder(Method::POST, MAIL_TEST)
        .body(axum::body::Body::empty())
        .unwrap();
    let p = c
        .send_on(&router, req)
        .await
        .assert_problem(422, "validation");
    assert_eq!(p["errors"][0]["field"], "mail", "{p}");
    // CSRF is required (the sweep covers the route too).
    let mut no_csrf = Client::new();
    no_csrf.cookie = c.cookie.clone();
    no_csrf
        .post_empty(&api, MAIL_TEST)
        .await
        .assert_problem(403, "csrf");
    assert!(
        api.mailer
            .wait_for(1, Duration::from_millis(100))
            .await
            .is_empty()
    );

    align_to_mail_window().await;
    let before = SystemTime::now();
    let r = c.post_empty(&api, MAIL_TEST).await;
    assert_eq!(r.status, StatusCode::ACCEPTED, "{r:?}");
    assert_eq!(r.json(), json!({"to": "admin@example.com"}), "{r:?}");
    let sent = api.mailer.wait_for(1, MAIL_WAIT).await;
    assert_eq!(sent.len(), 1);
    let mail = &sent[0];
    assert_eq!(mail.to, ["admin@example.com"]);
    assert!(mail.bcc.is_empty());
    assert_eq!(mail.subject, format!("Test e-mail from {SITE_TITLE}"));
    assert!(mail.text.contains(SITE_URL), "{}", mail.text);
    assert!(mail.text.contains("\"admin\""), "{}", mail.text);
    assert!(mail.text.contains("UTC"), "{}", mail.text);
    let year = chrono::DateTime::<chrono::Utc>::from(before)
        .format("%Y-%m-%d")
        .to_string();
    assert!(mail.text.contains(&year), "{}", mail.text);
    assert!(mail.text.contains("mail settings"), "{}", mail.text);

    let audit = api
        .store
        .read()
        .audit()
        .list(&ferrite_domain::AuditQuery::first(10))
        .await
        .unwrap()
        .items;
    let row = audit
        .iter()
        .find(|e| e.action == "mail.test")
        .unwrap_or_else(|| panic!("{audit:?}"));
    assert_eq!(row.actor_id, Some(admin_user.id));
    assert_eq!(row.target_kind, "user");
    assert_eq!(
        row.target_id.as_deref(),
        Some(admin_user.id.to_string().as_str())
    );
    assert!(
        !row.details.to_string().contains("example.com"),
        "{:?}",
        row.details
    );

    for _ in 0..2 {
        let r = c.post_empty(&api, MAIL_TEST).await;
        assert_eq!(r.status, StatusCode::ACCEPTED, "{r:?}");
    }
    let r = c.post_empty(&api, MAIL_TEST).await;
    r.assert_problem(429, "rate-limited");
    assert!(r.header("retry-after").is_some(), "{r:?}");
    assert_eq!(api.mailer.wait_for(3, MAIL_WAIT).await.len(), 3);
    assert_eq!(
        api.mailer
            .wait_for(4, Duration::from_millis(200))
            .await
            .len(),
        3
    );
    // Per user: another admin still has its three.
    let mut other = admin(&api, "other").await;
    let r = other.post_empty(&api, MAIL_TEST).await;
    assert_eq!(r.status, StatusCode::ACCEPTED, "{r:?}");
    assert_eq!(r.json()["to"], "other@example.com", "{r:?}");
    api
}

async fn register(api: &Api, name: &str, meta: Value) -> NodeId {
    let id = NodeId::new();
    api.store
        .read()
        .nodes()
        .register(&NewNode {
            id,
            name: name.into(),
            version: "0.0.9".into(),
            schema_version: 8,
            meta,
        })
        .await
        .unwrap();
    id
}

fn node_of(list: &Value, id: NodeId) -> Value {
    list["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["id"] == id.as_uuid().to_string())
        .cloned()
        .unwrap_or_else(|| panic!("{list}"))
}

/// Every known member comes back typed; unknown members are dropped; a member of the
/// wrong type is `null` without failing the list (an older or newer node in a rolling
/// deploy); the spec names the schema.
async fn nodes_meta_is_typed(api: Api) -> Api {
    let mut c = admin(&api, "admin").await;
    let full = register(
        &api,
        "full",
        json!({
            "theme": {"name": "blog", "hash": "abc", "origin": {"artifact": "abc"},
                      "error": null, "fallback": false, "future": 1},
            "plugins": {"hello": {"version": "0.1.0", "hash": "def", "status": "loaded",
                                  "error": null, "extra": true}},
            "plugins_error": null,
            "permalinks_error": "bad /etc/passwd",
            "job_kinds": ["media.variants", "sessions.prune"],
            "site_id": "s1",
            "site_url": SITE_URL,
            "mode": "cluster",
            "clock_skew_ms": -12,
            "unknown_member": {"x": 1},
        }),
    )
    .await;
    let odd = register(
        &api,
        "odd",
        json!({
            "theme": {"name": 7, "origin": "embedded", "fallback": "yes"},
            "plugins": {"a": "not an object", "b": {"version": 2, "status": "failed"}},
            "job_kinds": ["ok", 3],
            "clock_skew_ms": "soon",
            "mode": ["solo"],
        }),
    )
    .await;
    let bare = register(&api, "bare", json!({})).await;

    let r = c.get(&api, NODES).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    let list = r.json();

    let meta = node_of(&list, full)["meta"].clone();
    assert_eq!(
        meta["theme"],
        json!({"name": "blog", "hash": "abc", "origin": {"artifact": "abc"},
               "error": null, "fallback": false}),
        "{meta}"
    );
    assert_eq!(
        meta["plugins"],
        json!({"hello": {"version": "0.1.0", "hash": "def", "status": "loaded", "error": null}}),
        "{meta}"
    );
    assert!(meta["plugins_error"].is_null(), "{meta}");
    assert_eq!(meta["permalinks_error"], "bad <path>", "{meta}");
    assert_eq!(
        meta["job_kinds"],
        json!(["media.variants", "sessions.prune"])
    );
    assert_eq!(meta["site_id"], "s1");
    assert_eq!(meta["site_url"], SITE_URL);
    assert_eq!(meta["mode"], "cluster");
    assert_eq!(meta["clock_skew_ms"], -12);
    assert!(meta.get("unknown_member").is_none(), "{meta}");

    let meta = node_of(&list, odd)["meta"].clone();
    assert!(meta["theme"]["name"].is_null(), "{meta}");
    assert_eq!(meta["theme"]["origin"], "embedded", "{meta}");
    assert!(meta["theme"]["fallback"].is_null(), "{meta}");
    assert!(meta["plugins"].get("a").is_none(), "{meta}");
    assert!(meta["plugins"]["b"]["version"].is_null(), "{meta}");
    assert_eq!(meta["plugins"]["b"]["status"], "failed", "{meta}");
    assert_eq!(meta["job_kinds"], json!(["ok"]), "{meta}");
    assert!(meta["clock_skew_ms"].is_null(), "{meta}");
    assert!(meta["mode"].is_null(), "{meta}");

    let meta = node_of(&list, bare)["meta"].clone();
    for member in ["theme", "plugins", "job_kinds", "site_id", "clock_skew_ms"] {
        assert!(meta[member].is_null(), "{member}: {meta}");
    }

    let spec = api.get("/api/v1/openapi.json").await.json();
    let schemas = &spec["components"]["schemas"];
    assert!(schemas["NodeMetaDto"].is_object(), "NodeMetaDto missing");
    assert_eq!(
        schemas["NodeDto"]["properties"]["meta"]["$ref"],
        "#/components/schemas/NodeMetaDto"
    );
    api
}

/// The TOTP issuer, the reset mail and the test mail name the site by the `site.title`
/// option at use time, and by `ferrite.toml` again once it is reset.
async fn totp_issuer_follows_site_title_option(api: Api) -> Api {
    let mut c = admin(&api, "admin").await;
    let url = |r: &crate::harness::Resp| r.json()["otpauth_url"].as_str().unwrap().to_owned();
    let first = url(&c.post_empty(&api, TOTP_SETUP).await);
    assert!(first.contains("issuer=Test%20Site"), "{first}");

    set_title(&api, &mut c, json!("Night: Owl")).await;
    let second = url(&c.post_empty(&api, TOTP_SETUP).await);
    assert!(second.contains("issuer=Night%20Owl"), "{second}");

    // The reset mail and the test mail use it too.
    let reset = json_body(
        Client::new().builder(Method::POST, "/api/v1/auth/password-reset"),
        &json!({"login": "admin"}),
    );
    assert_eq!(api.send(reset).await.status, StatusCode::ACCEPTED);
    let sent = api.mailer.wait_for(1, MAIL_WAIT).await;
    assert_eq!(sent[0].subject, "[Night: Owl] Password reset");
    assert!(sent[0].text.contains("Night: Owl"), "{}", sent[0].text);
    let r = c.post_empty(&api, MAIL_TEST).await;
    assert_eq!(r.status, StatusCode::ACCEPTED, "{r:?}");
    let sent = api.mailer.wait_for(2, MAIL_WAIT).await;
    assert_eq!(sent[1].subject, "Test e-mail from Night: Owl");

    set_title(&api, &mut c, Value::Null).await;
    let third = url(&c.post_empty(&api, TOTP_SETUP).await);
    assert!(third.contains("issuer=Test%20Site"), "{third}");
    api
}

async fn audit_filters_over_the_api(api: Api) -> Api {
    let mut c = admin(&api, "admin").await;
    let admin_user = api
        .store
        .read()
        .users()
        .get_by_login("admin")
        .await
        .unwrap()
        .unwrap();
    let log = api.store.read().audit();
    for (action, kind, id) in [
        ("user.update", "user", Some("u1")),
        ("user.delete", "user", Some("u2")),
        ("users.thing", "user", Some("u1")),
        ("settings.update", "settings", None),
    ] {
        log.append(&NewAudit {
            actor_id: Some(admin_user.id),
            action: action.into(),
            target_kind: kind.into(),
            target_id: id.map(str::to_owned),
            details: json!({}),
        })
        .await
        .unwrap();
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    let actions = |page: &Value| -> Vec<String> {
        page["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["action"].as_str().unwrap().to_owned())
            .collect()
    };
    let get = async |c: &mut Client, q: &str| {
        let r = c.get(&api, &format!("{AUDIT}?{q}")).await;
        assert_eq!(r.status, StatusCode::OK, "{q}: {r:?}");
        r.json()
    };
    // `user.create` is the operator's account creation.
    let p = get(&mut c, "action=user.").await;
    assert_eq!(
        actions(&p),
        ["user.delete", "user.update", "user.create"],
        "{p}"
    );
    let p = get(&mut c, "action=user.update").await;
    assert_eq!(actions(&p), ["user.update"], "{p}");
    let p = get(&mut c, "target_kind=user&target_id=u1").await;
    assert_eq!(actions(&p), ["users.thing", "user.update"], "{p}");
    let p = get(
        &mut c,
        &format!("actor={}&target_kind=settings", admin_user.id),
    )
    .await;
    assert_eq!(actions(&p), ["settings.update"], "{p}");
    // Paged with the cursor, the filter holds.
    let p1 = get(&mut c, "action=user.&limit=1").await;
    assert_eq!(actions(&p1), ["user.delete"], "{p1}");
    let cursor = p1["next_cursor"].as_str().unwrap();
    let p2 = get(&mut c, &format!("action=user.&limit=2&cursor={cursor}")).await;
    assert_eq!(actions(&p2), ["user.update", "user.create"], "{p2}");
    assert!(p2["next_cursor"].is_null(), "{p2}");

    let p = c
        .get(&api, &format!("{AUDIT}?target_id=u1"))
        .await
        .assert_problem(422, "validation");
    assert_eq!(p["errors"][0]["field"], "target_id", "{p}");
    for (q, field) in [
        ("actor=nope", "actor"),
        ("action=", "action"),
        (&*format!("action={}", "a".repeat(101)), "action"),
        ("target_kind=", "target_kind"),
    ] {
        let p = c
            .get(&api, &format!("{AUDIT}?{q}"))
            .await
            .assert_problem(400, "validation");
        assert_eq!(p["errors"][0]["field"], field, "{q}: {p}");
    }
    api
}
