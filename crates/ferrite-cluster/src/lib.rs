//! Cluster runtime (design 04): the job runner, the periodic scheduler and the built-in
//! maintenance jobs. Everything here talks to the database through `ferrite-store`
//! traits only; the database is the only coordination service.

pub mod builtin;
mod error;
pub mod jobs;

pub use error::ClusterError;
pub use jobs::{
    EXTEND_EVERY, HANDLER_TIMEOUT, IDLE_POLL, JOB_DRAIN, JobError, JobHandler, JobRunner, LEASE,
    Periodic, RESUBSCRIBE_MAX, RESUBSCRIBE_MIN, RunnerConfig, SCHEDULER_TICK, Scheduler,
    period_key,
};
