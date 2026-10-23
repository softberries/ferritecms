//! Exit criterion part 1: a save on node 0 is served by node 1 within 1 s, with
//! `LISTEN` wake-ups and with node 1's listener cut at each save (reconnect catch-up +
//! 250 ms poll).

use std::time::Duration;

use ferrite::NodeOptions;
use tokio::time::Instant;

use crate::harness::{
    CONVERGE, Cluster, SETUP, admin, content, post_draft, published_post, summary, wait_until,
};

const REPS: usize = 10;
const SLUG: &str = "latency";
const PATH: &str = "/latency/";

/// Does node `i` serve `title` on the post page and the home page?
async fn shows(c: &Cluster, i: usize, title: &str) -> bool {
    let (s1, post) = c.get(i, PATH).await;
    let (s2, home) = c.get(i, "/").await;
    s1 == 200 && s2 == 200 && post.contains(title) && home.contains(title)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn invalidation_latency_under_1s() {
    let Some(c) = Cluster::new(2, |_| NodeOptions::default()).await else {
        return;
    };
    let svc = content(c.store(0));
    let actor = admin(c.store(0).as_ref()).await;
    let mut item = published_post(&svc, &actor, SLUG).await;
    wait_until(SETUP, "node 1 serves the new post", || {
        shows(&c, 1, "Title latency")
    })
    .await;
    // Both pages are cached on node 1 (a first render may still race the publish's
    // events), so what follows measures invalidation.
    wait_until(SETUP, "pages cached on node 1", || async {
        let renders = c.node(1).state.renders();
        shows(&c, 1, "Title latency").await && c.node(1).state.renders() == renders
    })
    .await;

    let listen = format!("{}:listen", c.app_name(1));
    let mut notified = Vec::new();
    let mut polled = Vec::new();
    let mut cuts = 0;
    for phase in ["notify", "poll"] {
        for k in 0..REPS {
            if phase == "poll" {
                // Cut node 1's subscriptions right before the save. The store reconnects
                // them at once (eager reconnect) and reports `Reconnected`, so the save's
                // notification is either heard by the new connection or lost, in which
                // case the reconnect's catch-up poll or the 250 ms poll finds the event.
                let cut = c.db.terminate_backends(&listen).await;
                assert!(cut > 0, "node 1 was listening before cut {k}");
                cuts += cut;
            }
            let title = format!("Edited {phase} {k}");
            // Timed from the start of the save on node 0 to node 1 serving it.
            let saved = Instant::now();
            item = svc
                .update(&actor, item.id, item.version, post_draft(&title, SLUG))
                .await
                .unwrap();
            wait_until(SETUP, "node 1 serves the edit", || shows(&c, 1, &title)).await;
            let took = saved.elapsed();
            if phase == "notify" {
                notified.push(took);
            } else {
                polled.push(took);
            }
        }
    }
    eprintln!("invalidation latency, LISTEN: {}", summary(&notified));
    eprintln!(
        "invalidation latency, listener cut ({cuts} connections cut over {REPS} saves): {}",
        summary(&polled)
    );
    for (what, samples) in [("LISTEN", &notified), ("listener cut", &polled)] {
        let max = samples.iter().max().copied().unwrap_or(Duration::ZERO);
        assert!(max < CONVERGE, "{what}: max latency {max:?} ≥ {CONVERGE:?}");
    }
    c.finish().await;
}
