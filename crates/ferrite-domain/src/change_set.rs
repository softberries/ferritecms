//! AI change sets (design 17 §5): the writes an assistant made over MCP, grouped per
//! connection and time window, with before- and after-images of each touched object.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{ChangeSetId, Timestamp, UserId, string_enum};

/// The site option holding how many days change sets are kept (1..=365, default 90).
pub const CHANGE_SETS_DAYS_OPTION: &str = "mcp.change_sets_days";

/// Cursor tag of the change-set list (`last_at` descending, then id).
pub const CHANGE_SET_CURSOR_TAG: &str = "cs:last";

string_enum!(
    /// The kind of connection that wrote: an OAuth grant or a personal API token.
    AgentKind {
        OAuth => "oauth",
        Token => "token",
    }
);

string_enum!(
    /// Whether an image was kept: absent (no object), present (bytes kept) or skipped
    /// (too large to keep: hash only).
    ImageState {
        Absent => "absent",
        Present => "present",
        Skipped => "skipped",
    }
);

/// One side of a change: the object before or after the write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Image {
    /// The object did not exist (before a create) or no longer exists (after a delete).
    Absent,
    /// The canonical JSON snapshot, zstd-compressed, and the SHA-256 hex of the
    /// uncompressed canonical bytes.
    Present { hash: String, bytes: Vec<u8> },
    /// Too large to keep: only the hash; the change cannot be reverted.
    Skipped { hash: String },
}

impl Image {
    /// The hash of the snapshot, `None` when absent.
    pub fn hash(&self) -> Option<&str> {
        match self {
            Image::Absent => None,
            Image::Present { hash, .. } | Image::Skipped { hash } => Some(hash),
        }
    }

    pub fn state(&self) -> ImageState {
        match self {
            Image::Absent => ImageState::Absent,
            Image::Present { .. } => ImageState::Present,
            Image::Skipped { .. } => ImageState::Skipped,
        }
    }

    /// Builds an image from its stored columns: `hash` NULL is absent, a hash without
    /// bytes is skipped.
    pub fn from_columns(hash: Option<String>, bytes: Option<Vec<u8>>) -> Self {
        match (hash, bytes) {
            (None, _) => Image::Absent,
            (Some(hash), Some(bytes)) => Image::Present { hash, bytes },
            (Some(hash), None) => Image::Skipped { hash },
        }
    }

    /// The stored columns (`hash`, `bytes`).
    pub fn columns(&self) -> (Option<&str>, Option<&[u8]>) {
        match self {
            Image::Absent => (None, None),
            Image::Present { hash, bytes } => (Some(hash), Some(bytes)),
            Image::Skipped { hash } => (Some(hash), None),
        }
    }
}

/// One change set (a row of `change_sets` plus its count of reverted changes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeSet {
    pub id: ChangeSetId,
    /// The user the agent acted as; `None` once that user is deleted.
    pub user_id: Option<UserId>,
    pub agent_kind: AgentKind,
    /// The OAuth grant or API token id (no foreign key: the history outlives them).
    pub agent_id: Uuid,
    /// The client's name at the time the set opened.
    pub client: String,
    /// Whether the set still holds its `open_key` (it may have gone idle since; core
    /// checks the time window).
    pub open: bool,
    pub started_at: Timestamp,
    pub last_at: Timestamp,
    pub changes: u32,
    pub reverted_changes: u32,
    /// Set when every change of the set is reverted.
    pub reverted_at: Option<Timestamp>,
    pub reverted_by: Option<UserId>,
}

/// A new set, opened by the first change of an agent's working session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewChangeSet {
    pub id: ChangeSetId,
    pub user_id: Option<UserId>,
    pub agent_kind: AgentKind,
    pub agent_id: Uuid,
    pub client: String,
    /// `oauth:<grant>` or `token:<token id>`.
    pub open_key: String,
    pub now: Timestamp,
    /// A set holding `open_key` whose `last_at` is before this is stale and closed.
    pub idle_since: Timestamp,
    /// A set holding `open_key` with this many changes or more is full and closed.
    pub max_changes: u32,
}

/// A change to insert.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewChange {
    pub set_id: ChangeSetId,
    /// 1-based, consecutive within the set.
    pub seq: u32,
    pub at: Timestamp,
    pub op: String,
    pub object_kind: String,
    pub object_id: String,
    pub label: String,
    pub before: Image,
    pub after: Image,
}

/// A stored change with its images.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub set_id: ChangeSetId,
    pub seq: u32,
    pub at: Timestamp,
    pub op: String,
    pub object_kind: String,
    pub object_id: String,
    pub label: String,
    pub before: Image,
    pub after: Image,
    pub reverted_at: Option<Timestamp>,
    pub reverted_by: Option<UserId>,
    pub revert_forced: bool,
}

/// A stored change without the image bytes (the set's list of changes).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeSummary {
    pub set_id: ChangeSetId,
    pub seq: u32,
    pub at: Timestamp,
    pub op: String,
    pub object_kind: String,
    pub object_id: String,
    pub label: String,
    pub before_state: ImageState,
    pub before_hash: Option<String>,
    pub after_state: ImageState,
    pub after_hash: Option<String>,
    pub reverted_at: Option<Timestamp>,
    pub reverted_by: Option<UserId>,
    pub revert_forced: bool,
}

/// A page of change sets, newest `last_at` first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeSetQuery {
    /// Only this user's sets; `None` for every user's (core decides who may).
    pub user: Option<UserId>,
    /// Only this agent's (grant or token id).
    pub agent_id: Option<Uuid>,
    /// `Some(true)`: sets holding their `open_key` with `last_at >= idle_since`;
    /// `Some(false)`: the others.
    pub open: Option<bool>,
    /// The start of the idle window the `open` filter uses.
    pub idle_since: Timestamp,
    /// 1..=[`MAX_PAGE`](crate::MAX_PAGE).
    pub limit: u32,
    pub cursor: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_columns_roundtrip() {
        for img in [
            Image::Absent,
            Image::Present {
                hash: "h".into(),
                bytes: vec![1, 2],
            },
            Image::Skipped { hash: "h".into() },
        ] {
            let (h, b) = img.columns();
            let back = Image::from_columns(h.map(str::to_owned), b.map(<[u8]>::to_vec));
            assert_eq!(back, img);
        }
        assert_eq!(Image::Absent.hash(), None);
        assert_eq!(
            Image::Skipped { hash: "x".into() }.state(),
            ImageState::Skipped
        );
    }

    #[test]
    fn kinds_roundtrip() {
        for k in AgentKind::ALL {
            assert_eq!(k.as_str().parse::<AgentKind>().unwrap(), *k);
        }
    }
}
