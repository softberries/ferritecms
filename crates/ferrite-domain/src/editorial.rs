//! Editorial process (design 15): releases and their entries, approval workflows,
//! reviews and preview links.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::{
    ContentId, DomainError, PreviewLinkId, ReleaseId, ReviewId, RevisionId, TermId, Timestamp,
    UserId, string_enum,
};

/// Most entries one release holds (D6).
pub const MAX_RELEASE_ENTRIES: usize = 200;

string_enum!(
    /// The life of a release (D35): `open` → `scheduled` → `applied`, or `failed`.
    ReleaseStatus {
        Open => "open",
        Scheduled => "scheduled",
        Applied => "applied",
        Failed => "failed",
    }
);

string_enum!(
    /// What an entry does to its item's status when the release is applied (D2).
    EntryTransition {
        None => "none",
        Publish => "publish",
        Unpublish => "unpublish",
    }
);

/// A named set of entries applied together (design 15 §4.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub id: ReleaseId,
    pub title: String,
    pub description: String,
    pub status: ReleaseStatus,
    pub scheduled_at: Option<Timestamp>,
    pub scheduled_by: Option<UserId>,
    pub override_reason: Option<String>,
    /// The last failure report (`{at, reason, entries}`), while `failed`.
    pub failure: Option<Value>,
    pub created_by: Option<UserId>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub applied_at: Option<Timestamp>,
    pub applied_by: Option<UserId>,
    pub version: i64,
}

/// The editable fields of an item as a release will write them (D1), the shape of
/// `ContentDraft` without the type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StagedChange {
    pub title: String,
    #[serde(default)]
    pub slug: Option<String>,
    #[serde(default)]
    pub parent_id: Option<ContentId>,
    #[serde(default)]
    pub excerpt: Option<String>,
    pub body: Value,
    #[serde(default = "empty_object")]
    pub fields: Value,
    #[serde(default)]
    pub menu_order: i32,
    #[serde(default)]
    pub terms: Vec<TermId>,
    /// An SEO patch (`SeoPatch` as JSON); `None` keeps the item's SEO meta.
    #[serde(default)]
    pub seo: Option<Value>,
}

fn empty_object() -> Value {
    Value::Object(Default::default())
}

/// One item in one release (design 15 §4.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseEntry {
    pub release_id: ReleaseId,
    pub item_id: ContentId,
    pub position: i32,
    pub transition: EntryTransition,
    pub changes: Option<StagedChange>,
    /// The item's version when the entry was added or last rebased.
    pub base_version: i64,
    /// Bumped on every change of the entry; entry reviews bind to it.
    pub entry_version: i64,
    pub added_by: Option<UserId>,
    pub updated_at: Timestamp,
}

/// A page of releases: newest first (`created_at DESC, id DESC`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReleaseQuery {
    pub status: Option<ReleaseStatus>,
    pub created_by: Option<UserId>,
    pub cursor: Option<String>,
    pub limit: u32,
}

/// Who may approve a workflow step (D17): roles (`editor`, `admin`) and named users.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Approvers {
    #[serde(default)]
    pub roles: Vec<crate::Role>,
    #[serde(default)]
    pub users: Vec<UserId>,
}

/// One review step of a workflow (design 15 §6.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowStep {
    pub key: String,
    pub label: String,
    pub approvers: Approvers,
    pub required: u32,
}

/// A content type's approval workflow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workflow {
    pub type_name: String,
    pub steps: Vec<WorkflowStep>,
    pub updated_by: Option<UserId>,
    pub updated_at: Timestamp,
    pub version: i64,
}

string_enum!(
    /// The state of a review (design 15 §6.2). `in_review` and `approved` are open.
    ReviewStatus {
        InReview => "in_review",
        Approved => "approved",
        ChangesRequested => "changes_requested",
        Withdrawn => "withdrawn",
        Stale => "stale",
    }
);

impl ReviewStatus {
    /// `in_review` or `approved`: at most one open review per target.
    pub fn is_open(self) -> bool {
        matches!(self, Self::InReview | Self::Approved)
    }
}

string_enum!(
    /// One approver's decision on one step.
    Decision {
        Approve => "approve",
        RequestChanges => "request_changes",
    }
);

/// What a review approves (D18).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReviewTarget {
    /// An item's newest revision.
    Item(ContentId),
    /// A release entry's staged change at an entry version.
    Entry(ReleaseId, ContentId),
}

impl ReviewTarget {
    /// The item under review.
    pub fn item(&self) -> ContentId {
        match self {
            Self::Item(i) | Self::Entry(_, i) => *i,
        }
    }

