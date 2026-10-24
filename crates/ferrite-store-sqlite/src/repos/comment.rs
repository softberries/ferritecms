//! Comments (`comments` since `0001`, indexes from `0006_media_comments`) on SQLite.

use async_trait::async_trait;
use ferrite_domain::{Comment, CommentId, CommentQuery, CommentStatus, ContentId, Page, UserId};
use ferrite_store::{
    CommentRepo, StoreError, check_comment, check_item_comments_limit, check_limit, comment_cursor,
    email_key, encode_comment_cursor, validate_text,
};
use sqlx::{AssertSqlSafe, QueryBuilder, Sqlite};

use super::SqliteRepos;
use crate::{
    codec::{self, COMMENT_COLUMNS},
    exec::run,
};

#[async_trait]
impl CommentRepo for SqliteRepos {
    async fn insert(&self, comment: &Comment) -> Result<(), StoreError> {
        check_comment(comment)?;
        run!(&self.writer, |e| sqlx::query(
            "INSERT INTO comments (id, item_id, parent_id, user_id, author_name, author_email, \
             author_url, body, status, ip_hash, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
        )
        .bind(comment.id.as_uuid())
        .bind(comment.item_id.as_uuid())
        .bind(comment.parent_id.map(|p| p.as_uuid()))
        .bind(comment.user_id.map(|u| u.as_uuid()))
        .bind(&comment.author_name)
        .bind(email_key(&comment.author_email))
        .bind(&comment.author_url)
        .bind(&comment.body)
        .bind(comment.status.as_str())
        .bind(comment.ip_hash.as_deref())
        .bind(comment.created_at.as_micros())
        .execute(e))?;
        Ok(())
    }

    async fn get(&self, id: CommentId) -> Result<Option<Comment>, StoreError> {
        let row = run!(&self.reader, |e| sqlx::query(AssertSqlSafe(format!(
            "SELECT {COMMENT_COLUMNS} FROM comments WHERE id = ?"
        )))
        .bind(id.as_uuid())
        .fetch_optional(e))?;
        row.as_ref().map(codec::comment).transpose()
    }

    async fn set_status(
        &self,
        id: CommentId,
        status: CommentStatus,
    ) -> Result<Option<Comment>, StoreError> {
        let row = run!(&self.writer, |e| sqlx::query(AssertSqlSafe(format!(
            "UPDATE comments SET status = ? WHERE id = ? RETURNING {COMMENT_COLUMNS}"
        )))
        .bind(status.as_str())
        .bind(id.as_uuid())
        .fetch_optional(e))?;
        row.as_ref().map(codec::comment).transpose()
    }

    async fn delete(&self, id: CommentId) -> Result<bool, StoreError> {
        let done = run!(&self.writer, |e| sqlx::query(
            "DELETE FROM comments WHERE id = ?"
        )
        .bind(id.as_uuid())
        .execute(e))?;
        Ok(done.rows_affected() > 0)
    }

    async fn list(&self, query: &CommentQuery) -> Result<Page<Comment>, StoreError> {
        let limit = check_limit(query.limit)?;
        let after = query.cursor.as_deref().map(comment_cursor).transpose()?;
        // Only the filters given are in the SQL, so the planner picks the matching index
        // (`comments_status_created_idx`, `comments_item_created_idx` or
        // `comments_created_idx`).
        let mut qb = QueryBuilder::<Sqlite>::new(format!(
            "SELECT {COMMENT_COLUMNS} FROM comments WHERE 1 = 1"
        ));
        if let Some(status) = query.status {
            qb.push(" AND status = ").push_bind(status.as_str());
        }
        if let Some(item) = query.item {
            qb.push(" AND item_id = ").push_bind(item.as_uuid());
        }
        if let Some((at, id)) = after {
            qb.push(" AND (created_at, id) < (")
                .push_bind(at)
                .push(", ")
                .push_bind(id)
                .push(")");
        }
        qb.push(" ORDER BY created_at DESC, id DESC LIMIT ")
            .push_bind(i64::from(limit) + 1);
        let rows = run!(&self.reader, |e| qb.build().fetch_all(e))?;
        let mut items = rows
            .iter()
            .map(codec::comment)
            .collect::<Result<Vec<_>, _>>()?;
        let next_cursor = if items.len() > limit as usize {
            items.truncate(limit as usize);
            items
                .last()
                .map(|c| encode_comment_cursor(c.created_at, c.id.as_uuid()))
        } else {
            None
        };
        Ok(Page { items, next_cursor })
    }

    async fn approved_for_item(
        &self,
        item: ContentId,
        limit: u32,
    ) -> Result<Vec<Comment>, StoreError> {
        let limit = check_item_comments_limit(limit)?;
        let rows = run!(&self.reader, |e| sqlx::query(AssertSqlSafe(format!(
            "SELECT {COMMENT_COLUMNS} FROM comments WHERE item_id = ? AND status = ? \
             ORDER BY created_at, id LIMIT ?"
        )))
        .bind(item.as_uuid())
        .bind(CommentStatus::Approved.as_str())
        .bind(limit)
        .fetch_all(e))?;
        rows.iter().map(codec::comment).collect()
    }

    async fn has_approved_from(&self, email: &str, name: &str) -> Result<bool, StoreError> {
        validate_text("comment author_email", email)?;
        validate_text("comment author_name", name)?;
        let found: bool = run!(&self.reader, |e| sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM comments WHERE author_email = ? AND status = ? \
             AND author_name = ? AND user_id IS NULL)"
        )
        .bind(email_key(email))
        .bind(CommentStatus::Approved.as_str())
        .bind(name)
        .fetch_one(e))?;
        Ok(found)
    }

    async fn has_approved_from_user(&self, user: UserId) -> Result<bool, StoreError> {
        let found: bool = run!(&self.reader, |e| sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM comments WHERE user_id = ? AND status = ?)"
        )
        .bind(user.as_uuid())
        .bind(CommentStatus::Approved.as_str())
        .fetch_one(e))?;
        Ok(found)
    }
}
