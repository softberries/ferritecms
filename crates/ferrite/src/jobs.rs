//! Core job handlers run by every node (ruling P12): scheduled publishing, the plugin
//! data purge (M2a-2 P9), plugin actions (P20), media variants (M2a-2b P8,
//! [`MediaVariantsHandler`]) and permalink-change redirects (M3-2 P30,
//! [`PermalinkRedirectsHandler`]). The kinds and payloads are fixed in
//! [`ferrite_core::jobs`]; the WordPress import (M3-2 P27, [`ImportWordpressHandler`])
//! is built by `serve` with the node's media store and data folder. The per-node 30 s
//! ticker of M1c-2 is the periodic
//! [`PUBLISH_DUE_KIND`] job now, so one node runs it per period.

mod import;
mod media;
mod permalinks;

use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use ferrite_cluster::{JobError, JobHandler, Periodic};
use ferrite_core::{
    ContentService, CoreError, ExtensionService, RedirectService, STRUCTURE_JOB_PAGES,
    jobs::{
        PLUGIN_ACTION_KIND, PLUGIN_ACTION_VERSION, PLUGIN_PURGE_KIND, PLUGIN_PURGE_SWEEP_EVERY,
        PLUGIN_PURGE_SWEEP_KIND, PLUGIN_PURGE_VERSION, PUBLISH_DUE_KIND, PUBLISH_SCHEDULED_KIND,
        PUBLISH_SCHEDULED_VERSION, PluginAction, PluginPurge, PublishScheduled,
    },
};
use ferrite_domain::Job;
use ferrite_plugin_host::{ActionEvent, PluginError, PluginHost};
use ferrite_store::Store;
use serde_json::json;
use tokio::sync::Semaphore;

pub use import::{IMPORT_TIMEOUT, ImportWordpressHandler};
#[cfg(feature = "test-seams")]
pub use media::ImageProcessor;
pub use media::{MediaVariantsHandler, VariantSpecs};
pub use permalinks::PermalinkRedirectsHandler;

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
            .map_err(|e| match e {
                // Retrying cannot change these.
                CoreError::Invalid(_) | CoreError::InvalidField { .. } | CoreError::NotFound => {
                    JobError::Permanent(e.to_string())
                }
                other => JobError::Retry(other.to_string()),
            })?;
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

/// `plugin.purge`: deletes a removed plugin's KV and settings
/// ([`ExtensionService::purge_plugin_data`]) unless it was installed again or deleted
/// later. A busy `extensions` lock or a store error is retried.
pub struct PluginPurgeHandler {
    pub extensions: Arc<ExtensionService>,
}

#[async_trait]
impl JobHandler for PluginPurgeHandler {
    fn kind(&self) -> &'static str {
        PLUGIN_PURGE_KIND
    }

    fn versions(&self) -> &'static [u32] {
        &[PLUGIN_PURGE_VERSION]
    }

    async fn run(&self, job: &Job) -> Result<(), JobError> {
        let payload: PluginPurge = serde_json::from_value(job.payload.clone())
            .map_err(|e| JobError::Permanent(format!("invalid payload: {e}")))?;
        let purged = self
            .extensions
            .purge_plugin_data(&payload.name, payload.deleted_at, &payload.settings)
            .await
            .map_err(|e| match e {
                CoreError::Invalid(_) | CoreError::InvalidField { .. } => {
                    JobError::Permanent(e.to_string())
                }
                other => JobError::Retry(other.to_string()),
            })?;
        if purged {
            tracing::info!(plugin = %payload.name, "purged deleted plugin's data");
        }
        Ok(())
    }
}

/// `plugin.purge_sweep` (daily periodic): enqueues the purge of every deleted plugin
/// past its retention whose purge has not happened
/// ([`ExtensionService::enqueue_overdue_purges`]), so a purge job that went dead is
/// retried the next day. The run's period start keys the enqueued jobs.
pub struct PluginPurgeSweepHandler {
    pub store: Arc<dyn Store>,
    pub extensions: Arc<ExtensionService>,
}

