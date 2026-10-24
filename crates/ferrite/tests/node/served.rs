//! Batched `request.served` on a real node (M2a-2 Task 12, P21): page responses reach
//! the `counter` example through `plugin.action` jobs and the runner, and the final
//! flush at shutdown leaves the last batch queued for the next runner.

use super::*;

/// One runner worker: `counter` adds to its KV count read-modify-write, so two of its
/// jobs running at once could lose an update.
fn one_worker(db: &TestDb) -> Site {
    let mut site = Site::new(db);
    site.config.cluster.workers = 1;
    site
}

async fn start_with_counter(site: &Site, db: &TestDb, flush: Duration) -> RunningNode {
    let opts = NodeOptions {
        periodics: Some(Vec::new()),
        served_flush: Some(flush),
        ..NodeOptions::default()
    };
    let node = site.start(db, opts).await;
    let store = db.shared_store();
    install_plugin_file(
        &store,
        &node.state.media,
        &site.config.plugins,
        &manifest("counter"),
        &test_support::plugin_wasm("counter"),
        true,
    )
    .await
    .unwrap();
    // No request before the plugin is loaded: those events would have no subscriber.
    eventually(PLUGIN_LOAD, "counter loaded", || async {
        node.state.plugins.is_enabled("counter")
    })
    .await;
    node
}

async fn count(store: &dyn Store) -> Option<String> {
    store
        .read()
        .plugin_kv()
        .get("counter", "count")
        .await
        .unwrap()
        .map(|v| String::from_utf8(v).unwrap())
}

/// Every page response (hits, misses, 404s) is counted once by `counter`.
async fn served_counts_through_the_runner(db: Arc<TestDb>) {
    let site = one_worker(&db);
    let node = start_with_counter(&site, &db, Duration::from_millis(200)).await;
    let store = db.shared_store();
    const REQUESTS: usize = 7;
    for i in 0..REQUESTS {
        let path = if i % 2 == 0 { "/" } else { "/missing/" };
        get(node.addr, path).await;
    }
    // Not page responses: not counted.
    get(node.addr, "/healthz").await;
    get(node.addr, "/_p/counter/x").await;
    eventually(Duration::from_secs(10), "counter's count", || {
        let store = store.clone();
        async move { count(store.as_ref()).await == Some(REQUESTS.to_string()) }
    })
    .await;
    eventually(Duration::from_secs(2), "action jobs done", || {
        let store = store.clone();
        async move {
            let c = counts(store.as_ref()).await;
            c[&JobStatus::Queued] == 0 && c[&JobStatus::Running] == 0
        }
    })
    .await;
    let c = counts(store.as_ref()).await;
    assert_eq!(c[&JobStatus::Dead], 0, "{c:?}");
    // Fewer jobs than requests: batched.
    assert!(c[&JobStatus::Done] < REQUESTS as u64, "{c:?}");
    assert_eq!(count(store.as_ref()).await, Some(REQUESTS.to_string()));
    node.shutdown().await.unwrap();
}

/// With a flush interval longer than the test, the shutdown's final flush enqueues the
/// batch (one job); the next node's runner delivers it.
async fn shutdown_flushes_the_buffer(db: Arc<TestDb>) {
    let site = one_worker(&db);
    let node = start_with_counter(&site, &db, Duration::from_secs(3600)).await;
    let store = db.shared_store();
    for _ in 0..3 {
        get(node.addr, "/").await;
    }
    assert_eq!(counts(store.as_ref()).await[&JobStatus::Queued], 0);
    let started = Instant::now();
    node.shutdown().await.unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(13),
        "within the bound"
    );
    let c = counts(store.as_ref()).await;
    assert_eq!(c[&JobStatus::Queued], 1, "the final batch: {c:?}");
    assert_eq!(count(store.as_ref()).await, None);

    let opts = NodeOptions {
        periodics: Some(Vec::new()),
        ..NodeOptions::default()
    };
    let node = site.start(&db, opts).await;
    eventually(
        Duration::from_secs(20),
        "delivered by the next node",
        || {
            let store = store.clone();
            async move { count(store.as_ref()).await == Some("3".to_owned()) }
        },
    )
    .await;
    node.shutdown().await.unwrap();
}

backend_tests_spawning!(
    served_counts_through_the_runner,
    shutdown_flushes_the_buffer
);
