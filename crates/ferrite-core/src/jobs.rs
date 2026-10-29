//! Background jobs core enqueues and their payloads (design 04 §6, ruling P12; plugin
//! purge M2a-2 P9; plugin actions P20; media variants M2a-2b P8; permalink redirects
//! M3-2 P30).
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

/// With `purge = true` the delete purges at once and its job sweeps again this long
/// after the deletion: a node that had not reloaded yet may still have written KV
/// (review round 1).
pub const PLUGIN_PURGE_GRACE: Duration = Duration::from_secs(120);

/// Daily safety net: enqueues a [`PLUGIN_PURGE_KIND`] job for every deletion marker
/// older than [`PLUGIN_DATA_RETENTION`] whose plugin is not installed, so a purge job
/// that went dead is retried. Payload `{"v":1}`.
pub const PLUGIN_PURGE_SWEEP_KIND: &str = "plugin.purge_sweep";

/// Period of the [`PLUGIN_PURGE_SWEEP_KIND`] job.
pub const PLUGIN_PURGE_SWEEP_EVERY: Duration = Duration::from_secs(24 * 3600);

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
    job.run_at = Some(after(purge.deleted_at, PLUGIN_DATA_RETENTION));
    job
}

/// Delivers one action hook to one plugin (M2a-2 P20); payload [`PluginAction`].
/// Enqueued by [`actions::enqueue`](crate::actions::enqueue) in the write `Tx`.
pub const PLUGIN_ACTION_KIND: &str = "plugin.action";

/// Current version of the [`PluginAction`] payload.
pub const PLUGIN_ACTION_VERSION: u32 = 1;

/// Payload of a [`PLUGIN_ACTION_KIND`] job:
/// `{"v":1,"plugin":"<name>","hook":"<action hook>","subject":"<id>"|null,"payload":{…}}`.
/// `subject` and `payload` are the guest's `action-event`; they carry ids only (never
/// content bodies or secrets): the plugin reads what it may see through its imports.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginAction {
    pub v: u32,
    pub plugin: String,
    pub hook: String,
    pub subject: Option<String>,
    pub payload: Value,
}

/// Priority of [`PLUGIN_ACTION_KIND`] jobs: below the default (0) of the core and
/// built-in jobs, so a burst of slow actions does not delay scheduled publishing or the
/// prune jobs on a busy runner (claims take higher priorities first).
pub const PLUGIN_ACTION_PRIORITY: i32 = -1;

/// The job for `action`: due now, no unique key (one job per plugin and event),
/// [`PLUGIN_ACTION_PRIORITY`], default attempts.
pub fn plugin_action_job(action: &PluginAction) -> NewJob {
    let mut payload = Map::new();
    payload.insert("v".into(), Value::from(action.v));
    payload.insert("plugin".into(), Value::String(action.plugin.clone()));
    payload.insert("hook".into(), Value::String(action.hook.clone()));
    payload.insert(
        "subject".into(),
        action.subject.clone().map_or(Value::Null, Value::String),
    );
    payload.insert("payload".into(), action.payload.clone());
    let mut job = NewJob::new(PLUGIN_ACTION_KIND, Value::Object(payload));
    job.priority = PLUGIN_ACTION_PRIORITY;
    job
}

/// Makes the size variants of one raster upload (M2a-2b P8); payload [`MediaVariants`].
/// Enqueued by [`MediaService::record_upload`](crate::MediaService::record_upload) in
/// the upload's `Tx`; the handler records its result through
/// [`MediaService::record_variants`](crate::MediaService::record_variants).
pub const MEDIA_VARIANTS_KIND: &str = "media.variants";

/// Current version of the [`MediaVariants`] payload.
pub const MEDIA_VARIANTS_VERSION: u32 = 1;

/// Payload of a [`MEDIA_VARIANTS_KIND`] job: `{"v":1,"item":"<uuid>","artifact":"<hash>"}`.
/// `artifact` is the original's hash at upload time: the result is recorded only while
/// the media row still holds it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaVariants {
    pub v: u32,
    pub item: ContentId,
    pub artifact: String,
}

/// The job for `variants`: due now, unique key `media.variants:<item>:<artifact>` (one
/// per item and original), default priority and attempts.
pub fn media_variants_job(variants: &MediaVariants) -> NewJob {
    let mut payload = Map::new();
    payload.insert("v".into(), Value::from(variants.v));
    payload.insert("item".into(), Value::String(variants.item.to_string()));
    payload.insert("artifact".into(), Value::String(variants.artifact.clone()));
    let mut job = NewJob::new(MEDIA_VARIANTS_KIND, Value::Object(payload));
    job.unique_key = Some(format!(
        "{MEDIA_VARIANTS_KIND}:{}:{}",
        variants.item, variants.artifact
    ));
    job
}

/// Redirects every published post or page from its path under the previous
/// `permalink.*` structures to its path under the current ones (M3-2 P30); payload
/// [`PermalinkRedirects`]. Enqueued by every `permalink.*` write in its `Tx`
/// ([`OptionService`](crate::OptionService) and the importer's
/// [`ImportBatch::options`](crate::import::ImportBatch::options)); the handler runs
/// [`RedirectService::structure_change`](crate::RedirectService::structure_change).
pub const PERMALINK_REDIRECTS_KIND: &str = "permalink.redirects";

