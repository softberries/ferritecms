//! Bodies of `/change-sets` (M6-5, design 17 §9).

use chrono::{DateTime, Utc};
use ferrite_core::changes::{ConflictReason, RevertConflict, RevertReport};
use ferrite_domain::{ChangeSummary, ImageState};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;
use uuid::Uuid;

/// Who a set belongs to.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ChangeSetUserDto {
    pub id: Uuid,
    pub login: String,
    pub display_name: String,
}

/// The connection that wrote.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ChangeSetAgentDto {
    /// `oauth` (a connected app) or `token` (a personal API token).
    #[schema(example = "oauth")]
    pub kind: String,
    /// The grant or token id.
    pub id: Uuid,
    /// The client's name when the set opened.
    pub client: String,
    /// Whether the grant or token still exists.
    pub connected: bool,
}

/// One change set.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ChangeSetDto {
    pub id: Uuid,
    /// `null` once the user is deleted.
    pub user: Option<ChangeSetUserDto>,
    pub agent: ChangeSetAgentDto,
    pub started_at: DateTime<Utc>,
    pub last_at: DateTime<Utc>,
    /// Whether the assistant's next write still lands here.
    pub open: bool,
    pub changes: u32,
    /// How many changes are reverted.
    pub reverted: u32,
    /// `open`, `closed`, `reverted` or `partly_reverted`.
    #[schema(example = "open")]
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ChangeSetList {
    pub items: Vec<ChangeSetDto>,
    pub next_cursor: Option<String>,
}

/// The object a change touched.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ChangeObjectDto {
    /// `content`, `media`, `term`, `menu`, `type`, `redirect`, `comment`, `option`,
    /// `theme` or `plugin`.
    pub kind: String,
    pub id: String,
    /// Its title or name at write time.
    pub label: String,
    /// The admin screen of the object when it still exists (relative to `/admin`).
    pub admin_path: Option<String>,
}

/// One change.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ChangeDto {
    pub seq: u32,
    pub at: DateTime<Utc>,
    #[schema(example = "content.update")]
    pub op: String,
    pub object: ChangeObjectDto,
    /// The write created the object.
    pub created: bool,
    /// The write deleted the object.
    pub deleted: bool,
    /// `false` when an image was too large to keep.
    pub revertable: bool,
    pub reverted_at: Option<DateTime<Utc>>,
    pub reverted_by: Option<Uuid>,
    /// Reverted over a later edit.
    pub forced: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ChangeSetDetailDto {
    pub set: ChangeSetDto,
    /// Newest first.
    pub changes: Vec<ChangeDto>,
}

/// One change with its images.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ChangeDetailDto {
    pub change: ChangeDto,
    /// `shown`, or `hidden` when you may not read the object's area now.
    #[schema(example = "shown")]
    pub images: String,
    /// The object before (`null`: it did not exist, or hidden, or too large).
    pub before: Option<Value>,
    /// The object after (`null`: deleted, hidden or too large).
    pub after: Option<Value>,
}

/// `POST /change-sets/{id}/revert`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RevertInput {
    /// The seqs to revert (at most 500); omitted: every change not reverted yet.
    pub changes: Option<Vec<u32>>,
    /// Overwrite later edits.
    #[serde(default)]
    pub force: bool,
    /// Check and report only; nothing is written.
    #[serde(default)]
    pub dry_run: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct RevertNoteDto {
    pub seq: u32,
    pub text: String,
}

/// What a revert did (or, with `dry_run`, would do).
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct RevertReportDto {
    /// Newest first.
    pub reverted: Vec<u32>,
    pub skipped: Vec<u32>,
    pub forced: Vec<u32>,
    pub notes: Vec<RevertNoteDto>,
    pub dry_run: bool,
}

impl From<RevertReport> for RevertReportDto {
    fn from(r: RevertReport) -> Self {
        Self {
            reverted: r.reverted,
            skipped: r.skipped,
            forced: r.forced,
            notes: r
                .notes
                .into_iter()
                .map(|(seq, text)| RevertNoteDto { seq, text })
                .collect(),
            dry_run: r.dry_run,
        }
    }
}

/// One conflict of a `409 change-conflict` (in `report.conflicts`).
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct RevertConflictDto {
    pub seq: u32,
    /// `changed_since` (forceable), `later_change`, `gone` or `too_large`.
    pub reason: String,
    pub object: ChangeObjectDto,
    /// For `changed_since`: who edited it last (content only).
    pub by: Option<Uuid>,
    pub at: Option<DateTime<Utc>>,
    /// For `later_change`: the later change's seq.
    pub later: Option<u32>,
}

impl From<RevertConflict> for RevertConflictDto {
    fn from(c: RevertConflict) -> Self {
        let (by, at, later) = match &c.reason {
            ConflictReason::ChangedSince { by, at } => {
                (by.map(|u| u.as_uuid()), at.map(|t| t.as_datetime()), None)
            }
            ConflictReason::LaterChange(seq) => (None, None, Some(*seq)),
            _ => (None, None, None),
        };
        Self {
            seq: c.seq,
            reason: c.reason.code().to_owned(),
            object: ChangeObjectDto {
                kind: c.object_kind,
                id: c.object_id,
                label: c.label,
                admin_path: None,
            },
            by,
            at,
            later,
        }
    }
}

/// The change DTO of a summary; `admin_path` is filled by the caller.
pub fn change_dto(c: &ChangeSummary, admin_path: Option<String>) -> ChangeDto {
    ChangeDto {
        seq: c.seq,
        at: c.at.as_datetime(),
        op: c.op.clone(),
        object: ChangeObjectDto {
            kind: c.object_kind.clone(),
            id: c.object_id.clone(),
            label: c.label.clone(),
            admin_path,
        },
        created: c.before_state == ImageState::Absent && c.after_state != ImageState::Absent,
        deleted: c.after_state == ImageState::Absent && c.before_state != ImageState::Absent,
        revertable: c.before_state != ImageState::Skipped && c.after_state != ImageState::Skipped,
        reverted_at: c.reverted_at.map(|t| t.as_datetime()),
        reverted_by: c.reverted_by.map(|u| u.as_uuid()),
        forced: c.revert_forced,
    }
}
