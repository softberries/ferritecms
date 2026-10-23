//! Core job handlers run by every node (ruling P12): scheduled publishing. The kinds and
//! payloads are fixed in [`ferrite_core::jobs`]; the per-node 30 s ticker of M1c-2 is
//! the periodic [`PUBLISH_DUE_KIND`] job now, so one node runs it per period.

use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use ferrite_cluster::{JobError, JobHandler, Periodic};
use ferrite_core::{
    ContentService,
    jobs::{PUBLISH_DUE_KIND, PUBLISH_SCHEDULED_KIND, PUBLISH_SCHEDULED_VERSION, PublishScheduled},
};
use ferrite_domain::Job;
use ferrite_store::Store;
use serde_json::json;

/// Period of the [`PUBLISH_DUE_KIND`] safety net.
pub const PUBLISH_DUE_EVERY: Duration = Duration::from_secs(30);

/// `content.publish_scheduled`: publishes the payload's item if it is still scheduled
/// for the payload's time ([`ContentService::publish_scheduled`]).
pub struct PublishScheduledHandler {
    pub content: Arc<ContentService>,
}

#[async_trait]
impl JobHandler for PublishScheduledHandler {
    fn kind(&self) -> &'static str {
        PUBLISH_SCHEDULED_KIND
    }

    fn versions(&self) -> &'static [u32] {
        &[PUBLISH_SCHEDULED_VERSION]
    }

    async fn run(&self, job: &Job) -> Result<(), JobError> {
        let payload: PublishScheduled = serde_json::from_value(job.payload.clone())
            .map_err(|e| JobError::Permanent(format!("invalid payload: {e}")))?;
        let published = self
            .content
            .publish_scheduled(payload.id, payload.at)
            .await
            .map_err(|e| JobError::Retry(e.to_string()))?;
        if published {
            tracing::info!(item = %payload.id, "published scheduled item");
        }
        Ok(())
    }
}

/// `content.publish_due`: publishes every item due by the database clock
/// ([`ContentService::publish_due`]); catches items scheduled before M1.5, by an older
/// binary, or whose own job is gone.
pub struct PublishDueHandler {
    pub store: Arc<dyn Store>,
    pub content: Arc<ContentService>,
}

#[async_trait]
impl JobHandler for PublishDueHandler {
    fn kind(&self) -> &'static str {
        PUBLISH_DUE_KIND
    }

    async fn run(&self, _job: &Job) -> Result<(), JobError> {
        let now = self
            .store
            .read()
            .nodes()
            .db_now()
            .await
            .map_err(|e| JobError::Retry(e.to_string()))?;
        let n = self
            .content
            .publish_due(now)
            .await
            .map_err(|e| JobError::Retry(e.to_string()))?;
        if n > 0 {
            tracing::info!(published = n, "published due scheduled items");
        }
        Ok(())
    }
}

/// Both handlers, on `content`.
pub fn handlers(store: Arc<dyn Store>, content: Arc<ContentService>) -> Vec<Arc<dyn JobHandler>> {
    vec![
        Arc::new(PublishScheduledHandler {
            content: content.clone(),
        }),
        Arc::new(PublishDueHandler { store, content }),
    ]
}

/// The [`PUBLISH_DUE_KIND`] periodic, every [`PUBLISH_DUE_EVERY`].
pub fn publish_due_periodic() -> Periodic {
    Periodic {
        kind: PUBLISH_DUE_KIND,
        every: PUBLISH_DUE_EVERY,
        payload: json!({"v": 1}),
    }
}
