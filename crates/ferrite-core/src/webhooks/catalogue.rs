//! The event catalogue (design 18 §3.1) and subscription matching.

use ferrite_domain::WebhookEndpoint;

use crate::CoreError;

/// The payload schema version (design 18 §3.3); only `1` exists.
pub const PAYLOAD_VERSION: i32 = 1;

/// The test event: sent by `POST /webhooks/{id}/test` only, never matched by a pattern.
pub const PING: &str = "webhook.ping";

/// At most this many patterns, and content types, per endpoint.
pub const MAX_PATTERNS: usize = 50;

/// One event type of the catalogue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventTypeInfo {
    pub kind: &'static str,
    pub group: &'static str,
    pub description: &'static str,
}

const fn t(kind: &'static str, group: &'static str, description: &'static str) -> EventTypeInfo {
    EventTypeInfo {
        kind,
        group,
        description,
    }
}

/// Every event type an endpoint can subscribe to, by group. Wave-B parts (M6-3 releases
/// `release.*`, M6-5 `change_set.reverted`, M6-4 `health.completed`) append theirs here.
pub const EVENT_TYPES: &[EventTypeInfo] = &[
    t("content.created", "content", "A content item was created."),
    t("content.updated", "content", "A content item was saved."),
    t(
        "content.published",
        "content",
        "A content item was published.",
    ),
    t(
        "content.unpublished",
        "content",
        "A published item went back to draft.",
    ),
    t(
        "content.scheduled",
        "content",
        "A content item was scheduled.",
    ),
    t(
        "content.submitted",
        "content",
        "A content item was submitted for review.",
    ),
    t("content.trashed", "content", "A content item was trashed."),
    t(
        "content.restored",
        "content",
        "A content item was restored from the trash.",
    ),
    t(
        "content.deleted",
        "content",
        "A content item was deleted permanently.",
    ),
    t("media.uploaded", "media", "A file was uploaded."),
    t(
        "media.updated",
        "media",
        "A media item's title or alt text changed.",
    ),
    t("media.trashed", "media", "A media item was trashed."),
    t("media.restored", "media", "A media item was restored."),
    t(
        "media.deleted",
        "media",
        "A media item was deleted permanently.",
    ),
    t("comment.created", "comment", "A comment was submitted."),
    t(
        "comment.moderated",
        "comment",
        "A comment's status changed.",
    ),
    t("comment.deleted", "comment", "A comment was deleted."),
    t("user.created", "user", "A user account was created."),
    t("user.updated", "user", "A user account changed."),
    t("user.deleted", "user", "A user account was deleted."),
    t("import.finished", "import", "An import run finished."),
    // M6-3 (design 15 §13).
    t("release.scheduled", "release", "A release was scheduled."),
    t(
        "release.unscheduled",
        "release",
        "A scheduled release was taken off the schedule.",
    ),
    t("release.applied", "release", "A release was applied."),
    t("release.failed", "release", "A scheduled release failed."),
    t(
        "review.submitted",
        "review",
        "Content was submitted for review.",
    ),
    t("review.approved", "review", "A review was approved."),
    t(
        "review.changes_requested",
        "review",
        "A reviewer asked for changes.",
    ),
    t(
        "review.overridden",
        "review",
        "An administrator published without the approvals.",
    ),
    // M6-5 (design 17 §7.4, ruling P2).
    t(
        "change_set.reverted",
        "change_set",
        "A person reverted changes an AI assistant made.",
    ),
    t(
        "health.completed",
        "health",
        "A content health run finished.",
    ),
];

/// The group of an event type (`content` for `content.published`).
pub fn group_of(kind: &str) -> &str {
    kind.split_once('.').map_or(kind, |(g, _)| g)
}

fn is_group(group: &str) -> bool {
    EVENT_TYPES.iter().any(|t| t.group == group)
}

fn is_type(kind: &str) -> bool {
    EVENT_TYPES.iter().any(|t| t.kind == kind)
}

