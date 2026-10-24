//! Core job handlers run by every node (ruling P12): scheduled publishing, the plugin
//! data purge (M2a-2 P9) and plugin actions (P20). The kinds and payloads are fixed in
//! [`ferrite_core::jobs`]; the per-node 30 s ticker of M1c-2 is the periodic
//! [`PUBLISH_DUE_KIND`] job now, so one node runs it per period.

use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use ferrite_cluster::{JobError, JobHandler, Periodic};
use ferrite_core::{
    ContentService, CoreError, ExtensionService,
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
/// - the call failed: retried (backoff, then dead after the job's attempts).
///
/// How many run at once on a node is bounded by the runner's workers
/// (`cluster.workers`).
pub struct PluginActionHandler {
    pub store: Arc<dyn Store>,
    pub host: Arc<PluginHost>,
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
/// sweep on `extensions`, plugin actions on `host`.
pub fn handlers(
    store: Arc<dyn Store>,
    content: Arc<ContentService>,
    extensions: Arc<ExtensionService>,
    host: Arc<PluginHost>,
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
        Arc::new(PluginActionHandler { store, host }),
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
