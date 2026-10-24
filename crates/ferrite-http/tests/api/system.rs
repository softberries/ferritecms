//! `/api/v1/system/{status,nodes,audit}` (M2a-2b P15, P18; Review Focus 5): admin-only
//! reads (`manage_system` + `system:read`), node diagnostics scrubbed on read.

use axum::http::{Method, StatusCode};
use ferrite_domain::{NewNode, NodeId, Role};
use serde_json::{Value, json};

use crate::{
    api_tests,
    harness::{Api, Client, SITE_URL},
};

api_tests!(
    system_reads_need_manage_system,
    status_reports_backends_and_schema,
    audit_lists_newest_first_with_actor,
    old_node_meta_is_scrubbed_on_read,
);

const STATUS: &str = "/api/v1/system/status";
const NODES: &str = "/api/v1/system/nodes";
const AUDIT: &str = "/api/v1/system/audit";

async fn admin(api: &Api) -> Client {
    api.user("admin", Role::Admin, false).await;
    api.logged_in("admin").await
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
            schema_version: 5,
            meta,
        })
        .await
        .unwrap();
    id
}

/// Nothing is revealed without credentials, to an editor, or to a token without
/// `system:read`.
async fn system_reads_need_manage_system(api: Api) -> Api {
    for path in [STATUS, NODES, AUDIT] {
        api.get(path).await.assert_problem(401, "unauthenticated");
    }
    api.user("ed", Role::Editor, false).await;
    let mut ed = api.logged_in("ed").await;
    let admin_user = api.user("root", Role::Admin, false).await;
    let content = api.bearer(&admin_user, &["content:read"]).await;
    let system = api.bearer(&admin_user, &["system:read"]).await;
    for path in [STATUS, NODES, AUDIT] {
        let p = ed.get(&api, path).await.assert_problem(403, "forbidden");
        assert!(
            p["detail"].as_str().unwrap().contains("manage_system"),
            "{path}: {p}"
        );
        let p = content
            .get(&api, path)
            .await
            .assert_problem(403, "forbidden");
        assert!(
            p["detail"].as_str().unwrap().contains("system:read"),
            "{path}: {p}"
        );
        let r = system.get(&api, path).await;
        assert_eq!(r.status, StatusCode::OK, "{path}: {r:?}");
    }
    api
}

async fn status_reports_backends_and_schema(api: Api) -> Api {
    let mut c = admin(&api).await;
    let r = c.get(&api, STATUS).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert_eq!(r.header("cache-control"), Some("no-store"));
    let s = r.json();
    let cluster = api.store.backend() == ferrite_store::Backend::Postgres;
    assert_eq!(s["version"], env!("CARGO_PKG_VERSION"), "{s}");
    assert_eq!(s["site_url"], SITE_URL, "{s}");
    assert_eq!(s["mode"], if cluster { "cluster" } else { "solo" }, "{s}");
    assert_eq!(
        s["database"],
        if cluster { "postgres" } else { "sqlite" },
        "{s}"
    );
    assert_eq!(s["media"], if cluster { "s3" } else { "local" }, "{s}");
    assert_eq!(s["schema"]["verdict"], "up_to_date", "{s}");
    assert_eq!(s["schema"]["pending"], json!([]), "{s}");
    assert_eq!(
        s["schema"]["db_version"], s["schema"]["binary_version"],
        "{s}"
    );
    assert!(s["schema"]["db_version"].as_i64().unwrap() >= 6, "{s}");
    assert!(s["theme"]["active"].is_null(), "{s}");
    assert_eq!(s["theme"]["fallback_nodes"], json!([]), "{s}");
    for status in ["queued", "running", "done", "dead"] {
        assert!(s["jobs"][status].is_u64(), "{status}: {s}");
    }
    assert_eq!(s["nodes"], json!({"live": 0, "stale": 0}), "{s}");
    assert_eq!(
        s["node"]["id"],
        api.api.node.id.as_uuid().to_string(),
        "{s}"
    );
    assert!(s["node"]["started_at"].is_string(), "{s}");

    // A live node serving a fallback is named; a queued job is counted.
    register(
        &api,
        "web-1",
        json!({"theme": {"name": "blog", "fallback": true, "error": "x"}}),
    )
    .await;
    register(
        &api,
        "web-2",
        json!({"theme": {"name": "blog", "fallback": false}}),
    )
    .await;
    let queued = s["jobs"]["queued"].as_u64().unwrap();
    api.store
        .read()
        .jobs()
        .enqueue(&ferrite_domain::NewJob::new("test.kind", json!({"v": 1})))
        .await
        .unwrap();
    let s = c.get(&api, STATUS).await.json();
    assert_eq!(s["theme"]["fallback_nodes"], json!(["web-1"]), "{s}");
    assert_eq!(s["nodes"], json!({"live": 2, "stale": 0}), "{s}");
    assert_eq!(s["jobs"]["queued"], queued + 1, "{s}");

    // An activated theme row is named.
    let r = c.post_empty(&api, "/api/v1/themes/blog/activate").await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    let s = c.get(&api, STATUS).await.json();
    assert_eq!(s["theme"]["active"], "blog", "{s}");
    api
}

