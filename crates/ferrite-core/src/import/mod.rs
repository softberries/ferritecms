//! The WordPress import writer (M3-1 P1, P5, P6): runs fenced by a token, and batches
//! that write many rows in one transaction with one invalidation.
//!
//! The import does not call the public services (each commits its own transaction and
//! emits its own events); [`ImportBatch`] calls their crate-private `*_in` helpers, so
//! the validation is the services'. The capability [`Import`](Capability::Import) is
//! checked once, in [`ImportService::start`].
//!
//! **The fence (P5).** [`start`](ImportService::start) inserts a `running` run with a
//! random 128-bit token. Every batch, [`ImportHandle::heartbeat`] and
//! [`ImportHandle::finish`] first run `ImportRepo::fence` in their transaction: it moves
//! the heartbeat and succeeds only while the run is still `running` with that token, so
//! a run that was taken over cannot write any more ([`CoreError::Changed`]). A takeover
//! abandons the old run only if it is still `running` with a heartbeat older than
//! [`STALE_AFTER`] at the moment of the write (`ImportRepo::abandon_stale`, one
//! statement); on Postgres that statement waits for a batch holding the row lock and
//! then sees the heartbeat it committed. The named lock [`LOCK_IMPORT`] is also taken,
//! best-effort, for the life of the handle; nothing relies on it (R17).
//!
//! **Jobs (M3-2).** [`ImportService::enqueue`] queues an `import.wordpress` job for an
//! upload (P25, P27); the run reads, the importer check and the report artifacts are in
//! `jobs.rs`.

mod batch;
mod comments;
mod content;
mod jobs;
mod media;
mod menus;
mod redirects;
mod terms;
mod users;

use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use bytes::Bytes;
use ferrite_domain::{
    ArtifactHash, ImportRun, ImportRunId, ImportRunState, JobId, JobStatus, Timestamp,
};
use ferrite_store::{LOCK_IMPORT, LockGuard, Store, StoreError};
use serde_json::{Value, json};

pub use batch::{ImportBatch, MD_ID_META, Mapped, TypeWritten, WP_ID_META};
pub use comments::{CommentOutcome, ImportComment};
pub use content::{ContentOutcome, ImportContent, Written};
pub use jobs::{ENQUEUE_LOCK_WAIT, ImportJobView, ImportRunView, MAX_REPORT_BYTES, ReportFormat};
pub use media::ImportMedia;
pub use redirects::RedirectOutcome;
pub use terms::{ImportTerm, TermMatched, TermOutcome};
pub use users::{ImportUser, Matched, UserOutcome};

use crate::{
    Actor, CoreError, PermalinkRules, Permalinks,
    actor::{Capability, require},
    tx::finish,
};

/// A running run whose heartbeat is older than this may be taken over (P5).
pub const STALE_AFTER: Duration = Duration::from_secs(120);

/// Refused start: another run is `running` with a fresh heartbeat.
const BUSY: &str = "an import is already running";
/// Refused start: another run is `running` but stale, and no takeover was asked for.
const BUSY_STALE: &str = "an import is already running (no heartbeat for 120 s; take it over)";
/// The fence failed: the run is no longer `running` under this handle's token.
const TAKEN_OVER: &str = "import run was taken over";
/// The constraint that allows one `running` run.
const RUNNING_UQ: &str = "import_runs_running_uq";

/// The step that reads the WXR into the work file (P28).
pub const STEP_PARSE: &str = "parse";
/// Pass 1: users, terms and content types.
pub const STEP_PASS1: &str = "pass1";
/// Pass 2: media.
pub const STEP_PASS2: &str = "pass2";
/// Pass 3: content.
pub const STEP_PASS3: &str = "pass3";
/// Pass 4: comments, menus, options and redirects.
pub const STEP_PASS4: &str = "pass4";
/// The fix-up of internal links.
pub const STEP_FIXUP: &str = "fixup";
/// The step that builds the report.
pub const STEP_REPORT: &str = "report";

