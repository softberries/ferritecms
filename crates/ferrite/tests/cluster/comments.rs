//! The per-IP comment limit holds across nodes (M2a-2b Task 7, P13, Review Focus 3): the
//! count lives in the shared `rate_limits` table, so submits spread over two nodes are
//! cut at 3 a minute.

use std::time::Duration;

use ferrite::NodeOptions;
use ferrite_store_tests::window::in_one_window;

use crate::harness::{Cluster, SITE_URL, Spec, admin, content, published_post, send};

const WINDOW: Duration = Duration::from_secs(60);

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn comment_limits_hold_across_nodes() {
    // Trusting the test clients' 127.0.0.1 makes `X-Forwarded-For` the client address,
    // so one test can be two clients (M2a-2a F27).
    let spec = Spec::new(2).env("FERRITE_SERVER__TRUSTED_PROXIES", "[\"127.0.0.1/32\"]");
    let Some(c) = Cluster::with(spec, |_| NodeOptions::default()).await else {
        return;
    };
    let store = c.store(0);
    let actor = admin(store.as_ref()).await;
    let post = published_post(&content(store.clone()), &actor, "commented").await;
    let path = format!("/api/v1/content/post/{}/comments", post.id);
    let body = br#"{"body": "Hello", "author_name": "Ann", "author_email": "ann@example.com"}"#;

    let submit = |node: usize, client: &'static str| {
        let addr = c.node(node).addr;
        let path = path.clone();
        async move {
            send(
                addr,
                "POST",
                &path,
                &[
                    ("X-Forwarded-For", client),
                    ("Origin", SITE_URL),
                    ("Content-Type", "application/json"),
                ],
                body,
            )
            .await
        }
    };
    // Every submit in one window (F10, F27); a run again starts in the next one.
    let store = &store;
    let list = || async move {
        let query = ferrite_domain::CommentQuery {
            limit: 50,
            ..Default::default()
        };
        store.read().comments().list(&query).await.unwrap().items
    };
    in_one_window(
        store.as_ref(),
        WINDOW,
        Duration::from_secs(30),
        |_| async move {
            // A run again counts its own comments.
            let before = list().await.len();
            for i in 0..3 {
                let (status, head, body) = submit(i % 2, "198.51.100.1").await;
                assert_eq!(
                    status,
                    202,
                    "submit {} on node {}: {head}\n{body}",
                    i + 1,
                    i % 2
                );
            }
            // The fourth, on the other node, is refused: the count is shared.
            let (status, head, body) = submit(1, "198.51.100.1").await;
            assert_eq!(status, 429, "{head}\n{body}");
            assert!(head.contains("retry-after: "), "{head}");
            assert!(body.contains("/api/v1/problems/rate-limited"), "{body}");
            // A second client address still passes, on both nodes.
            for node in 0..2 {
                let (status, head, _) = submit(node, "198.51.100.2").await;
                assert_eq!(status, 202, "second client on node {node}: {head}");
            }
            // Five stored, all pending (a guest's first comments are held).
            let pending = list().await;
            assert_eq!(pending.len() - before, 5);
            assert!(
                pending
                    .iter()
                    .all(|c| c.status == ferrite_domain::CommentStatus::Pending)
            );
        },
    )
    .await;
    c.finish().await;
}
