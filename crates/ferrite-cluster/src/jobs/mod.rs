//! Background jobs (design 04 §6, rulings P5, P9–P11): handlers, the per-node runner and
//! the periodic scheduler.

mod runner;
mod scheduler;

use std::time::Duration;

use async_trait::async_trait;
use ferrite_domain::Job;

pub use runner::{
    ABORT_WAIT, EXTEND_EVERY, IDLE_POLL, JOB_DRAIN, JobCounters, JobRunner, LEASE, RELEASE_BUDGET,
    RunnerConfig,
};
pub use scheduler::{Periodic, SCHEDULER_TICK, Scheduler, period_key};

/// Default [`JobHandler::timeout`] (P9).
pub const HANDLER_TIMEOUT: Duration = Duration::from_secs(300);
/// First delay before the runner re-subscribes to job notices after a failure.
pub const RESUBSCRIBE_MIN: Duration = Duration::from_secs(1);
/// The re-subscribe delay doubles up to this.
pub const RESUBSCRIBE_MAX: Duration = Duration::from_secs(30);

/// Shortest delay before a [`JobError::Busy`] job is due again.
pub const BUSY_RETRY_MIN: Duration = Duration::from_secs(1);
/// Most random delay added to [`BUSY_RETRY_MIN`] (spreads a burst of busy retries).
pub const BUSY_RETRY_JITTER: Duration = Duration::from_secs(4);

/// The delay of one [`JobError::Busy`] release: [`BUSY_RETRY_MIN`] plus a random part
/// below [`BUSY_RETRY_JITTER`].
pub fn busy_delay() -> Duration {
    use std::hash::{BuildHasher, Hasher};
    // `RandomState` is seeded randomly per instance: enough jitter, no new dependency.
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u64(0);
    let millis = u64::try_from(BUSY_RETRY_JITTER.as_millis())
        .unwrap_or(u64::MAX)
        .max(1);
    BUSY_RETRY_MIN + Duration::from_millis(h.finish() % millis)
}

/// A failed job run.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum JobError {
    /// Worth retrying: the job goes back to `queued` with backoff until its attempts are
    /// used up, then `dead`.
    #[error("{0}")]
    Retry(String),
    /// Not run for want of capacity (a full slot, a busy plugin host: M2a-2 P22, R20(3)):
    /// the job goes back to `queued` with the attempt refunded, due again after
    /// [`busy_delay`] ([`BUSY_RETRY_MIN`] plus up to [`BUSY_RETRY_JITTER`]), so a burst
    /// of busy retries can never push a job to `dead`.
    #[error("{0}")]
    Busy(String),
    /// Retrying cannot help: the job goes `dead` at once.
    #[error("{0}")]
    Permanent(String),
}

/// Runs the jobs of one kind. Execution is at-least-once, so `run` must be idempotent
/// (04 §6). The runner renews the lease while `run` is pending; `run` needs no lease
/// code, but its future may be dropped at any await point: when a renewal reports the
/// lease lost, when no renewal succeeded for `lease - extend_every / 2` (the local
/// fence, R4: the drop comes before the lease can lapse), on timeout and on shutdown.
///
/// **Remaining bound.** The fence runs on this process's clock and runtime. A frozen
/// or paused process (SIGSTOP, VM pause, a starved runtime) cannot drop the handler in
/// time: after it resumes, the handler may run on next to the job's new owner until the
/// next renewal or fence check. Its job-store writes are no-ops then (lease token, P6),
/// but side effects outside the job row are not fenced, which is one more reason
/// handlers must be idempotent.
///
/// **Do not block.** `run` must yield regularly: a handler blocking its thread cannot be
/// cancelled and holds up shutdown (after [`ABORT_WAIT`] its job is left to expire on
/// its lease instead of being released). Work moved to `spawn_blocking` is not
/// cancelled when the future is dropped.
#[async_trait]
pub trait JobHandler: Send + Sync + 'static {
    /// The job kind this handler runs (`[a-z0-9._-]{1,64}`).
    fn kind(&self) -> &'static str;
    /// Payload versions (`payload.v`, absent = 1) this handler understands; jobs with
    /// another version go back to the queue without using an attempt (P5).
    fn versions(&self) -> &'static [u32] {
        &[1]
    }
    /// Longest one run may take before it counts as a retryable failure (P9).
    fn timeout(&self) -> Duration {
        HANDLER_TIMEOUT
    }
    /// Runs one attempt of `job`.
    async fn run(&self, job: &Job) -> Result<(), JobError>;
}
