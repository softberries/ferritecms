//! Node registry: join validation (P15), solo wait, heartbeat and leave on both backends
//! (04 §3, P14). Timings are shortened with `join_with` so the tests run in about a second.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{sync::Arc, time::Duration};

use ferrite_cluster::{ClusterError, ClusterMode, JoinTimings, NodeIdentity, Registry};
use ferrite_domain::{NewNode, Node, NodeId};
use ferrite_store::Store;
use ferrite_store_tests::{TestDb, backend_tests_spawning};
use serde_json::{Value, json};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

const URL: &str = "https://example.com/";

fn identity(mode: ClusterMode) -> NodeIdentity {
    NodeIdentity {
        id: NodeId::new(),
        name: "me".into(),
        version: semver::Version::new(0, 1, 0),
        schema_version: 4,
        mode,
        site_url: URL.into(),
    }
}

/// 1/50 of the production values (30 s stale, 35 s solo wait).
fn timings() -> JoinTimings {
    JoinTimings {
        stale_after: Duration::from_millis(600),
        solo_wait: Duration::from_millis(700),
    }
}

async fn site_id(store: &dyn Store) -> String {
    store
        .read()
        .schema_meta()
        .get("site_id")
        .await
        .unwrap()
        .expect("site_id written by migrations")
}

/// Registers another node directly through `NodeRepo` (as another process would).
async fn register_other(store: &dyn Store, name: &str, meta: Value) -> NodeId {
    let id = NodeId::new();
    store
        .read()
        .nodes()
        .register(&NewNode {
            id,
            name: name.into(),
            version: "0.1.0".into(),
            schema_version: 4,
            meta,
        })
        .await
        .unwrap();
    id
}

async fn row(store: &dyn Store, id: NodeId) -> Option<Node> {
    store
        .read()
        .nodes()
        .list()
        .await
        .unwrap()
        .into_iter()
        .find(|n| n.id == id)
}

async fn join_refuses_other_site_id(db: Arc<TestDb>) {
    let store = db.shared_store();
    register_other(
        store.as_ref(),
        "other-node",
        json!({"site_id": "another-site", "site_url": URL, "mode": "cluster"}),
    )
    .await;
    for mode in [ClusterMode::Cluster, ClusterMode::Solo] {
        let me = identity(mode);
        let id = me.id;
        let e = Registry::join(store.clone(), me)
            .await
            .err()
            .expect("refused");
        assert!(matches!(e, ClusterError::SiteMismatch { .. }), "{e:?}");
        let msg = e.to_string();
        assert!(
            msg.contains("other-node") && msg.contains("another-site"),
            "{msg}"
        );
        assert!(row(store.as_ref(), id).await.is_none(), "not registered");
    }
}

async fn join_refuses_other_site_url(db: Arc<TestDb>) {
    let store = db.shared_store();
    let site = site_id(store.as_ref()).await;
    register_other(
        store.as_ref(),
        "other-node",
        json!({"site_id": site, "site_url": "https://other.example/", "mode": "cluster"}),
    )
    .await;
    let me = identity(ClusterMode::Cluster);
    let id = me.id;
    let e = Registry::join(store.clone(), me)
        .await
        .err()
        .expect("refused");
    assert!(matches!(e, ClusterError::SiteUrlMismatch { .. }), "{e:?}");
    let msg = e.to_string();
    assert!(
        msg.contains("other-node") && msg.contains("https://other.example/"),
        "{msg}"
    );
    assert!(row(store.as_ref(), id).await.is_none(), "not registered");
}

async fn join_ignores_stale_nodes(db: Arc<TestDb>) {
    let store = db.shared_store();
    register_other(
        store.as_ref(),
        "crashed",
        json!({"site_id": "another-site", "site_url": "https://other.example/", "mode": "solo"}),
    )
    .await;
    // 31 s of 30 s, scaled.
    tokio::time::sleep(timings().stale_after + Duration::from_millis(20)).await;
    for mode in [ClusterMode::Solo, ClusterMode::Cluster] {
        let me = identity(mode);
        let id = me.id;
        let start = Instant::now();
        let reg = Registry::join_with(store.clone(), me, timings())
            .await
            .unwrap();
        assert!(start.elapsed() < Duration::from_millis(300), "no waiting");
        let mine = row(store.as_ref(), id).await.expect("registered");
        assert_eq!(mine.name, "me");
        reg.leave().await.unwrap();
    }
}

