//! Exit criterion part 1: a save on node 0 is served by node 1 within 1 s.
//!
//! * `invalidation_latency_under_1s`: both nodes poll only every 5 s, so the bound can
//!   only be met by the `LISTEN` wake-up, and, with node 1's listener cut at each save,
//!   by the reconnect's `Reconnected` catch-up poll (P3).
//! * `poll_only_node_converges_within_1s`: node 1 does not listen at all and relies on
//!   the 250 ms poll.

use std::time::Duration;

use ferrite::NodeOptions;
use ferrite_core::{Actor, ContentService};
use ferrite_domain::ContentItem;
use tokio::time::Instant;

use crate::harness::{
    CONVERGE, Cluster, SETUP, Spec, admin, content, post_draft, published_post, summary, wait_until,
};

const REPS: usize = 10;
const SLUG: &str = "latency";
const PATH: &str = "/latency/";
/// Poll interval of the LISTEN test: far above the bound, so polling cannot meet it.
const LONG_POLL_MS: &str = "5000";

/// Does node `i` serve `title` on the post page and the home page?
async fn shows(c: &Cluster, i: usize, title: &str) -> bool {
    let (s1, post) = c.get(i, PATH).await;
    let (s2, home) = c.get(i, "/").await;
    s1 == 200 && s2 == 200 && post.contains(title) && home.contains(title)
}

/// Publishes the test post through node 0 and waits until node 1 serves both pages from
/// its cache (a first render may still race the publish's events), so later
/// measurements are of invalidation.
async fn warm(c: &Cluster, svc: &ContentService, actor: &Actor) -> ContentItem {
    let item = published_post(svc, actor, SLUG).await;
    wait_until(SETUP, "pages cached on node 1", || async {
        let renders = c.node(1).state.renders();
        shows(c, 1, "Title latency").await && c.node(1).state.renders() == renders
    })
    .await;
    item
}

/// Saves a new title through node 0; how long until node 1 serves it, timed from the
/// start of the save.
async fn save_and_measure(
    c: &Cluster,
    svc: &ContentService,
    actor: &Actor,
    item: &mut ContentItem,
    title: &str,
) -> Duration {
    let saved = Instant::now();
    *item = svc
        .update(actor, item.id, item.version, post_draft(title, SLUG))
        .await
        .unwrap();
    wait_until(SETUP, "node 1 serves the edit", || shows(c, 1, title)).await;
    saved.elapsed()
}

fn max(samples: &[Duration]) -> Duration {
    samples.iter().max().copied().unwrap_or(Duration::ZERO)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn invalidation_latency_under_1s() {
    let spec = Spec::new(2).env("FERRITE_CACHE__POLL_INTERVAL_MS", LONG_POLL_MS);
    let Some(c) = Cluster::with(spec, |_| NodeOptions::default()).await else {
        return;
    };
    let svc = content(c.store(0));
    let actor = admin(c.store(0).as_ref()).await;
    let mut item = warm(&c, &svc, &actor).await;

    let listen = format!("{}:listen", c.app_name(1));
    let mut notified = Vec::new();
    let mut cut_samples = Vec::new();
    let mut cuts = 0;
    for phase in ["notify", "cut"] {
        for k in 0..REPS {
            if phase == "cut" {
                // Cut node 1's subscriptions right before the save. The store reconnects
                // them at once (eager reconnect: connect + LISTEN, then `Reconnected`), so
                // the save is either notified on the new connection or committed before
                // it and found by the catch-up poll that `Reconnected` triggers.
                let cut = c.db.terminate_backends(&listen).await;
                assert!(cut > 0, "node 1 was listening before cut {k}");
                cuts += cut;
            }
            let title = format!("Edited {phase} {k}");
            let took = save_and_measure(&c, &svc, &actor, &mut item, &title).await;
            if phase == "notify" {
                notified.push(took);
            } else {
                cut_samples.push(took);
            }
        }
    }
    eprintln!(
        "invalidation latency, LISTEN, {LONG_POLL_MS} ms poll: {}",
        summary(&notified)
    );
    eprintln!(
        "invalidation latency, listener cut, {LONG_POLL_MS} ms poll ({cuts} connections cut \
         over {REPS} saves): {}",
        summary(&cut_samples)
    );
    for (what, samples) in [("LISTEN", &notified), ("listener cut", &cut_samples)] {
        let max = max(samples);
        assert!(max < CONVERGE, "{what}: max latency {max:?} ≥ {CONVERGE:?}");
    }
    c.finish().await;
}

/// A node without `LISTEN` (a listener that is silently gone) converges through the
/// 250 ms poll alone.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn poll_only_node_converges_within_1s() {
    let Some(c) = Cluster::new(2, |i| NodeOptions {
        listen: (i == 1).then_some(false),
        ..NodeOptions::default()
    })
    .await
    else {
        return;
    };
    assert_eq!(
        c.db.active_backends(&format!("{}:listen", c.app_name(1)))
            .await,
        0,
        "node 1 does not listen"
    );
    let svc = content(c.store(0));
    let actor = admin(c.store(0).as_ref()).await;
    let mut item = warm(&c, &svc, &actor).await;
    let mut samples = Vec::new();
    for k in 0..REPS {
        // Saving right after the previous edit was seen would put every save just after
        // a poll (the worst phase); offsets of 0..225 ms spread them over the 250 ms
        // period, so the p50 is that of an arbitrary save.
        tokio::time::sleep(Duration::from_millis(25 * k as u64)).await;
        let title = format!("Edited poll {k}");
        samples.push(save_and_measure(&c, &svc, &actor, &mut item, &title).await);
    }
    eprintln!(
        "invalidation latency, poll only (250 ms): {}",
        summary(&samples)
    );
    let mut sorted = samples.clone();
    sorted.sort();
    let p50 = sorted[sorted.len() / 2];
    let max = max(&samples);
    assert!(max < CONVERGE, "max latency {max:?} ≥ {CONVERGE:?}");
    // A sanity bound only (listening is off, so the poll is the only path): two poll
    // periods, loose enough for a loaded CI runner.
    assert!(p50 <= Duration::from_millis(500), "p50 {p50:?} > 500 ms");
    c.finish().await;
}
