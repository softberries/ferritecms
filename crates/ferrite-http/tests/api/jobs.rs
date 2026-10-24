//! `/api/v1/system/{jobs,metrics/summary}` and `/api/v1/plugins/{name}/metrics`
//! (M2a-2b P19, P20, F16): the job list without payloads and with scrubbed errors, the
//! dead-job retry, the per-plugin metrics and the node's metrics snapshot.

use std::time::Duration;

use axum::http::StatusCode;
use ferrite_domain::{JobId, NewJob, NewNode, NodeId, Role, Timestamp};
use ferrite_store::JobRepo;
use serde_json::{Value, json};

use crate::{
    api_tests,
    harness::{Api, Client, plugin_manifest, plugin_wasm},
};

api_tests!(
    dead_job_is_listed_and_retried,
    unknown_kind_stays_visible,
    job_list_input_is_bounded,
    plugin_metrics_shape,
    metrics_summary_counts_cache_hits,
    system_routes_refuse_editors,
);

const JOBS: &str = "/api/v1/system/jobs";
const SUMMARY: &str = "/api/v1/system/metrics/summary";

/// A storage error as a `media.variants` retry could carry it (F16).
const LEAKY: &str = "storage backend error: Unable to open file \
                     /srv/ferrite/data/media/ab/abcd: Permission denied; \
                     https://AKIAKEY:hunter2@minio.internal:9000/bucket";

async fn admin(api: &Api) -> Client {
    api.user("admin", Role::Admin, false).await;
    api.logged_in("admin").await
}

/// A live node whose runner handles `kinds`.
async fn register(api: &Api, kinds: &[&str]) {
    api.store
        .read()
        .nodes()
        .register(&NewNode {
            id: NodeId::new(),
            name: "worker".into(),
            version: "0.0.9".into(),
            schema_version: 6,
            meta: json!({ "job_kinds": kinds }),
        })
        .await
        .unwrap();
}

/// A job of `kind` with one attempt that failed with `error`: `dead`.
async fn dead_job(jobs: &dyn JobRepo, kind: &str, error: &str) -> JobId {
    let mut new = NewJob::new(kind, json!({"v": 1, "token": "PAYLOAD-SECRET"}));
    new.max_attempts = 1;
    let id = jobs.enqueue(&new).await.unwrap().unwrap();
    let claimed = jobs
        .claim(
            NodeId::new(),
            &[kind.to_owned()],
            1,
            Duration::from_secs(60),
        )
        .await
        .unwrap();
    let lease = claimed[0].lease().unwrap();
    assert_eq!(
        jobs.fail(&lease, error, false).await.unwrap(),
        Some(ferrite_domain::JobStatus::Dead)
    );
    id
}