/// Checks and normalises a subscription: 1..=[`MAX_PATTERNS`] patterns, each `*`,
/// `<known group>.*` or a known type (not [`PING`]); duplicates are dropped, the order
/// kept. Anything else is `InvalidField("events")`.
pub fn check_patterns(patterns: &[String]) -> Result<Vec<String>, CoreError> {
    let bad = |m: String| CoreError::invalid_field("events", m);
    if patterns.is_empty() {
        return Err(bad("choose at least one event".into()));
    }
    if patterns.len() > MAX_PATTERNS {
        return Err(bad(format!("at most {MAX_PATTERNS} events")));
    }
    let mut out: Vec<String> = Vec::with_capacity(patterns.len());
    for p in patterns {
        let ok = p == "*"
            || p.strip_suffix(".*").is_some_and(is_group)
            || (p.as_str() != PING && is_type(p));
        if !ok {
            let shown: String = p.chars().take(64).collect();
            return Err(bad(format!("unknown event type {shown:?}")));
        }
        if !out.contains(p) {
            out.push(p.clone());
        }
    }
    Ok(out)
}

/// Whether `endpoint` subscribes to `kind`: `*`, `<group>.*` or the exact type.
/// [`PING`] never matches. `content_types` narrows `content.*` events to those types and
/// has no effect on other groups.
pub fn matches(endpoint: &WebhookEndpoint, kind: &str, content_type: Option<&str>) -> bool {
    if kind == PING {
        return false;
    }
    let group = group_of(kind);
    let subscribed = endpoint
        .events
        .iter()
        .any(|p| p == "*" || p == kind || p.strip_suffix(".*").is_some_and(|g| g == group));
    if !subscribed {
        return false;
    }
    match (&endpoint.content_types, group) {
        (Some(types), "content") => content_type.is_some_and(|t| types.iter().any(|x| x == t)),
        _ => true,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use ferrite_domain::{Timestamp, WebhookId};

    use super::*;

    fn endpoint(events: &[&str], types: Option<&[&str]>) -> WebhookEndpoint {
        let now = Timestamp::now();
        WebhookEndpoint {
            id: WebhookId::new(),
            name: "n".into(),
            url: "https://h.example/".into(),
            events: events.iter().map(|e| (*e).to_owned()).collect(),
            content_types: types.map(|t| t.iter().map(|x| (*x).to_owned()).collect()),
            enabled: true,
            disabled_reason: None,
            disabled_at: None,
            install_id: "i".into(),
            secret_sealed: "s".into(),
            previous_secret_sealed: None,
            previous_secret_expires_at: None,
            payload_version: 1,
            failing_since: None,
            consecutive_failures: 0,
            last_success_at: None,
            last_failure_at: None,
            created_by: None,
            created_at: now,
            updated_at: now,
        }
    }

    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn patterns_and_groups() {
        assert_eq!(
            check_patterns(&strings(&["content.*", "media.uploaded", "content.*", "*"])).unwrap(),
            strings(&["content.*", "media.uploaded", "*"])
        );
        for bad in [
            "",
            "content",
            "content.nope",
            "nope.*",
            "webhook.ping",
            "webhook.*",
            "*.*",
        ] {
            assert!(
                matches!(
                    check_patterns(&strings(&[bad])),
                    Err(CoreError::InvalidField {
                        field: "events",
                        ..
                    })
                ),
                "{bad:?}"
            );
        }
        assert!(check_patterns(&[]).is_err());
        let many: Vec<String> = (0..51).map(|_| "*".to_owned()).collect();
        assert!(check_patterns(&many).is_err());
        // Every catalogue type is accepted and grouped by its prefix.
        for t in EVENT_TYPES {
            assert_eq!(group_of(t.kind), t.group);
            assert!(check_patterns(&[t.kind.to_owned()]).is_ok());
        }
        let e = endpoint(&["content.*", "user.deleted"], None);
        assert!(matches(&e, "content.published", Some("post")));
        assert!(matches(&e, "user.deleted", None));
        assert!(!matches(&e, "user.created", None));
        assert!(!matches(&e, "media.uploaded", None));
        let all = endpoint(&["*"], None);
        // `*` includes types added later.
        assert!(matches(&all, "release.applied", None));
    }

    #[test]
    fn content_types_narrow_content_only() {
        let e = endpoint(&["content.*", "media.*"], Some(&["post"]));
        assert!(matches(&e, "content.created", Some("post")));
        assert!(!matches(&e, "content.created", Some("page")));
        assert!(!matches(&e, "content.created", None));
        assert!(matches(&e, "media.uploaded", None));
    }

    #[test]
    fn ping_never_matches() {
        assert!(!matches(&endpoint(&["*"], None), PING, None));
    }
}
