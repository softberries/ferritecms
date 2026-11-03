//! Job runner, scheduler and built-in prune jobs on both backends (04 §6, P5, P9–P11).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{
    collections::BTreeMap,
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
    ClusterError, JobCounters, JobError, JobHandler, JobRunner, Periodic, RunnerConfig, Scheduler,
    builtin,
};
use ferrite_domain::{
    ApiToken, ApiTokenId, ClientKind, IDEMPOTENCY_RETENTION, IDEMPOTENCY_STALE, IdemBegin,
    IdempotencyKey, Job, JobId, JobLease, JobQuery, JobStatus, NewEvent, NewJob, NewNode, NodeId,
    OAuthClient, OAuthClientId, OAuthGrant, OAuthGrantId, Page, Secret, Session, Timestamp,
};
use ferrite_store::{
    ApiTokenRepo, ArtifactRepo, AuditRepo, AutosaveRepo, Backend, Channel, CommentRepo,
    ContentRepo, ContentTypeRepo, EventRepo, IdempotencyRepo, ImportRepo, JobRepo, LockGuard,
    MediaRepo, MenuRepo, MetaRepo, MigrationStatus, NodeMetricsRepo, NodeRepo, OAuthRepo,
    OptionRepo, PluginFailureRepo, PluginKvRepo, PluginRepo, RateLimitRepo, RedirectRepo, Repos,
    RevisionRepo, SchemaMetaRepo, SearchRepo, SessionRepo, Store, StoreError, Subscription,
    TaxonomyRepo, TermRepo, ThemeRepo, Tx, UserRepo,
};
use ferrite_store_tests::{
    TestDb, backend_tests_spawning, fixtures::fixture_user, window::in_one_window,
};
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
    let job = wait_first_failure(store.as_ref(), id).await;
    assert_eq!((job.attempts, job.last_error.as_deref()), (1, Some("boom")));
    db.force_job_due(id).await;
    let job = wait_status(store.as_ref(), id, JobStatus::Dead, Duration::from_secs(5)).await;
    stop_runner(stop, handle).await;
    assert_eq!(job.attempts, 2);
    assert_eq!(job.last_error.as_deref(), Some("boom"));
    assert_eq!(calls(&log), vec![id, id]);
}

/// M4-2 P35: the runner counts the jobs it completed and the attempts that failed
/// (retry or permanent), never a `Busy` release.
pub async fn runner_counts_finished_jobs(db: Arc<TestDb>) {
    let store = db.shared_store();
    for outcome in ["ok", "ok", "retry", "permanent", "busy"] {
        let mut new = NewJob::new(KIND, json!({"v": 1, "outcome": outcome}));
        new.max_attempts = 1;
        enqueue(store.as_ref(), new).await;
    }
    let (handler, _log) = TestHandler::new(KIND, |job| async move {
        match job.payload["outcome"].as_str() {
            Some("ok") => Ok(()),
            Some("retry") => Err(JobError::Retry("again".into())),
            Some("busy") => Err(JobError::Busy("later".into())),
            _ => Err(JobError::Permanent("never".into())),
        }
    })
    .build();
    let counters = Arc::new(JobCounters::default());
    let stop = CancellationToken::new();
    let handle = JobRunner::new(
        store.clone(),
        RunnerConfig::new(NodeId::new(), 2),
        vec![handler],
    )
    .unwrap()
    .with_counters(counters.clone())
    .spawn(stop.clone());
    wait_for(Duration::from_secs(5), "jobs counted", || async {
        (counters.done() == 2 && counters.failed() == 2).then_some(())
    })
    .await;
    stop_runner(stop, handle).await;
    assert_eq!((counters.done(), counters.failed()), (2, 2));
}

/// R20(3): `Busy` puts the job back with the attempt refunded, due again after the
/// short busy delay, so a burst of busy runs never uses up `max_attempts`.
pub async fn runner_busy_refunds_the_attempt(db: Arc<TestDb>) {
    let store = db.shared_store();
    let mut new = NewJob::new(KIND, json!({"v": 1}));
    new.max_attempts = 2;
    let id = enqueue(store.as_ref(), new).await;
    const BUSY_RUNS: usize = 5;
    let runs = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    // When each run started: the busy release is due a delay after it, which a slow test
    // can read only after that time has passed (`run_at - now` is then negative).
    let started = Arc::new(Mutex::new(Vec::<Timestamp>::new()));
    let (handler, log) = TestHandler::new(KIND, {
        let runs = runs.clone();
        let started = started.clone();
        move |_| {
            started.lock().unwrap().push(Timestamp::now());
            let n = runs.fetch_add(1, Ordering::SeqCst);
            async move {
                if n < BUSY_RUNS {
                    Err(JobError::Busy("no slot".into()))
                } else {
                    Ok(())
                }
            }
        }
    })
    .build();
    let (stop, handle) = start(
        store.clone(),
        RunnerConfig::new(NodeId::new(), 2),
        vec![handler],
    );
    for n in 1..=BUSY_RUNS {
        let job = wait_for(Duration::from_secs(5), "busy release", || async {
            let job = get(store.as_ref(), id).await;
            (calls(&log).len() == n && job.status == JobStatus::Queued).then_some(job)
        })
        .await;
        assert_eq!(job.attempts, 0, "run {n}: attempt refunded");
        assert_eq!(job.last_error, None, "run {n}");
        let ran_at = started.lock().unwrap()[n - 1];
        let due_after = job.run_at.as_micros() - ran_at.as_micros();
        assert!(
            (1_000_000..=5_500_000).contains(&due_after),
            "run {n}: due {due_after} µs after the run started"
        );
        db.force_job_due(id).await;
    }
    let job = wait_status(store.as_ref(), id, JobStatus::Done, Duration::from_secs(5)).await;
    stop_runner(stop, handle).await;
    assert_eq!(job.attempts, 1, "only the run that did the work counted");
    assert_eq!(calls(&log).len(), BUSY_RUNS + 1);
}

