//! `/api/v1/import` (M3-2 Task 5, P23, P25, P26-1): starting a WordPress import from an
//! upload, the job and run reads, and the report files.

use axum::{
    body::Body,
    http::{Method, header},
};
use bytes::Bytes;
use ferrite_core::{
    Actor,
    jobs::{IMPORT_WORDPRESS_KIND, ImportJobPayload},
};
use ferrite_domain::{
    ArtifactHash, ArtifactKind, ImportRun, ImportRunId, ImportRunState, JobId, JobQuery, Role,
    Timestamp, User, to_hex,
};
use ferrite_http::api::{
    idempotency::{IDEMPOTENCY_HEADER, REPLAYED_HEADER},
    import::IMPORT_ROUTE_LIMIT,
};
use ferrite_http::media::Uploader;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{
    api_tests,
    harness::{
        Api, Client, MAX_UPLOAD, Part, Resp, gated_body, multipart_body, part_head,
        upload_content_type,
    },
};

api_tests!(
    start_import_returns_202_with_location,
    start_import_replays_with_idempotency_key,
    start_import_conflicts_while_one_is_queued,
    start_import_413_over_cap_before_storing,
    start_import_400_on_bad_options,
    import_routes_need_import_capability,
    job_status_then_run_then_report_json_and_html,
    runs_page_cursor,
    start_import_precheck_refuses_before_reading,
    start_import_dropped_request_leaves_no_blob,
    start_import_idle_part_is_aborted,
    start_import_422_on_bad_bases,
);

const START: &str = "/api/v1/import/wordpress";
const RUNS: &str = "/api/v1/import/runs";
const SOURCE: &str = "https://wp.example";

/// A small WXR, distinct per `tag`.
fn wxr(tag: &str) -> Vec<u8> {
    format!(
        "<?xml version=\"1.0\"?><rss version=\"2.0\"><channel><title>{tag}</title></channel></rss>"
    )
    .into_bytes()
}

/// Bytes standing for an uploads zip (the API stores the part as it is).
fn zip(tag: &str) -> Vec<u8> {
    let mut v = b"PK\x03\x04".to_vec();
    v.extend_from_slice(tag.as_bytes());
    v
}

fn sha(bytes: &[u8]) -> String {
    to_hex(&Sha256::digest(bytes))
}

async fn admin(api: &Api) -> (User, Client) {
    let u = api.user("admin", Role::Admin, false).await;
    (u, api.logged_in("admin").await)
}

/// `POST /import/wordpress` with `parts`, and `Idempotency-Key: key` when given.
async fn start(api: &Api, c: &mut Client, parts: &[(&str, &[u8])], key: Option<&str>) -> Resp {
    let (content_type, body) = multipart_body(parts);
    let mut b = c
        .builder(Method::POST, START)
        .header(header::CONTENT_TYPE, content_type);
    if let Some(k) = key {
        b = b.header(IDEMPOTENCY_HEADER, k);
    }
    c.send_on(&api.router, b.body(Body::from(body)).unwrap())
        .await
}

/// The job id of an accepted start.
fn accepted(r: &Resp) -> JobId {
    assert_eq!(r.status.as_u16(), 202, "{r:?}");
    let body = r.json();
    let id: Uuid = body["job_id"].as_str().unwrap().parse().unwrap();
    assert_eq!(
        r.header("location"),
        Some(format!("/api/v1/import/jobs/{id}").as_str())
    );
    JobId::from_uuid(id)
}

async fn import_jobs(api: &Api) -> usize {
    let query = JobQuery {
        status: None,
        kind: Some(IMPORT_WORDPRESS_KIND.into()),
        limit: 100,
        cursor: None,
    };
    api.store
        .read()
        .jobs()
        .list(&query)
        .await
        .unwrap()
        .items
        .len()
}

