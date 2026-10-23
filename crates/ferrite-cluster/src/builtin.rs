//! Built-in maintenance jobs (P11, P14): hourly pruning of the events outbox, finished
//! jobs and stale node rows. Ages are measured on the database clock.

use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use ferrite_domain::{Job, Timestamp};
use ferrite_store::{Store, StoreError};
use serde_json::json;

use crate::jobs::{JobError, JobHandler, Periodic};

/// How often each prune job runs.
pub const PRUNE_EVERY: Duration = Duration::from_secs(3600);
/// Events older than this are deleted.
pub const EVENT_RETENTION: Duration = Duration::from_secs(24 * 3600);
/// `done` jobs finished longer ago than this are deleted.
pub const DONE_JOB_RETENTION: Duration = Duration::from_secs(7 * 24 * 3600);
/// `dead` jobs finished longer ago than this are deleted.
pub const DEAD_JOB_RETENTION: Duration = Duration::from_secs(30 * 24 * 3600);
/// Node rows whose last heartbeat is older than this are deleted.
pub const NODE_RETENTION: Duration = Duration::from_secs(24 * 3600);

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
        let n = self.0.read().events().prune(before).await.map_err(retry)?;
        tracing::info!(deleted = n, "pruned events");
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
        let n = self
            .0
            .read()
            .jobs()
            .prune(done, dead)
            .await
            .map_err(retry)?;
        tracing::info!(deleted = n, "pruned jobs");
        Ok(())
    }
}

/// `nodes.prune`: deletes node rows whose heartbeat is older than [`NODE_RETENTION`].
pub struct NodesPrune(pub Arc<dyn Store>);

#[async_trait]
impl JobHandler for NodesPrune {
    fn kind(&self) -> &'static str {
        "nodes.prune"
    }

    async fn run(&self, _job: &Job) -> Result<(), JobError> {
        let before = db_now_minus(self.0.as_ref(), NODE_RETENTION).await?;
        let n = self.0.read().nodes().prune(before).await.map_err(retry)?;
        tracing::info!(deleted = n, "pruned node rows");
        Ok(())
    }
}

/// The built-in handlers, on `store`.
pub fn handlers(store: Arc<dyn Store>) -> Vec<Arc<dyn JobHandler>> {
    vec![
        Arc::new(EventsPrune(store.clone())),
        Arc::new(JobsPrune(store.clone())),
        Arc::new(NodesPrune(store)),
    ]
}

/// The built-in periodic jobs: every handler in [`handlers`], each [`PRUNE_EVERY`].
pub fn periodics() -> Vec<Periodic> {
    ["events.prune", "jobs.prune", "nodes.prune"]
        .into_iter()
        .map(|kind| Periodic {
            kind,
            every: PRUNE_EVERY,
            payload: json!({"v": 1}),
        })
        .collect()
}