#[test]
fn busy_delay_is_short_and_jittered() {
    let delays: Vec<Duration> = (0..50).map(|_| ferrite_cluster::busy_delay()).collect();
    assert!(
        delays
            .iter()
            .all(|d| *d >= Duration::from_secs(1) && *d < Duration::from_secs(5)),
        "{delays:?}"
    );
    assert!(
        delays.iter().any(|d| *d != delays[0]),
        "jittered: {delays:?}"
    );
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
/// (With working renewals a lease only lapses under A while A is stalled, so the test
/// expires it in the database directly.)
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
    a_config.lease = Duration::from_secs(4);
    a_config.extend_every = Duration::from_secs(2);
    let a_started = Instant::now();
    let (a_stop, a) = start(store.clone(), a_config, vec![a_handler]);
    wait_running_on(store.as_ref(), id, node_a).await;
    // A stalls (paused process, R4's remaining bound): its lease lapses in the database
    // while A's own fence (3 s) has not fired yet.
    backdate(&db, "jobs", "locked_until", job_row(id), 1).await;

    let node_b = NodeId::new();
    let mut b_config = RunnerConfig::new(node_b, 2);
    b_config.idle_poll = Duration::from_millis(100);
    let (b_stop, b) = start(other_node_store(&db).await, b_config, vec![b_handler]);
    let done = wait_status(store.as_ref(), id, JobStatus::Done, Duration::from_secs(5)).await;
    assert_eq!(done.attempts, 2, "B's claim is the second attempt");
    assert!(!cancelled.load(Ordering::SeqCst), "A's handler still runs");

    wait_for(Duration::from_secs(4), "A's handler cancelled", || async {
        cancelled.load(Ordering::SeqCst).then_some(())
    })
    .await;
    let after = a_started.elapsed();
    assert!(
        after >= Duration::from_millis(1900) && after < Duration::from_secs(3),
        "cancelled at the renewal tick (2 s), not by the fence (3 s): {after:?}"
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(get(store.as_ref(), id).await, done, "A wrote nothing");
    stop_runner(a_stop, a).await;
    stop_runner(b_stop, b).await;
    assert_eq!(get(store.as_ref(), id).await, done);
    assert_eq!(calls(&a_log), vec![id]);
    assert_eq!(calls(&b_log), vec![id]);
}

/// What a [`FaultyStore`] breaks in its `JobRepo`; everything else goes to the real
/// store.
#[derive(Clone, Copy)]
struct Faults {
    /// Claims that go through before every later claim hangs forever.
    claims_before_hang: usize,
    /// `extend` hangs forever (black-holed connection) instead of failing at once.
    hang_extend: bool,
}

/// A store whose job renewals always fail (error, or hang with `hang_extend`) and whose
/// claims can hang (database unreachable).
struct FaultyStore {
    inner: Arc<dyn Store>,
    repos: FaultyRepos,
}

struct FaultyRepos {
    inner: Arc<dyn Store>,
    jobs: FaultyJobs,
}

struct FaultyJobs {
    inner: Arc<dyn Store>,
    faults: Faults,
    claims: std::sync::atomic::AtomicUsize,
}

impl FaultyStore {
    fn new(inner: Arc<dyn Store>, faults: Faults) -> Self {
        Self {
            repos: FaultyRepos {
                jobs: FaultyJobs {
                    inner: inner.clone(),
                    faults,
                    claims: Default::default(),
                },
                inner: inner.clone(),
            },
            inner,
        }
    }
}

#[async_trait]
impl Store for FaultyStore {
    fn backend(&self) -> Backend {
        self.inner.backend()
    }
    fn pool_stats(&self) -> ferrite_store::PoolStats {
        self.inner.pool_stats()
    }
    fn read(&self) -> &dyn Repos {
        &self.repos
    }
    async fn begin(&self) -> Result<Box<dyn Tx>, StoreError> {
        self.inner.begin().await
    }
    async fn subscribe(
        &self,
        channel: Channel,
    ) -> Result<Option<Box<dyn Subscription>>, StoreError> {
        self.inner.subscribe(channel).await
    }
    async fn try_lock(&self, name: &str) -> Result<Option<Box<dyn LockGuard>>, StoreError> {
        self.inner.try_lock(name).await
    }
    async fn migration_status(
        &self,
        binary: &semver::Version,
    ) -> Result<MigrationStatus, StoreError> {
        self.inner.migration_status(binary).await
    }
    async fn migrate_up(
        &self,
        to: Option<i64>,
    ) -> Result<ferrite_store::MigrateReport, StoreError> {
        self.inner.migrate_up(to).await
    }
    async fn migrate_down(&self, to: i64, allow_lossy: bool) -> Result<Vec<i64>, StoreError> {
        self.inner.migrate_down(to, allow_lossy).await
    }
    async fn verify_migrations(&self) -> Result<(), StoreError> {
        self.inner.verify_migrations().await
    }
    async fn export_reader(&self) -> Result<Box<dyn ferrite_store::ExportReader>, StoreError> {
        self.inner.export_reader().await
    }
    async fn import_writer(&self) -> Result<Box<dyn ferrite_store::ImportWriter>, StoreError> {
        self.inner.import_writer().await
    }
    async fn snapshot(&self, dest: &std::path::Path) -> Result<(), StoreError> {
        self.inner.snapshot(dest).await
    }
    async fn close(&self) {
        self.inner.close().await;
    }
}

impl Repos for FaultyRepos {
    fn content(&self) -> &dyn ContentRepo {
        self.inner.read().content()
    }
    fn meta(&self) -> &dyn MetaRepo {
        self.inner.read().meta()
    }
    fn revisions(&self) -> &dyn RevisionRepo {
        self.inner.read().revisions()
    }
    fn types(&self) -> &dyn ContentTypeRepo {
        self.inner.read().types()
    }
    fn taxonomies(&self) -> &dyn TaxonomyRepo {
        self.inner.read().taxonomies()
    }
    fn terms(&self) -> &dyn TermRepo {
        self.inner.read().terms()
    }
    fn menus(&self) -> &dyn MenuRepo {
        self.inner.read().menus()
    }
    fn options(&self) -> &dyn OptionRepo {
        self.inner.read().options()
    }
    fn redirects(&self) -> &dyn RedirectRepo {
        self.inner.read().redirects()
    }
    fn users(&self) -> &dyn UserRepo {
        self.inner.read().users()
    }
    fn artifacts(&self) -> &dyn ArtifactRepo {
        self.inner.read().artifacts()
    }
    fn media(&self) -> &dyn MediaRepo {
        self.inner.read().media()
    }
    fn plugins(&self) -> &dyn PluginRepo {
        self.inner.read().plugins()
    }
    fn plugin_failures(&self) -> &dyn PluginFailureRepo {
        self.inner.read().plugin_failures()
    }
    fn plugin_kv(&self) -> &dyn PluginKvRepo {
        self.inner.read().plugin_kv()
    }
    fn themes(&self) -> &dyn ThemeRepo {
        self.inner.read().themes()
    }
    fn events(&self) -> &dyn EventRepo {
        self.inner.read().events()
    }
    fn jobs(&self) -> &dyn JobRepo {
        &self.jobs
    }
    fn nodes(&self) -> &dyn NodeRepo {
        self.inner.read().nodes()
    }
    fn search(&self) -> &dyn SearchRepo {
        self.inner.read().search()
    }
    fn schema_meta(&self) -> &dyn SchemaMetaRepo {
        self.inner.read().schema_meta()
    }
    fn sessions(&self) -> &dyn SessionRepo {
        self.inner.read().sessions()
    }
    fn api_tokens(&self) -> &dyn ApiTokenRepo {
        self.inner.read().api_tokens()
    }
    fn rate_limits(&self) -> &dyn RateLimitRepo {
        self.inner.read().rate_limits()
    }
    fn audit(&self) -> &dyn AuditRepo {
        self.inner.read().audit()
    }
    fn comments(&self) -> &dyn CommentRepo {
        self.inner.read().comments()
    }
    fn idempotency(&self) -> &dyn IdempotencyRepo {
        self.inner.read().idempotency()
    }
    fn autosaves(&self) -> &dyn AutosaveRepo {
        self.inner.read().autosaves()
    }
    fn imports(&self) -> &dyn ImportRepo {
        self.inner.read().imports()
    }
    fn node_metrics(&self) -> &dyn NodeMetricsRepo {
        self.inner.read().node_metrics()
    }
    fn oauth(&self) -> &dyn OAuthRepo {
        self.inner.read().oauth()
    }
    fn locales(&self) -> &dyn ferrite_store::LocaleRepo {
        self.inner.read().locales()
    }

    fn webhooks(&self) -> &dyn ferrite_store::WebhookRepo {
        self.inner.read().webhooks()
    }

    fn plugin_trust(&self) -> &dyn ferrite_store::PluginTrustRepo {
        self.inner.read().plugin_trust()
    }
}

#[async_trait]
impl JobRepo for FaultyJobs {
    async fn enqueue(&self, job: &NewJob) -> Result<Option<JobId>, StoreError> {
        self.inner.read().jobs().enqueue(job).await
    }
    async fn claim(
        &self,
        node: NodeId,
        kinds: &[String],
        limit: u32,
        lease: Duration,
    ) -> Result<Vec<Job>, StoreError> {
        if self.claims.fetch_add(1, Ordering::SeqCst) >= self.faults.claims_before_hang {
            std::future::pending::<()>().await;
        }
        self.inner
            .read()
            .jobs()
            .claim(node, kinds, limit, lease)
            .await
    }
    async fn extend(&self, _lease: &JobLease, _duration: Duration) -> Result<bool, StoreError> {
        if self.faults.hang_extend {
            std::future::pending::<()>().await;
        }
        Err(StoreError::Database("injected renewal failure".into()))
    }
    async fn complete(&self, lease: &JobLease) -> Result<bool, StoreError> {
        self.inner.read().jobs().complete(lease).await
    }
    async fn fail(
        &self,
        lease: &JobLease,
        error: &str,
        permanent: bool,
    ) -> Result<Option<JobStatus>, StoreError> {
        self.inner.read().jobs().fail(lease, error, permanent).await
    }
    async fn release(
        &self,
        lease: &JobLease,
        delay: Duration,
        count_attempt: bool,
    ) -> Result<bool, StoreError> {
        self.inner
            .read()
            .jobs()
            .release(lease, delay, count_attempt)
            .await
    }
    async fn get(&self, id: JobId) -> Result<Option<Job>, StoreError> {
        self.inner.read().jobs().get(id).await
    }
    async fn list(&self, query: &JobQuery) -> Result<Page<Job>, StoreError> {
        self.inner.read().jobs().list(query).await
    }
    async fn retry_dead(&self, id: JobId) -> Result<Option<Job>, StoreError> {
        self.inner.read().jobs().retry_dead(id).await
    }
    async fn counts(&self) -> Result<BTreeMap<JobStatus, u64>, StoreError> {
        self.inner.read().jobs().counts().await
    }
    async fn prune(
        &self,
        done_before: Timestamp,
        dead_before: Timestamp,
        limit: u32,
    ) -> Result<u64, StoreError> {
        self.inner
            .read()
            .jobs()
            .prune(done_before, dead_before, limit)
            .await
    }
}

/// R4: renewals fail (no answer whether the lease still holds), so the runner drops the
/// handler at `lease - extend_every / 2` after the claim, before the lease can lapse
/// and another node could take the job, and writes nothing for it.
pub async fn runner_fences_handler_when_renewals_fail(db: Arc<TestDb>) {
    let store = db.shared_store();
    let id = enqueue(store.as_ref(), NewJob::new(KIND, json!({"v": 1}))).await;
    let cancelled = Arc::new(AtomicBool::new(false));
    let flag = cancelled.clone();
    let (handler, log) = TestHandler::new(KIND, move |_| {
        let guard = DropFlag(flag.clone());
        async move {
            let _guard = guard;
            tokio::time::sleep(Duration::from_secs(60)).await;
            Ok(())
        }
    })
    .build();
    let node = NodeId::new();
    let mut config = RunnerConfig::new(node, 1);
    config.lease = Duration::from_secs(2);
    config.extend_every = Duration::from_millis(400);
    let failing: Arc<dyn Store> = Arc::new(FaultyStore::new(
        store.clone(),
        Faults {
            claims_before_hang: usize::MAX,
            hang_extend: false,
        },
    ));
    let before_claim = Instant::now();
    let (stop, handle) = start(failing, config, vec![handler]);
    let running = wait_running_on(store.as_ref(), id, node).await;
    wait_for(Duration::from_secs(3), "handler fenced", || async {
        cancelled.load(Ordering::SeqCst).then_some(())
    })
    .await;
    let fenced_after = before_claim.elapsed();
    // Fence at 2 s - 0.2 s after the claim; this test polls every 20 ms.
    assert!(
        fenced_after >= Duration::from_millis(1700) && fenced_after < Duration::from_millis(2500),
        "fenced after {fenced_after:?}"
    );
    // Nothing was written: the job is still running under this node's live lease.
    let now = get(store.as_ref(), id).await;
    assert_eq!(now, running);
    assert!(now.locked_until.unwrap() > store.read().nodes().db_now().await.unwrap());
    stop_runner(stop, handle).await;
    // The fenced job is not released at shutdown either: its lease just expires.
    assert_eq!(get(store.as_ref(), id).await, running);
    assert_eq!(calls(&log), vec![id]);
}

/// R4 with a black-holed connection: after the first claim every claim and renewal
/// hangs forever. Inline store calls are bounded (`extend_every / 2`), so the loop
/// still reaches the fence and drops the in-flight handler near its deadline.
pub async fn runner_fences_handler_while_store_hangs(db: Arc<TestDb>) {
    let store = db.shared_store();
    let id = enqueue(store.as_ref(), NewJob::new(KIND, json!({"v": 1}))).await;
    let cancelled = Arc::new(AtomicBool::new(false));
    let flag = cancelled.clone();
    let (handler, log) = TestHandler::new(KIND, move |_| {
        let guard = DropFlag(flag.clone());
        async move {
            let _guard = guard;
            tokio::time::sleep(Duration::from_secs(60)).await;
            Ok(())
        }
    })
    .build();
    let node = NodeId::new();
    // Two slots and a fast poll: the loop keeps claiming (and hanging) while the job runs.
    let mut config = RunnerConfig::new(node, 2);
    config.lease = Duration::from_secs(2);
    config.extend_every = Duration::from_millis(400);
    config.idle_poll = Duration::from_millis(50);
    let faulty: Arc<dyn Store> = Arc::new(FaultyStore::new(
        store.clone(),
        Faults {
            claims_before_hang: 1,
            hang_extend: true,
        },
    ));
    let before_claim = Instant::now();
    let (stop, handle) = start(faulty, config, vec![handler]);
    let running = wait_running_on(store.as_ref(), id, node).await;
    wait_for(Duration::from_secs(4), "handler fenced", || async {
        cancelled.load(Ordering::SeqCst).then_some(())
    })
    .await;
    let fenced_after = before_claim.elapsed();
    // Fence at 1.8 s; a stuck call is cut after 0.2 s, so the fence is at most that late.
    assert!(
        fenced_after >= Duration::from_millis(1700) && fenced_after < Duration::from_millis(2500),
        "fenced after {fenced_after:?}"
    );
    assert_eq!(get(store.as_ref(), id).await, running, "nothing written");
    stop_runner(stop, handle).await;
    assert_eq!(calls(&log), vec![id]);
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

/// A periodic whose offset is not below its period is refused when the scheduler is built
/// (in release builds too), not shifted into a later period under this period's key.
pub async fn scheduler_refuses_an_offset_past_its_period(db: Arc<TestDb>) {
    let periodic = |offset| Periodic {
        kind: "test.tick",
        every: Duration::from_secs(60),
        payload: json!({"v": 1}),
        max_attempts: 3,
        offset,
    };
    let tick = Duration::from_secs(10);
    for offset in [Duration::from_secs(60), Duration::from_secs(3600)] {
        let err = Scheduler::new(db.shared_store(), vec![periodic(offset)], tick)
            .err()
            .expect("refused");
        assert!(
            matches!(
                &err,
                ClusterError::PeriodicOffset {
                    kind: "test.tick",
                    ..
                }
            ),
            "{err}"
        );
    }
    // The last second of the period is fine, and one bad periodic among good ones is
    // still refused.
    Scheduler::new(
        db.shared_store(),
        vec![periodic(Duration::from_secs(59))],
        tick,
    )
    .unwrap();
    let mixed = vec![periodic(Duration::ZERO), periodic(Duration::from_secs(61))];
    assert!(Scheduler::new(db.shared_store(), mixed, tick).is_err());
}

pub async fn scheduler_inserts_one_job_per_period(db: Arc<TestDb>) {
    let first = db.shared_store();
    let second = other_node_store(&db).await;
    let periodic = Periodic {
        kind: "test.tick",
        every: Duration::from_secs(1),
        payload: json!({"v": 1}),
        // Not the default (5): the inserted jobs must carry the periodic's own value.
        max_attempts: 3,
        offset: Duration::ZERO,
    };
    let stop = CancellationToken::new();
    let start_now = first.read().nodes().db_now().await.unwrap();
    let tick = Duration::from_millis(100);
    let a = Scheduler::new(first.clone(), vec![periodic.clone()], tick)
        .unwrap()
        .spawn(stop.clone());
    let b = Scheduler::new(second, vec![periodic], tick)
        .unwrap()
        .spawn(stop.clone());
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
    let listed = jobs
        .list(&JobQuery {
            status: None,
            kind: Some("test.tick".into()),
            limit: 100,
            cursor: None,
        })
        .await
        .unwrap();
    assert!(!listed.items.is_empty());
    for job in &listed.items {
        assert_eq!(job.max_attempts, 3, "{job:?}");
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

/// A `done` (or, with `dead`, a `dead`) job of a kind no runner here handles.
async fn finished_job(store: &dyn Store, dead: bool) -> JobId {
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
    let lease = claimed[0].lease().unwrap();
    if dead {
        let status = jobs.fail(&lease, "gave up", true).await.unwrap();
        assert_eq!(status, Some(JobStatus::Dead));
    } else {
        assert!(jobs.complete(&lease).await.unwrap());
    }
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
    let old_job = finished_job(store.as_ref(), false).await;
    let fresh_job = finished_job(store.as_ref(), false).await;
    backdate(&db, "jobs", "finished_at", job_row(old_job), 8 * 24).await;
    // Dead jobs stay 30 days: one at 31 days goes, one at 10 days (past the done
    // cut-off) stays.
    let old_dead = finished_job(store.as_ref(), true).await;
    let recent_dead = finished_job(store.as_ref(), true).await;
    backdate(&db, "jobs", "finished_at", job_row(old_dead), 31 * 24).await;
    backdate(&db, "jobs", "finished_at", job_row(recent_dead), 10 * 24).await;
    // Failure buckets: retention is max(24 h, 2 × 10 min) = 24 h.
    let failures = store.read().plugin_failures();
    let now = store.read().nodes().db_now().await.unwrap().as_micros();
    let hours_ago = |h: i64| Timestamp::from_micros(now - h * 3_600_000_000).unwrap();
    failures
        .add("old", "render.head", hours_ago(25), 3)
        .await
        .unwrap();
    failures
        .add("fresh", "render.head", hours_ago(23), 2)
        .await
        .unwrap();
    // Node metrics: kept 7 days.
    let metrics = store.read().node_metrics();
    let metrics_node = NodeId::new();
    let minute_ago = |h: i64| {
        let t = now - h * 3_600_000_000;
        Timestamp::from_micros(t - t.rem_euclid(60_000_000)).unwrap()
    };
    let one = ferrite_domain::MetricCounts {
        requests: 1,
        ..Default::default()
    };
    metrics
        .insert(metrics_node, minute_ago(7 * 24 + 1), &one)
        .await
        .unwrap();
    metrics
        .insert(metrics_node, minute_ago(7 * 24 - 1), &one)
        .await
        .unwrap();

    let stop = CancellationToken::new();
    let scheduler = Scheduler::new(store.clone(), builtin::periodics(), Duration::from_secs(10))
        .unwrap()
        .spawn(stop.clone());
    let runner = JobRunner::new(
        store.clone(),
        RunnerConfig::new(NodeId::new(), 2),
        builtin::handlers(
            store.clone(),
            builtin::failure_retention(Duration::from_secs(600)),
        ),
    )
    .unwrap()
    .spawn(stop.clone());
    let jobs = store.read().jobs();
    wait_for(Duration::from_secs(5), "six prune jobs done", || async {
        let counts = jobs.counts().await.unwrap();
        // Once the old fixture job is pruned: the fresh one plus the six prune jobs.
        let gone = jobs.get(old_job).await.unwrap().is_none()
            && jobs.get(old_dead).await.unwrap().is_none();
        (gone && counts[&JobStatus::Done] == 7).then_some(())
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
    assert_eq!(
        get(store.as_ref(), recent_dead).await.status,
        JobStatus::Dead
    );
    let since = Timestamp::from_micros(0).unwrap();
    assert_eq!(failures.sum_since("old", since).await.unwrap(), 0);
    assert_eq!(failures.sum_since("fresh", since).await.unwrap(), 2);
    let left = metrics
        .series(
            since,
            Timestamp::from_micros(now + 1).unwrap(),
            ferrite_domain::SeriesStep::Day,
            Some(metrics_node),
        )
        .await
        .unwrap();
    assert_eq!(
        left.iter().map(|b| b.counts.requests).sum::<u64>(),
        1,
        "the 8-day-old minute went, the 6-day-old one stayed"
    );
    let counts = jobs.counts().await.unwrap();
    assert_eq!(
        counts[&JobStatus::Done],
        7,
        "fresh fixture + six prune jobs"
    );
    assert_eq!(counts[&JobStatus::Dead], 1, "the 10-day-old dead job");
}

/// Moves the rate-limit bucket's window start back by `hours` (raw SQL per backend).
async fn backdate_bucket(db: &TestDb, bucket: &str, hours: i64) {
    let sql = if db.store().backend() == Backend::Sqlite {
        format!(
            "UPDATE rate_limits SET window_start = window_start - {} WHERE bucket = '{bucket}'",
            hours * 3600 * 1_000_000
        )
    } else {
        format!(
            "UPDATE rate_limits SET window_start = window_start - interval '{hours} hours' \
             WHERE bucket = '{bucket}'"
        )
    };
    db.exec_raw(&sql).await;
}

/// P23: `sessions.prune` deletes sessions past their absolute end or idle for longer
/// than `SESSION_IDLE`, tokens past `expires_at`, expired OAuth grants (design 12 §4) and
/// rate-limit rows whose window started over an hour ago; fresh rows of each stay.
pub async fn sessions_prune_job_deletes_expired_and_idle(db: Arc<TestDb>) {
    let store = db.shared_store();
    let repos = store.read();
    let user = fixture_user(store.as_ref()).await;
    let now = repos.nodes().db_now().await.unwrap();
    let ago = |d: Duration| {
        Timestamp::from_micros(now.as_micros() - i64::try_from(d.as_micros()).unwrap()).unwrap()
    };
    let day = Duration::from_secs(24 * 3600);
    let session = |id: &str, created: Timestamp, seen: Timestamp, expires: Timestamp| Session {
        id_hash: id.into(),
        user_id: user.id,
        csrf_secret: Secret::new(format!("csrf-{id}")),
        created_at: created,
        last_seen_at: seen,
        expires_at: expires,
        ip_hash: None,
        user_agent: None,
    };
    let sessions = repos.sessions();
    // Past the absolute end, though seen a minute ago.
    sessions
        .insert(&session(
            "expired",
            ago(31 * day),
            ago(Duration::from_secs(60)),
            ago(day),
        ))
        .await
        .unwrap();
    // Idle for 8 days, absolute end ahead.
    sessions
        .insert(&session(
            "idle",
            ago(10 * day),
            ago(8 * day),
            plus(now, 20 * day),
        ))
        .await
        .unwrap();
    // Seen 6 days ago, absolute end ahead.
    sessions
        .insert(&session(
            "fresh",
            ago(6 * day),
            ago(6 * day),
            plus(now, 24 * day),
        ))
        .await
        .unwrap();

    let token = |hash: &str, expires_at: Option<Timestamp>| ApiToken {
        id: ApiTokenId::new(),
        user_id: user.id,
        name: hash.into(),
        token_hash: hash.into(),
        scopes: vec!["content:read".into()],
        created_at: ago(40 * day),
        last_used_at: None,
        expires_at,
        oauth_grant_id: None,
    };
    let tokens = repos.api_tokens();
    tokens
        .insert(&token("expired", Some(ago(Duration::from_secs(1)))))
        .await
        .unwrap();
    tokens.insert(&token("forever", None)).await.unwrap();
    tokens
        .insert(&token("ahead", Some(plus(now, day))))
        .await
        .unwrap();

    let oauth = repos.oauth();
    let client = OAuthClient {
        id: OAuthClientId::new("ferc_prune_client_00000001"),
        kind: ClientKind::Registered,
        name: "Prune".into(),
        redirect_uris: vec!["http://127.0.0.1/cb".into()],
        client_uri: None,
        logo_uri: None,
        created_at: ago(40 * day),
        fetched_at: None,
        last_used_at: None,
    };
    oauth.upsert_client(&client).await.unwrap();
    let grant = |expires_at: Timestamp| OAuthGrant {
        id: OAuthGrantId::new(),
        client_id: client.id.clone(),
        user_id: user.id,
        scopes: vec!["content:read".into()],
        created_at: ago(40 * day),
        last_used_at: ago(31 * day),
        expires_at,
    };
    let (ended, alive) = (grant(ago(day)), grant(plus(now, day)));
    oauth.insert_grant(&ended, "refresh-ended").await.unwrap();
    oauth.insert_grant(&alive, "refresh-alive").await.unwrap();

    let limits = repos.rate_limits();
    let minute = Duration::from_secs(60);
    let handler = builtin::handlers(store.clone(), builtin::MIN_FAILURE_RETENTION)
        .into_iter()
        .find(|h| h.kind() == "sessions.prune")
        .expect("sessions.prune is a built-in handler");
    assert!(
        builtin::periodics()
            .iter()
            .any(|p| p.kind == "sessions.prune" && p.every == builtin::PRUNE_EVERY),
        "sessions.prune runs hourly"
    );
    let id = enqueue(
        store.as_ref(),
        NewJob::new("sessions.prune", json!({"v": 1})),
    )
    .await;

    // Both fresh hits and the last one must share one window (F10): at least 30 s of it
    // left, and a run again in the next window when it rolled over (the prune is
    // idempotent; the hits start over in the new window).
    let (db, store, limits, handler, sessions, tokens, user) =
        (&db, &store, &limits, &handler, &sessions, &tokens, &user);
    in_one_window(
        store.as_ref(),
        minute,
        Duration::from_secs(30),
        |_| async move {
            limits.hit("login:ip:old", minute).await.unwrap();
            limits.hit("login:ip:fresh", minute).await.unwrap();
            backdate_bucket(db, "login:ip:old", 2).await;

            handler.run(&get(store.as_ref(), id).await).await.unwrap();

            let ids: Vec<String> = sessions
                .list_for_user(user.id)
                .await
                .unwrap()
                .into_iter()
                .map(|s| s.id_hash)
                .collect();
            assert_eq!(ids, ["fresh"]);
            let mut hashes: Vec<String> = tokens
                .list_for_user(user.id)
                .await
                .unwrap()
                .into_iter()
                .map(|t| t.token_hash)
                .collect();
            hashes.sort();
            assert_eq!(hashes, ["ahead", "forever"]);
            assert_eq!(
                limits.hit("login:ip:fresh", minute).await.unwrap().count,
                2,
                "the current window survived"
            );
            // The old row is gone: nothing older than the cut remains to prune.
            assert_eq!(
                limits
                    .prune(ago(builtin::RATE_LIMIT_RETENTION), 10)
                    .await
                    .unwrap(),
                0
            );
        },
    )
    .await;

    // A second run finds nothing more.
    handler.run(&get(store.as_ref(), id).await).await.unwrap();
    assert_eq!(sessions.list_for_user(user.id).await.unwrap().len(), 1);
    assert_eq!(oauth.get_grant(ended.id).await.unwrap(), None);
    assert_eq!(oauth.get_grant(alive.id).await.unwrap(), Some(alive));
}

/// P10: `sessions.prune` deletes idempotency keys older than `IDEMPOTENCY_RETENTION`
/// (24 h), finished or pending; younger keys stay.
pub async fn prune_removes_old_keys(db: Arc<TestDb>) {
    let store = db.shared_store();
    let repo = store.read().idempotency();
    let user = fixture_user(store.as_ref()).await;
    let key = |c: char| IdempotencyKey {
        user: user.id,
        key_hash: c.to_string().repeat(64),
        method: "POST".into(),
        path: "/api/v1/media".into(),
    };
    let start = |c: char| {
        let k = key(c);
        async move {
            match repo.begin(&k, IDEMPOTENCY_STALE).await.unwrap() {
                IdemBegin::Started { token } => token,
                other => panic!("{other:?}"),
            }
        }
    };
    // `a` finished and `b` pending, both 25 h old; `c` finished 23 h ago.
    let a = start('a').await;
    assert!(
        repo.finish(user.id, &"a".repeat(64), a, 201, &json!({"n": 1}))
            .await
            .unwrap()
    );
    start('b').await;
    let sql = if db.store().backend() == Backend::Sqlite {
        format!(
            "UPDATE idempotency_keys SET created_at = created_at - {}",
            25 * 3_600_000_000_i64
        )
    } else {
        "UPDATE idempotency_keys SET created_at = created_at - interval '25 hours'".to_owned()
    };
    db.exec_raw(&sql).await;
    let c = start('c').await;
    assert!(
        repo.finish(user.id, &"c".repeat(64), c, 201, &json!({"n": 3}))
            .await
            .unwrap()
    );
    let sql = if db.store().backend() == Backend::Sqlite {
        format!(
            "UPDATE idempotency_keys SET created_at = created_at - {} WHERE key_hash = '{}'",
            23 * 3_600_000_000_i64,
            "c".repeat(64)
        )
    } else {
        format!(
            "UPDATE idempotency_keys SET created_at = created_at - interval '23 hours' \
             WHERE key_hash = '{}'",
            "c".repeat(64)
        )
    };
    db.exec_raw(&sql).await;

    let handler = builtin::handlers(store.clone(), builtin::MIN_FAILURE_RETENTION)
        .into_iter()
        .find(|h| h.kind() == "sessions.prune")
        .unwrap();
    let id = enqueue(
        store.as_ref(),
        NewJob::new("sessions.prune", json!({"v": 1})),
    )
    .await;
    handler.run(&get(store.as_ref(), id).await).await.unwrap();

    // Nothing older than the retention is left; the 23 h key still replays.
    let now = store.read().nodes().db_now().await.unwrap();
    let cut = Timestamp::from_micros(
        now.as_micros() - i64::try_from(IDEMPOTENCY_RETENTION.as_micros()).unwrap(),
    )
    .unwrap();
    assert_eq!(repo.prune(cut, 10).await.unwrap(), 0);
    assert!(matches!(
        repo.begin(&key('c'), IDEMPOTENCY_STALE).await.unwrap(),
        IdemBegin::Replay { status: 201, .. }
    ));
    for c in ['a', 'b'] {
        assert!(
            matches!(
                repo.begin(&key(c), IDEMPOTENCY_STALE).await.unwrap(),
                IdemBegin::Started { .. }
            ),
            "{c} was pruned"
        );
    }
}

backend_tests_spawning!(
    runner_runs_and_completes,
    runner_counts_finished_jobs,
    runner_retries_then_dead,
    runner_busy_refunds_the_attempt,
    runner_permanent_error_is_dead_at_once,
    runner_handler_timeout_counts_as_failure,
    runner_handler_panic_is_a_failure,
    runner_invalid_payload_version_is_dead,
    runner_extends_long_jobs,
    runner_cancels_handler_when_lease_lost,
    runner_fences_handler_when_renewals_fail,
    runner_fences_handler_while_store_hangs,
    runner_releases_unknown_payload_version,
    runner_shutdown_releases_in_flight,
    scheduler_inserts_one_job_per_period,
    scheduler_refuses_an_offset_past_its_period,
    builtin_prune_jobs_delete_by_age,
    sessions_prune_job_deletes_expired_and_idle,
    prune_removes_old_keys,
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
        latency < Duration::from_secs(2),
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