#[async_trait]
impl JobHandler for PluginPurgeSweepHandler {
    fn kind(&self) -> &'static str {
        PLUGIN_PURGE_SWEEP_KIND
    }

    async fn run(&self, job: &Job) -> Result<(), JobError> {
        let now = self
            .store
            .read()
            .nodes()
            .db_now()
            .await
            .map_err(|e| JobError::Retry(e.to_string()))?;
        let n = self
            .extensions
            .enqueue_overdue_purges(now, job.run_at)
            .await
            .map_err(|e| JobError::Retry(e.to_string()))?;
        if n > 0 {
            tracing::info!(enqueued = n, "re-enqueued overdue plugin purges");
        }
        Ok(())
    }
}

/// `plugin.action`: delivers one action hook to one plugin on this node's host
/// (M2a-2 P20). The plugin's row is read first: a missing or disabled row drops the
/// action (done, P29), so a plugin disabled or deleted after the enqueue never runs
/// it. Then [`PluginHost::dispatch_action`] looks the plugin up once and holds that
/// instance for the whole call (R13); the call is bounded by the host's fuel, memory
/// and action deadline, and a failure counts towards auto-disable (R12). Outcomes:
///
/// - success, or the plugin no longer handles the hook (an update dropped it, F35):
///   done;
/// - not loaded on this node, or loaded without the hook while the stored row lists it
///   (the reload has not caught up), or disabled on this host only (the shared disable
///   follows, F35): retried;
/// - the call failed: retried (backoff, then dead after the job's attempts);
/// - busy (P22, never a plugin failure): no action slot free on this node, or no host
///   call slot within the action deadline (F35): [`JobError::Busy`], put back with the
///   attempt refunded and due again after 1–5 s (R20(3)), so a burst never pushes an
///   action to `dead`.
///
/// At most `slots` actions run at once on a node ([`action_slots`]: one below
/// `cluster.workers`, Task 11 carry), so slow plugins cannot hold every job worker and
/// core jobs (scheduled publishing, purges) always find one. An action over that is
/// released at once (`Busy`) rather than waiting for a slot, which would hold a worker
/// too.
pub struct PluginActionHandler {
    store: Arc<dyn Store>,
    host: Arc<PluginHost>,
    slots: Semaphore,
}

impl PluginActionHandler {
    /// A handler running at most `slots` (at least 1) actions at once.
    pub fn new(store: Arc<dyn Store>, host: Arc<PluginHost>, slots: usize) -> Self {
        Self {
            store,
            host,
            slots: Semaphore::new(slots.clamp(1, Semaphore::MAX_PERMITS)),
        }
    }
}

/// Actions that may run at once on a node with `workers` job workers and a plugin host
/// of `max_calls` call slots: `min(workers - 1, max_calls)`, at least 1. With
/// `cluster.workers = 1` that is the only worker (documented: give a node with plugin
/// actions at least 2).
pub fn action_slots(workers: usize, max_calls: usize) -> usize {
    workers.saturating_sub(1).min(max_calls).max(1)
}

