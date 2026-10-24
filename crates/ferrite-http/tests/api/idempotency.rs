//! `Idempotency-Key` on `POST /media` (M2a-2b Task 5, P10, F5–F7): a finished key
//! replays the first `201` without reading the body, a pending one is `409`, a key
//! reused on another request is `422`, a failed or dropped upload frees its key, and a
//! key is scoped to (user, key).

use std::{sync::Arc, time::Duration};

use axum::{
    Router,
    body::Body,
    http::{HeaderValue, Method, Request, header},
};
use ferrite_domain::{IDEMPOTENCY_STALE, IdemBegin, IdempotencyKey, Role, User};
use ferrite_http::api::idempotency::{IDEMPOTENCY_HEADER, REPLAYED_HEADER, key_hash};
use serde_json::Value;
use tokio::sync::Barrier;

use crate::{
    api_tests,
    harness::{
        Api, Bearer, MAX_UPLOAD, Part, Resp, body_end, gated_body, part_head, send, upload_body,
        upload_content_type,
    },
};

api_tests!(
    idempotent_retry_creates_one_item,
    concurrent_duplicate_is_409,
    concurrent_duplicates_across_nodes_create_one_item,
    failed_upload_releases_the_key,
    key_scoped_to_user_and_path,
    malformed_keys,
    stale_pending_key_is_taken_over,
);

const PNG_MAGIC: &[u8] = &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

/// `len` bytes that sniff as PNG, with `tag` after the magic (distinct artifacts).
fn png(len: usize, tag: &str) -> Vec<u8> {
    let mut v = PNG_MAGIC.to_vec();
    v.extend_from_slice(b"\0\0\0\rIHDR");
    v.extend_from_slice(tag.as_bytes());
    v.resize(len.max(v.len()), 0);
    v
}

/// A PDF (no variants job), so uploads finish without background work.
fn pdf(tag: &str) -> Vec<u8> {
    let mut v = b"%PDF-1.7\n%".to_vec();
    v.extend_from_slice(tag.as_bytes());
    v.resize(2_000, b' ');
    v
}

async fn account(api: &Api, login: &str, role: Role) -> (User, Bearer) {
    let u = api.user(login, role, false).await;
    let b = api.bearer(&u, &["*"]).await;
    (u, b)
}

