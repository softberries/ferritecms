//! Jobs across nodes: exclusive claims, a crashed node's lease (exit criterion part 2,
//! P25) and periodic jobs once per period cluster-wide (P11).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ferrite::{NodeOptions, RunnerOverrides};
use ferrite_cluster::{Periodic, STALE_AFTER};
use ferrite_domain::{JobId, JobStatus, NewJob};
use ferrite_store::Store;
use serde_json::json;
use sqlx::Connection;

use crate::harness::{Cluster, Recorder, Run, SETUP, Spec, Then, runs, wait_until};

/// No built-in periodics: the only jobs are the test's.
fn quiet() -> NodeOptions {
    NodeOptions {
        periodics: Some(Vec::new()),
        ..NodeOptions::default()
    }
}

async fn status_counts(store: &dyn Store) -> BTreeMap<JobStatus, u64> {
    store.read().jobs().counts().await.unwrap()
}

const RECORD: &str = "test.record";
const JOBS: usize = 300;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn job_exclusivity_three_nodes() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let spec = Spec::new(3).env("FERRITE_CLUSTER__WORKERS", "4");
    let Some(c) = Cluster::with(spec, |i| NodeOptions {
        extra_handlers: vec![Recorder::handler(RECORD, &[1], i, &log, Then::ShortSleep)],
        ..quiet()
    })
    .await
    else {
        return;
    };
    // All 300 become visible at one commit, so every node competes for them.
    let tx = c.db.store().begin().await.unwrap();
    let mut ids = BTreeSet::new();
    for _ in 0..JOBS {
        let id = tx
            .jobs()
            .enqueue(&NewJob::new(RECORD, json!({"v": 1})))
            .await
            .unwrap()
            .unwrap();
        ids.insert(id);
    }
    tx.commit().await.unwrap();
    let took = wait_until(Duration::from_secs(60), "300 jobs done", || async {
        status_counts(c.db.store()).await.get(&JobStatus::Done) == Some(&(JOBS as u64))
    })
    .await;

    let counts = status_counts(c.db.store()).await;
    for s in [JobStatus::Queued, JobStatus::Running, JobStatus::Dead] {
        assert_eq!(counts.get(&s).copied().unwrap_or(0), 0, "{s:?}: {counts:?}");
    }
    let runs = runs(&log);
    let ran: BTreeSet<JobId> = runs.iter().map(|r| r.job).collect();
    assert_eq!(runs.len(), JOBS, "one run per job");
    assert_eq!(ran, ids, "every job ran exactly once");
    for id in &ids {
        let job = c.db.store().read().jobs().get(*id).await.unwrap().unwrap();
        assert_eq!(job.attempts, 1, "{job:?}");
        assert_eq!(job.locked_by, None, "{job:?}");
    }
    let mut per_node = BTreeMap::new();
    for r in &runs {
        *per_node.entry(r.node).or_insert(0usize) += 1;
    }
    eprintln!("job exclusivity: {JOBS} jobs in {took:?}, runs per node {per_node:?}");
    assert!(
        per_node.len() >= 2,
        "at least 2 nodes did work: {per_node:?}"
    );
    c.finish().await;
}

const STALL: &str = "test.stall";
const LEASE: Duration = Duration::from_secs(2);

