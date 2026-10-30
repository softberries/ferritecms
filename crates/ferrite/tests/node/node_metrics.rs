//! The per-node minute writer (M4-2 P35): a running node writes its `node_metrics`
//! rows from the registry's deltas, and its shutdown flushes the partial minute.

use ferrite_domain::{MetricCounts, NodeId, SeriesStep};

use super::*;

/// `node`'s figures summed over the day around now (and the day before).
async fn totals(store: &dyn Store, node: NodeId) -> MetricCounts {
    let now = Timestamp::now().as_micros();
    let day = SeriesStep::Day.micros();
    let rows = store
        .read()
        .node_metrics()
        .series(
            Timestamp::from_micros(now - 2 * day).unwrap(),
            Timestamp::from_micros(now + day).unwrap(),
            SeriesStep::Day,
            Some(node),
        )
        .await
        .unwrap();
    let mut sum = MetricCounts::default();
    for b in &rows {
        sum.merge(&b.counts);
    }
    sum
}

/// With a short test tick the writer adds rows while the node runs (health checks are
/// not counted); with a tick that never comes, the shutdown flush writes the requests.
async fn minute_writer_writes_rows(db: Arc<TestDb>) {
    let store = db.shared_store();
    let site = Site::new(&db);
    let opts = |tick: Duration| NodeOptions {
        periodics: Some(Vec::new()),
        metrics_tick: Some(tick),
        ..NodeOptions::default()
    };

    let node = site.start(&db, opts(Duration::from_millis(200))).await;
    let id = node.node_id;
    for _ in 0..3 {
        let (status, body) = get(node.addr, "/").await;
        assert_eq!(status, 200, "{body}");
    }
    get(node.addr, "/healthz").await;
    eventually(Duration::from_secs(10), "rows of the running node", || {
        let store = store.clone();
        async move {
            let t = totals(store.as_ref(), id).await;
            t.requests == 3 && t.page_views == 3
        }
    })
    .await;
    let t = totals(store.as_ref(), id).await;
    assert_eq!(t.errors_5xx, 0, "{t:?}");
    assert!(t.p95_ms > 0, "page latency recorded: {t:?}");
    assert_eq!(
        t.cache_hits + t.cache_misses,
        3,
        "every page looked up: {t:?}"
    );
    node.shutdown().await.unwrap();

    let node = site.start(&db, opts(Duration::from_secs(3600))).await;
    let id = node.node_id;
    get(node.addr, "/").await;
    get(node.addr, "/missing/").await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        totals(store.as_ref(), id).await,
        MetricCounts::default(),
        "nothing written before the tick"
    );
    node.shutdown().await.unwrap();
    let t = totals(store.as_ref(), id).await;
    assert_eq!(
        (t.requests, t.page_views),
        (2, 1),
        "the shutdown flush: {t:?}"
    );
}

backend_tests_spawning!(minute_writer_writes_rows);