/// The steps of a run, in order (P28): what [`ImportHandle::set_step`] accepts.
pub const IMPORT_STEPS: [&str; 7] = [
    STEP_PARSE,
    STEP_PASS1,
    STEP_PASS2,
    STEP_PASS3,
    STEP_PASS4,
    STEP_FIXUP,
    STEP_REPORT,
];

/// Where artifact bytes live (the binary's `MediaStore`; `ferrite-core` does not depend
/// on `ferrite-media`, 01 §1): content-addressed by SHA-256.
#[async_trait]
pub trait ArtifactBlobs: Send + Sync {
    /// Stores `data` (idempotent) and returns its hash.
    async fn put(&self, data: Bytes) -> Result<ArtifactHash, CoreError>;
    /// The bytes of `hash`: [`CoreError::NotFound`] when there is no such artifact, and
    /// an error without reading them when it has more than `max` bytes.
    async fn get(&self, hash: &ArtifactHash, max: u64) -> Result<Bytes, CoreError>;
}

/// Starts import runs and opens their batches; enqueues import jobs and reads runs.
pub struct ImportService {
    store: Arc<dyn Store>,
    permalinks: Arc<dyn Permalinks>,
    /// Report storage; `None` makes the report methods `Unavailable`.
    artifacts: Option<Arc<dyn ArtifactBlobs>>,
}

/// A started run, owned by one process. Pass it to every batch; end it with
/// [`finish`](Self::finish). Holds the [`LOCK_IMPORT`] lock when it could be taken;
/// dropping the handle releases it (the run itself stays `running` until it goes stale).
pub struct ImportHandle {
    run: ImportRun,
    actor: Actor,
    /// In a mutex only so the handle is `Sync` (batches and drivers hold `&ImportHandle`
    /// across awaits on a multi-threaded runtime); it is taken once, by `finish`.
    lock: std::sync::Mutex<Option<Box<dyn LockGuard>>>,
}

impl ImportService {
    /// `permalinks` are the site's live rules; each batch pins them when it opens.
    pub fn new(store: Arc<dyn Store>, permalinks: Arc<dyn Permalinks>) -> Self {
        Self {
            store,
            permalinks,
            artifacts: None,
        }
    }

    /// The same service storing and reading run reports in `artifacts` (P29).
    pub fn with_artifacts(self, artifacts: Arc<dyn ArtifactBlobs>) -> Self {
        Self {
            artifacts: Some(artifacts),
            ..self
        }
    }

    /// The permalink rules the stored `permalink.*` options give, with `overrides` on
    /// top ([`rules_from`](crate::permalink::rules_from)): what a driver swaps in after
    /// a schema batch changed them (overrides stand in for a rolled-back dry run's
    /// rows).
    pub async fn permalink_rules(
        &self,
        overrides: &[(&str, Option<&str>)],
    ) -> Result<PermalinkRules, CoreError> {
        crate::permalink::rules_from(self.store.read(), overrides).await
    }

    /// Starts a run of `source` (the normalised `<wp:base_blog_url>`). Needs
    /// [`Capability::Import`]. Another `running` run makes this
    /// [`CoreError::Unavailable`], unless its heartbeat is older than [`STALE_AFTER`]
    /// and it may be taken over: then it is marked `abandoned` first (P5). `job_id` is
    /// the `import.wordpress` job running it, if any (P27). A CLI start (no `job_id`)
    /// takes over only with `takeover`; a job's start ignores `takeover` and takes over
    /// a stale run that has no job, is its own, or whose job is not `running` (P27 as
    /// built; checked in the abandoning statement itself).
    ///
    /// On SQLite (one writer) never call this while a batch of this process is open:
    /// it would wait for that batch's transaction and time out (`Store(Busy)`).
    pub async fn start(
        &self,
        actor: &Actor,
        source: &str,
        dry_run: bool,
        takeover: bool,
        job_id: Option<JobId>,
    ) -> Result<ImportHandle, CoreError> {
        require(actor, Capability::Import)?;
        let mut token = [0u8; 16];
        getrandom::fill(&mut token).map_err(|_| CoreError::Random)?;
        let lock = match self.store.try_lock(LOCK_IMPORT).await {
            Ok(Some(guard)) => Some(guard),
            Ok(None) => {
                tracing::info!("the import lock is held elsewhere; relying on the run fence");
                None
            }
            Err(e) => {
                tracing::warn!(error = %e, "taking the import lock failed; relying on the run fence");
                None
            }
        };
        let now = Timestamp::now();
        let run = ImportRun {
            id: ImportRunId::new(),
            source: source.to_owned(),
            token,
            state: ImportRunState::Running,
            started_by: actor.user,
            started_at: now,
            heartbeat_at: now,
            finished_at: None,
            stats: json!({}),
            dry_run,
            job_id,
            step: None,
            report_hash: None,
        };
        let created = self.create_run(&run, takeover, job_id, now).await;
        match created {
            Ok(()) => Ok(ImportHandle {
                run,
                actor: *actor,
                lock: std::sync::Mutex::new(lock),
            }),
            Err(e) => {
                release(lock).await;
                Err(e)
            }
        }
    }

