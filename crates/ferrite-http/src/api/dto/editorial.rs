//! Releases, preview links, workflows and reviews (M6-3, design 15 §10).

use chrono::{DateTime, Utc};
use ferrite_core::{
    release::{EntryCheck, EntryOutcome, EntryView, ReleaseCheck, ReleaseView},
    workflow::ReviewSummary,
};
use ferrite_domain::{
    EntryTransition, PreviewLink, Release, Review, ReviewDecision, Role, StagedChange, Workflow,
    WorkflowStep,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

fn ts(t: ferrite_domain::Timestamp) -> DateTime<Utc> {
    t.into()
}

/// An entry's transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema, Default)]
#[serde(rename_all = "lowercase")]
pub enum TransitionDto {
    #[default]
    None,
    Publish,
    Unpublish,
}

impl From<TransitionDto> for EntryTransition {
    fn from(t: TransitionDto) -> Self {
        match t {
            TransitionDto::None => Self::None,
            TransitionDto::Publish => Self::Publish,
            TransitionDto::Unpublish => Self::Unpublish,
        }
    }
}

impl From<EntryTransition> for TransitionDto {
    fn from(t: EntryTransition) -> Self {
        match t {
            EntryTransition::None => Self::None,
            EntryTransition::Publish => Self::Publish,
            EntryTransition::Unpublish => Self::Unpublish,
        }
    }
}

/// A staged change: what the item will hold once the release is applied (the editable
/// fields of a content `PATCH`, all present).
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct StagedChangeDto {
    pub title: String,
    #[serde(default)]
    pub slug: Option<String>,
    #[serde(default)]
    pub parent_id: Option<Uuid>,
    #[serde(default)]
    pub excerpt: Option<String>,
    /// The block document (`{"type":"doc",…}`).
    pub body: serde_json::Value,
    #[serde(default)]
    pub fields: Option<serde_json::Value>,
    #[serde(default)]
    pub menu_order: i32,
    #[serde(default)]
    pub terms: Vec<Uuid>,
    /// An SEO patch (`title`, `description`, `canonical`, `robots`).
    #[serde(default)]
    pub seo: Option<serde_json::Value>,
}

impl From<StagedChangeDto> for StagedChange {
    fn from(c: StagedChangeDto) -> Self {
        Self {
            title: c.title,
            slug: c.slug,
            parent_id: c.parent_id.map(ferrite_domain::ContentId::from_uuid),
            excerpt: c.excerpt,
            body: c.body,
            fields: c.fields.unwrap_or_else(|| serde_json::json!({})),
            menu_order: c.menu_order,
            terms: c
                .terms
                .into_iter()
                .map(ferrite_domain::TermId::from_uuid)
                .collect(),
            seo: c.seo,
        }
    }
}

impl From<StagedChange> for StagedChangeDto {
    fn from(c: StagedChange) -> Self {
        Self {
            title: c.title,
            slug: c.slug,
            parent_id: c.parent_id.map(|p| p.as_uuid()),
            excerpt: c.excerpt,
            body: c.body,
            fields: Some(c.fields),
            menu_order: c.menu_order,
            terms: c.terms.into_iter().map(|t| t.as_uuid()).collect(),
            seo: c.seo,
        }
    }
}

/// `POST /releases/{id}/items`, `PUT /releases/{id}/items/{item}`.
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct EntryInput {
    /// Required on `POST`; on `PUT` the path names the item.
    #[serde(default)]
    pub item_id: Option<Uuid>,
    #[serde(default)]
    pub transition: TransitionDto,
    #[serde(default)]
    pub changes: Option<StagedChangeDto>,
}

/// `POST /releases`.
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct NewReleaseInput {
    pub title: String,
    #[serde(default)]
    pub description: String,
}

/// `PATCH /releases/{id}`.
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ReleasePatch {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
}

/// `POST /releases/{id}/schedule`.
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ScheduleReleaseInput {
    pub at: DateTime<Utc>,
    #[serde(default)]
    pub override_reason: Option<String>,
}

/// An optional override of missing approvals (admins, design 15 §6.5).
#[derive(Debug, Clone, Default, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct OverrideInput {
    #[serde(default)]
    pub override_reason: Option<String>,
}

/// An optional submit body: assignees of the first step and a note.
#[derive(Debug, Clone, Default, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SubmitInput {
    #[serde(default)]
    pub reviewers: Vec<Uuid>,
    #[serde(default)]
    pub note: Option<String>,
}

impl From<SubmitInput> for ferrite_core::workflow::SubmitInput {
    fn from(s: SubmitInput) -> Self {
        Self {
            reviewers: s
                .reviewers
                .into_iter()
                .map(ferrite_domain::UserId::from_uuid)
                .collect(),
            note: s.note,
        }
    }
}

