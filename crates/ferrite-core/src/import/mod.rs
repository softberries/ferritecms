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

mod batch;
mod content;
mod terms;
mod users;

use std::{sync::Arc, time::Duration};

use ferrite_domain::{ImportRun, ImportRunId, ImportRunState, Timestamp};
use ferrite_store::{LOCK_IMPORT, LockGuard, Store, StoreError};
use serde_json::{Value, json};

pub use batch::{ImportBatch, Mapped, WP_ID_META};
pub use content::{ContentOutcome, ImportContent, Written};
pub use terms::{ImportTerm, TermMatched, TermOutcome};
pub use users::{ImportUser, Matched, UserOutcome};

use crate::{
    Actor, CoreError, Permalinks,
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

/// Starts import runs and opens their batches.
pub struct ImportService {
    store: Arc<dyn Store>,
    permalinks: Arc<dyn Permalinks>,
}

/// A started run, owned by one process. Pass it to every batch; end it with
/// [`finish`](Self::finish). Holds the [`LOCK_IMPORT`] lock when it could be taken;
/// dropping the handle releases it (the run itself stays `running` until it goes stale).
pub struct ImportHandle {
    pub run: ImportRun,
    pub actor: Actor,
    lock: Option<Box<dyn LockGuard>>,
}

impl ImportService {
    /// `permalinks` are the site's live rules; each batch pins them when it opens.
    pub fn new(store: Arc<dyn Store>, permalinks: Arc<dyn Permalinks>) -> Self {
        Self { store, permalinks }
    }

    /// Starts a run of `source` (the normalised `<wp:base_blog_url>`). Needs
    /// [`Capability::Import`]. Another `running` run makes this
    /// [`CoreError::Unavailable`], unless `takeover` is set and that run's heartbeat is
    /// older than [`STALE_AFTER`]: then it is marked `abandoned` first (P5).
    ///
    /// On SQLite (one writer) never call this while a batch of this process is open:
    /// it would wait for that batch's transaction and time out (`Store(Busy)`).
    pub async fn start(
        &self,
        actor: &Actor,
        source: &str,
        dry_run: bool,
        takeover: bool,
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
        };
        let created = self.create_run(&run, takeover, now).await;
        match created {
            Ok(()) => Ok(ImportHandle {
                run,
                actor: *actor,
                lock,
            }),
            Err(e) => {
                release(lock).await;
                Err(e)
            }
        }
    }

    /// Inserts `run` in one transaction, after abandoning a stale running run when
    /// `takeover` allows it.
    async fn create_run(
        &self,
        run: &ImportRun,
        takeover: bool,
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
                if !takeover {
                    return Err(CoreError::Unavailable(BUSY_STALE));
                }
                // Re-checked by the write itself: a batch that fenced since our read
                // makes this a no-op.
                if !imports.abandon_stale(other.id, stale_before, now).await? {
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

impl ImportHandle {
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
        release(self.lock).await;
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
