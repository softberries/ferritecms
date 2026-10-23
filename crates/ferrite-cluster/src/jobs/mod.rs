//! Background jobs (design 04 §6, rulings P5, P9–P11): handlers, the per-node runner and
//! the periodic scheduler.

mod runner;
mod scheduler;

use std::time::Duration;

use async_trait::async_trait;
use ferrite_domain::Job;

pub use runner::{EXTEND_EVERY, IDLE_POLL, JOB_DRAIN, JobRunner, LEASE, RunnerConfig};
pub use scheduler::{Periodic, SCHEDULER_TICK, Scheduler, period_key};

/// Default [`JobHandler::timeout`] (P9).
pub const HANDLER_TIMEOUT: Duration = Duration::from_secs(300);
/// First delay before the runner re-subscribes to job notices after a failure.
pub const RESUBSCRIBE_MIN: Duration = Duration::from_secs(1);
/// The re-subscribe delay doubles up to this.
pub const RESUBSCRIBE_MAX: Duration = Duration::from_secs(30);

/// A failed job run.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum JobError {
    /// Worth retrying: the job goes back to `queued` with backoff until its attempts are
    /// used up, then `dead`.
    #[error("{0}")]
    Retry(String),
    /// Retrying cannot help: the job goes `dead` at once.
    #[error("{0}")]
    Permanent(String),
}

/// Runs the jobs of one kind. Execution is at-least-once, so `run` must be idempotent
/// (04 §6). The runner renews the lease while `run` is pending; `run` needs no lease
/// code, but its future may be dropped at any await point (lease lost, shutdown).
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
