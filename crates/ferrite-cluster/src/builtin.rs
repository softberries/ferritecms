//! Built-in maintenance jobs (P11, P14, P20): hourly pruning of the events outbox,
//! finished jobs, stale node rows and old plugin failure buckets. Ages are measured on the database clock; rows go in
//! batches of [`MAX_PRUNE_BATCH`] (R5) so no single statement holds locks for long.

use std::{future::Future, sync::Arc, time::Duration};

use async_trait::async_trait;
use ferrite_domain::{Job, Timestamp};
use ferrite_store::{MAX_PRUNE_BATCH, Store, StoreError};
use serde_json::json;
use tokio::time::Instant;

use crate::jobs::{JobError, JobHandler, Periodic};

/// How often each prune job runs.
pub const PRUNE_EVERY: Duration = Duration::from_secs(3600);
/// Events older than this are deleted.
pub const EVENT_RETENTION: Duration = Duration::from_secs(24 * 3600);
/// `done` jobs finished longer ago than this are deleted.
pub const DONE_JOB_RETENTION: Duration = Duration::from_secs(7 * 24 * 3600);
/// `dead` jobs finished longer ago than this are deleted.
pub const DEAD_JOB_RETENTION: Duration = Duration::from_secs(30 * 24 * 3600);

/// The shortest time plugin failure buckets are kept.
pub const MIN_FAILURE_RETENTION: Duration = Duration::from_secs(24 * 3600);

/// How long plugin failure buckets are kept for a failure window of `window`
/// (`plugins.failure_window`): `max(24 h, 2 × window)`, so the auto-disable sum never
/// misses a bucket and a day of history stays for operators.
pub fn failure_retention(window: Duration) -> Duration {
    MIN_FAILURE_RETENTION.max(window.saturating_mul(2))
}

/// How long one prune run keeps deleting batches; well inside the default handler
/// timeout (300 s), so a large backlog ends the run cleanly and the next hour goes on.
pub const PRUNE_BUDGET: Duration = Duration::from_secs(240);

/// Calls `batch` until it deletes fewer than [`MAX_PRUNE_BATCH`] rows or
/// [`PRUNE_BUDGET`] is used up; returns the total.
async fn prune_batches<F, Fut>(what: &'static str, mut batch: F) -> Result<u64, JobError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<u64, StoreError>>,
{
    let started = Instant::now();
    let mut total = 0;
    loop {
        let n = batch().await.map_err(retry)?;
        total += n;
        if n < u64::from(MAX_PRUNE_BATCH) {
            break;
        }
        if started.elapsed() >= PRUNE_BUDGET {
            tracing::info!(
                what,
                deleted = total,
                "prune budget used; the rest goes next run"
            );
            break;
        }
    }
    tracing::info!(what, deleted = total, "pruned");
    Ok(total)
}

/// The database clock minus `age`.
async fn db_now_minus(store: &dyn Store, age: Duration) -> Result<Timestamp, JobError> {
    let now = store.read().nodes().db_now().await.map_err(retry)?;
    let age = i64::try_from(age.as_micros()).unwrap_or(i64::MAX);
    Timestamp::from_micros(now.as_micros().saturating_sub(age))
        .map_err(|e| JobError::Permanent(e.to_string()))
}

fn retry(e: StoreError) -> JobError {
    JobError::Retry(e.to_string())
}

/// `events.prune`: deletes events older than [`EVENT_RETENTION`].
pub struct EventsPrune(pub Arc<dyn Store>);

#[async_trait]
impl JobHandler for EventsPrune {
    fn kind(&self) -> &'static str {
        "events.prune"
    }

    async fn run(&self, _job: &Job) -> Result<(), JobError> {
        let before = db_now_minus(self.0.as_ref(), EVENT_RETENTION).await?;
        let events = self.0.read().events();
        prune_batches("events", || events.prune(before, MAX_PRUNE_BATCH)).await?;
        Ok(())
    }
}

