//! Cluster runtime (design 04): the node registry, the job runner, the periodic scheduler
//! and the built-in maintenance jobs. Everything here talks to the database through `ferrite-store`
//! traits only; the database is the only coordination service.

pub mod builtin;
mod error;
pub mod jobs;
mod node;

pub use error::ClusterError;
pub use jobs::{
    ABORT_WAIT, BUSY_RETRY_JITTER, BUSY_RETRY_MIN, EXTEND_EVERY, HANDLER_TIMEOUT, IDLE_POLL,
    JOB_DRAIN, JobError, JobHandler, JobRunner, LEASE, Periodic, RELEASE_BUDGET, RESUBSCRIBE_MAX,
    RESUBSCRIBE_MIN, RunnerConfig, SCHEDULER_TICK, Scheduler, busy_delay, period_key,
};
pub use node::{
    CLOCK_SKEW_RECOVERED, ClusterMode, HEARTBEAT_EVERY, JOIN_LOCK_WAIT, JoinTimings, LOCK_JOIN,
    MAX_CLOCK_SKEW, MAX_NODE_NAME, NODE_EXPIRY, NodeIdentity, Registry, SOLO_WAIT, STALE_AFTER,
    clock_skew, node_name,
};