/// Current version of the [`PermalinkRedirects`] payload.
pub const PERMALINK_REDIRECTS_VERSION: u32 = 1;

/// The post and page structures in effect (defaults filled in).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermalinkStructures {
    pub post: String,
    pub page: String,
}

/// Payload of a [`PERMALINK_REDIRECTS_KIND`] job:
/// `{"v":1,"prev":{"post":…,"page":…},"next":{"post":…,"page":…},"version":<seq>}`.
/// `prev` are the structures before the write, `next` the ones it left (informational:
/// the job redirects to the structures stored when it runs, so jobs of two quick changes
/// give the same result in either order). `version` is the options version of the write:
/// the event sequence number of its last `options.changed` event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermalinkRedirects {
    pub v: u32,
    pub prev: PermalinkStructures,
    pub next: PermalinkStructures,
    pub version: i64,
}

/// The job for `redirects`: due now, unique key `permalink.redirects:<version>` (one per
/// options version), default priority and attempts.
pub fn permalink_redirects_job(redirects: &PermalinkRedirects) -> NewJob {
    let structures = |s: &PermalinkStructures| {
        let mut m = Map::new();
        m.insert("post".into(), Value::String(s.post.clone()));
        m.insert("page".into(), Value::String(s.page.clone()));
        Value::Object(m)
    };
    let mut payload = Map::new();
    payload.insert("v".into(), Value::from(redirects.v));
    payload.insert("prev".into(), structures(&redirects.prev));
    payload.insert("next".into(), structures(&redirects.next));
    payload.insert("version".into(), Value::from(redirects.version));
    let mut job = NewJob::new(PERMALINK_REDIRECTS_KIND, Value::Object(payload));
    job.unique_key = Some(format!("{PERMALINK_REDIRECTS_KIND}:{}", redirects.version));
    job
}

/// `at + d`; `at` itself if that overflowed (unreachable for the durations used here,
/// it only avoids a panic path).
pub(crate) fn after(at: Timestamp, d: Duration) -> Timestamp {
    chrono::TimeDelta::from_std(d)
        .ok()
        .and_then(|d| at.as_datetime().checked_add_signed(d))
        .map_or(at, Timestamp::from)
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
    fn plugin_action_job_shape() {
        let typed = PluginAction {
            v: PLUGIN_ACTION_VERSION,
            plugin: "seo".into(),
            hook: "post.saved".into(),
            subject: Some("abc".into()),
            payload: serde_json::json!({"post": "abc"}),
        };
        let job = plugin_action_job(&typed);
        assert_eq!(job.kind, "plugin.action");
        assert_eq!(job.unique_key, None);
        assert_eq!(job.run_at, None);
        assert_eq!(job.priority, -1);
        assert_eq!(job.max_attempts, ferrite_domain::DEFAULT_MAX_ATTEMPTS);
        assert_eq!(job.payload, serde_json::to_value(&typed).unwrap());
        let back: PluginAction = serde_json::from_value(job.payload).unwrap();
        assert_eq!(back, typed);
        let none = plugin_action_job(&PluginAction {
            subject: None,
            ..typed
        });
        assert_eq!(none.payload["subject"], Value::Null);
    }

    #[test]
    fn media_variants_job_shape() {
        let item = ContentId::new();
        let typed = MediaVariants {
            v: MEDIA_VARIANTS_VERSION,
            item,
            artifact: "ab".repeat(32),
        };
        let job = media_variants_job(&typed);
        assert_eq!(job.kind, "media.variants");
        assert_eq!(
            job.unique_key,
            Some(format!("media.variants:{item}:{}", "ab".repeat(32)))
        );
        assert!(job.unique_key.as_ref().unwrap().len() <= 200);
        assert_eq!(job.run_at, None);
        assert_eq!(job.priority, 0);
        assert_eq!(job.max_attempts, ferrite_domain::DEFAULT_MAX_ATTEMPTS);
        assert_eq!(job.payload, serde_json::to_value(&typed).unwrap());
        let back: MediaVariants = serde_json::from_value(job.payload).unwrap();
        assert_eq!(back, typed);
    }

    #[test]
    fn permalink_redirects_job_shape() {
        let typed = PermalinkRedirects {
            v: PERMALINK_REDIRECTS_VERSION,
            prev: PermalinkStructures {
                post: "/%slug%/".into(),
                page: "/%parent_path%/%slug%/".into(),
            },
            next: PermalinkStructures {
                post: "/blog/%slug%/".into(),
                page: "/%parent_path%/%slug%/".into(),
            },
            version: 42,
        };
        let job = permalink_redirects_job(&typed);
        assert_eq!(job.kind, "permalink.redirects");
        assert_eq!(job.unique_key.as_deref(), Some("permalink.redirects:42"));
        assert_eq!(job.run_at, None);
        assert_eq!(job.priority, 0);
        assert_eq!(job.max_attempts, ferrite_domain::DEFAULT_MAX_ATTEMPTS);
        assert_eq!(job.payload, serde_json::to_value(&typed).unwrap());
        let back: PermalinkRedirects = serde_json::from_value(job.payload).unwrap();
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