async fn solo_join_waits_for_stale_node(db: Arc<TestDb>) {
    let store = db.shared_store();
    let site = site_id(store.as_ref()).await;
    let meta = json!({"site_id": site, "site_url": URL, "mode": "solo"});

    // A predecessor that stopped heartbeating 20 s ago (scaled): stale 10 s later.
    register_other(store.as_ref(), "predecessor", meta.clone()).await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    let me = identity(ClusterMode::Solo);
    let id = me.id;
    let start = Instant::now();
    let reg = Registry::join_with(store.clone(), me, timings())
        .await
        .unwrap();
    let waited = start.elapsed();
    assert!(
        (Duration::from_millis(150)..Duration::from_millis(600)).contains(&waited),
        "waited {waited:?}"
    );
    assert!(row(store.as_ref(), id).await.is_some());
    reg.leave().await.unwrap();

    // A node that keeps heartbeating is still live at the deadline.
    let live = register_other(store.as_ref(), "still-running", meta.clone()).await;
    let stop = CancellationToken::new();
    let beat = {
        let store = store.clone();
        let stop = stop.clone();
        tokio::spawn(async move {
            while !stop.is_cancelled() {
                store.read().nodes().heartbeat(live, &meta).await.unwrap();
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
    };
    let me = identity(ClusterMode::Solo);
    let id = me.id;
    let start = Instant::now();
    let e = Registry::join_with(store.clone(), me, timings())
        .await
        .err()
        .expect("refused");
    let waited = start.elapsed();
    stop.cancel();
    beat.await.unwrap();
    match &e {
        ClusterError::SoloNodeLive { name, age } => {
            assert_eq!(name, "still-running");
            assert!(*age < timings().stale_after, "{age:?}");
        }
        other => panic!("{other:?}"),
    }
    assert!(e.to_string().contains("still-running"), "{e}");
    assert!(waited >= timings().solo_wait, "gave up after {waited:?}");
    assert!(waited < timings().solo_wait + Duration::from_millis(400));
    assert!(row(store.as_ref(), id).await.is_none(), "not registered");
}

async fn cluster_join_allows_other_live_nodes(db: Arc<TestDb>) {
    let store = db.shared_store();
    let site = site_id(store.as_ref()).await;
    let other = register_other(
        store.as_ref(),
        "peer",
        json!({"site_id": site, "site_url": URL, "mode": "cluster"}),
    )
    .await;
    let me = identity(ClusterMode::Cluster);
    let id = me.id;
    let start = Instant::now();
    let reg = Registry::join(store.clone(), me).await.unwrap();
    assert!(start.elapsed() < Duration::from_secs(1));
    assert!(row(store.as_ref(), other).await.is_some());
    let mine = row(store.as_ref(), id).await.expect("registered");
    assert_eq!(mine.version, "0.1.0");
    assert_eq!(mine.schema_version, 4);
    assert_eq!(mine.meta["site_id"], json!(site));
    assert_eq!(mine.meta["site_url"], json!(URL));
    assert_eq!(mine.meta["mode"], json!("cluster"));
    assert!(mine.meta["clock_skew_ms"].is_i64(), "{}", mine.meta);
    assert_eq!(reg.id(), id);
    reg.leave().await.unwrap();
}

/// Polls until `f` holds for the node's row (or `None`), at most 5 s.
async fn wait_row(
    store: &dyn Store,
    id: NodeId,
    f: impl Fn(Option<&Node>) -> bool,
) -> Option<Node> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let r = row(store, id).await;
        if f(r.as_ref()) {
            return r;
        }
        assert!(Instant::now() < deadline, "row condition not met: {r:?}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn heartbeat_updates_and_reregisters(db: Arc<TestDb>) {
    let store = db.shared_store();
    let site = site_id(store.as_ref()).await;
    let me = identity(ClusterMode::Cluster);
    let id = me.id;
    let reg = Registry::join(store.clone(), me).await.unwrap();
    let joined = row(store.as_ref(), id).await.expect("registered");
    assert!(joined.meta.get("theme").is_none());

    let stop = CancellationToken::new();
    // The closure cannot override the registry's own keys.
    let meta: Arc<dyn Fn() -> Value + Send + Sync> =
        Arc::new(|| json!({"theme": "blog", "site_id": "spoofed"}));
    let task = reg.spawn_heartbeat(meta, stop.clone());
    reg.beat_now();
    let beat = wait_row(store.as_ref(), id, |r| {
        r.is_some_and(|n| n.meta["theme"] == json!("blog"))
    })
    .await
    .unwrap();
    assert!(beat.heartbeat_at > joined.heartbeat_at);
    assert_eq!(beat.meta["site_id"], json!(site));
    assert_eq!(beat.meta["mode"], json!("cluster"));
    assert!(beat.meta["clock_skew_ms"].is_i64());

    // Pruned (or deleted) under the running node: the next beat registers it again.
    store.read().nodes().deregister(id).await.unwrap();
    assert!(row(store.as_ref(), id).await.is_none());
    reg.beat_now();
    let back = wait_row(store.as_ref(), id, |r| r.is_some()).await.unwrap();
    assert_eq!(back.meta["theme"], json!("blog"));
    assert_eq!(back.meta["site_url"], json!(URL));
    assert_eq!(back.name, "me");

    stop.cancel();
    tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .expect("heartbeat task stops")
        .unwrap();
    reg.leave().await.unwrap();
}

async fn leave_removes_row(db: Arc<TestDb>) {
    let store = db.shared_store();
    let me = identity(ClusterMode::Solo);
    let id = me.id;
    let reg = Registry::join(store.clone(), me).await.unwrap();
    assert!(row(store.as_ref(), id).await.is_some());
    reg.leave().await.unwrap();
    assert!(row(store.as_ref(), id).await.is_none());
    // Idempotent.
    reg.leave().await.unwrap();
}

backend_tests_spawning!(
    join_refuses_other_site_id,
    join_refuses_other_site_url,
    join_ignores_stale_nodes,
    solo_join_waits_for_stale_node,
    cluster_join_allows_other_live_nodes,
    heartbeat_updates_and_reregisters,
    leave_removes_row,
);