#[async_trait]
impl JobHandler for PluginActionHandler {
    fn kind(&self) -> &'static str {
        PLUGIN_ACTION_KIND
    }

    fn versions(&self) -> &'static [u32] {
        &[PLUGIN_ACTION_VERSION]
    }

    async fn run(&self, job: &Job) -> Result<(), JobError> {
        let action: PluginAction = serde_json::from_value(job.payload.clone())
            .map_err(|e| JobError::Permanent(format!("invalid payload: {e}")))?;
        let (plugin, hook) = (action.plugin.as_str(), action.hook.as_str());
        let row = self
            .store
            .read()
            .plugins()
            .get(plugin)
            .await
            .map_err(|e| JobError::Retry(e.to_string()))?;
        let Some(row) = row.filter(|r| r.enabled) else {
            tracing::info!(plugin, hook, "plugin removed or disabled; action dropped");
            return Ok(());
        };
        let ev = ActionEvent {
            subject: action.subject,
            payload: action.payload,
        };
        let Ok(_slot) = self.slots.try_acquire() else {
            return Err(JobError::Busy(
                "busy: every plugin action slot of this node is in use".to_owned(),
            ));
        };
        match self.host.dispatch_action(plugin, hook, ev).await {
            Ok(()) => Ok(()),
            // The stored version subscribes but the loaded one does not: this node has
            // not reloaded the update yet (review round 1, m4).
            Err(PluginError::NotSubscribed { .. }) if manifest_lists(&row.manifest, hook) => {
                Err(JobError::Retry("not reloaded on this node yet".to_owned()))
            }
            Err(PluginError::NotSubscribed { .. }) => {
                tracing::info!(plugin, hook, "plugin does not handle the action; dropped");
                Ok(())
            }
            Err(PluginError::UnknownPlugin(_)) => {
                Err(JobError::Retry("not loaded on this node yet".to_owned()))
            }
            Err(e @ (PluginError::Disabled { .. } | PluginError::CallFailed { .. })) => {
                Err(JobError::Retry(e.to_string()))
            }
            Err(e @ PluginError::Busy { .. }) => Err(JobError::Busy(e.to_string())),
            // `dispatch_action` returns none of these; retrying is the safe answer.
            Err(
                e @ (PluginError::Engine(_)
                | PluginError::InvalidConfig(_)
                | PluginError::Inspect(_)
                | PluginError::LimitAboveDefault { .. }
                | PluginError::LimitAboveCeiling { .. }
                | PluginError::Link { .. }
                | PluginError::RouteNotFound { .. }),
            ) => Err(JobError::Retry(e.to_string())),
        }
    }
}

/// Whether a stored manifest (the TOML table as JSON) has `hook` in its `hooks` table.
fn manifest_lists(manifest: &serde_json::Value, hook: &str) -> bool {
    manifest
        .get("hooks")
        .and_then(serde_json::Value::as_object)
        .is_some_and(|hooks| hooks.contains_key(hook))
}

/// The core handlers: scheduled publishing on `content`, the plugin purge and its
/// sweep on `extensions`, plugin actions on `host` (at most `action_slots` at once,
/// [`action_slots`]), and the permalink-change redirects.
pub fn handlers(
    store: Arc<dyn Store>,
    content: Arc<ContentService>,
    extensions: Arc<ExtensionService>,
    host: Arc<PluginHost>,
    action_slots: usize,
) -> Vec<Arc<dyn JobHandler>> {
    vec![
        Arc::new(PublishScheduledHandler {
            content: content.clone(),
        }),
        Arc::new(PublishDueHandler {
            store: store.clone(),
            content,
        }),
        Arc::new(PluginPurgeHandler {
            extensions: extensions.clone(),
        }),
        Arc::new(PluginPurgeSweepHandler {
            store: store.clone(),
            extensions,
        }),
        Arc::new(PermalinkRedirectsHandler {
            redirects: Arc::new(RedirectService::new(store.clone())),
            pages: STRUCTURE_JOB_PAGES,
        }),
        Arc::new(PluginActionHandler::new(store, host, action_slots)),
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

/// The [`PLUGIN_PURGE_SWEEP_KIND`] periodic, every [`PLUGIN_PURGE_SWEEP_EVERY`].
pub fn plugin_purge_sweep_periodic() -> Periodic {
    Periodic {
        kind: PLUGIN_PURGE_SWEEP_KIND,
        every: PLUGIN_PURGE_SWEEP_EVERY,
        payload: json!({"v": 1}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Task 11 carry: plugin actions leave one job worker free, never exceed the host's
    /// call slots, and always have at least one slot.
    #[test]
    fn action_slots_stay_below_the_workers() {
        assert_eq!(action_slots(2, 32), 1);
        assert_eq!(action_slots(8, 32), 7);
        assert_eq!(action_slots(8, 3), 3);
        assert_eq!(action_slots(1, 32), 1);
        assert_eq!(action_slots(0, 32), 1);
        assert_eq!(action_slots(256, 1), 1);
    }
}
