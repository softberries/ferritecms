//! Background jobs core enqueues and their payloads (design 04 §6, ruling P12).
//!
//! The handlers live with the job runner wiring (`ferrite`); this module fixes the kinds,
//! keys and payload shapes both sides agree on. Payloads carry a version `v` (04 §9).

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
    // Same form as `Timestamp`'s serde (chrono's RFC 3339 with `Z`).
    payload.insert(
        "at".into(),
        Value::String(
            at.as_datetime()
                .to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true),
        ),
    );
    let mut job = NewJob::new(PUBLISH_SCHEDULED_KIND, Value::Object(payload));
    job.unique_key = Some(format!("publish:{id}:{}", at.as_micros()));
    job.run_at = Some(at);
    job
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
}
