//! Job runner, scheduler and built-in prune jobs on both backends (04 §6, P5, P9–P11).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{
    future::Future,
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use ferrite_cluster::{
    ClusterError, JobError, JobHandler, JobRunner, Periodic, RunnerConfig, Scheduler, builtin,
};
use ferrite_domain::{Job, JobId, JobStatus, NewEvent, NewJob, NewNode, NodeId, Timestamp};
use ferrite_store::{Backend, Store};
use ferrite_store_tests::{TestDb, backend_tests_spawning};
use serde_json::json;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

type RunFuture = Pin<Box<dyn Future<Output = Result<(), JobError>> + Send>>;

/// A handler whose behaviour the test supplies; records every job id it was called with.
struct TestHandler {
    kind: &'static str,
    timeout: Duration,
    calls: Arc<Mutex<Vec<JobId>>>,
    run: Box<dyn Fn(Job) -> RunFuture + Send + Sync>,
}

impl TestHandler {
    fn new<F, Fut>(kind: &'static str, run: F) -> Self
    where
        F: Fn(Job) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<(), JobError>> + Send + 'static,
    {
        Self {
            kind,
            timeout: Duration::from_secs(300),
            calls: Arc::default(),
            run: Box::new(move |job| Box::pin(run(job))),
        }
    }

    fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// The handler for the runner plus its call log.
    fn build(self) -> (Arc<dyn JobHandler>, Arc<Mutex<Vec<JobId>>>) {
        let calls = self.calls.clone();
        (Arc::new(self), calls)
    }
}

#[async_trait]
impl JobHandler for TestHandler {
    fn kind(&self) -> &'static str {
        self.kind
    }

    fn timeout(&self) -> Duration {
        self.timeout
    }

    async fn run(&self, job: &Job) -> Result<(), JobError> {
        self.calls.lock().unwrap().push(job.id);
        (self.run)(job.clone()).await
    }
}

/// Sets its flag when dropped: observes a cancelled handler future.
struct DropFlag(Arc<AtomicBool>);

