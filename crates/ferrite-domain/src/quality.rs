//! Content quality rows (M6-4, design 16 §7, migration `0019_content_health`):
//! suppressions, health runs and their findings, and the external link cache.

use serde_json::Value;

use crate::{ContentId, HealthRunId, RedirectId, Timestamp, UserId, string_enum};

/// One finding a person decided to accept (`quality_suppressions`): on `item_id`, for
/// `rule` and `key` (`""`: every finding of the rule on the item).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Suppression {
    pub item_id: ContentId,
    pub rule: String,
    pub key: String,
    /// Why (1..=500 characters).
    pub reason: String,
    pub user_id: Option<UserId>,
    pub created_at: Timestamp,
}

string_enum!(
    /// The state of a health run.
    HealthRunStatus {
        Running => "running",
        Done => "done",
        Failed => "failed",
        Abandoned => "abandoned",
    }
);

string_enum!(
    /// What started a health run.
    HealthTrigger {
        Schedule => "schedule",
        Manual => "manual",
    }
);

/// What a run counted (`health_runs.items_checked`, `paths_checked`, `urls_checked`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HealthCounters {
    pub items: i64,
    pub paths: i64,
    pub urls: i64,
}

/// One content health run (`health_runs`).
#[derive(Debug, Clone, PartialEq)]
pub struct HealthRun {
    pub id: HealthRunId,
    pub status: HealthRunStatus,
    pub trigger: HealthTrigger,
    pub requested_by: Option<UserId>,
    pub node: String,
    pub started_at: Timestamp,
    pub finished_at: Option<Timestamp>,
    pub counters: HealthCounters,
    /// Unsuppressed findings per rule and per severity:
    /// `{"rules": {"image-alt-missing": 3}, "severity": {"error": 3}}`.
    pub totals: Value,
    pub truncated: bool,
    /// A sanitized error of a failed run.
    pub error: Option<String>,
}

/// A finding to store for a run.
#[derive(Debug, Clone, PartialEq)]
pub struct NewFinding {
    pub rule: String,
    pub severity: String,
    pub category: String,
    pub item_id: Option<ContentId>,
    pub redirect_id: Option<RedirectId>,
    /// Copied from the item for the filters.
    pub item_type: Option<String>,
    pub locale: Option<String>,
    pub key: String,
    pub context: Option<String>,
    pub detail: Value,
}

/// A stored finding: its run, its position in the run, and the finding.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredFinding {
    pub run_id: HealthRunId,
    pub seq: i64,
    pub finding: NewFinding,
}

/// A page of a run's findings, by `seq`; `None` filters are not applied.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FindingQuery {
    pub run: Option<HealthRunId>,
    pub category: Option<String>,
    pub rule: Option<String>,
    pub severity: Option<String>,
    pub item_type: Option<String>,
    pub locale: Option<String>,
    pub limit: u32,
    pub cursor: Option<String>,
}

string_enum!(
    /// What the last check of an external URL found (`link_checks.status`).
    LinkStatus {
        Ok => "ok",
        Failing => "failing",
        Broken => "broken",
        Skipped => "skipped",
    }
);

/// The cached result of an external link check (`link_checks`), shared by every node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkCheck {
    pub url: String,
    pub host: String,
    pub status: LinkStatus,
    pub http_status: Option<i32>,
    /// A short class of what happened (`timeout`, `nxdomain`, `robots`), never a body.
    pub detail: Option<String>,
    /// Failing checks in a row.
    pub failures: i32,
    pub first_failed_at: Option<Timestamp>,
    pub checked_at: Timestamp,
    /// The last run that found the URL in a published item.
    pub last_seen_at: Timestamp,
}
