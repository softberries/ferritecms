//! Comments (`comments` since `0001`, indexes in `0006_media_comments`; M2a-2b P11).

use serde::{Deserialize, Serialize};

use crate::{CommentId, ContentId, Timestamp, UserId, string_enum};

string_enum!(
    /// Moderation state of a comment. Only `approved` comments are shown on the site.
    CommentStatus {
        Pending => "pending",
        Approved => "approved",
        Spam => "spam",
        Trash => "trash",
    }
);

/// A stored comment. Core validates the text (lengths, URL scheme, depth); the store
/// only refuses NUL.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Comment {
    pub id: CommentId,
    pub item_id: ContentId,
    pub parent_id: Option<CommentId>,
    /// The signed-in commenter; `None` for anonymous comments (and after the user is
    /// deleted).
    pub user_id: Option<UserId>,
    pub author_name: String,
    /// ASCII-lowercased (the store folds it on insert, as `email_key`).
    pub author_email: String,
    /// `""` means none.
    pub author_url: String,
    /// Plain text; rendered (escaped) by core.
    pub body: String,
    pub status: CommentStatus,
    /// Keyed hash of the client address (never the address).
    pub ip_hash: Option<String>,
    pub created_at: Timestamp,
}

/// A comment listing (moderation queue): newest first, optional filters.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommentQuery {
    pub status: Option<CommentStatus>,
    pub item: Option<ContentId>,
    /// 1..=[`MAX_PAGE`](crate::MAX_PAGE).
    pub limit: u32,
    pub cursor: Option<String>,
}