/// A release without its entries.
#[derive(Debug, Serialize, ToSchema)]
pub struct ReleaseDto {
    pub id: Uuid,
    pub title: String,
    pub description: String,
    /// `open`, `scheduled`, `applied` or `failed`.
    pub status: String,
    pub scheduled_at: Option<DateTime<Utc>>,
    pub scheduled_by: Option<Uuid>,
    pub override_reason: Option<String>,
    /// The last failure (`{at, reason, entries}`), while `failed`.
    pub failure: Option<serde_json::Value>,
    pub created_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub applied_at: Option<DateTime<Utc>>,
    pub applied_by: Option<Uuid>,
    pub version: i64,
    /// Number of entries (lists; the detail has them).
    pub entry_count: u32,
}

impl ReleaseDto {
    pub fn new(r: Release, entry_count: u32) -> Self {
        Self {
            id: r.id.as_uuid(),
            title: r.title,
            description: r.description,
            status: r.status.as_str().to_owned(),
            scheduled_at: r.scheduled_at.map(ts),
            scheduled_by: r.scheduled_by.map(|u| u.as_uuid()),
            override_reason: r.override_reason,
            failure: r.failure,
            created_by: r.created_by.map(|u| u.as_uuid()),
            created_at: ts(r.created_at),
            updated_at: ts(r.updated_at),
            applied_at: r.applied_at.map(ts),
            applied_by: r.applied_by.map(|u| u.as_uuid()),
            version: r.version,
            entry_count,
        }
    }
}

/// A page of releases.
#[derive(Debug, Serialize, ToSchema)]
pub struct ReleaseList {
    pub items: Vec<ReleaseDto>,
    pub next_cursor: Option<String>,
}

/// The open review of an item or entry.
#[derive(Debug, Serialize, ToSchema)]
pub struct ReviewSummaryDto {
    pub id: Uuid,
    /// `in_review` or `approved`.
    pub status: String,
    pub step: u32,
    pub step_label: Option<String>,
    pub steps: u32,
    /// It approves what is stored now.
    pub current: bool,
    pub version: i64,
}

impl From<ReviewSummary> for ReviewSummaryDto {
    fn from(r: ReviewSummary) -> Self {
        Self {
            id: r.id.as_uuid(),
            status: r.status.as_str().to_owned(),
            step: r.step,
            step_label: r.step_label,
            steps: r.steps,
            current: r.current,
            version: r.version,
        }
    }
}

/// One entry with its item.
#[derive(Debug, Serialize, ToSchema)]
pub struct ReleaseEntryDto {
    pub item_id: Uuid,
    #[serde(rename = "type")]
    pub kind: String,
    pub title: String,
    pub locale: String,
    /// The item's status now.
    pub status: String,
    pub position: i32,
    pub transition: TransitionDto,
    pub has_changes: bool,
    pub changes: Option<StagedChangeDto>,
    pub base_version: i64,
    pub entry_version: i64,
    /// The item's version now.
    pub item_version: i64,
    /// The item moved since the entry's base: rebase first.
    pub conflict: bool,
    pub review: Option<ReviewSummaryDto>,
    pub updated_at: DateTime<Utc>,
}

impl From<EntryView> for ReleaseEntryDto {
    fn from(v: EntryView) -> Self {
        Self {
            item_id: v.item.id.as_uuid(),
            kind: v.item.kind,
            title: v.item.title,
            locale: v.item.locale,
            status: v.item.status.as_str().to_owned(),
            position: v.entry.position,
            transition: v.entry.transition.into(),
            has_changes: v.entry.changes.is_some(),
            changes: v.entry.changes.map(Into::into),
            base_version: v.entry.base_version,
            entry_version: v.entry.entry_version,
            item_version: v.item.version,
            conflict: v.conflict,
            review: v.review.map(Into::into),
            updated_at: ts(v.entry.updated_at),
        }
    }
}

/// A release with its entries.
#[derive(Debug, Serialize, ToSchema)]
pub struct ReleaseDetailDto {
    #[serde(flatten)]
    pub release: ReleaseDto,
    pub entries: Vec<ReleaseEntryDto>,
}

impl From<ReleaseView> for ReleaseDetailDto {
    fn from(v: ReleaseView) -> Self {
        let n = u32::try_from(v.entries.len()).unwrap_or(u32::MAX);
        Self {
            release: ReleaseDto::new(v.release, n),
            entries: v.entries.into_iter().map(Into::into).collect(),
        }
    }
}

