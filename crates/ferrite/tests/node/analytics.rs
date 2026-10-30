//! The `analytics` example plugin on a real node (M4-3a Task 3, plan P44): page responses
//! reach it through the batched `request.served` jobs, with the timestamps and event
//! shape the node really produces, and end up in the day blob.

use super::*;

const FIREFOX: &str = "Mozilla/5.0 (X11; Linux x86_64) Firefox/130.0";

/// `GET path` with this user agent and extra header lines (each ending in CRLF): status.
async fn visit(addr: SocketAddr, path: &str, ua: &str, extra: &str) -> u16 {
    let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
    s.write_all(
        format!(
            "GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nUser-Agent: {ua}\r\n\
             {extra}Connection: close\r\n\r\n"
        )
        .as_bytes(),
    )
    .await
    .unwrap();
    let mut raw = Vec::new();
    s.read_to_end(&mut raw).await.unwrap();
    String::from_utf8_lossy(&raw)
        .split(' ')
        .nth(1)
        .and_then(|c| c.parse().ok())
        .unwrap()
}

/// The summed counts of every stored day (a run across midnight stores two).
async fn totals(store: &dyn Store) -> Value {
    let kv = store.read().plugin_kv();
    let mut sum = json!({"views": 0, "bots": 0, "status": {}, "paths": {}, "referrers": {}});
    for key in kv.list_keys("analytics", "day:", 100).await.unwrap() {
        let blob: Value =
            serde_json::from_slice(&kv.get("analytics", &key).await.unwrap().unwrap()).unwrap();
        for field in ["views", "bots"] {
            sum[field] = json!(sum[field].as_u64().unwrap() + blob[field].as_u64().unwrap());
        }
        for map in ["status", "paths", "referrers"] {
            for (k, n) in blob[map].as_object().unwrap() {
                let was = sum[map][k].as_u64().unwrap_or(0);
                sum[map][k] = json!(was + n.as_u64().unwrap());
            }
        }
    }
    sum
}

/// Real requests: a browser view with and without a foreign referrer, one from the site
/// itself, a 404, and a bot. One runner worker, because the plugin reads then writes its
/// blob.
async fn page_views_reach_the_day_blob(db: Arc<TestDb>) {
    let mut site = Site::new(&db);
    site.config.cluster.workers = 1;
    let opts = NodeOptions {
        periodics: Some(Vec::new()),
        served_flush: Some(Duration::from_millis(200)),
        ..NodeOptions::default()
    };
    let node = site.start(&db, opts).await;
    let store = db.shared_store();
    install_plugin_file(
        &store,
        &node.state.media,
        &site.config.plugins,
        &manifest("analytics"),
        &test_support::plugin_wasm("analytics"),
        true,
    )
    .await
    .unwrap();
    // No request before the plugin is loaded: those events would have no subscriber.
    eventually(PLUGIN_LOAD, "analytics loaded", || async {
        node.state.plugins.is_enabled("analytics")
    })
    .await;

    let news = "Referer: https://News.Example.org/story\r\n";
    let own = format!("Referer: {SITE_URL}/about/\r\n");
    assert_eq!(visit(node.addr, "/", FIREFOX, news).await, 200);
    assert_eq!(visit(node.addr, "/", FIREFOX, "").await, 200);
    assert_eq!(visit(node.addr, "/", FIREFOX, &own).await, 200);
    assert_eq!(visit(node.addr, "/missing/", FIREFOX, "").await, 404);
    assert_eq!(visit(node.addr, "/", "Googlebot/2.1", "").await, 200);
    // Not page responses.
    get(node.addr, "/healthz").await;

    eventually(Duration::from_secs(10), "analytics' day blob", || {
        let store = store.clone();
        async move {
            // Every request, not just the first batch: they may be flushed apart.
            let t = totals(store.as_ref()).await;
            t["views"] == 3 && t["bots"] == 1 && t["status"]["4xx"] == 1
        }
    })
    .await;
    let sum = totals(store.as_ref()).await;
    assert_eq!(sum["paths"], json!({"/": 3}), "{sum}");
    // The site's own host is not a referral; the foreign host arrives lowercased.
    assert_eq!(sum["referrers"], json!({"news.example.org": 1}), "{sum}");
    assert_eq!(sum["status"], json!({"2xx": 4, "4xx": 1}), "{sum}");
    assert_eq!(sum["bots"], 1, "{sum}");
    node.shutdown().await.unwrap();
}

backend_tests_spawning!(page_views_reach_the_day_blob);