/// `jobs.prune`: deletes `done` jobs after [`DONE_JOB_RETENTION`] and `dead` ones after
/// [`DEAD_JOB_RETENTION`].
pub struct JobsPrune(pub Arc<dyn Store>);

#[async_trait]
impl JobHandler for JobsPrune {
    fn kind(&self) -> &'static str {
        "jobs.prune"
    }

    async fn run(&self, _job: &Job) -> Result<(), JobError> {
        let done = db_now_minus(self.0.as_ref(), DONE_JOB_RETENTION).await?;
        let dead = db_now_minus(self.0.as_ref(), DEAD_JOB_RETENTION).await?;
        let jobs = self.0.read().jobs();
        prune_batches("jobs", || jobs.prune(done, dead, MAX_PRUNE_BATCH)).await?;
        Ok(())
    }
}

/// `nodes.prune`: deletes node rows whose heartbeat is older than [`crate::node::NODE_EXPIRY`].
pub struct NodesPrune(pub Arc<dyn Store>);

#[async_trait]
impl JobHandler for NodesPrune {
    fn kind(&self) -> &'static str {
        "nodes.prune"
    }

    async fn run(&self, _job: &Job) -> Result<(), JobError> {
        let before = db_now_minus(self.0.as_ref(), crate::node::NODE_EXPIRY).await?;
        let nodes = self.0.read().nodes();
        prune_batches("nodes", || nodes.prune(before, MAX_PRUNE_BATCH)).await?;
        Ok(())
    }
}

/// `plugin_failures.prune`: deletes failure buckets older than its retention (see
/// [`failure_retention`]).
pub struct PluginFailuresPrune {
    pub store: Arc<dyn Store>,
    pub retention: Duration,
}

#[async_trait]
impl JobHandler for PluginFailuresPrune {
    fn kind(&self) -> &'static str {
        "plugin_failures.prune"
    }

    async fn run(&self, _job: &Job) -> Result<(), JobError> {
        let before = db_now_minus(self.store.as_ref(), self.retention).await?;
        let failures = self.store.read().plugin_failures();
        prune_batches("plugin_failures", || {
            failures.prune(before, MAX_PRUNE_BATCH)
        })
        .await?;
        Ok(())
    }
}

/// The built-in handlers, on `store`; plugin failure buckets are kept for
/// `failure_retention` (from [`failure_retention`]).
pub fn handlers(store: Arc<dyn Store>, failure_retention: Duration) -> Vec<Arc<dyn JobHandler>> {
    vec![
        Arc::new(EventsPrune(store.clone())),
        Arc::new(JobsPrune(store.clone())),
        Arc::new(NodesPrune(store.clone())),
        Arc::new(PluginFailuresPrune {
            store,
            retention: failure_retention,
        }),
    ]
}

/// The built-in periodic jobs: every handler in [`handlers`], each [`PRUNE_EVERY`].
pub fn periodics() -> Vec<Periodic> {
    [
        "events.prune",
        "jobs.prune",
        "nodes.prune",
        "plugin_failures.prune",
    ]
    .into_iter()
    .map(|kind| Periodic {
        kind,
        every: PRUNE_EVERY,
        payload: json!({"v": 1}),
    })
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failure_retention_is_at_least_a_day() {
        let min = Duration::from_secs(60);
        assert_eq!(failure_retention(10 * min), MIN_FAILURE_RETENTION);
        assert_eq!(failure_retention(Duration::ZERO), MIN_FAILURE_RETENTION);
        assert_eq!(
            failure_retention(Duration::from_secs(20 * 3600)),
            Duration::from_secs(40 * 3600)
        );
        assert_eq!(failure_retention(Duration::MAX), Duration::MAX);
    }

    #[test]
    fn every_handler_has_a_periodic() {
        let periodic: Vec<&str> = periodics().iter().map(|p| p.kind).collect();
        let expected: Vec<&str> = [
            "events.prune",
            "jobs.prune",
            "nodes.prune",
            "plugin_failures.prune",
        ]
        .into();
        assert_eq!(periodic, expected);
        assert!(periodics().iter().all(|p| p.every == PRUNE_EVERY));
    }
}
