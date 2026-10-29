//! WordPress import bookkeeping (`0009_import`, M3-1 P3, P5): the run row that fences a
//! running import, and the map from source ids to FerriteCMS ids that makes a re-run
//! idempotent.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::{ImportRunId, JobId, Timestamp, UserId, string_enum};

string_enum!(
    /// Where an import run stands. At most one run is `running` at a time.
    ImportRunState {
        Running => "running",
        Done => "done",
        Failed => "failed",
        Abandoned => "abandoned",
    }
);

string_enum!(
    /// What an [`ImportMapRow`] maps.
    ImportMapKind {
        User => "user",
        Term => "term",
        Media => "media",
        Content => "content",
        Comment => "comment",
        Menu => "menu",
        Option => "option",
        Redirect => "redirect",
    }
);

/// One import run. `token` is the 128-bit secret of the process that owns the run
/// (stored as lowercase hex); every batch proves it still holds it (`ImportRepo::fence`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImportRun {
    pub id: ImportRunId,
    /// The normalised WXR `<wp:base_blog_url>`.
    pub source: String,
    pub token: [u8; 16],
    pub state: ImportRunState,
    pub started_by: UserId,
    pub started_at: Timestamp,
    pub heartbeat_at: Timestamp,
    /// `None` while `running`.
    pub finished_at: Option<Timestamp>,
    /// The report counters, a JSON object.
    pub stats: Value,
    pub dry_run: bool,
    /// The `import.wordpress` job that started the run (`0010`, M3-2 P27); `None` for a
    /// CLI run. No foreign key: finished jobs are pruned, their runs stay.
    pub job_id: Option<JobId>,
    /// The step the run is in (M3-2 P28): `parse`, `pass1`…`pass4`, `fixup` or `report`;
    /// `None` until the first one begins.
    pub step: Option<String>,
    /// The SHA-256 of the run's report artifact (M3-2 P29), once one was stored.
    pub report_hash: Option<String>,
}

/// What the import wrote for one source row: unique by (`source`, `kind`, `wp_id`).
/// `hash` is the SHA-256 of the item's IR record; `version` is the item's version right
/// after the import wrote it (P4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportMapRow {
    pub source: String,
    pub kind: ImportMapKind,
    pub wp_id: i64,
    pub ferrite_id: Uuid,
    pub hash: String,
    pub version: i64,
}
