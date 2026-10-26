//! Bodies of the comment endpoints (M2a-2b P11–P13).
//!
//! [`CommentDto`] is the moderation view: every column, the e-mail and the keyed
//! client-address hash included. [`CommentPublicDto`] is what a commenter gets back and
//! never carries either. Ids are UUID strings and times RFC 3339 UTC (`…Z`).

use chrono::{DateTime, Utc};
use ferrite_core::comments::{CommentCounts, CommentItem, QueuedComment};
use ferrite_domain::Comment;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

/// A comment as moderators see it (`GET /comments`, `PATCH /comments/{id}`).
#[derive(Debug, Serialize, ToSchema)]
pub struct CommentDto {
    pub id: Uuid,
    /// The content item commented on.
    pub item_id: Uuid,
    /// That item as read with the comment.
    pub item: CommentItemDto,
    /// The comment replied to.
    pub parent_id: Option<Uuid>,
    /// The signed-in commenter; `null` for a guest (and after the account is deleted).
    pub user_id: Option<Uuid>,
    pub author_name: String,
    /// ASCII-lowercased.
    pub author_email: String,
    /// `""`: none.
    pub author_url: String,
    /// Plain text as submitted (trimmed; escape it when showing it).
    pub body: String,
    /// `pending`, `approved`, `spam` or `trash`.
    pub status: String,
    /// Keyed hash of the client address (HMAC under the site key): tells commenters
    /// apart, cannot be reversed without the key.
    pub ip_hash: Option<String>,
    pub created_at: DateTime<Utc>,
}

impl From<QueuedComment> for CommentDto {
    fn from(q: QueuedComment) -> Self {
        let c = q.comment;
        Self {
            id: c.id.as_uuid(),
            item_id: c.item_id.as_uuid(),
            item: q.item.into(),
            parent_id: c.parent_id.map(|p| p.as_uuid()),
            user_id: c.user_id.map(|u| u.as_uuid()),
            author_name: c.author_name,
            author_email: c.author_email,
            author_url: c.author_url,
            body: c.body,
            status: c.status.as_str().to_owned(),
            ip_hash: c.ip_hash,
            created_at: c.created_at.as_datetime(),
        }
    }
}

/// The item a comment is on (`CommentDto.item`).
#[derive(Debug, Serialize, ToSchema)]
pub struct CommentItemDto {
    pub id: Uuid,
    /// The content type (`post`, `page`, …).
    #[serde(rename = "type")]
    pub kind: String,
    /// `""` for an item without a public page when the caller may not read content
    /// (a token without `content:read`).
    pub title: String,
    /// The public path, only while the item has a public page now (published, its
    /// publish time reached); `null` otherwise.
    #[schema(required = true)]
    pub permalink: Option<String>,
    /// `draft`, `pending`, `scheduled`, `published`, `private` or `trash`.
    pub status: String,
}

impl From<CommentItem> for CommentItemDto {
    fn from(i: CommentItem) -> Self {
        Self {
            id: i.id.as_uuid(),
            kind: i.kind,
            title: i.title,
            permalink: i.permalink,
            status: i.status.as_str().to_owned(),
        }
    }
}

/// `GET /comments/counts`: comments per status.
#[derive(Debug, Serialize, ToSchema)]
pub struct CommentCountsDto {
    pub pending: u64,
    pub approved: u64,
    pub spam: u64,
    pub trash: u64,
}

impl From<CommentCounts> for CommentCountsDto {
    fn from(c: CommentCounts) -> Self {
        Self {
            pending: c.pending,
            approved: c.approved,
            spam: c.spam,
            trash: c.trash,
        }
    }
}

/// `GET /comments`.
#[derive(Debug, Serialize, ToSchema)]
pub struct CommentList {
    pub items: Vec<CommentDto>,
    /// Pass as `cursor` for the next page; `null` on the last page.
    pub next_cursor: Option<String>,
}

/// `PATCH /comments/{id}`.
#[derive(Debug, Deserialize, ToSchema)]
pub struct CommentModeration {
    /// `pending`, `approved`, `spam` or `trash`.
    pub status: String,
}

/// `POST /content/{type}/{id}/comments`. With a session or token the name and e-mail
/// come from the account and these members are ignored. Unknown members are ignored.
#[derive(Debug, Deserialize, ToSchema)]
pub struct CommentSubmit {
    /// Plain text, 1 to 10 000 characters (no HTML; line breaks kept).
    pub body: String,
    /// 1 to 100 characters; required without an account.
    #[serde(default)]
    pub author_name: Option<String>,
    /// At most 254 bytes; required without an account.
    #[serde(default)]
    pub author_email: Option<String>,
    /// An `http` or `https` address of at most 200 characters.
    #[serde(default)]
    pub author_url: Option<String>,
    /// An approved comment of the same post to reply to (at most 5 deep).
    #[serde(default)]
    pub parent_id: Option<Uuid>,
    /// Leave empty (a field people never see; anything in it drops the comment).
    #[serde(default)]
    pub website: Option<String>,
}

/// A submitted comment as its author sees it: no e-mail, no address hash.
#[derive(Debug, Serialize, ToSchema)]
pub struct CommentPublicDto {
    pub id: Uuid,
    pub parent_id: Option<Uuid>,
    /// Plain text (escape it).
    pub author_name: String,
    pub author_url: Option<String>,
    /// The body as the site shows it: escaped, `<p>` paragraphs, `<br>` line breaks.
    pub html: String,
    pub date: DateTime<Utc>,
    /// `approved` (`201`) or `pending` (`202`).
    pub status: String,
}

impl CommentPublicDto {
    /// A stored comment's public view.
    pub fn of(c: &Comment) -> Self {
        Self {
            id: c.id.as_uuid(),
            parent_id: c.parent_id.map(|p| p.as_uuid()),
            author_name: c.author_name.clone(),
            author_url: (!c.author_url.is_empty()).then(|| c.author_url.clone()),
            html: ferrite_core::comments::render_body(&c.body),
            date: c.created_at.as_datetime(),
            status: c.status.as_str().to_owned(),
        }
    }
}