    /// The release of an entry review.
    pub fn release(&self) -> Option<ReleaseId> {
        match self {
            Self::Item(_) => None,
            Self::Entry(r, _) => Some(*r),
        }
    }

    /// `item:<id>` or `entry:<release>:<item>`, the stored key.
    pub fn key(&self) -> String {
        match self {
            Self::Item(i) => format!("item:{i}"),
            Self::Entry(r, i) => format!("entry:{r}:{i}"),
        }
    }

    /// Parses [`key`](Self::key).
    pub fn parse(key: &str) -> Result<Self, DomainError> {
        let bad = || DomainError::InvalidEnum {
            kind: "ReviewTarget",
            value: key.to_owned(),
        };
        if let Some(i) = key.strip_prefix("item:") {
            return Ok(Self::Item(i.parse().map_err(|_| bad())?));
        }
        let rest = key.strip_prefix("entry:").ok_or_else(bad)?;
        let (r, i) = rest.split_once(':').ok_or_else(bad)?;
        Ok(Self::Entry(
            r.parse().map_err(|_| bad())?,
            i.parse().map_err(|_| bad())?,
        ))
    }
}

/// One round of approval of a target (design 15 §6.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Review {
    pub id: ReviewId,
    pub target: ReviewTarget,
    /// Item reviews: the newest revision when submitted.
    pub revision_id: Option<RevisionId>,
    /// Entry reviews: the entry version when submitted.
    pub entry_version: Option<i64>,
    pub status: ReviewStatus,
    /// The current step (index into `steps`).
    pub step: u32,
    /// The workflow's steps when the review started (D23).
    pub steps: Vec<WorkflowStep>,
    pub note: Option<String>,
    pub submitted_by: Option<UserId>,
    pub submitted_at: Timestamp,
    pub decided_at: Option<Timestamp>,
    pub version: i64,
}

/// One recorded decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewDecision {
    pub id: Uuid,
    pub review_id: ReviewId,
    pub step: u32,
    pub user_id: Option<UserId>,
    pub decision: Decision,
    pub comment: Option<String>,
    pub at: Timestamp,
}

/// A page of reviews, oldest first (`submitted_at, id`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReviewQuery {
    pub status: Option<ReviewStatus>,
    /// Reviews whose current step lists this user as an assignee.
    pub assignee: Option<UserId>,
    pub submitted_by: Option<UserId>,
    /// The item's content type.
    pub kind: Option<String>,
    pub cursor: Option<String>,
    pub limit: u32,
}

string_enum!(
    /// What a preview link opens.
    PreviewTargetKind {
        Item => "item",
        Release => "release",
    }
);

/// What a preview link opens (design 15 §7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PreviewTarget {
    Item(ContentId),
    Release(ReleaseId),
}

impl PreviewTarget {
    pub fn kind(&self) -> PreviewTargetKind {
        match self {
            Self::Item(_) => PreviewTargetKind::Item,
            Self::Release(_) => PreviewTargetKind::Release,
        }
    }

    pub fn id(&self) -> Uuid {
        match self {
            Self::Item(i) => i.as_uuid(),
            Self::Release(r) => r.as_uuid(),
        }
    }

    pub fn from_parts(kind: PreviewTargetKind, id: Uuid) -> Self {
        match kind {
            PreviewTargetKind::Item => Self::Item(ContentId::from_uuid(id)),
            PreviewTargetKind::Release => Self::Release(ReleaseId::from_uuid(id)),
        }
    }
}

/// A signed, expiring preview link (D26). The token hash is not part of the read type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewLink {
    pub id: PreviewLinkId,
    pub target: PreviewTarget,
    pub created_by: UserId,
    pub created_at: Timestamp,
    pub expires_at: Timestamp,
    pub last_used_at: Option<Timestamp>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn review_target_key_roundtrip() {
        let i = ContentId::new();
        let r = ReleaseId::new();
        for t in [ReviewTarget::Item(i), ReviewTarget::Entry(r, i)] {
            assert_eq!(ReviewTarget::parse(&t.key()).unwrap(), t);
            assert_eq!(t.item(), i);
        }
        assert!(ReviewTarget::parse("item:x").is_err());
        assert!(ReviewTarget::parse("other:1").is_err());
    }

    #[test]
    fn staged_change_json_defaults() {
        let c: StagedChange =
            serde_json::from_value(serde_json::json!({"title": "T", "body": []})).unwrap();
        assert_eq!(c.fields, serde_json::json!({}));
        assert!(c.terms.is_empty() && c.seo.is_none());
        assert!(
            serde_json::from_value::<StagedChange>(
                serde_json::json!({"title": "T", "body": [], "x": 1})
            )
            .is_err()
        );
    }
}