/// `POST path` with `key` (raw header bytes) and a `multipart/form-data` body of `parts`.
fn keyed(who: &Bearer, path: &str, key: Option<&[u8]>, parts: &[Part<'_>]) -> Request<Body> {
    let (content_type, body) = upload_body(parts);
    let mut b = who
        .builder(Method::POST, path)
        .header(header::CONTENT_TYPE, content_type);
    if let Some(k) = key {
        b = b.header(IDEMPOTENCY_HEADER, HeaderValue::from_bytes(k).unwrap());
    }
    b.body(Body::from(body)).unwrap()
}

/// A keyed upload of `bytes` as `name` whose body stops after the part head and the
/// first 100 bytes until the returned gate fires: the request holds its key meanwhile.
fn gated_upload(
    who: &Bearer,
    key: &str,
    name: &str,
    bytes: &[u8],
) -> (Request<Body>, tokio::sync::oneshot::Sender<()>) {
    let mut first = part_head(&Part::file(name, "image/png", &[]));
    first.extend_from_slice(&bytes[..100]);
    let mut rest = bytes[100..].to_vec();
    rest.extend_from_slice(b"\r\n");
    rest.extend_from_slice(&body_end());
    let (body, gate) = gated_body(first, rest);
    let req = who
        .builder(Method::POST, "/api/v1/media")
        .header(header::CONTENT_TYPE, upload_content_type())
        .header(IDEMPOTENCY_HEADER, key)
        .body(body)
        .unwrap();
    (req, gate)
}

/// Media items `who` can list (every status but trash).
async fn media_count(api: &Api, who: &Bearer) -> usize {
    let r = who.get(api, "/api/v1/media").await;
    assert_eq!(r.status.as_u16(), 200, "{r:?}");
    r.json()["items"].as_array().unwrap().len()
}

fn created(r: &Resp) -> Value {
    assert_eq!(r.status.as_u16(), 201, "{r:?}");
    assert_eq!(r.header("content-type"), Some("application/json"), "{r:?}");
    r.json()
}

/// A replay of `first`: same status, body, `Location` and `ETag`, marked replayed.
fn assert_replay_of(r: &Resp, first: &Resp) {
    assert_eq!(r.status.as_u16(), 201, "{r:?}");
    assert_eq!(r.header(REPLAYED_HEADER), Some("true"), "{r:?}");
    assert_eq!(r.json(), first.json());
    assert_eq!(r.header("location"), first.header("location"));
    assert!(r.header("location").is_some());
    assert_eq!(r.header("etag"), first.header("etag"));
    assert!(r.header("etag").is_some());
    assert_eq!(r.header("content-type"), Some("application/json"));
    assert_eq!(r.header("cache-control"), Some("no-store"));
}

async fn wait_until(what: &str, mut cond: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !cond() {
        assert!(tokio::time::Instant::now() < deadline, "timed out: {what}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Review Focus 2: a retried upload creates one item.
async fn idempotent_retry_creates_one_item(api: Api) -> Api {
    let (_, admin) = account(&api, "admin", Role::Admin).await;
    let bytes = pdf("retry");
    let parts = [
        Part::file("report.pdf", "application/pdf", &bytes),
        Part::text("title", "Report"),
    ];
    let first = api
        .send(keyed(&admin, "/api/v1/media", Some(b"retry-1"), &parts))
        .await;
    created(&first);
    assert!(first.header(REPLAYED_HEADER).is_none(), "{first:?}");

    let second = api
        .send(keyed(&admin, "/api/v1/media", Some(b"retry-1"), &parts))
        .await;
    assert_replay_of(&second, &first);
    assert_eq!(media_count(&api, &admin).await, 1);

    // The replay does not read the body: a body that would be refused (no `file`
    // part, another type) still replays.
    let r = api
        .send(keyed(
            &admin,
            "/api/v1/media",
            Some(b"retry-1"),
            &[Part::text("title", "Other")],
        ))
        .await;
    assert_replay_of(&r, &first);
    let req = admin
        .builder(Method::POST, "/api/v1/media")
        .header(header::CONTENT_TYPE, "text/plain")
        .header(IDEMPOTENCY_HEADER, "retry-1")
        .body(Body::from("not multipart"))
        .unwrap();
    assert_replay_of(&api.send(req).await, &first);

    // Without a key, the same body is a new item.
    let r = api.send(keyed(&admin, "/api/v1/media", None, &parts)).await;
    created(&r);
    assert!(r.header(REPLAYED_HEADER).is_none());
    assert_eq!(media_count(&api, &admin).await, 2);
    api
}

async fn concurrent_duplicate_is_409(api: Api) -> Api {
    let (_, admin) = account(&api, "admin", Role::Admin).await;
    let bytes = png(20_000, "held");
    let (req, gate) = gated_upload(&admin, "dup", "held.png", &bytes);
    let router = api.router.clone();
    let first = tokio::spawn(async move { send(&router, req).await });
    let uploader = api.api.uploader.clone();
    wait_until("the first upload is streaming", || {
        uploader.in_flight() == 1
    })
    .await;

    let other = png(64, "second");
    let r = api
        .send(keyed(
            &admin,
            "/api/v1/media",
            Some(b"dup"),
            &[Part::file("second.png", "image/png", &other)],
        ))
        .await;
    let p = r.assert_problem(409, "conflict");
    assert_eq!(r.header("retry-after"), Some("5"));
    assert!(p["detail"].as_str().unwrap().contains("in progress"), "{p}");
    // The duplicate never took a slot.
    assert_eq!(api.api.uploader.started(), 1);

    gate.send(()).unwrap();
    let first = first.await.unwrap();
    created(&first);
    let third = api
        .send(keyed(
            &admin,
            "/api/v1/media",
            Some(b"dup"),
            &[Part::file("second.png", "image/png", &other)],
        ))
        .await;
    assert_replay_of(&third, &first);
    assert_eq!(media_count(&api, &admin).await, 1);
    api
}

/// Four requests with one key on two nodes (a Postgres peer in cluster mode), started
/// together: exactly one uploads; the others are `409` while it holds the key.
async fn concurrent_duplicates_across_nodes_create_one_item(api: Api) -> Api {
    let (_, admin) = account(&api, "admin", Role::Admin).await;
    let peer: Router = match api.db.peer().await {
        Some(store) => api.router_on(store).await,
        None => api.router.clone(),
    };
    let routers = [api.router.clone(), peer.clone(), api.router.clone(), peer];
    let barrier = Arc::new(Barrier::new(routers.len()));
    let mut gates = Vec::new();
    let mut tasks = Vec::new();
    for (i, router) in routers.into_iter().enumerate() {
        let bytes = png(20_000, &format!("race-{i}"));
        let (req, gate) = gated_upload(&admin, "race", &format!("race-{i}.png"), &bytes);
        gates.push(gate);
        let barrier = barrier.clone();
        tasks.push(tokio::spawn(async move {
            barrier.wait().await;
            send(&router, req).await
        }));
    }
    // The losers answer while the winner's body is still gated.
    let mut done = Vec::new();
    let mut pending = Vec::new();
    for task in tasks {
        pending.push(task);
    }
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while done.len() < 3 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "losers did not answer"
        );
        let mut still = Vec::new();
        for t in pending {
            if t.is_finished() {
                done.push(t.await.unwrap());
            } else {
                still.push(t);
            }
        }
        pending = still;
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(pending.len(), 1, "one request holds the key");
    for r in &done {
        r.assert_problem(409, "conflict");
        assert_eq!(r.header("retry-after"), Some("5"));
    }
    for gate in gates {
        let _ = gate.send(());
    }
    let winner = pending.pop().unwrap().await.unwrap();
    created(&winner);
    assert_eq!(media_count(&api, &admin).await, 1);
    let r = api
        .send(keyed(
            &admin,
            "/api/v1/media",
            Some(b"race"),
            &[Part::text("title", "x")],
        ))
        .await;
    assert_replay_of(&r, &winner);
    api
}

async fn failed_upload_releases_the_key(api: Api) -> Api {
    let (_, admin) = account(&api, "admin", Role::Admin).await;
    let over = png(MAX_UPLOAD as usize + 1, "over");
    let r = api
        .send(keyed(
            &admin,
            "/api/v1/media",
            Some(b"fail-1"),
            &[Part::file("big.png", "image/png", &over)],
        ))
        .await;
    r.assert_problem(413, "too-large");
    // Refused before the body: also frees the key.
    let req = admin
        .builder(Method::POST, "/api/v1/media")
        .header(header::CONTENT_TYPE, "text/plain")
        .header(IDEMPOTENCY_HEADER, "fail-1")
        .body(Body::from("x"))
        .unwrap();
    api.send(req)
        .await
        .assert_problem(415, "unsupported-media-type");

    let ok = pdf("after-failure");
    let r = api
        .send(keyed(
            &admin,
            "/api/v1/media",
            Some(b"fail-1"),
            &[Part::file("ok.pdf", "application/pdf", &ok)],
        ))
        .await;
    created(&r);
    assert!(r.header(REPLAYED_HEADER).is_none());
    assert_eq!(media_count(&api, &admin).await, 1);

    // A request dropped mid-upload (the client went away) frees its key too.
    let bytes = png(20_000, "dropped");
    let (req, gate) = gated_upload(&admin, "drop-1", "dropped.png", &bytes);
    let router = api.router.clone();
    let task = tokio::spawn(async move { send(&router, req).await });
    let uploader = api.api.uploader.clone();
    wait_until("the upload is streaming", || uploader.in_flight() == 1).await;
    task.abort();
    let _ = task.await;
    drop(gate);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let r = api
            .send(keyed(
                &admin,
                "/api/v1/media",
                Some(b"drop-1"),
                &[Part::file("ok2.pdf", "application/pdf", &pdf("after-drop"))],
            ))
            .await;
        if r.status.as_u16() == 201 {
            assert!(r.header(REPLAYED_HEADER).is_none());
            break;
        }
        r.assert_problem(409, "conflict");
        assert!(
            tokio::time::Instant::now() < deadline,
            "the dropped request never released its key"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(media_count(&api, &admin).await, 2);
    api
}

async fn key_scoped_to_user_and_path(api: Api) -> Api {
    let (_, alice) = account(&api, "alice", Role::Admin).await;
    let (_, bob) = account(&api, "bob", Role::Editor).await;
    let a = pdf("alice");
    let first = api
        .send(keyed(
            &alice,
            "/api/v1/media",
            Some(b"shared-key"),
            &[Part::file("alice.pdf", "application/pdf", &a)],
        ))
        .await;
    let alice_item = created(&first);

    // Another user with the same key: an independent upload, never alice's response.
    let b = pdf("bob");
    let r = api
        .send(keyed(
            &bob,
            "/api/v1/media",
            Some(b"shared-key"),
            &[Part::file("bob.pdf", "application/pdf", &b)],
        ))
        .await;
    let bob_item = created(&r);
    assert!(r.header(REPLAYED_HEADER).is_none());
    assert_ne!(bob_item["id"], alice_item["id"]);
    assert_eq!(bob_item["original_name"], "bob.pdf");
    // And bob's own retry replays bob's item.
    let again = api
        .send(keyed(
            &bob,
            "/api/v1/media",
            Some(b"shared-key"),
            &[Part::text("title", "x")],
        ))
        .await;
    assert_replay_of(&again, &r);

    // The same user on another path (the query counts): 422, nothing uploaded.
    let r = api
        .send(keyed(
            &alice,
            "/api/v1/media?x=1",
            Some(b"shared-key"),
            &[Part::file("other.pdf", "application/pdf", &pdf("other"))],
        ))
        .await;
    let p = r.assert_problem(422, "validation");
    assert_eq!(p["errors"][0]["field"], "Idempotency-Key", "{p}");
    assert_eq!(media_count(&api, &alice).await, 2);
    // The stored key still replays on its own path.
    let r = api
        .send(keyed(
            &alice,
            "/api/v1/media",
            Some(b"shared-key"),
            &[Part::text("title", "x")],
        ))
        .await;
    assert_replay_of(&r, &first);
    api
}

async fn malformed_keys(api: Api) -> Api {
    let (_, admin) = account(&api, "admin", Role::Admin).await;
    let file = pdf("malformed");
    let parts = [Part::file("m.pdf", "application/pdf", &file)];
    let long = vec![b'k'; 256];
    let bad: [&[u8]; 5] = [b"", &long, b"has space", b"tab\there", &[b'a', 0xE9]];
    for key in bad {
        let r = api
            .send(keyed(&admin, "/api/v1/media", Some(key), &parts))
            .await;
        let p = r.assert_problem(400, "malformed-header");
        assert_eq!(p["errors"][0]["field"], "Idempotency-Key", "{p}");
    }
    // Sent twice.
    let (content_type, body) = upload_body(&parts);
    let req = admin
        .builder(Method::POST, "/api/v1/media")
        .header(header::CONTENT_TYPE, content_type)
        .header(IDEMPOTENCY_HEADER, "one")
        .header(IDEMPOTENCY_HEADER, "two")
        .body(Body::from(body))
        .unwrap();
    api.send(req).await.assert_problem(400, "malformed-header");
    assert_eq!(media_count(&api, &admin).await, 0);
    assert_eq!(api.api.uploader.started(), 0, "refused before the body");

    // The bounds: 255 bytes of every visible ASCII character are accepted.
    let visible: Vec<u8> = (0x21u8..=0x7E).cycle().take(255).collect();
    created(
        &api.send(keyed(&admin, "/api/v1/media", Some(&visible), &parts))
            .await,
    );
    api
}

/// Moves `user`'s idempotency rows `minutes` into the past (database side).
async fn age_keys(api: &Api, minutes: i64) {
    let sql = if api.db.is_sqlite() {
        format!(
            "UPDATE idempotency_keys SET created_at = created_at - {}",
            minutes * 60 * 1_000_000
        )
    } else {
        format!(
            "UPDATE idempotency_keys SET created_at = created_at - interval '{minutes} minutes'"
        )
    };
    api.db.exec_raw(&sql).await;
}

/// F5: a pending key older than an hour is taken over by a retry; the original owner's
/// late `finish` and `release` are then no-ops.
async fn stale_pending_key_is_taken_over(api: Api) -> Api {
    let (user, admin) = account(&api, "admin", Role::Admin).await;
    let repo = api.store.read().idempotency();
    let key = IdempotencyKey {
        user: user.id,
        key_hash: key_hash(user.id, "stale-1"),
        method: "POST".into(),
        path: "/api/v1/media".into(),
    };
    // An upload that started and never finished (a node that died mid-upload).
    let IdemBegin::Started { token: old } = repo.begin(&key, IDEMPOTENCY_STALE).await.unwrap()
    else {
        panic!("a fresh key starts");
    };
    let file = pdf("stale");
    let parts = [Part::file("s.pdf", "application/pdf", &file)];
    // Fresh: still in progress.
    api.send(keyed(&admin, "/api/v1/media", Some(b"stale-1"), &parts))
        .await
        .assert_problem(409, "conflict");

    age_keys(&api, 61).await;
    let first = api
        .send(keyed(&admin, "/api/v1/media", Some(b"stale-1"), &parts))
        .await;
    created(&first);
    assert!(first.header(REPLAYED_HEADER).is_none());

    // The original owner wakes up: its outcome is not recorded, the row is not freed.
    assert!(
        !repo
            .finish(user.id, &key.key_hash, old, 201, &serde_json::json!({}))
            .await
            .unwrap()
    );
    assert!(!repo.release(user.id, &key.key_hash, old).await.unwrap());
    let r = api
        .send(keyed(&admin, "/api/v1/media", Some(b"stale-1"), &parts))
        .await;
    assert_replay_of(&r, &first);
    assert_eq!(media_count(&api, &admin).await, 1);
    api
}