/// One line of a check report.
#[derive(Debug, Serialize, ToSchema)]
pub struct EntryCheckDto {
    pub item: Uuid,
    /// `ok`, `conflict`, `needs_approval`, `forbidden` or `invalid`.
    pub outcome: String,
    pub current_version: Option<i64>,
    pub step: Option<String>,
    pub capability: Option<String>,
    pub field: Option<String>,
    pub message: Option<String>,
}

impl From<EntryCheck> for EntryCheckDto {
    fn from(c: EntryCheck) -> Self {
        let mut d = Self {
            item: c.item.as_uuid(),
            outcome: String::new(),
            current_version: None,
            step: None,
            capability: None,
            field: None,
            message: None,
        };
        d.outcome = match c.outcome {
            EntryOutcome::Ok => "ok",
            EntryOutcome::Conflict { current_version } => {
                d.current_version = Some(current_version);
                "conflict"
            }
            EntryOutcome::NeedsApproval { step } => {
                d.step = step;
                "needs_approval"
            }
            EntryOutcome::Forbidden { capability } => {
                d.capability = Some(capability);
                "forbidden"
            }
            EntryOutcome::Invalid { field, message } => {
                d.field = field;
                d.message = Some(message);
                "invalid"
            }
        }
        .to_owned();
        d
    }
}

/// `GET /releases/{id}/check`.
#[derive(Debug, Serialize, ToSchema)]
pub struct ReleaseCheckDto {
    /// Every entry is `ok`.
    pub clean: bool,
    pub entries: Vec<EntryCheckDto>,
}

impl From<ReleaseCheck> for ReleaseCheckDto {
    fn from(c: ReleaseCheck) -> Self {
        Self {
            clean: c.is_clean(),
            entries: c.entries.into_iter().map(Into::into).collect(),
        }
    }
}

/// A release holding a content item (on the content DTO).
#[derive(Debug, Serialize, ToSchema)]
pub struct ReleaseRefDto {
    pub id: Uuid,
    pub title: String,
    pub status: String,
}

impl From<Release> for ReleaseRefDto {
    fn from(r: Release) -> Self {
        Self {
            id: r.id.as_uuid(),
            title: r.title,
            status: r.status.as_str().to_owned(),
        }
    }
}

/// `POST …/preview-links`.
#[derive(Debug, Clone, Default, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PreviewLinkInput {
    /// 1 to 168; default 24.
    #[serde(default)]
    pub expires_in_hours: Option<u32>,
}

/// A preview link. `token` and `url` only in the create answer (shown once).
#[derive(Debug, Serialize, ToSchema)]
pub struct PreviewLinkDto {
    pub id: Uuid,
    /// `item` or `release`.
    pub target_kind: String,
    pub target_id: Uuid,
    pub created_by: Uuid,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

impl From<PreviewLink> for PreviewLinkDto {
    fn from(l: PreviewLink) -> Self {
        Self {
            id: l.id.as_uuid(),
            target_kind: l.target.kind().as_str().to_owned(),
            target_id: l.target.id(),
            created_by: l.created_by.as_uuid(),
            created_at: ts(l.created_at),
            expires_at: ts(l.expires_at),
            last_used_at: l.last_used_at.map(ts),
            token: None,
            url: None,
        }
    }
}

/// The links of a target.
#[derive(Debug, Serialize, ToSchema)]
pub struct PreviewLinkList {
    pub items: Vec<PreviewLinkDto>,
}

/// Who may approve a step.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ApproversDto {
    /// `editor` and/or `admin`.
    #[serde(default)]
    pub roles: Vec<String>,
    #[serde(default)]
    pub users: Vec<Uuid>,
}

/// One workflow step.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowStepDto {
    pub key: String,
    pub label: String,
    pub approvers: ApproversDto,
    pub required: u32,
}

impl From<WorkflowStep> for WorkflowStepDto {
    fn from(s: WorkflowStep) -> Self {
        Self {
            key: s.key,
            label: s.label,
            approvers: ApproversDto {
                roles: s
                    .approvers
                    .roles
                    .into_iter()
                    .map(|r| r.as_str().to_owned())
                    .collect(),
                users: s.approvers.users.into_iter().map(|u| u.as_uuid()).collect(),
            },
            required: s.required,
        }
    }
}

