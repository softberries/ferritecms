//! Imported comments (P18): any status, the WP date, threading and account.

use ferrite_domain::{
    Comment, CommentId, CommentStatus, ContentId, ImportMapKind, Timestamp, UserId,
};

use super::ImportBatch;
use crate::{CoreError, events};

/// One WXR comment, its item, parent and user already mapped; the body is plain text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportComment {
    pub wp_id: i64,
    pub item: ContentId,
    /// A comment of the same item, already imported (in this or an earlier batch).
    pub parent: Option<CommentId>,
    /// The commenter's account; also marks the comment `by_account` (02 §comments).
    pub user: Option<UserId>,
    pub author_name: String,
    /// ASCII-lowercased by the store.
    pub author_email: String,
    /// `""` means none.
    pub author_url: String,
    pub body: String,
    pub status: CommentStatus,
    pub created_at: Timestamp,
}

impl ImportBatch<'_> {
    /// Writes one imported comment and returns its id (P18). A comment whose `wp_id` is
    /// mapped to a comment that still exists is returned as it is, without a write (a
    /// re-run never duplicates; edits made in WordPress are not applied).
    ///
    /// Unlike [`CommentService::submit`](crate::CommentService::submit) there is no rate
    /// limit, no moderation, no honeypot, no depth limit and no size limit (the
    /// converter decides what to cut), and the item may be in any status. Checked: the
    /// item exists (`InvalidField` `item`), the parent exists and belongs to the same
    /// item (`parent`), the user exists (`user`). `ip_hash` is `None`; the store sets
    /// `by_account` when `user` is given. An approved comment adds its item's key to the
    /// batch's event.
    ///
    /// **Errors.** As for [`content`](Self::content): any `Err` leaves the batch
    /// unusable, and the caller must [`rollback`](Self::rollback) it.
    pub async fn comment(&mut self, c: ImportComment) -> Result<CommentId, CoreError> {
        if let Some(mapped) = self.lookup(ImportMapKind::Comment, c.wp_id).await? {
            return Ok(CommentId::from_uuid(mapped.id));
        }
        let repos = self.tx.as_ref();
        if repos.content().get(c.item).await?.is_none() {
            return Err(CoreError::invalid_field("item", "unknown item"));
        }
        if let Some(parent) = c.parent
            && !repos
                .comments()
                .get(parent)
                .await?
                .is_some_and(|p| p.item_id == c.item)
        {
            return Err(CoreError::invalid_field(
                "parent",
                "the parent must be a comment of the same item",
            ));
        }
        if let Some(user) = c.user
            && repos.users().get(user).await?.is_none()
        {
            return Err(CoreError::invalid_field("user", "unknown user"));
        }
        let id = CommentId::new();
        repos
            .comments()
            .insert(&Comment {
                id,
                item_id: c.item,
                parent_id: c.parent,
                user_id: c.user,
                author_name: c.author_name,
                author_email: c.author_email,
                author_url: c.author_url,
                body: c.body,
                status: c.status,
                ip_hash: None,
                created_at: c.created_at,
            })
            .await?;
        if c.status == CommentStatus::Approved {
            self.keys.insert(events::post_key(c.item));
        }
        self.remember(ImportMapKind::Comment, c.wp_id, id.as_uuid(), "", 0)
            .await?;
        self.items += 1;
        Ok(id)
    }
}