    /// Inserts `run` in one transaction, after abandoning a stale running run when
    /// `takeover` (CLI) or the job rule (`job`, see [`start`](Self::start)) allows it.
    async fn create_run(
        &self,
        run: &ImportRun,
        takeover: bool,
        job: Option<JobId>,
        now: Timestamp,
    ) -> Result<(), CoreError> {
        let tx = self.store.begin().await?;
        let done = async {
            let imports = tx.imports();
            if let Some(other) = imports.running().await? {
                let stale_before = stale_before(now);
                if other.heartbeat_at >= stale_before {
                    return Err(CoreError::Unavailable(BUSY));
                }
                // Re-checked by the write itself: a batch that fenced since our read
                // (or, for a job, the other run's job running again) makes this a no-op.
                let abandoned = match job {
                    Some(job) => {
                        imports
                            .abandon_stale_for_job(other.id, stale_before, now, job)
                            .await?
                    }
                    None if !takeover => return Err(CoreError::Unavailable(BUSY_STALE)),
                    None => imports.abandon_stale(other.id, stale_before, now).await?,
                };
                if !abandoned {
                    return Err(CoreError::Unavailable(BUSY));
                }
                tracing::warn!(run = %other.id, "took over a stale import run");
            }
            imports.create_run(run).await.map_err(|e| match e {
                StoreError::UniqueViolation { constraint } if constraint == RUNNING_UQ => {
                    CoreError::Unavailable(BUSY)
                }
                e => e.into(),
            })
        }
        .await;
        finish(tx, done).await
    }
}

impl ImportService {
    /// Whether job `job` could start a run now (P27 as built): no run is `running`, or
    /// the running one is stale and has no job, is `job`'s own, or its job is not live:
    /// `running` with a lease that has not expired (M4-4 P80; a job whose runner died
    /// keeps `running` until the reaper reclaims it). A pre-check on this node's clock, so
    /// a busy job fetches nothing; [`start`](Self::start) decides for real, in the store
    /// statement, on the lease's own clock (the database's on Postgres).
    pub async fn job_may_start(&self, job: JobId) -> Result<bool, CoreError> {
        let repos = self.store.read();
        let Some(run) = repos.imports().running().await? else {
            return Ok(true);
        };
        let now = Timestamp::now();
        if run.heartbeat_at >= stale_before(now) {
            return Ok(false);
        }
        Ok(match run.job_id {
            None => true,
            Some(other) if other == job => true,
            Some(other) => repos.jobs().get(other).await?.is_none_or(|j| {
                j.status != JobStatus::Running || j.locked_until.is_some_and(|until| until < now)
            }),
        })
    }
}

impl ImportHandle {
    /// The run as it was started (its id, source and start time).
    pub fn run(&self) -> &ImportRun {
        &self.run
    }

