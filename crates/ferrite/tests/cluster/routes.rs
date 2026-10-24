//! Plugin route rate limits hold across nodes (M2a-2 Task 8, P16, Review Focus 2): the
//! count lives in the shared `rate_limits` table, so a flood spread over two nodes is
//! cut at the route's limit.

use std::time::Duration;

use ferrite::NodeOptions;
use ferrite::extensions::install_plugin_file;

use crate::harness::{Cluster, SETUP, Spec, manifest, send, wait_until};
use crate::test_support;

/// `route-flood`'s `rate_limit` ("5/min").
const LIMIT: usize = 5;
const WINDOW: Duration = Duration::from_secs(60);

/// Sleeps into the next window when fewer than `need` are left of this one (F10, F27).
async fn align_to_window(c: &Cluster, need: Duration) {
    let w = i64::try_from(WINDOW.as_micros()).unwrap();
    let now = c
        .store(0)
        .read()
        .nodes()
        .db_now()
        .await
        .unwrap()
        .as_micros();
    let left = w - now.rem_euclid(w);
    if left < i64::try_from(need.as_micros()).unwrap() {
        tokio::time::sleep(Duration::from_micros(left.unsigned_abs()) + Duration::from_millis(50))
            .await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn route_flood_is_limited_across_nodes() {
    // The nodes see every test client as 127.0.0.1; trusting it makes
    // `X-Forwarded-For` the client address, so one test can be two clients (F27).
    let spec = Spec::new(2).env("FERRITE_SERVER__TRUSTED_PROXIES", "[\"127.0.0.1/32\"]");
    let Some(c) = Cluster::with(spec, |_| NodeOptions::default()).await else {
        return;
    };
    let store = c.store(0);
    let media = c.node(0).state.media.clone();
    install_plugin_file(
        &store,
        &media,
        &c.config(0).plugins,
        &manifest("route-flood"),
        &test_support::plugin_wasm("route-flood"),
        true,
    )
    .await
    .unwrap();
    // Loaded everywhere: the route exists (GET is 405, before any rate-limit hit).
    wait_until(SETUP, "route-flood loaded on every node", || async {
        for i in 0..c.len() {
            let (status, _, _) =
                send(c.node(i).addr, "GET", "/_p/route-flood/submit", &[], b"").await;
            if status != 405 {
                return false;
            }
        }
        true
    })
    .await;

    let post = |node: usize, client: &'static str| {
        let addr = c.node(node).addr;
        async move {
            send(
                addr,
                "POST",
                "/_p/route-flood/submit",
                &[
                    ("X-Forwarded-For", client),
                    ("Origin", "https://elsewhere.example"),
                    ("Content-Type", "application/x-www-form-urlencoded"),
                ],
                b"a=1",
            )
            .await
        }
    };
    align_to_window(&c, Duration::from_secs(30)).await;
    for i in 0..LIMIT {
        let (status, head, body) = post(i % 2, "198.51.100.1").await;
        assert_eq!(
            status,
            200,
            "post {} on node {}: {head}\n{body}",
            i + 1,
            i % 2
        );
        assert!(
            head.contains("content-security-policy: sandbox allow-forms"),
            "{head}"
        );
    }
    // The sixth, on whichever node, is refused: the count is shared.
    let (status, head, _) = post(LIMIT % 2, "198.51.100.1").await;
    assert_eq!(status, 429, "{head}");
    assert!(head.contains("retry-after: "), "{head}");
    let (status, _, _) = post((LIMIT + 1) % 2, "198.51.100.1").await;
    assert_eq!(status, 429);
    // A second client still passes, on both nodes.
    for node in 0..2 {
        let (status, head, _) = post(node, "198.51.100.2").await;
        assert_eq!(status, 200, "second client on node {node}: {head}");
    }
    // The site keeps serving.
    let (status, _) = c.get(0, "/").await;
    assert_eq!(status, 200);
    c.finish().await;
}