fn item(list: &Value, id: JobId) -> &Value {
    list["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|j| j["id"] == id.to_string())
        .unwrap_or_else(|| panic!("{id} not listed: {list}"))
}

async fn dead_job_is_listed_and_retried(api: Api) -> Api {
    let mut c = admin(&api).await;
    let jobs = api.store.read().jobs();
    let id = dead_job(jobs, "test.fail", LEAKY).await;
    register(&api, &["test.fail"]).await;

    let r = c.get(&api, &format!("{JOBS}?status=dead")).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert_eq!(r.header("cache-control"), Some("no-store"));
    assert!(
        !r.body.contains("PAYLOAD-SECRET"),
        "payloads never returned"
    );
    for leak in ["/srv/ferrite", "AKIAKEY", "hunter2", "minio"] {
        assert!(!r.body.contains(leak), "{leak}: {}", r.body);
    }
    let list = r.json();
    assert_eq!(list["items"].as_array().unwrap().len(), 1, "{list}");
    assert!(list["next_cursor"].is_null());
    let job = item(&list, id);
    assert!(job.get("payload").is_none(), "{job}");
    assert_eq!(job["kind"], "test.fail");
    assert_eq!(job["status"], "dead");
    assert_eq!(
        (&job["attempts"], &job["max_attempts"]),
        (&json!(1), &json!(1))
    );
    assert_eq!(job["priority"], 0);
    assert!(job["finished_at"].is_string(), "{job}");
    assert!(job["locked_by"].is_null(), "{job}");
    assert_eq!(job["handled"], true, "a live node lists the kind: {job}");
    let error = job["last_error"].as_str().unwrap();
    assert!(error.starts_with("storage backend error"), "{error}");
    assert!(
        error.contains("<path>") && error.contains("<url>"),
        "{error}"
    );

    // Retry: queued, due now, attempts reset, the (scrubbed) error kept; audited.
    let before = Timestamp::now();
    let r = c.post_empty(&api, &format!("{JOBS}/{id}/retry")).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert!(!r.body.contains("PAYLOAD-SECRET") && !r.body.contains("/srv/ferrite"));
    let retried = r.json();
    assert_eq!(retried["id"], id.to_string());
    assert_eq!(retried["status"], "queued");
    assert_eq!(retried["attempts"], 0);
    assert!(retried["finished_at"].is_null(), "{retried}");
    assert_eq!(retried["last_error"], error, "{retried}");
    assert_eq!(retried["handled"], true);
    let row = jobs.get(id).await.unwrap().unwrap();
    assert_eq!(row.status, ferrite_domain::JobStatus::Queued);
    assert!(row.run_at.as_micros() >= before.as_micros() - 5_000_000);
    assert_eq!(
        row.last_error.as_deref(),
        Some(LEAKY),
        "the store keeps the full text"
    );
    let audit = api.store.read().audit().list(10, None).await.unwrap();
    let entry = &audit.items[0];
    assert_eq!(entry.action, "job.retry");
    assert_eq!(entry.target_kind, "job");
    assert_eq!(entry.target_id.as_deref(), Some(id.to_string().as_str()));
    assert_eq!(
        entry.details,
        json!({"id": id.to_string(), "kind": "test.fail"})
    );
    assert!(entry.actor_id.is_some());

    // Not dead any more: 422 on `status`, nothing audited; unknown: 404.
    let p = c
        .post_empty(&api, &format!("{JOBS}/{id}/retry"))
        .await
        .assert_problem(422, "validation");
    assert_eq!(p["errors"][0]["field"], "status", "{p}");
    let after = api.store.read().audit().list(10, None).await.unwrap();
    assert_eq!(after.items.len(), audit.items.len(), "refusal not audited");
    c.post_empty(&api, &format!("{JOBS}/{}/retry", JobId::new()))
        .await
        .assert_problem(404, "not-found");
    // A queued job from the start: 422 too.
    let queued = jobs
        .enqueue(&NewJob::new("test.fail", json!({"v": 1})))
        .await
        .unwrap()
        .unwrap();
    let p = c
        .post_empty(&api, &format!("{JOBS}/{queued}/retry"))
        .await
        .assert_problem(422, "validation");
    assert_eq!(p["errors"][0]["field"], "status", "{p}");
    assert!(
        c.get(&api, &format!("{JOBS}?status=dead")).await.json()["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    api
}

/// A job whose kind no live node handles is listed with `handled = false` (and can be
/// retried, waiting for a node that knows it).
async fn unknown_kind_stays_visible(api: Api) -> Api {
    let mut c = admin(&api).await;
    let jobs = api.store.read().jobs();
    let id = jobs
        .enqueue(&NewJob::new("x.unknown", json!({"v": 1})))
        .await
        .unwrap()
        .unwrap();
    let list = c.get(&api, JOBS).await.json();
    assert_eq!(item(&list, id)["handled"], false, "no node at all: {list}");
    register(&api, &["other.kind"]).await;
    // A node whose `meta.job_kinds` is not a list of strings handles nothing.
    api.store
        .read()
        .nodes()
        .register(&NewNode {
            id: NodeId::new(),
            name: "odd".into(),
            version: "0.0.9".into(),
            schema_version: 6,
            meta: json!({ "job_kinds": "x.unknown" }),
        })
        .await
        .unwrap();
    let list = c.get(&api, &format!("{JOBS}?kind=x.unknown")).await.json();
    assert_eq!(list["items"].as_array().unwrap().len(), 1, "{list}");
    let job = item(&list, id);
    assert_eq!(job["status"], "queued");
    assert_eq!(job["handled"], false, "{job}");
    // A dead unknown job is retried all the same, still unhandled.
    let dead = dead_job(jobs, "y.unknown", "no handler").await;
    let r = c.post_empty(&api, &format!("{JOBS}/{dead}/retry")).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert_eq!(r.json()["handled"], false);
    api
}

/// `status`, `kind`, `limit` and `cursor` are checked; pages follow `next_cursor`.
async fn job_list_input_is_bounded(api: Api) -> Api {
    let mut c = admin(&api).await;
    let jobs = api.store.read().jobs();
    let mut ids = Vec::new();
    for _ in 0..3 {
        ids.push(
            jobs.enqueue(&NewJob::new("page.kind", json!({"v": 1})))
                .await
                .unwrap()
                .unwrap(),
        );
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    let p1 = c
        .get(&api, &format!("{JOBS}?kind=page.kind&limit=2"))
        .await
        .json();
    let got: Vec<&Value> = p1["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|j| &j["id"])
        .collect();
    assert_eq!(
        got,
        [&json!(ids[2].to_string()), &json!(ids[1].to_string())]
    );
    let cursor = p1["next_cursor"].as_str().unwrap();
    let p2 = c
        .get(
            &api,
            &format!("{JOBS}?kind=page.kind&limit=2&cursor={cursor}"),
        )
        .await
        .json();
    assert_eq!(p2["items"][0]["id"], ids[0].to_string(), "{p2}");
    assert!(p2["next_cursor"].is_null(), "{p2}");

    for (query, field) in [
        ("status=failed", "status"),
        ("kind=Bad%20Kind", "kind"),
        ("limit=0", "limit"),
        ("limit=201", "limit"),
        ("cursor=garbage", "cursor"),
    ] {
        let p = c
            .get(&api, &format!("{JOBS}?{query}"))
            .await
            .assert_problem(400, "validation");
        assert_eq!(p["errors"][0]["field"], field, "{query}: {p}");
    }
    // An audit cursor is not a job cursor.
    let audit = c.get(&api, "/api/v1/system/audit?limit=1").await.json();
    if let Some(other) = audit["next_cursor"].as_str() {
        c.get(&api, &format!("{JOBS}?cursor={other}"))
            .await
            .assert_problem(400, "validation");
    }
    api
}

/// Installs, enables and loads plugin `name` through the API.
async fn install_enabled(c: &mut Client, api: &Api, name: &str) {
    let (manifest, wasm) = (plugin_manifest(name), plugin_wasm(name));
    let r = c
        .multipart(
            api,
            "/api/v1/plugins/inspect",
            &[("manifest", manifest.as_bytes()), ("component", &wasm)],
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    let fingerprint = r.json()["fingerprint"].as_str().unwrap().to_owned();
    let r = c
        .multipart(
            api,
            "/api/v1/plugins",
            &[
                ("manifest", manifest.as_bytes()),
                ("component", &wasm),
                ("approve", fingerprint.as_bytes()),
            ],
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{r:?}");
    let r = c
        .post_empty(api, &format!("/api/v1/plugins/{name}/enable"))
        .await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    api.reloader.reload().await.unwrap();
}

/// `flapper` fails every `render.head`: the shared buckets (as other nodes' flushers
/// wrote them) appear summed per hour in `failures`, and this node's calls in its
/// hook stats.
async fn plugin_metrics_shape(api: Api) -> Api {
    let mut c = admin(&api).await;
    install_enabled(&mut c, &api, "flapper").await;
    let r = api.get("/").await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");

    const HOUR: i64 = 3_600_000_000;
    let hour = Timestamp::now().as_micros() / HOUR * HOUR;
    let at = |m: i64| Timestamp::from_micros(m).unwrap();
    let failures = api.store.read().plugin_failures();
    failures
        .add("flapper", "render.head", at(hour), 2)
        .await
        .unwrap();
    failures
        .add("flapper", "render.head", at(hour + 60_000_000), 3)
        .await
        .unwrap();
    failures
        .add("flapper", "render.head", at(hour - 2 * HOUR), 4)
        .await
        .unwrap();
    // Outside the 24 h window.
    failures
        .add("flapper", "render.head", at(hour - 30 * HOUR), 9)
        .await
        .unwrap();

    let r = c.get(&api, "/api/v1/plugins/flapper/metrics").await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert_eq!(r.header("cache-control"), Some("no-store"));
    let m = r.json();
    assert_eq!(m["window_secs"], 86_400, "{m}");
    let rows: Vec<(&str, i64)> = m["failures"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| (b["hook"].as_str().unwrap(), b["count"].as_i64().unwrap()))
        .collect();
    assert_eq!(rows, [("render.head", 5), ("render.head", 4)], "{m}");
    let first =
        chrono::DateTime::parse_from_rfc3339(m["failures"][0]["bucket_start"].as_str().unwrap())
            .unwrap();
    assert_eq!(first.timestamp_micros(), hour, "{m}");
    assert_eq!(
        m["node"]["id"],
        api.api.node.id.as_uuid().to_string(),
        "{m}"
    );
    assert!(m["node"]["since"].is_string(), "{m}");
    let hooks = m["node"]["hooks"].as_array().unwrap();
    assert_eq!(hooks.len(), 1, "{m}");
    let head = &hooks[0];
    assert_eq!(head["hook"], "render.head");
    assert!(head["calls"].as_u64().unwrap() >= 1, "{m}");
    assert_eq!(head["calls"], head["failures"], "flapper always fails: {m}");
    assert_eq!((&head["busy"], &head["budget_cut"]), (&json!(0), &json!(0)));
    let buckets: u64 = head["buckets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b.as_u64().unwrap())
        .sum();
    assert_eq!(json!(buckets), head["calls"], "{m}");
    assert!(head["max_micros"].as_u64() <= head["total_micros"].as_u64());

    // Another installed plugin's metrics are empty; an unknown one is 404.
    install_enabled(&mut c, &api, "hello").await;
    let m = c.get(&api, "/api/v1/plugins/hello/metrics").await.json();
    assert_eq!(m["failures"], json!([]), "{m}");
    assert_eq!(m["node"]["hooks"], json!([]), "not called yet: {m}");
    c.get(&api, "/api/v1/plugins/nope/metrics")
        .await
        .assert_problem(404, "not-found");
    api
}

/// The snapshot counts the page cache's hits and misses, renders, plugin totals and
/// the job counts.
async fn metrics_summary_counts_cache_hits(api: Api) -> Api {
    let mut c = admin(&api).await;
    let s0 = c.get(&api, SUMMARY).await;
    assert_eq!(s0.status, StatusCode::OK, "{s0:?}");
    assert_eq!(s0.header("cache-control"), Some("no-store"));
    let s0 = s0.json();
    for path in ["/", "/", "/"] {
        assert_eq!(api.get(path).await.status, StatusCode::OK);
    }
    api.store
        .read()
        .jobs()
        .enqueue(&NewJob::new("sum.kind", json!({"v": 1})))
        .await
        .unwrap();
    let s = c.get(&api, SUMMARY).await.json();
    let delta = |path: &str| {
        let ptr = |v: &Value| v.pointer(path).and_then(Value::as_u64).unwrap();
        ptr(&s) - ptr(&s0)
    };
    assert_eq!(delta("/page_cache/misses"), 1, "{s0} -> {s}");
    assert_eq!(delta("/page_cache/hits"), 2, "{s0} -> {s}");
    assert_eq!(delta("/renders"), 1, "{s0} -> {s}");
    assert!(s["page_cache"]["entries"].as_u64().unwrap() >= 1, "{s}");
    assert!(s["page_cache"]["bytes"].as_u64().unwrap() > 0, "{s}");
    assert_eq!(delta("/jobs/queued"), 1, "{s}");
    for status in ["running", "done", "dead"] {
        assert!(s["jobs"][status].is_u64(), "{s}");
    }
    assert_eq!(s["plugins"], json!({"calls": 0, "failures": 0}), "{s}");
    assert_eq!(
        s["node"]["id"],
        api.api.node.id.as_uuid().to_string(),
        "{s}"
    );
    assert!(s["node"]["started_at"].is_string(), "{s}");
    api
}

/// Editors and tokens without the scope get `403` on every `/system` route and the
/// plugin metrics; nothing is retried.
async fn system_routes_refuse_editors(api: Api) -> Api {
    let jobs = api.store.read().jobs();
    let id = dead_job(jobs, "test.fail", "boom").await;
    api.user("ed", Role::Editor, false).await;
    let mut ed = api.logged_in("ed").await;
    let root = api.user("root", Role::Admin, false).await;
    let reader = api.bearer(&root, &["system:read"]).await;
    let mut c = admin(&api).await;
    install_enabled(&mut c, &api, "hello").await;
    for path in [
        "/api/v1/system/status",
        "/api/v1/system/nodes",
        "/api/v1/system/audit",
        JOBS,
        SUMMARY,
        "/api/v1/plugins/hello/metrics",
    ] {
        let p = ed.get(&api, path).await.assert_problem(403, "forbidden");
        assert!(p["detail"].is_string(), "{path}: {p}");
    }
    let retry = format!("{JOBS}/{id}/retry");
    let p = ed
        .post_empty(&api, &retry)
        .await
        .assert_problem(403, "forbidden");
    assert!(
        p["detail"].as_str().unwrap().contains("manage_system"),
        "{p}"
    );
    // `system:read` lists but cannot retry.
    let r = reader.get(&api, JOBS).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    let p = reader
        .send(&api, axum::http::Method::POST, &retry, None, None)
        .await
        .assert_problem(403, "forbidden");
    assert!(
        p["detail"].as_str().unwrap().contains("system:write"),
        "{p}"
    );
    assert_eq!(
        jobs.get(id).await.unwrap().unwrap().status,
        ferrite_domain::JobStatus::Dead,
        "nothing retried"
    );
    api
}
