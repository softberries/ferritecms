//! Background jobs core enqueues and their payloads (design 04 §6, ruling P12; plugin
//! purge M2a-2 P9).
//!
//! The handlers live with the job runner wiring (`ferrite`); this module fixes the kinds,
//! keys and payload shapes both sides agree on. Payloads carry a version `v` (04 §9).

use std::time::Duration;

use ferrite_domain::{ContentId, NewJob, Timestamp};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Publishes one scheduled item at its scheduled time; payload [`PublishScheduled`].
pub const PUBLISH_SCHEDULED_KIND: &str = "content.publish_scheduled";
/// Periodic safety net (every 30 s): publishes every due scheduled item, including items
/// scheduled before M1.5 or by an older binary. Empty payload.
pub const PUBLISH_DUE_KIND: &str = "content.publish_due";

/// Current version of the [`PublishScheduled`] payload.
pub const PUBLISH_SCHEDULED_VERSION: u32 = 1;

/// Payload of a [`PUBLISH_SCHEDULED_KIND`] job: `{"v":1,"id":"<uuid>","at":"<RFC 3339>"}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublishScheduled {
    pub v: u32,
    pub id: ContentId,
    /// The `scheduled_at` the job was enqueued for.
    pub at: Timestamp,
}

/// The job [`ContentService::schedule`](crate::ContentService::schedule) enqueues for
/// item `id` scheduled at `at`: due at `at`, unique key `publish:<id>:<at unix micros>`.
/// The time is part of the key so that rescheduling enqueues a new job while the row of
/// the earlier one still exists; that earlier job then finds the item scheduled for
/// another time and does nothing.
pub fn publish_scheduled_job(id: ContentId, at: Timestamp) -> NewJob {
    let mut payload = Map::new();
    payload.insert("v".into(), Value::from(PUBLISH_SCHEDULED_VERSION));
    payload.insert("id".into(), Value::String(id.to_string()));
    payload.insert("at".into(), Value::String(rfc3339(at)));
    let mut job = NewJob::new(PUBLISH_SCHEDULED_KIND, Value::Object(payload));
    job.unique_key = Some(format!("publish:{id}:{}", at.as_micros()));
    job.run_at = Some(at);
    job
}

/// Deletes a removed plugin's KV and settings once its retention ends; payload
/// [`PluginPurge`].
pub const PLUGIN_PURGE_KIND: &str = "plugin.purge";

/// Current version of the [`PluginPurge`] payload.
pub const PLUGIN_PURGE_VERSION: u32 = 1;

/// How long a deleted plugin's KV and settings are kept (M2a-2 P9): a reinstall within
/// this time finds them.
pub const PLUGIN_DATA_RETENTION: Duration = Duration::from_secs(30 * 24 * 3600);

/// Payload of a [`PLUGIN_PURGE_KIND`] job:
/// `{"v":1,"name":"<plugin>","deleted_at":"<RFC 3339>","settings":["<key>",…]}`.
/// `settings` are the keys of the deleted row's `manifest.settings` table; the job
/// deletes option `plugin.<name>.settings.<key>` for each.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginPurge {
    pub v: u32,
    pub name: String,
    /// When the plugin was deleted; the purge runs only if this is still the latest
    /// deletion (F14).
    pub deleted_at: Timestamp,
    pub settings: Vec<String>,
}

/// The job a plugin delete enqueues: due [`PLUGIN_DATA_RETENTION`] after
/// `deleted_at`, unique key `plugin.purge:<name>:<deleted_at unix micros>`.
pub fn plugin_purge_job(purge: &PluginPurge) -> NewJob {
    let mut payload = Map::new();
    payload.insert("v".into(), Value::from(purge.v));
    payload.insert("name".into(), Value::String(purge.name.clone()));
    payload.insert(
        "deleted_at".into(),
        Value::String(rfc3339(purge.deleted_at)),
    );
    payload.insert(
        "settings".into(),
        Value::Array(purge.settings.iter().cloned().map(Value::String).collect()),
    );
    let mut job = NewJob::new(PLUGIN_PURGE_KIND, Value::Object(payload));
    job.unique_key = Some(format!(
        "{PLUGIN_PURGE_KIND}:{}:{}",
        purge.name,
        purge.deleted_at.as_micros()
    ));
    // 30 days cannot overflow a timestamp the store accepted; `deleted_at` is the
    // fallback only to avoid a panic path.
    let due = chrono::TimeDelta::from_std(PLUGIN_DATA_RETENTION)
        .ok()
        .and_then(|d| purge.deleted_at.as_datetime().checked_add_signed(d))
        .map_or(purge.deleted_at, Timestamp::from);
    job.run_at = Some(due);
    job
}

/// Same form as `Timestamp`'s serde (chrono's RFC 3339 with `Z`).
fn rfc3339(at: Timestamp) -> String {
    at.as_datetime()
        .to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn publish_scheduled_job_shape() {
        let id = ContentId::new();
        let at = Timestamp::from_micros(1_767_225_600_123_456).unwrap();
        let job = publish_scheduled_job(id, at);
        assert_eq!(job.kind, "content.publish_scheduled");
        assert_eq!(
            job.unique_key.as_deref(),
            Some(format!("publish:{id}:1767225600123456").as_str())
        );
        assert_eq!(job.run_at, Some(at));
        assert_eq!(job.priority, 0);
        assert_eq!(job.max_attempts, ferrite_domain::DEFAULT_MAX_ATTEMPTS);
        let typed = PublishScheduled { v: 1, id, at };
        assert_eq!(job.payload, serde_json::to_value(typed).unwrap());
        assert_eq!(job.payload["at"], "2026-01-01T00:00:00.123456Z");
        let back: PublishScheduled = serde_json::from_value(job.payload).unwrap();
        assert_eq!(back, typed);
    }

    #[test]
    fn plugin_purge_job_shape() {
        let at = Timestamp::from_micros(1_767_225_600_123_456).unwrap();
        let typed = PluginPurge {
            v: PLUGIN_PURGE_VERSION,
            name: "seo".into(),
            deleted_at: at,
            settings: vec!["api_key".into(), "title".into()],
        };
        let job = plugin_purge_job(&typed);
        assert_eq!(job.kind, "plugin.purge");
        assert_eq!(
            job.unique_key.as_deref(),
            Some("plugin.purge:seo:1767225600123456")
        );
        let due = job.run_at.unwrap();
        assert_eq!(
            due.as_micros() - at.as_micros(),
            30 * 24 * 3600 * 1_000_000_i64
        );
        assert_eq!(job.max_attempts, ferrite_domain::DEFAULT_MAX_ATTEMPTS);
        assert_eq!(job.payload, serde_json::to_value(&typed).unwrap());
        assert_eq!(job.payload["deleted_at"], "2026-01-01T00:00:00.123456Z");
        let back: PluginPurge = serde_json::from_value(job.payload).unwrap();
        assert_eq!(back, typed);
    }
}