/// Lease 2 s, renewed every 500 ms; node 0's handler never returns.
fn short_lease(log: &Arc<Mutex<Vec<Run>>>, i: usize) -> NodeOptions {
    NodeOptions {
        extra_handlers: vec![Recorder::handler(
            STALL,
            &[1],
            i,
            log,
            if i == 0 { Then::Block } else { Then::Done },
        )],
        runner: RunnerOverrides {
            lease: Some(LEASE),
            extend_every: Some(Duration::from_millis(500)),
            idle_poll: None,
        },
        ..quiet()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn lease_expiry_after_crashed_node() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let Some(mut c) = Cluster::prepare(Spec::new(3)).await else {
        return;
    };
    c.start(0, short_lease(&log, 0)).await;
    let node0 = c.node(0).node_id;
    let store = c.db.shared_store();
    let id = store
        .read()
        .jobs()
        .enqueue(&NewJob::new(STALL, json!({"v": 1})))
        .await
        .unwrap()
        .unwrap();
    wait_until(SETUP, "node 0 holds the lease", || {
        let store = store.clone();
        async move {
            let job = store.read().jobs().get(id).await.unwrap().unwrap();
            job.status == JobStatus::Running && job.locked_by == Some(node0)
        }
    })
    .await;
    // The claim is not enough: the crash must hit the handler mid-run.
    wait_until(SETUP, "node 0's handler started", || async {
        runs(&log).iter().any(|r| r.node == 0 && r.job == id)
    })
    .await;
    // Node 1 joins while node 0 is alive: node 0 renews its lease, so node 1 must not
    // take the job for longer than one lease.
    c.start(1, short_lease(&log, 1)).await;
    tokio::time::sleep(LEASE + Duration::from_millis(500)).await;
    let job = store.read().jobs().get(id).await.unwrap().unwrap();
    assert_eq!(job.status, JobStatus::Running, "{job:?}");
    assert_eq!(
        job.locked_by,
        Some(node0),
        "a live node keeps its lease: {job:?}"
    );
    assert_eq!(job.attempts, 1, "{job:?}");
    c.crash(0).await;
    let took =
        wait_until(
            LEASE + Duration::from_secs(2),
            "node 1 finishes the job",
            || {
                let store = store.clone();
                async move {
                    store.read().jobs().get(id).await.unwrap().unwrap().status == JobStatus::Done
                }
            },
        )
        .await;
    eprintln!("crashed node's job finished by node 1 {took:?} after the crash (lease {LEASE:?})");
    let job = store.read().jobs().get(id).await.unwrap().unwrap();
    assert_eq!(job.attempts, 2, "{job:?}");
    assert_eq!(job.locked_by, None, "{job:?}");
    assert_eq!(job.locked_until, None, "{job:?}");
    let runs = runs(&log);
    assert_eq!(
        runs.iter().filter(|r| r.node == 0).count(),
        1,
        "node 0 started it once: {runs:?}"
    );
    assert_eq!(
        runs.iter().filter(|r| r.node == 1).count(),
        1,
        "node 1 ran it once: {runs:?}"
    );

    // The dead node's row stays and goes stale (no heartbeat for STALE_AFTER = 30 s;
    // pruned after 24 h). Instead of waiting 30 s, its heartbeat is backdated by 31 s:
    // it leaves the live set (heartbeat within STALE_AFTER of the database clock, the
    // registry's rule), the row is kept, and it blocks nothing: a new node joins and a
    // new job runs at once.
    c.db.exec_raw(&format!(
        "UPDATE nodes SET heartbeat_at = heartbeat_at - interval '31 seconds' \
         WHERE id = '{node0}'"
    ))
    .await;
    let nodes = store.read().nodes().list().await.unwrap();
    let now = store.read().nodes().db_now().await.unwrap();
    let live = |n: &ferrite_domain::Node| {
        now.as_micros() - n.heartbeat_at.as_micros() <= STALE_AFTER.as_micros() as i64
    };
    let row0 = nodes.iter().find(|n| n.id == node0).expect("row kept");
    assert!(!live(row0), "node 0 is stale: {row0:?}");
    let node1 = c.node(1).node_id;
    assert!(
        nodes.iter().any(|n| n.id == node1 && live(n)),
        "node 1 is live: {nodes:?}"
    );
    c.start(2, short_lease(&log, 2)).await;
    let next = store
        .read()
        .jobs()
        .enqueue(&NewJob::new(STALL, json!({"v": 1})))
        .await
        .unwrap()
        .unwrap();
    wait_until(SETUP, "a job after the crash", || {
        let store = store.clone();
        async move {
            store.read().jobs().get(next).await.unwrap().unwrap().status == JobStatus::Done
        }
    })
    .await;
    let job = store.read().jobs().get(next).await.unwrap().unwrap();
    assert_eq!(job.attempts, 1, "{job:?}");
    let ids: Vec<_> = store
        .read()
        .nodes()
        .list()
        .await
        .unwrap()
        .into_iter()
        .map(|n| n.id)
        .collect();
    assert!(ids.contains(&node0), "stale row still kept: {ids:?}");
    c.finish().await;
}

const TICK: &str = "test.tick";

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn periodic_job_runs_once_per_period() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let Some(mut c) = Cluster::new(3, |i| NodeOptions {
        extra_handlers: vec![Recorder::handler(TICK, &[1], i, &log, Then::Done)],
        periodics: Some(vec![Periodic {
            kind: TICK,
            every: Duration::from_secs(1),
            payload: json!({"v": 1}),
            max_attempts: ferrite_domain::DEFAULT_MAX_ATTEMPTS,
            offset: Duration::ZERO,
        }]),
        scheduler_tick: Some(Duration::from_millis(100)),
        ..NodeOptions::default()
    })
    .await
    else {
        return;
    };
    tokio::time::sleep(Duration::from_millis(4200)).await;
    // Stop the schedulers; a job claimed now finishes in the drain, one not yet claimed
    // stays queued.
    let mut pg = c.pg().await;
    c.stop_all().await;

    let rows: Vec<(uuid::Uuid, String, String)> = sqlx::query_as(
        "SELECT id, unique_key, status FROM jobs WHERE kind = $1 ORDER BY unique_key",
    )
    .bind(TICK)
    .fetch_all(&mut pg)
    .await
    .unwrap();
    let runs = runs(&log);
    let mut runs_per_job: BTreeMap<uuid::Uuid, usize> = BTreeMap::new();
    for r in &runs {
        *runs_per_job.entry(r.job.as_uuid()).or_default() += 1;
    }
    let starts: Vec<i64> = rows
        .iter()
        .map(|(_, key, _)| key.strip_prefix("test.tick:").unwrap().parse().unwrap())
        .collect();
    let mut nodes = BTreeSet::new();
    for r in &runs {
        nodes.insert(r.node);
    }
    eprintln!(
        "periodic: {} rows for period starts {starts:?}, runs by nodes {nodes:?}",
        rows.len()
    );
    assert!(
        starts.windows(2).all(|w| w[1] == w[0] + 1),
        "one row per period, no gaps: {starts:?}"
    );
    let done: Vec<_> = rows.iter().filter(|(_, _, s)| s == "done").collect();
    assert!(
        done.len() >= 4,
        "4.2 s cover at least 4 period starts: {rows:?}"
    );
    for (id, key, status) in &rows {
        let n = runs_per_job.get(id).copied().unwrap_or(0);
        match status.as_str() {
            "done" => assert_eq!(n, 1, "{key}: one handler run"),
            "queued" => assert_eq!(n, 0, "{key}: queued at shutdown, never run"),
            other => panic!("{key}: status {other}"),
        }
    }
    assert_eq!(runs_per_job.len(), runs.len(), "no job ran twice: {runs:?}");
    assert!(
        runs_per_job
            .keys()
            .all(|id| rows.iter().any(|(r, _, _)| r == id)),
        "every run belongs to a row"
    );
    pg.close().await.unwrap();
    c.finish().await;
}