async fn audit_lists_newest_first_with_actor(api: Api) -> Api {
    let mut c = admin(&api).await;
    let r = c
        .send(
            &api,
            Method::PATCH,
            "/api/v1/settings",
            Some(&json!({"site.description": "Hello"})),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    let r = c.get(&api, AUDIT).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    let page = r.json();
    let items = page["items"].as_array().unwrap();
    let first = &items[0];
    assert_eq!(first["action"], "settings.update", "{page}");
    assert_eq!(first["target_kind"], "settings", "{page}");
    assert!(first["target_id"].is_null(), "{page}");
    assert_eq!(
        first["details"],
        json!({"keys": ["site.description"]}),
        "{page}"
    );
    assert_eq!(first["actor"]["login"], "admin", "{page}");
    assert!(first["actor"]["id"].is_string(), "{page}");
    assert!(first["id"].is_string() && first["at"].is_string(), "{page}");
    // Older: the operator-created account (no actor).
    let created = items
        .iter()
        .find(|e| e["action"] == "user.create")
        .unwrap_or_else(|| panic!("{page}"));
    assert!(created["actor"].is_null(), "{page}");
    assert_eq!(created["details"]["login"], "admin", "{page}");
    let ats: Vec<&str> = items.iter().map(|e| e["at"].as_str().unwrap()).collect();
    let mut sorted = ats.clone();
    sorted.sort_by(|a, b| b.cmp(a));
    assert_eq!(ats, sorted, "newest first: {page}");

    // Cursor paging: one per page, same order.
    let p1 = c.get(&api, &format!("{AUDIT}?limit=1")).await.json();
    assert_eq!(p1["items"].as_array().unwrap().len(), 1, "{p1}");
    assert_eq!(p1["items"][0]["id"], first["id"], "{p1}");
    let cursor = p1["next_cursor"].as_str().unwrap();
    let p2 = c
        .get(&api, &format!("{AUDIT}?limit=1&cursor={cursor}"))
        .await
        .json();
    assert_eq!(p2["items"][0]["id"], items[1]["id"], "{p2}");

    // Bounded input.
    let p = c
        .get(&api, &format!("{AUDIT}?limit=0"))
        .await
        .assert_problem(400, "validation");
    assert_eq!(p["errors"][0]["field"], "limit", "{p}");
    c.get(&api, &format!("{AUDIT}?limit=201"))
        .await
        .assert_problem(400, "validation");
    let p = c
        .get(&api, &format!("{AUDIT}?cursor=nope"))
        .await
        .assert_problem(400, "validation");
    assert_eq!(p["errors"][0]["field"], "cursor", "{p}");
    // Another listing's cursor is refused too.
    let users = c.get(&api, "/api/v1/users?limit=1").await.json();
    if let Some(other) = users["next_cursor"].as_str() {
        c.get(&api, &format!("{AUDIT}?cursor={other}"))
            .await
            .assert_problem(400, "validation");
    }
    api
}

/// Review Focus 5: a node row heartbeated with unscrubbed texts (an older node) shows
/// none of its URL, path, address or credentials in `GET /system/nodes` or
/// `GET /themes`.
async fn old_node_meta_is_scrubbed_on_read(api: Api) -> Api {
    let mut c = admin(&api).await;
    let leak = "theme \"x\": storage backend error: Unable to open file \
                /var/lib/ferrite/media/artifacts/ab/abcd: Permission denied; \
                GET https://AKIAKEY:hunter2@minio.internal:9000/bucket/x from 10.0.0.5:9000";
    let want = "theme \"x\": storage backend error: Unable to open file <path>: Permission \
                denied; GET <url> from <addr>";
    let id = register(&api, "old-node", json!({})).await;
    let meta = json!({
        "site_url": SITE_URL,
        "theme": {"name": "blog", "hash": "0123", "origin": "embedded", "error": leak, "fallback": true},
        "plugins": {"seo": {"version": "1.0.0", "status": "failed", "error": leak}},
        "plugins_error": leak,
        "permalinks_error": leak,
    });
    assert!(api.store.read().nodes().heartbeat(id, &meta).await.unwrap());

    let r = c.get(&api, NODES).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    let list = r.json();
    assert!(list["next_cursor"].is_null(), "{list}");
    let node = list["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["id"] == id.as_uuid().to_string())
        .unwrap_or_else(|| panic!("{list}"));
    assert_eq!(node["name"], "old-node", "{node}");
    assert_eq!(node["version"], "0.0.9", "{node}");
    assert_eq!(node["schema_version"], 5, "{node}");
    assert_eq!(node["live"], true, "{node}");
    assert!(node["started_at"].is_string() && node["heartbeat_at"].is_string());
    assert_eq!(node["meta"]["theme"]["error"], want, "{node}");
    assert_eq!(node["meta"]["plugins"]["seo"]["error"], want, "{node}");
    assert_eq!(node["meta"]["plugins_error"], want, "{node}");
    assert_eq!(node["meta"]["permalinks_error"], want, "{node}");
    assert_eq!(node["meta"]["site_url"], SITE_URL, "{node}");

    let themes = c.get(&api, "/api/v1/themes").await;
    assert_eq!(themes.status, StatusCode::OK, "{themes:?}");
    let blog = themes.json()["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "blog")
        .cloned()
        .unwrap();
    assert_eq!(blog["nodes"][0]["error"], want, "{blog}");

    for body in [&r.body, &themes.body] {
        for secret in [
            "/var/lib",
            "AKIAKEY",
            "hunter2",
            "minio.internal",
            "10.0.0.5",
        ] {
            assert!(!body.contains(secret), "{secret} in {body}");
        }
    }
    api
}
