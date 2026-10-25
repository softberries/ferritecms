//! Comments (`comments` since `0001`, indexes from `0006_media_comments`) on Postgres.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use ferrite_domain::{
    Comment, CommentId, CommentQuery, CommentStatus, ContentId, Page, Timestamp, UserId,
};
use ferrite_store::{
    CommentRepo, StoreError, check_comment, check_item_comments_limit, check_limit, comment_cursor,
    email_key, encode_comment_cursor, validate_text,
};
use sqlx::AssertSqlSafe;

use super::PgRepos;
use crate::{
    codec::{self, COMMENT_COLUMNS},
    exec::run,
};

/// The moderation list (`CommentRepo::list`). Only the filters given are in the SQL, so
/// the planner picks the matching index (`comments_status_created_idx`,
/// `comments_item_status_idx`, `comments_item_created_idx` or `comments_created_idx`).
/// Binds, in order: status, item, cursor (created_at, id), limit. Public so the plan
/// tests can `EXPLAIN` it.
pub fn comment_list_sql(status: bool, item: bool, cursor: bool) -> String {
    let mut sql = format!("SELECT {COMMENT_COLUMNS} FROM comments WHERE 1 = 1");
    let mut n = 0;
    let mut next = || {
        n += 1;
        format!("${n}")
    };
    if status {
        sql.push_str(&format!(" AND status = {}", next()));
    }
    if item {
        sql.push_str(&format!(" AND item_id = {}", next()));
    }
    if cursor {
        let (a, b) = (next(), next());
        sql.push_str(&format!(" AND (created_at, id) < ({a}, {b})"));
    }
    sql.push_str(&format!(
        " ORDER BY created_at DESC, id DESC LIMIT {}",
        next()
    ));
    sql
}

#[async_trait]
impl CommentRepo for PgRepos {
    async fn insert(&self, comment: &Comment) -> Result<(), StoreError> {
        check_comment(comment)?;
        run!(&self.exec, |e| sqlx::query(
            "INSERT INTO comments (id, item_id, parent_id, user_id, author_name, author_email, \
             author_url, body, status, ip_hash, created_at, by_account) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)"
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
        .bind(DateTime::<Utc>::from(comment.created_at))
        .bind(comment.user_id.is_some())
        .execute(e))?;
        Ok(())
    }

    async fn get(&self, id: CommentId) -> Result<Option<Comment>, StoreError> {
        let row = run!(&self.exec, |e| sqlx::query(AssertSqlSafe(format!(
            "SELECT {COMMENT_COLUMNS} FROM comments WHERE id = $1"
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
        let row = run!(&self.exec, |e| sqlx::query(AssertSqlSafe(format!(
            "UPDATE comments SET status = $1 WHERE id = $2 RETURNING {COMMENT_COLUMNS}"
        )))
        .bind(status.as_str())
        .bind(id.as_uuid())
        .fetch_optional(e))?;
        row.as_ref().map(codec::comment).transpose()
    }

    async fn delete(&self, id: CommentId) -> Result<bool, StoreError> {
        let done = run!(&self.exec, |e| sqlx::query(
            "DELETE FROM comments WHERE id = $1"
        )
        .bind(id.as_uuid())
        .execute(e))?;
        Ok(done.rows_affected() > 0)
    }

    async fn list(&self, query: &CommentQuery) -> Result<Page<Comment>, StoreError> {
        let limit = check_limit(query.limit)?;
        let after = query.cursor.as_deref().map(comment_cursor).transpose()?;
        let sql = comment_list_sql(
            query.status.is_some(),
            query.item.is_some(),
            after.is_some(),
        );
        let mut q = sqlx::query(AssertSqlSafe(sql));
        if let Some(status) = query.status {
            q = q.bind(status.as_str());
        }
        if let Some(item) = query.item {
            q = q.bind(item.as_uuid());
        }
        if let Some((at, id)) = after {
            q = q
                .bind(DateTime::<Utc>::from(Timestamp::from_micros(at)?))
                .bind(id);
        }
        let q = q.bind(i64::from(limit) + 1);
        let rows = run!(&self.exec, |e| q.fetch_all(e))?;
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
        let rows = run!(&self.exec, |e| sqlx::query(AssertSqlSafe(format!(
            "SELECT {COMMENT_COLUMNS} FROM comments WHERE item_id = $1 AND status = $2 \
             ORDER BY created_at, id LIMIT $3"
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
        let found: bool = run!(&self.exec, |e| sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM comments WHERE author_email = $1 AND status = $2 \
             AND author_name = $3 AND user_id IS NULL AND NOT by_account)"
        )
        .bind(email_key(email))
        .bind(CommentStatus::Approved.as_str())
        .bind(name)
        .fetch_one(e))?;
        Ok(found)
    }

    async fn has_approved_from_user(&self, user: UserId) -> Result<bool, StoreError> {
        let found: bool = run!(&self.exec, |e| sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM comments WHERE user_id = $1 AND status = $2)"
        )
        .bind(user.as_uuid())
        .bind(CommentStatus::Approved.as_str())
        .fetch_one(e))?;
        Ok(found)
    }
}
