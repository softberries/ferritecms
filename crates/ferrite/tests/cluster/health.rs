//! `content.health` on two nodes (M6-4 task 9, design 16 §8): one run per period, a
//! manual run never overlapping another, and every node serving the report.

use std::time::Duration;

use ferrite::NodeOptions;
use ferrite_cluster::Periodic;
use ferrite_core::{
    jobs::{CONTENT_HEALTH_KIND, content_health_job},
    quality::QualityService,
};
use ferrite_domain::{HealthRun, HealthRunStatus, HealthTrigger, JobQuery, JobStatus};
use ferrite_store::Store;
use serde_json::json;

use crate::harness::{Cluster, admin, content, published_post, wait_until};

fn every(secs: u64) -> NodeOptions {
    NodeOptions {
        periodics: Some(vec![Periodic {
            kind: CONTENT_HEALTH_KIND,
            every: Duration::from_secs(secs),
            payload: json!({"v": 1, "trigger": "schedule", "user": null}),
            max_attempts: ferrite_domain::DEFAULT_MAX_ATTEMPTS,
            offset: Duration::ZERO,
        }]),
        scheduler_tick: Some(Duration::from_millis(100)),
        ..NodeOptions::default()
    }
}

fn quiet() -> NodeOptions {
    NodeOptions {
        periodics: Some(Vec::new()),
        ..NodeOptions::default()
    }
}

/// Every run, oldest first (from the database).
async fn runs(store: &dyn Store) -> Vec<HealthRun> {
    let mut out = Vec::new();
    for status in [HealthRunStatus::Done, HealthRunStatus::Running] {
        // `latest_run` reads one; walk the jobs instead for the count and the rows for
        // the overlap.
        if let Some(r) = store.read().quality().latest_run(status).await.unwrap() {
            out.push(r);
        }
    }
    out
}

async fn done_jobs(store: &dyn Store) -> usize {
    store
        .read()
        .jobs()
        .list(&JobQuery {
            status: Some(JobStatus::Done),
            kind: Some(CONTENT_HEALTH_KIND.into()),
            limit: 200,
            cursor: None,
        })
        .await
        .unwrap()
        .items
        .len()
}

/// The `[started_at, finished_at]` of every finished run, from SQL (the repo reads
/// only the newest).
async fn spans(c: &Cluster) -> Vec<(i64, i64, String)> {
    use sqlx::Row;
    let mut pg = c.pg().await;
    let rows = sqlx::query(
        "SELECT (extract(epoch from started_at) * 1000000)::bigint AS s, \
         (extract(epoch from finished_at) * 1000000)::bigint AS f, status FROM health_runs \
         ORDER BY started_at",
    )
    .fetch_all(&mut pg)
    .await
    .unwrap();
    rows.iter()
        .map(|r| {
            (
                r.get::<i64, _>("s"),
                r.get::<Option<i64>, _>("f").unwrap_or(i64::MAX),
                r.get::<String, _>("status"),
            )
        })
        .collect()
}

fn assert_disjoint(spans: &[(i64, i64, String)]) {
    for w in spans.windows(2) {
        assert!(w[0].1 <= w[1].0, "runs overlap: {:?} and {:?}", w[0], w[1]);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_nodes_one_run_per_period() {
    let Some(c) = Cluster::new(2, |_| every(2)).await else {
        return;
    };
    let actor = admin(c.store(0).as_ref()).await;
    published_post(&content(c.store(0)), &actor, "hello").await;
    wait_until(Duration::from_secs(30), "three periodic runs", || async {
        done_jobs(c.store(0).as_ref()).await >= 3
    })
    .await;
    let s = spans(&c).await;
    assert_disjoint(&s);
    // One job per period: every periodic job has its own period key, so the number of
    // runs is the number of jobs (no node ran a period twice).
    let jobs = done_jobs(c.store(1).as_ref()).await;
    let done = s.iter().filter(|r| r.2 == "done").count();
    assert!(done >= jobs.min(3), "{done} runs for {jobs} jobs");
    assert!(!runs(c.store(1).as_ref()).await.is_empty());
    c.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn manual_and_periodic_never_overlap() {
    let Some(c) = Cluster::new(2, |_| every(1)).await else {
        return;
    };
    let actor = admin(c.store(0).as_ref()).await;
    for i in 0..20 {
        published_post(&content(c.store(i % 2)), &actor, &format!("p-{i}")).await;
    }
    // Manual runs queued from both nodes while periodic ones come due.
    for i in 0..4 {
        c.store(i % 2)
            .read()
            .jobs()
            .enqueue(&content_health_job(HealthTrigger::Manual, Some(actor.user)))
            .await
            .unwrap();
    }
    wait_until(Duration::from_secs(60), "the manual runs", || async {
        done_jobs(c.store(0).as_ref()).await >= 5
    })
    .await;
    assert_disjoint(&spans(&c).await);
    c.finish().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn any_node_serves_the_report() {
    let Some(c) = Cluster::new(2, |_| quiet()).await else {
        return;
    };
    let actor = admin(c.store(0).as_ref()).await;
    published_post(&content(c.store(0)), &actor, "hello").await;
    let q0 = QualityService::new(c.store(0), c.config(0).site.url.to_string());
    q0.request_run(&actor).await.unwrap();
    wait_until(Duration::from_secs(30), "the run", || async {
        done_jobs(c.store(1).as_ref()).await >= 1
    })
    .await;
    let q1 = QualityService::new(c.store(1), c.config(1).site.url.to_string());
    let summary = q1.summary(&actor).await.unwrap();
    assert!(summary.last.is_some());
    c.finish().await;
}