impl WorkflowStepDto {
    /// The domain step; an unknown role is `InvalidField { field: "steps" }`.
    pub fn into_step(self) -> Result<WorkflowStep, ferrite_core::CoreError> {
        let roles = self
            .approvers
            .roles
            .iter()
            .map(|r| {
                r.parse::<Role>().map_err(|_| {
                    ferrite_core::CoreError::invalid_field("steps", format!("unknown role {r:?}"))
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(WorkflowStep {
            key: self.key,
            label: self.label,
            approvers: ferrite_domain::Approvers {
                roles,
                users: self
                    .approvers
                    .users
                    .into_iter()
                    .map(ferrite_domain::UserId::from_uuid)
                    .collect(),
            },
            required: self.required,
        })
    }
}

/// A content type's workflow.
#[derive(Debug, Serialize, ToSchema)]
pub struct WorkflowDto {
    #[serde(rename = "type")]
    pub type_name: String,
    pub steps: Vec<WorkflowStepDto>,
    pub updated_by: Option<Uuid>,
    pub updated_at: DateTime<Utc>,
    pub version: i64,
}

impl From<Workflow> for WorkflowDto {
    fn from(w: Workflow) -> Self {
        Self {
            type_name: w.type_name,
            steps: w.steps.into_iter().map(Into::into).collect(),
            updated_by: w.updated_by.map(|u| u.as_uuid()),
            updated_at: ts(w.updated_at),
            version: w.version,
        }
    }
}

/// Every workflow.
#[derive(Debug, Serialize, ToSchema)]
pub struct WorkflowList {
    pub items: Vec<WorkflowDto>,
}

/// `PUT /workflows/{type}`.
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowInput {
    pub steps: Vec<WorkflowStepDto>,
}

/// One decision.
#[derive(Debug, Serialize, ToSchema)]
pub struct DecisionDto {
    pub step: u32,
    pub user_id: Option<Uuid>,
    /// `approve` or `request_changes`.
    pub decision: String,
    pub comment: Option<String>,
    pub at: DateTime<Utc>,
}

impl From<ReviewDecision> for DecisionDto {
    fn from(d: ReviewDecision) -> Self {
        Self {
            step: d.step,
            user_id: d.user_id.map(|u| u.as_uuid()),
            decision: d.decision.as_str().to_owned(),
            comment: d.comment,
            at: ts(d.at),
        }
    }
}

/// An assignee of a step.
#[derive(Debug, Serialize, ToSchema)]
pub struct AssigneeDto {
    pub step: u32,
    pub user_id: Uuid,
}

/// A review.
#[derive(Debug, Serialize, ToSchema)]
pub struct ReviewDto {
    pub id: Uuid,
    /// `item` or `entry`.
    pub target_kind: String,
    pub item_id: Uuid,
    pub release_id: Option<Uuid>,
    /// `in_review`, `approved`, `changes_requested`, `withdrawn` or `stale`.
    pub status: String,
    pub step: u32,
    pub steps: Vec<WorkflowStepDto>,
    pub note: Option<String>,
    pub submitted_by: Option<Uuid>,
    pub submitted_at: DateTime<Utc>,
    pub decided_at: Option<DateTime<Utc>>,
    pub version: i64,
    /// The item's title and type (detail and queue).
    pub item_title: Option<String>,
    pub item_type: Option<String>,
    /// Detail only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decisions: Option<Vec<DecisionDto>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assignees: Option<Vec<AssigneeDto>>,
}

impl From<Review> for ReviewDto {
    fn from(r: Review) -> Self {
        Self {
            id: r.id.as_uuid(),
            target_kind: match r.target {
                ferrite_domain::ReviewTarget::Item(_) => "item",
                ferrite_domain::ReviewTarget::Entry(..) => "entry",
            }
            .to_owned(),
            item_id: r.target.item().as_uuid(),
            release_id: r.target.release().map(|x| x.as_uuid()),
            status: r.status.as_str().to_owned(),
            step: r.step,
            steps: r.steps.into_iter().map(Into::into).collect(),
            note: r.note,
            submitted_by: r.submitted_by.map(|u| u.as_uuid()),
            submitted_at: ts(r.submitted_at),
            decided_at: r.decided_at.map(ts),
            version: r.version,
            item_title: None,
            item_type: None,
            decisions: None,
            assignees: None,
        }
    }
}

/// A page of reviews.
#[derive(Debug, Serialize, ToSchema)]
pub struct ReviewList {
    pub items: Vec<ReviewDto>,
    pub next_cursor: Option<String>,
    /// In-review reviews assigned to the caller at their current step.
    pub assigned_to_me: u64,
}

/// `POST /reviews/{id}/decisions`.
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct DecisionInput {
    /// `approve` or `request_changes`.
    pub decision: String,
    #[serde(default)]
    pub comment: Option<String>,
}

/// `PUT /reviews/{id}/assignees`.
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AssigneesInput {
    pub step: u32,
    pub users: Vec<Uuid>,
}

/// The assignees of a review.
#[derive(Debug, Serialize, ToSchema)]
pub struct AssigneeList {
    pub items: Vec<AssigneeDto>,
}