impl Drop for DropFlag {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

const KIND: &str = "test.work";

fn calls(log: &Arc<Mutex<Vec<JobId>>>) -> Vec<JobId> {
    log.lock().unwrap().clone()
}

async fn enqueue(store: &dyn Store, job: NewJob) -> JobId {
    store.read().jobs().enqueue(&job).await.unwrap().unwrap()
}

async fn get(store: &dyn Store, id: JobId) -> Job {
    store.read().jobs().get(id).await.unwrap().unwrap()
}

/// Polls `check` every 20 ms until it returns `Some`, panicking after `limit`.
async fn wait_for<T, F, Fut>(limit: Duration, what: &str, mut check: F) -> T
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Option<T>>,
{
    let deadline = Instant::now() + limit;
    loop {
        if let Some(v) = check().await {
            return v;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn wait_status(store: &dyn Store, id: JobId, status: JobStatus, limit: Duration) -> Job {
    wait_for(limit, &format!("job {status}"), || async {
        let job = get(store, id).await;
        (job.status == status).then_some(job)
    })
    .await
}

/// Waits until `node` holds the job.
async fn wait_running_on(store: &dyn Store, id: JobId, node: NodeId) -> Job {
    wait_for(Duration::from_secs(5), "job running", || async {
        let job = get(store, id).await;
        (job.status == JobStatus::Running && job.locked_by == Some(node)).then_some(job)
    })
    .await
}

fn start(
    store: Arc<dyn Store>,
    config: RunnerConfig,
    handlers: Vec<Arc<dyn JobHandler>>,
) -> (CancellationToken, tokio::task::JoinHandle<()>) {
    let stop = CancellationToken::new();
    let handle = JobRunner::new(store, config, handlers)
        .unwrap()
        .spawn(stop.clone());
    (stop, handle)
}

async fn stop_runner(stop: CancellationToken, handle: tokio::task::JoinHandle<()>) {
    stop.cancel();
    tokio::time::timeout(Duration::from_secs(15), handle)
        .await
        .expect("runner stops")
        .expect("runner task ok");
}

fn near(a: Timestamp, b: Timestamp, tolerance: Duration) -> bool {
    (a.as_micros() - b.as_micros()).abs() <= i64::try_from(tolerance.as_micros()).unwrap()
}

fn plus(t: Timestamp, d: Duration) -> Timestamp {
    Timestamp::from_micros(t.as_micros() + i64::try_from(d.as_micros()).unwrap()).unwrap()
}

/// A second store on the same database (Postgres peer), or the same store (SQLite).
async fn other_node_store(db: &TestDb) -> Arc<dyn Store> {
    match db.peer().await {
        Some(peer) => peer,
        None => db.shared_store(),
    }
}

pub async fn runner_runs_and_completes(db: Arc<TestDb>) {
    let store = db.shared_store();
    let mut ids = Vec::new();
    for _ in 0..20 {
        ids.push(enqueue(store.as_ref(), NewJob::new(KIND, json!({"v": 1}))).await);
    }
    let (handler, log) = TestHandler::new(KIND, |_| async {
        tokio::time::sleep(Duration::from_millis(10)).await;
        Ok(())
    })
    .build();
    let (stop, handle) = start(
        store.clone(),
        RunnerConfig::new(NodeId::new(), 2),
        vec![handler],
    );
    wait_for(Duration::from_secs(5), "20 jobs done", || async {
        let counts = store.read().jobs().counts().await.unwrap();
        (counts[&JobStatus::Done] == 20).then_some(())
    })
    .await;
    stop_runner(stop, handle).await;
    let mut seen = calls(&log);
    seen.sort();
    ids.sort();
    assert_eq!(seen, ids, "each job ran exactly once");
}

pub async fn runner_retries_then_dead(db: Arc<TestDb>) {
    let store = db.shared_store();
    let mut new = NewJob::new(KIND, json!({"v": 1}));
    new.max_attempts = 2;
    let id = enqueue(store.as_ref(), new).await;
    let (handler, log) =
        TestHandler::new(KIND, |_| async { Err(JobError::Retry("boom".into())) }).build();
    let (stop, handle) = start(
        store.clone(),
        RunnerConfig::new(NodeId::new(), 2),
        vec![handler],
    );
    let job = wait_status(
        store.as_ref(),
        id,
        JobStatus::Queued,
        Duration::from_secs(5),
    )
    .await;
    let job = if job.attempts == 1 {
        job
    } else {
        wait_for(Duration::from_secs(5), "first failure", || async {
            let job = get(store.as_ref(), id).await;
            (job.status == JobStatus::Queued && job.attempts == 1).then_some(job)
        })
        .await
    };
    assert_eq!(job.last_error.as_deref(), Some("boom"));
    db.force_job_due(id).await;
    let job = wait_status(store.as_ref(), id, JobStatus::Dead, Duration::from_secs(5)).await;
    stop_runner(stop, handle).await;
    assert_eq!(job.attempts, 2);
    assert_eq!(job.last_error.as_deref(), Some("boom"));
    assert_eq!(calls(&log), vec![id, id]);
}

pub async fn runner_permanent_error_is_dead_at_once(db: Arc<TestDb>) {
    let store = db.shared_store();
    let id = enqueue(store.as_ref(), NewJob::new(KIND, json!({"v": 1}))).await;
    let (handler, log) =
        TestHandler::new(KIND, |_| async { Err(JobError::Permanent("bad".into())) }).build();
    let (stop, handle) = start(
        store.clone(),
        RunnerConfig::new(NodeId::new(), 2),
        vec![handler],
    );
    let job = wait_status(store.as_ref(), id, JobStatus::Dead, Duration::from_secs(5)).await;
    stop_runner(stop, handle).await;
    assert_eq!((job.attempts, job.last_error.as_deref()), (1, Some("bad")));
    assert_eq!(calls(&log), vec![id]);
}

/// Waits until the job's first attempt failed (queued again with an error).
async fn wait_first_failure(store: &dyn Store, id: JobId) -> Job {
    wait_for(Duration::from_secs(5), "first failure", || async {
        let job = get(store, id).await;
        (job.status == JobStatus::Queued && job.last_error.is_some()).then_some(job)
    })
    .await
}

pub async fn runner_handler_timeout_counts_as_failure(db: Arc<TestDb>) {
    let store = db.shared_store();
    let id = enqueue(store.as_ref(), NewJob::new(KIND, json!({"v": 1}))).await;
    let (handler, _log) = TestHandler::new(KIND, |_| async {
        tokio::time::sleep(Duration::from_secs(10)).await;
        Ok(())
    })
    .timeout(Duration::from_millis(200))
    .build();
    let (stop, handle) = start(
        store.clone(),
        RunnerConfig::new(NodeId::new(), 2),
        vec![handler],
    );
    let job = wait_first_failure(store.as_ref(), id).await;
    stop_runner(stop, handle).await;
    assert_eq!(job.attempts, 1);
    let error = job.last_error.unwrap();
    assert!(error.contains("timed out"), "{error}");
}

pub async fn runner_handler_panic_is_a_failure(db: Arc<TestDb>) {
    let store = db.shared_store();
    let id = enqueue(store.as_ref(), NewJob::new(KIND, json!({}))).await;
    let (handler, _log) = TestHandler::new(KIND, |_| async {
        panic!("handler bug");
    })
    .build();
    let (stop, handle) = start(
        store.clone(),
        RunnerConfig::new(NodeId::new(), 2),
        vec![handler],
    );
    let job = wait_first_failure(store.as_ref(), id).await;
    stop_runner(stop, handle).await;
    assert_eq!(job.attempts, 1);
    assert_eq!(job.last_error.as_deref(), Some("handler panicked"));
}

pub async fn runner_invalid_payload_version_is_dead(db: Arc<TestDb>) {
    let store = db.shared_store();
    let id = enqueue(store.as_ref(), NewJob::new(KIND, json!({"v": "one"}))).await;
    let (handler, log) = TestHandler::new(KIND, |_| async { Ok(()) }).build();
    let (stop, handle) = start(
        store.clone(),
        RunnerConfig::new(NodeId::new(), 2),
        vec![handler],
    );
    let job = wait_status(store.as_ref(), id, JobStatus::Dead, Duration::from_secs(5)).await;
    stop_runner(stop, handle).await;
    assert_eq!(job.last_error.as_deref(), Some("payload has no valid v"));
    assert!(calls(&log).is_empty());
}

pub async fn runner_extends_long_jobs(db: Arc<TestDb>) {
    let store = db.shared_store();
    let id = enqueue(store.as_ref(), NewJob::new(KIND, json!({"v": 1}))).await;
    let slow = |_| async {
        tokio::time::sleep(Duration::from_millis(2500)).await;
        Ok(())
    };
    let (a_handler, a_log) = TestHandler::new(KIND, slow).build();
    let (b_handler, b_log) = TestHandler::new(KIND, |_| async { Ok(()) }).build();
    let node_a = NodeId::new();
    let mut a_config = RunnerConfig::new(node_a, 1);
    a_config.lease = Duration::from_secs(1);
    a_config.extend_every = Duration::from_millis(300);
    let (a_stop, a) = start(store.clone(), a_config, vec![a_handler]);
    wait_running_on(store.as_ref(), id, node_a).await;

    let mut b_config = RunnerConfig::new(NodeId::new(), 2);
    b_config.idle_poll = Duration::from_millis(100);
    let (b_stop, b) = start(other_node_store(&db).await, b_config, vec![b_handler]);

    let job = wait_status(store.as_ref(), id, JobStatus::Done, Duration::from_secs(6)).await;
    stop_runner(a_stop, a).await;
    stop_runner(b_stop, b).await;
    assert_eq!(calls(&a_log), vec![id]);
    assert!(calls(&b_log).is_empty(), "node B never claimed the job");
    assert_eq!(job.attempts, 1);
}

/// Review Focus 3: node A's lease lapses, node B reclaims and completes; at its next
/// renewal A learns the lease is gone, drops the handler future and writes nothing.
pub async fn runner_cancels_handler_when_lease_lost(db: Arc<TestDb>) {
    let store = db.shared_store();
    let id = enqueue(store.as_ref(), NewJob::new(KIND, json!({"v": 1}))).await;
    let cancelled = Arc::new(AtomicBool::new(false));
    let flag = cancelled.clone();
    let (a_handler, a_log) = TestHandler::new(KIND, move |_| {
        let guard = DropFlag(flag.clone());
        async move {
            let _guard = guard;
            tokio::time::sleep(Duration::from_secs(60)).await;
            Ok(())
        }
    })
    .build();
    let (b_handler, b_log) = TestHandler::new(KIND, |_| async { Ok(()) }).build();
    let node_a = NodeId::new();
    let mut a_config = RunnerConfig::new(node_a, 1);
    a_config.lease = Duration::from_secs(1);
    a_config.extend_every = Duration::from_secs(5);
    let a_started = Instant::now();
    let (a_stop, a) = start(store.clone(), a_config, vec![a_handler]);
    wait_running_on(store.as_ref(), id, node_a).await;

    let node_b = NodeId::new();
    let mut b_config = RunnerConfig::new(node_b, 2);
    b_config.idle_poll = Duration::from_millis(100);
    let (b_stop, b) = start(other_node_store(&db).await, b_config, vec![b_handler]);
    let done = wait_status(store.as_ref(), id, JobStatus::Done, Duration::from_secs(5)).await;
    assert_eq!(done.attempts, 2, "B's claim is the second attempt");
    assert!(!cancelled.load(Ordering::SeqCst), "A's handler still runs");

    wait_for(Duration::from_secs(8), "A's handler cancelled", || async {
        cancelled.load(Ordering::SeqCst).then_some(())
    })
    .await;
    assert!(
        a_started.elapsed() >= Duration::from_secs(4),
        "cancelled at the renewal tick"
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(get(store.as_ref(), id).await, done, "A wrote nothing");
    stop_runner(a_stop, a).await;
    stop_runner(b_stop, b).await;
    assert_eq!(get(store.as_ref(), id).await, done);
    assert_eq!(calls(&a_log), vec![id]);
    assert_eq!(calls(&b_log), vec![id]);
}

pub async fn runner_releases_unknown_payload_version(db: Arc<TestDb>) {
    let store = db.shared_store();
    let id = enqueue(store.as_ref(), NewJob::new(KIND, json!({"v": 2}))).await;
    let (handler, log) = TestHandler::new(KIND, |_| async { Ok(()) }).build();
    let (stop, handle) = start(
        store.clone(),
        RunnerConfig::new(NodeId::new(), 2),
        vec![handler],
    );
    let job = wait_for(Duration::from_secs(5), "job released", || async {
        let job = get(store.as_ref(), id).await;
        let now = store.read().nodes().db_now().await.unwrap();
        (job.status == JobStatus::Queued && job.run_at > plus(now, Duration::from_secs(5)))
            .then_some((job, now))
    })
    .await;
    stop_runner(stop, handle).await;
    let (job, now) = job;
    assert_eq!(job.attempts, 0, "attempt refunded");
    assert_eq!((job.locked_by, job.last_error), (None, None));
    assert!(
        near(
            job.run_at,
            plus(now, Duration::from_secs(10)),
            Duration::from_secs(2)
        ),
        "run_at {:?} vs now {:?}",
        job.run_at,
        now
    );
    assert!(calls(&log).is_empty());
}

pub async fn runner_shutdown_releases_in_flight(db: Arc<TestDb>) {
    let store = db.shared_store();
    let id = enqueue(store.as_ref(), NewJob::new(KIND, json!({"v": 1}))).await;
    let (handler, _log) = TestHandler::new(KIND, |_| async {
        tokio::time::sleep(Duration::from_secs(10)).await;
        Ok(())
    })
    .build();
    let node = NodeId::new();
    let mut config = RunnerConfig::new(node, 2);
    config.drain = Duration::from_millis(300);
    let (stop, handle) = start(store.clone(), config, vec![handler]);
    wait_running_on(store.as_ref(), id, node).await;
    stop.cancel();
    tokio::time::timeout(Duration::from_secs(1), handle)
        .await
        .expect("runner ends within 1 s of stop")
        .unwrap();
    let job = get(store.as_ref(), id).await;
    assert_eq!(job.status, JobStatus::Queued);
    assert_eq!(
        (job.attempts, job.locked_by, job.locked_until),
        (0, None, None)
    );
}

pub async fn scheduler_inserts_one_job_per_period(db: Arc<TestDb>) {
    let first = db.shared_store();
    let second = other_node_store(&db).await;
    let periodic = Periodic {
        kind: "test.tick",
        every: Duration::from_secs(1),
        payload: json!({"v": 1}),
    };
    let stop = CancellationToken::new();
    let start_now = first.read().nodes().db_now().await.unwrap();
    let tick = Duration::from_millis(100);
    let a = Scheduler::new(first.clone(), vec![periodic.clone()], tick).spawn(stop.clone());
    let b = Scheduler::new(second, vec![periodic], tick).spawn(stop.clone());
    tokio::time::sleep(Duration::from_millis(3200)).await;
    stop.cancel();
    a.await.unwrap();
    b.await.unwrap();
    let end_now = first.read().nodes().db_now().await.unwrap();

    let jobs = first.read().jobs();
    let rows: u64 = jobs.counts().await.unwrap().values().sum();
    let secs = |t: Timestamp| t.as_micros().div_euclid(1_000_000);
    let (lo, hi) = (secs(start_now), secs(end_now));
    assert!(rows >= 3, "{rows} rows");
    assert!(
        rows <= u64::try_from(hi - lo + 1).unwrap(),
        "{rows} rows for {} periods",
        hi - lo + 1
    );
    // Every period fully inside the run has its row (the key is taken).
    for s in lo + 1..hi {
        let mut probe = NewJob::new("test.tick", json!({"v": 1}));
        probe.unique_key = Some(format!("test.tick:{s}"));
        assert_eq!(jobs.enqueue(&probe).await.unwrap(), None, "period {s}");
    }
}

/// Which row [`backdate`] moves.
enum Row {
    Seq(i64),
    /// A UUID primary key as 32 hex digits (SQLite stores its 16 bytes as a BLOB).
    Id(String),
}

fn job_row(id: JobId) -> Row {
    Row::Id(id.as_uuid().simple().to_string())
}

fn node_row(id: NodeId) -> Row {
    Row::Id(id.as_uuid().simple().to_string())
}

/// Moves a row's timestamp column back by `hours` (raw SQL per backend).
async fn backdate(db: &TestDb, table: &str, column: &str, row: Row, hours: i64) {
    let sqlite = db.store().backend() == Backend::Sqlite;
    let filter = match (row, sqlite) {
        (Row::Seq(seq), _) => format!("seq = {seq}"),
        (Row::Id(hex), true) => format!("id = X'{hex}'"),
        (Row::Id(hex), false) => format!("id = '{hex}'::uuid"),
    };
    let sql = if sqlite {
        format!(
            "UPDATE {table} SET {column} = {column} - {} WHERE {filter}",
            hours * 3600 * 1_000_000
        )
    } else {
        format!("UPDATE {table} SET {column} = {column} - interval '{hours} hours' WHERE {filter}")
    };
    db.exec_raw(&sql).await;
}

async fn add_node(store: &dyn Store) -> NodeId {
    let id = NodeId::new();
    store
        .read()
        .nodes()
        .register(&NewNode {
            id,
            name: format!("node-{id}"),
            version: "0.1.0".into(),
            schema_version: 4,
            meta: json!({}),
        })
        .await
        .unwrap();
    id
}

/// A `done` job of a kind no runner here handles.
async fn done_job(store: &dyn Store) -> JobId {
    let jobs = store.read().jobs();
    let id = enqueue(store, NewJob::new("test.finished", json!({}))).await;
    let claimed = jobs
        .claim(
            NodeId::new(),
            &["test.finished".to_owned()],
            1,
            Duration::from_secs(60),
        )
        .await
        .unwrap();
    assert!(jobs.complete(&claimed[0].lease().unwrap()).await.unwrap());
    id
}

pub async fn builtin_prune_jobs_delete_by_age(db: Arc<TestDb>) {
    let store = db.shared_store();
    let events = store.read().events();
    let event = |kind: &str| NewEvent {
        kind: kind.into(),
        keys: vec!["all".into()],
        origin_node: None,
    };
    let old_event = events.append(&event("test.old")).await.unwrap();
    let fresh_event = events.append(&event("test.fresh")).await.unwrap();
    backdate(&db, "events", "created_at", Row::Seq(old_event), 25).await;
    let stale_node = add_node(store.as_ref()).await;
    let live_node = add_node(store.as_ref()).await;
    backdate(&db, "nodes", "heartbeat_at", node_row(stale_node), 25).await;
    let old_job = done_job(store.as_ref()).await;
    let fresh_job = done_job(store.as_ref()).await;
    backdate(&db, "jobs", "finished_at", job_row(old_job), 8 * 24).await;

    let stop = CancellationToken::new();
    let scheduler = Scheduler::new(store.clone(), builtin::periodics(), Duration::from_secs(10))
        .spawn(stop.clone());
    let runner = JobRunner::new(
        store.clone(),
        RunnerConfig::new(NodeId::new(), 2),
        builtin::handlers(store.clone()),
    )
    .unwrap()
    .spawn(stop.clone());
    let jobs = store.read().jobs();
    wait_for(Duration::from_secs(5), "three prune jobs done", || async {
        let counts = jobs.counts().await.unwrap();
        // Once the old fixture job is pruned: the fresh one plus the three prune jobs.
        let gone = jobs.get(old_job).await.unwrap().is_none();
        (gone && counts[&JobStatus::Done] == 4).then_some(())
    })
    .await;
    stop.cancel();
    scheduler.await.unwrap();
    runner.await.unwrap();

    let seqs: Vec<i64> = events
        .since(0, 100)
        .await
        .unwrap()
        .iter()
        .map(|e| e.seq)
        .collect();
    assert_eq!(seqs, vec![fresh_event]);
    let nodes: Vec<NodeId> = store
        .read()
        .nodes()
        .list()
        .await
        .unwrap()
        .iter()
        .map(|n| n.id)
        .collect();
    assert_eq!(nodes, vec![live_node]);
    assert!(jobs.get(fresh_job).await.unwrap().is_some());
    let counts = jobs.counts().await.unwrap();
    assert_eq!(
        counts[&JobStatus::Done],
        4,
        "fresh fixture + three prune jobs"
    );
}

backend_tests_spawning!(
    runner_runs_and_completes,
    runner_retries_then_dead,
    runner_permanent_error_is_dead_at_once,
    runner_handler_timeout_counts_as_failure,
    runner_handler_panic_is_a_failure,
    runner_invalid_payload_version_is_dead,
    runner_extends_long_jobs,
    runner_cancels_handler_when_lease_lost,
    runner_releases_unknown_payload_version,
    runner_shutdown_releases_in_flight,
    scheduler_inserts_one_job_per_period,
    builtin_prune_jobs_delete_by_age,
);

/// Postgres only: with `listen`, a job enqueued while the runner idles (30 s poll)
/// starts on its notice.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn runner_wakes_on_notify() {
    let Some(db) = TestDb::postgres().await else {
        return;
    };
    let store = db.shared_store();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let (handler, _log) = TestHandler::new(KIND, move |_| {
        let tx = tx.clone();
        async move {
            tx.send(Instant::now()).unwrap();
            Ok(())
        }
    })
    .build();
    let mut config = RunnerConfig::new(NodeId::new(), 2);
    config.idle_poll = Duration::from_secs(30);
    config.listen = true;
    let (stop, handle) = start(store.clone(), config, vec![handler]);
    // Let the first claim and the subscription settle.
    tokio::time::sleep(Duration::from_secs(1)).await;
    let enqueued = Instant::now();
    enqueue(store.as_ref(), NewJob::new(KIND, json!({"v": 1}))).await;
    let started = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("job started")
        .unwrap();
    let latency = started - enqueued;
    assert!(
        latency < Duration::from_millis(500),
        "started after {latency:?}"
    );
    stop_runner(stop, handle).await;
    db.teardown().await;
}

#[tokio::test]
async fn duplicate_handler_kinds_are_refused() {
    let db = TestDb::sqlite().await;
    let (a, _) = TestHandler::new(KIND, |_| async { Ok(()) }).build();
    let (b, _) = TestHandler::new(KIND, |_| async { Ok(()) }).build();
    let err = JobRunner::new(
        db.shared_store(),
        RunnerConfig::new(NodeId::new(), 2),
        vec![a, b],
    )
    .err()
    .expect("duplicate refused");
    assert!(matches!(err, ClusterError::DuplicateHandler(k) if k == KIND));
    let (bad, _) = TestHandler::new("Bad Kind", |_| async { Ok(()) }).build();
    assert!(matches!(
        JobRunner::new(
            db.shared_store(),
            RunnerConfig::new(NodeId::new(), 2),
            vec![bad]
        ),
        Err(ClusterError::Store(_))
    ));
}