/// The stored WXR and zip become `import` artifacts; the job carries their hashes, the
/// options (defaults filled in), the caller and the WXR's file name.
async fn start_import_returns_202_with_location(api: Api) -> Api {
    let (user, mut c) = admin(&api).await;
    let (x, z) = (wxr("one"), zip("one"));
    let r = start(
        &api,
        &mut c,
        &[
            ("wxr", &x),
            ("uploads", &z),
            ("options", br#"{"dry_run":true,"tag_base":"topic"}"#),
        ],
        None,
    )
    .await;
    let job = accepted(&r);
    let body = r.json();
    assert_eq!(body["status"], "queued");
    assert_eq!(body["attempts"], 0);
    assert_eq!(body["run"], Value::Null);

    let row = api.store.read().jobs().get(job).await.unwrap().unwrap();
    assert_eq!(row.kind, IMPORT_WORDPRESS_KIND);
    let payload: ImportJobPayload = serde_json::from_value(row.payload).unwrap();
    assert_eq!(payload.user, user.id);
    assert_eq!(payload.wxr, sha(&x));
    assert_eq!(payload.uploads.as_deref(), Some(sha(&z).as_str()));
    assert_eq!(payload.source_name, "wxr");
    assert!(payload.options.dry_run && payload.options.keep_permalinks);
    assert_eq!(payload.options.tag_base, "topic");
    for (bytes, len) in [(&x, x.len()), (&z, z.len())] {
        let hash = ArtifactHash::parse(&sha(bytes)).unwrap();
        let a = api
            .store
            .read()
            .artifacts()
            .get(&hash)
            .await
            .unwrap()
            .unwrap();
        assert_eq!((a.kind, a.bytes), (ArtifactKind::Import, len as i64));
        assert_eq!(
            api.api.media.get(&hash).await.unwrap(),
            Bytes::from(bytes.clone())
        );
    }

    // Polling the Location answers the same job.
    let r = c.get(&api, &format!("/api/v1/import/jobs/{job}")).await;
    assert_eq!(r.status.as_u16(), 200, "{r:?}");
    assert_eq!(r.json()["job_id"], job.to_string());
    api
}

/// A retry with the same `Idempotency-Key` replays the `202`; one job exists.
async fn start_import_replays_with_idempotency_key(api: Api) -> Api {
    let (_, mut c) = admin(&api).await;
    let x = wxr("idem");
    let first = start(&api, &mut c, &[("wxr", &x)], Some("import-1")).await;
    let job = accepted(&first);
    let again = start(&api, &mut c, &[("wxr", &x)], Some("import-1")).await;
    assert_eq!(accepted(&again), job);
    assert_eq!(again.header(REPLAYED_HEADER), Some("true"));
    assert_eq!(again.json(), first.json());
    assert_eq!(import_jobs(&api).await, 1);
    api
}

/// P25: a second start while the first job is queued is `409 conflict`, and the files
/// it stored are removed again.
async fn start_import_conflicts_while_one_is_queued(api: Api) -> Api {
    let (_, mut c) = admin(&api).await;
    accepted(&start(&api, &mut c, &[("wxr", &wxr("first"))], None).await);
    let files = api.stored_files();
    let r = start(
        &api,
        &mut c,
        &[("wxr", &wxr("second")), ("uploads", &zip("second"))],
        None,
    )
    .await;
    r.assert_problem(409, "conflict");
    assert_eq!(api.stored_files(), files);
    assert_eq!(import_jobs(&api).await, 1);
    let hash = ArtifactHash::parse(&sha(&wxr("second"))).unwrap();
    assert_eq!(api.store.read().artifacts().get(&hash).await.unwrap(), None);
    api
}

/// A body declared over the route limit is `413` before anything is read or stored.
async fn start_import_413_over_cap_before_storing(api: Api) -> Api {
    let (_, c) = admin(&api).await;
    let (content_type, body) = multipart_body(&[("wxr", &wxr("big"))]);
    let req = c
        .builder(Method::POST, START)
        .header(header::CONTENT_TYPE, content_type)
        .header(
            header::CONTENT_LENGTH,
            (IMPORT_ROUTE_LIMIT as u64 + 1).to_string(),
        )
        .body(Body::from(body))
        .unwrap();
    api.send(req).await.assert_problem(413, "too-large");
    assert_eq!(api.stored_files(), Vec::<std::path::PathBuf>::new());
    assert_eq!(import_jobs(&api).await, 0);
    api
}

/// A malformed `options` part is `400` (unknown field, not JSON), one over 4 KiB is
/// `413`, no `wxr` part is `422`; a WXR stored before the bad part is removed again.
async fn start_import_400_on_bad_options(api: Api) -> Api {
    let (_, mut c) = admin(&api).await;
    let x = wxr("opts");
    for options in [
        &br#"{"takeover":true}"#[..],
        b"not json",
        br#"{"dry_run":"yes"}"#,
    ] {
        let r = start(&api, &mut c, &[("wxr", &x), ("options", options)], None).await;
        let body = r.assert_problem(400, "malformed-body");
        assert!(
            body["detail"].as_str().unwrap().contains("options"),
            "{body}"
        );
    }
    let big = format!(r#"{{"category_base":"{}"}}"#, "c".repeat(5000));
    let r = start(
        &api,
        &mut c,
        &[("wxr", &x), ("options", big.as_bytes())],
        None,
    )
    .await;
    r.assert_problem(413, "too-large");
    let r = start(&api, &mut c, &[("options", b"{}")], None).await;
    r.assert_problem(422, "validation");
    let r = start(&api, &mut c, &[("wxr", &x), ("extra", b"1")], None).await;
    r.assert_problem(400, "malformed-body");
    assert_eq!(api.stored_files(), Vec::<std::path::PathBuf>::new());
    assert_eq!(import_jobs(&api).await, 0);
    api
}

/// Every import route needs `import`: an editor and a token without `import:*` get
/// `403`; `import:read` reads but does not start.
async fn import_routes_need_import_capability(api: Api) -> Api {
    let (admin_user, mut admin_client) = admin(&api).await;
    let job = accepted(&start(&api, &mut admin_client, &[("wxr", &wxr("cap"))], None).await);
    let reads = [
        format!("/api/v1/import/jobs/{job}"),
        RUNS.to_owned(),
        format!("{RUNS}/{}", Uuid::now_v7()),
        format!("{RUNS}/{}/report.json", Uuid::now_v7()),
        format!("{RUNS}/{}/report.html", Uuid::now_v7()),
    ];

    api.user("ed", Role::Editor, false).await;
    let mut editor = api.logged_in("ed").await;
    start(&api, &mut editor, &[("wxr", &wxr("ed"))], None)
        .await
        .assert_problem(403, "forbidden");
    for path in &reads {
        editor
            .get(&api, path)
            .await
            .assert_problem(403, "forbidden");
    }

    let content_only = api
        .bearer(&admin_user, &["content:read", "content:write"])
        .await;
    let reader = api.bearer(&admin_user, &["import:read"]).await;
    for who in [&content_only, &reader] {
        let (content_type, body) = multipart_body(&[("wxr", &wxr("tok"))]);
        let req = who
            .builder(Method::POST, START)
            .header(header::CONTENT_TYPE, content_type)
            .body(Body::from(body))
            .unwrap();
        api.send(req).await.assert_problem(403, "forbidden");
    }
    for path in &reads {
        content_only
            .get(&api, path)
            .await
            .assert_problem(403, "forbidden");
    }
    let r = reader.get(&api, &reads[0]).await;
    assert_eq!(r.status.as_u16(), 200, "{r:?}");
    let r = reader.get(&api, RUNS).await;
    assert_eq!(r.status.as_u16(), 200, "{r:?}");
    assert_eq!(import_jobs(&api).await, 1);
    api
}

/// The job answers without a run, then with it once the job started one; the run and
/// both report files read back, the HTML sandboxed.
async fn job_status_then_run_then_report_json_and_html(api: Api) -> Api {
    let (user, mut c) = admin(&api).await;
    let job = accepted(&start(&api, &mut c, &[("wxr", &wxr("rep"))], None).await);
    let actor = Actor::new(user.id, Role::Admin);
    let handle = api
        .api
        .import
        .start(&actor, SOURCE, false, false, Some(job))
        .await
        .unwrap();
    handle.set_step(&api.api.import, "pass2").await.unwrap();
    let run = handle.run().id;
    let r = c.get(&api, &format!("/api/v1/import/jobs/{job}")).await;
    assert_eq!(r.status.as_u16(), 200, "{r:?}");
    let body = r.json();
    assert_eq!(body["run"]["id"], run.to_string());
    assert_eq!(body["run"]["state"], "running");
    assert_eq!(body["run"]["step"], "pass2");
    assert_eq!(body["run"]["report"], false);
    assert_eq!(body["run"]["job_id"], job.to_string());
    assert!(body["run"].get("token").is_none(), "{body}");

    let json_report = br#"{"source":"https://wp.example"}"#;
    let html_report = b"<!doctype html><title>Report</title><p>done</p>";
    api.api
        .import
        .store_report(
            run,
            Bytes::from_static(json_report),
            Bytes::from_static(html_report),
        )
        .await
        .unwrap();
    handle
        .finish(
            &api.api.import,
            ImportRunState::Failed,
            json!({"posts": {"created": 2, "updated": 1, "unchanged": 0, "kept": 0,
                             "skipped": 0, "failed": 0},
                   "error": "the WXR is malformed"}),
        )
        .await
        .unwrap();

    let r = c.get(&api, &format!("{RUNS}/{run}")).await;
    assert_eq!(r.status.as_u16(), 200, "{r:?}");
    let body = r.json();
    assert_eq!(body["state"], "failed");
    assert_eq!(body["report"], true);
    assert_eq!(body["source"], SOURCE);
    assert_eq!(body["counts"]["posts"]["created"], 2);
    assert_eq!(body["counts"]["posts"]["updated"], 1);
    assert_eq!(body["error"], "the WXR is malformed");
    assert!(body["finished_at"].is_string());

    let r = c.get(&api, &format!("{RUNS}/{run}/report.json")).await;
    assert_eq!(r.status.as_u16(), 200, "{r:?}");
    assert_eq!(r.header("content-type"), Some("application/json"));
    assert_eq!(r.header("cache-control"), Some("private, no-store"));
    assert_eq!(r.body.as_bytes(), json_report);

    let r = c.get(&api, &format!("{RUNS}/{run}/report.html")).await;
    assert_eq!(r.status.as_u16(), 200, "{r:?}");
    assert_eq!(r.header("content-type"), Some("text/html; charset=utf-8"));
    assert_eq!(
        r.header("content-security-policy"),
        Some("default-src 'none'; style-src 'unsafe-inline'; sandbox")
    );
    assert_eq!(r.header("x-content-type-options"), Some("nosniff"));
    assert_eq!(r.header("cache-control"), Some("private, no-store"));
    assert_eq!(r.body.as_bytes(), html_report);

    // Unknown run, run without a report, malformed id: all `404`.
    let missing = Uuid::now_v7();
    c.get(&api, &format!("{RUNS}/{missing}"))
        .await
        .assert_problem(404, "not-found");
    c.get(&api, &format!("{RUNS}/{missing}/report.html"))
        .await
        .assert_problem(404, "not-found");
    c.get(&api, &format!("/api/v1/import/jobs/{missing}"))
        .await
        .assert_problem(404, "not-found");
    c.get(&api, &format!("{RUNS}/not-a-uuid"))
        .await
        .assert_problem(404, "not-found");
    api
}

/// `GET /import/runs` pages newest first with `next_cursor`; a cursor of another list
/// is `400`.
async fn runs_page_cursor(api: Api) -> Api {
    let (user, mut c) = admin(&api).await;
    let actor = Actor::new(user.id, Role::Admin);
    let mut ids = Vec::new();
    for _ in 0..3 {
        let run = api
            .api
            .import
            .start(&actor, SOURCE, true, false, None)
            .await
            .unwrap();
        ids.push(run.run().id.to_string());
        run.finish(&api.api.import, ImportRunState::Done, json!({}))
            .await
            .unwrap();
    }
    ids.reverse();
    let mut seen = Vec::new();
    let mut path = format!("{RUNS}?limit=2");
    loop {
        let r = c.get(&api, &path).await;
        assert_eq!(r.status.as_u16(), 200, "{r:?}");
        let body = r.json();
        for item in body["items"].as_array().unwrap() {
            seen.push(item["id"].as_str().unwrap().to_owned());
        }
        match body["next_cursor"].as_str() {
            Some(cursor) => path = format!("{RUNS}?limit=2&cursor={cursor}"),
            None => break,
        }
    }
    assert_eq!(seen, ids);
    let r = c.get(&api, &format!("{RUNS}?cursor=bogus")).await;
    r.assert_problem(400, "validation");
    let r = c.get(&api, &format!("{RUNS}?limit=0")).await;
    r.assert_problem(400, "validation");
    api
}

/// The first part complete, then the head of `next` and a few of its bytes; the rest
/// waits for the returned gate (keep it alive to hold the request in its body).
fn stalled(
    c: &Client,
    first: (&str, &[u8]),
    next: &str,
) -> (axum::http::Request<Body>, tokio::sync::oneshot::Sender<()>) {
    let mut sent = part_head(&Part {
        name: first.0,
        filename: Some("export.xml"),
        content_type: Some("application/xml"),
        bytes: &[],
    });
    sent.extend_from_slice(first.1);
    sent.extend_from_slice(b"\r\n");
    sent.extend_from_slice(&part_head(&Part {
        name: next,
        filename: Some("next.bin"),
        content_type: Some("application/octet-stream"),
        bytes: &[],
    }));
    sent.extend_from_slice(b"PK\x03\x04 partial");
    let (body, gate) = gated_body(sent, b"rest".to_vec());
    let req = c
        .builder(Method::POST, START)
        .header(header::CONTENT_TYPE, upload_content_type())
        .body(body)
        .unwrap();
    (req, gate)
}

/// Waits up to 5 s for the media store to hold no files (a spawned cleanup).
async fn eventually_empty(api: &Api) {
    for _ in 0..100 {
        if api.stored_files().is_empty() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("files left behind: {:?}", api.stored_files());
}

/// A running run written straight to the store, its heartbeat `secs` ago.
async fn running_run(api: &Api, by: &User, secs: i64) -> ImportRunId {
    let at = Timestamp::from(Timestamp::now().as_datetime() - chrono::Duration::seconds(secs));
    let run = ImportRun {
        id: ImportRunId::new(),
        source: SOURCE.into(),
        token: [9; 16],
        state: ImportRunState::Running,
        started_by: by.id,
        started_at: at,
        heartbeat_at: at,
        finished_at: None,
        stats: json!({}),
        dry_run: false,
        job_id: None,
        step: None,
        report_hash: None,
    };
    api.store.read().imports().create_run(&run).await.unwrap();
    run.id
}

/// While a job is queued, or a run is live, a start is `409` before its body is read
/// (the body here never ends); a stale run does not refuse it.
async fn start_import_precheck_refuses_before_reading(api: Api) -> Api {
    let (user, mut c) = admin(&api).await;
    let job = accepted(&start(&api, &mut c, &[("wxr", &wxr("queued"))], None).await);
    let files = api.stored_files();
    let (req, _gate) = stalled(&c, ("wxr", &wxr("second")), "uploads");
    let r = tokio::time::timeout(std::time::Duration::from_secs(5), api.send(req))
        .await
        .expect("answered before the body ended");
    r.assert_problem(409, "conflict");
    assert_eq!(api.stored_files(), files);

    // The job is done; a live run refuses too, a stale one does not.
    let jobs = api.store.read().jobs();
    let claimed = jobs
        .claim(
            ferrite_domain::NodeId::new(),
            &[IMPORT_WORDPRESS_KIND.to_owned()],
            1,
            std::time::Duration::from_secs(60),
        )
        .await
        .unwrap();
    assert_eq!(claimed[0].id, job);
    assert!(jobs.complete(&claimed[0].lease().unwrap()).await.unwrap());
    let live = running_run(&api, &user, 5).await;
    let (req, _gate) = stalled(&c, ("wxr", &wxr("third")), "uploads");
    let r = tokio::time::timeout(std::time::Duration::from_secs(5), api.send(req))
        .await
        .expect("answered before the body ended");
    r.assert_problem(409, "conflict");
    api.store
        .read()
        .imports()
        .set_state(live, ImportRunState::Failed, &json!({}), Timestamp::now())
        .await
        .unwrap();
    let stale = running_run(&api, &user, 300).await;
    accepted(&start(&api, &mut c, &[("wxr", &wxr("fourth"))], None).await);
    let run = api
        .store
        .read()
        .imports()
        .get_run(stale)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(run.state, ImportRunState::Abandoned);
    api
}

/// A client that goes away while `uploads` streams (the request future is dropped)
/// leaves no stored `wxr` behind.
async fn start_import_dropped_request_leaves_no_blob(api: Api) -> Api {
    let (_, c) = admin(&api).await;
    let big = {
        let mut v = wxr("dropped");
        v.resize(64 * 1024, b' ');
        v
    };
    let (req, gate) = stalled(&c, ("wxr", &big), "uploads");
    let dropped = tokio::time::timeout(std::time::Duration::from_millis(500), api.send(req)).await;
    assert!(
        dropped.is_err(),
        "the request should still be reading its body"
    );
    drop(gate);
    eventually_empty(&api).await;
    assert_eq!(import_jobs(&api).await, 0);
    api
}

/// A part that receives nothing for `media.upload_idle_timeout` is aborted like a media
/// upload (`400`, the part named), and what was stored is removed.
async fn start_import_idle_part_is_aborted(api: Api) -> Api {
    let (_, c) = admin(&api).await;
    let uploader = Uploader::new(api.api.media.clone(), MAX_UPLOAD)
        .with_idle_timeout(std::time::Duration::from_millis(300));
    let (router, _) = api.router_with_uploader(uploader);
    let (req, _gate) = stalled(&c, ("wxr", &wxr("idle")), "uploads");
    let r = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        crate::harness::send(&router, req),
    )
    .await
    .expect("aborted by the idle timeout");
    let body = r.assert_problem(400, "malformed-body");
    assert_eq!(body["errors"][0]["field"], "uploads", "{body}");
    eventually_empty(&api).await;
    assert_eq!(import_jobs(&api).await, 0);
    api
}

/// An empty, `/`-only or otherwise unusable category or tag base is `422` (core), and the
/// stored WXR is removed.
async fn start_import_422_on_bad_bases(api: Api) -> Api {
    let (_, mut c) = admin(&api).await;
    let x = wxr("bases");
    for (options, field) in [
        (r#"{"category_base":""}"#, "category_base"),
        (r#"{"category_base":"/"}"#, "category_base"),
        (r#"{"tag_base":"a//b"}"#, "tag_base"),
        (r#"{"tag_base":"../up"}"#, "tag_base"),
        (r#"{"tag_base":"has space"}"#, "tag_base"),
    ] {
        let r = start(
            &api,
            &mut c,
            &[("wxr", &x), ("options", options.as_bytes())],
            None,
        )
        .await;
        let body = r.assert_problem(422, "validation");
        assert_eq!(body["errors"][0]["field"], field, "{options}: {body}");
    }
    let ok = r#"{"category_base":"/topics/","tag_base":"blog/tags"}"#;
    accepted(
        &start(
            &api,
            &mut c,
            &[("wxr", &x), ("options", ok.as_bytes())],
            None,
        )
        .await,
    );
    api
}