    /// Opens a batch: a transaction whose first statement is the fence (P5). A run that
    /// was taken over is [`CoreError::Changed`].
    ///
    /// On SQLite (one writer) never open a batch while another batch of the same handle
    /// (or any other write transaction of this process) is still open: it waits for the
    /// writer and times out (`Store(Busy)`). Commit or roll back the first one.
    pub async fn batch(&self, svc: &ImportService) -> Result<ImportBatch<'_>, CoreError> {
        ImportBatch::open(self, svc.store.as_ref(), svc.permalinks.pin(), false).await
    }

    /// [`batch`](Self::batch) for registering content types and taxonomies and writing
    /// `permalink.*` options: it takes the `types` lock before its transaction, as the
    /// services do, and holds it until the batch ends. Use it only for those writes, so
    /// the lock is not held during content batches.
    pub async fn schema_batch(&self, svc: &ImportService) -> Result<ImportBatch<'_>, CoreError> {
        ImportBatch::open(self, svc.store.as_ref(), svc.permalinks.pin(), true).await
    }

    /// Proves the run is still this handle's and moves its heartbeat, outside a batch
    /// (for long steps that write nothing). Taken over: [`CoreError::Changed`].
    ///
    /// On SQLite (one writer) never call this while a batch of this handle is open: the
    /// fence's write waits for that batch's transaction and times out (`Store(Busy)`).
    pub async fn heartbeat(&self, svc: &ImportService) -> Result<(), CoreError> {
        let fenced = svc
            .store
            .read()
            .imports()
            .fence(self.run.id, &self.run.token, Timestamp::now())
            .await?;
        if fenced {
            Ok(())
        } else {
            Err(CoreError::Changed(TAKEN_OVER))
        }
    }

    /// Records that the run begins `step`, one of [`IMPORT_STEPS`] (else `Invalid`),
    /// under the fence (P28): a run that was taken over is [`CoreError::Changed`].
    ///
    /// On SQLite (one writer) never call this while a batch of this handle is open.
    pub async fn set_step(&self, svc: &ImportService, step: &str) -> Result<(), CoreError> {
        if !IMPORT_STEPS.contains(&step) {
            return Err(CoreError::Invalid(format!("unknown import step {step:?}")));
        }
        let fenced = svc
            .store
            .read()
            .imports()
            .set_step(self.run.id, &self.run.token, step)
            .await?;
        if fenced {
            Ok(())
        } else {
            Err(CoreError::Changed(TAKEN_OVER))
        }
    }

    /// Ends the run with `state` (`done`, `failed` or `abandoned`; `running` is
    /// `Invalid`) and the report counters `stats`, then releases the lock. The fence runs
    /// first in the same transaction, so a run that was taken over keeps its
    /// `abandoned` state and this is [`CoreError::Changed`].
    pub async fn finish(
        self,
        svc: &ImportService,
        state: ImportRunState,
        stats: Value,
    ) -> Result<(), CoreError> {
        let done = self.write_finish(svc, state, &stats).await;
        release(self.lock.into_inner().unwrap_or_else(|e| e.into_inner())).await;
        done
    }

    async fn write_finish(
        &self,
        svc: &ImportService,
        state: ImportRunState,
        stats: &Value,
    ) -> Result<(), CoreError> {
        if state == ImportRunState::Running {
            return Err(CoreError::Invalid(
                "an import run cannot finish as running".into(),
            ));
        }
        let tx = svc.store.begin().await?;
        let done = async {
            let now = Timestamp::now();
            let imports = tx.imports();
            if !imports.fence(self.run.id, &self.run.token, now).await? {
                return Err(CoreError::Changed(TAKEN_OVER));
            }
            imports.set_state(self.run.id, state, stats, now).await?;
            Ok(())
        }
        .await;
        finish(tx, done).await
    }
}

/// Heartbeats before this are stale at `now`.
fn stale_before(now: Timestamp) -> Timestamp {
    let limit = chrono::Duration::from_std(STALE_AFTER).unwrap_or(chrono::Duration::MAX);
    Timestamp::from(now.as_datetime() - limit)
}

/// Releases a held lock; a failure is logged (the lock goes away with its connection).
pub(crate) async fn release(lock: Option<Box<dyn LockGuard>>) {
    if let Some(guard) = lock
        && let Err(e) = guard.release().await
    {
        tracing::warn!(error = %e, "releasing a lock held by the import failed");
    }
}
