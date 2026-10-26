//! Comments (`comments` since `0001`, indexes from `0006_media_comments`) on Postgres.

use std::collections::BTreeMap;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use ferrite_domain::{
    Comment, CommentId, CommentQuery, CommentStatus, ContentId, Page, Timestamp, UserId,
};
use ferrite_store::{
    ApprovedTotals, CommentRepo, StoreError, check_comment, check_comment_page, check_limit,
    comment_counts, comment_cursor, email_key, encode_comment_cursor, validate_text,
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

/// `approved`: the approved comments of item `$1` (status `$2`), each with `is_root`
/// (1 when its parent is null, itself, or not an approved comment of the item; P34).
macro_rules! approved_cte {
    () => {
        "approved AS ( \
           SELECT c.id, c.parent_id, c.created_at, \
             CASE WHEN c.parent_id IS NULL OR c.parent_id = c.id OR NOT EXISTS ( \
               SELECT 1 FROM comments p \
               WHERE p.id = c.parent_id AND p.item_id = $1 AND p.status = $2) \
             THEN 1 ELSE 0 END AS is_root \
           FROM comments c WHERE c.item_id = $1 AND c.status = $2)"
    };
}

/// See [`approved_cte`].
const APPROVED_CTE: &str = approved_cte!();

/// [`APPROVED_CTE`] plus `roots`: each root with its 0-based position `pos` in
/// `(created_at, id)` order.
const THREAD_CTES: &str = concat!(
    approved_cte!(),
    ", roots AS ( \
       SELECT id, ROW_NUMBER() OVER (ORDER BY created_at, id) - 1 AS pos \
       FROM approved WHERE is_root = 1)"
);

/// Most ancestors `root_position` walks (a cycle of approved comments ends there).
const MAX_ANCESTORS: u32 = 1000;

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

    async fn counts(
        &self,
        item: Option<ContentId>,
    ) -> Result<BTreeMap<CommentStatus, u64>, StoreError> {
        let rows: Vec<(String, i64)> = match item {
            Some(item) => run!(&self.exec, |e| sqlx::query_as(
                "SELECT status, COUNT(*) FROM comments WHERE item_id = $1 GROUP BY status"
            )
            .bind(item.as_uuid())
            .fetch_all(e))?,
            None => run!(&self.exec, |e| sqlx::query_as(
                "SELECT status, COUNT(*) FROM comments GROUP BY status"
            )
            .fetch_all(e))?,
        };
        comment_counts(rows)
    }

    async fn approved_page(
        &self,
        item: ContentId,
        offset: u32,
        roots: u32,
        max_rows: u32,
    ) -> Result<Vec<Comment>, StoreError> {
        let (roots, max_rows) = check_comment_page(roots, max_rows)?;
        // Every root of the page (`d = 0`), then replies numbered in the page's thread
        // order while they fit the budget left after the roots.
        let rows = run!(&self.exec, |e| sqlx::query(AssertSqlSafe(format!(
            "WITH RECURSIVE {THREAD_CTES}, \
             kin AS (SELECT id, parent_id, \
               ROW_NUMBER() OVER (PARTITION BY parent_id ORDER BY created_at, id) AS sib \
               FROM approved WHERE is_root = 0), \
             thread_rows(id, pos, tpath, d) AS ( \
               SELECT id, pos, ARRAY[]::bigint[], 0 FROM roots WHERE pos >= $3 AND pos < $3 + $4 \
               UNION ALL \
               SELECT k.id, t.pos, t.tpath || k.sib, t.d + 1 \
               FROM kin k JOIN thread_rows t ON k.parent_id = t.id) \
             SELECT {COMMENT_COLUMNS} FROM ( \
               SELECT c.*, t.pos AS root_pos, t.tpath AS tpath, t.d AS d, \
                 ROW_NUMBER() OVER (PARTITION BY t.d = 0 ORDER BY t.pos, t.tpath) AS n \
               FROM thread_rows t JOIN comments c ON c.id = t.id) AS page \
             WHERE d = 0 OR n <= $5 - (SELECT COUNT(*) FROM thread_rows WHERE d = 0) \
             ORDER BY root_pos, tpath"
        )))
        .bind(item.as_uuid())
        .bind(CommentStatus::Approved.as_str())
        .bind(i64::from(offset))
        .bind(roots)
        .bind(max_rows)
        .fetch_all(e))?;
        rows.iter().map(codec::comment).collect()
    }

    async fn approved_totals(&self, item: ContentId) -> Result<ApprovedTotals, StoreError> {
        let (comments, roots): (i64, i64) =
            run!(&self.exec, |e| sqlx::query_as(AssertSqlSafe(format!(
                "WITH {APPROVED_CTE} \
                 SELECT COUNT(*), COALESCE(SUM(is_root), 0)::bigint FROM approved"
            )))
            .bind(item.as_uuid())
            .bind(CommentStatus::Approved.as_str())
            .fetch_one(e))?;
        Ok(ApprovedTotals {
            comments: u64::try_from(comments).unwrap_or(0),
            roots: u64::try_from(roots).unwrap_or(0),
        })
    }

    async fn root_position(
        &self,
        item: ContentId,
        comment: CommentId,
    ) -> Result<Option<u64>, StoreError> {
        let pos: Option<i64> = run!(&self.exec, |e| sqlx::query_scalar(AssertSqlSafe(format!(
            "WITH RECURSIVE {THREAD_CTES}, \
             up(id, parent_id, is_root, n) AS ( \
               SELECT id, parent_id, is_root, 0 FROM approved WHERE id = $3 \
               UNION ALL \
               SELECT a.id, a.parent_id, a.is_root, up.n + 1 \
               FROM approved a JOIN up ON a.id = up.parent_id \
               WHERE up.is_root = 0 AND up.n < {MAX_ANCESTORS}) \
             SELECT roots.pos FROM up JOIN roots ON roots.id = up.id WHERE up.is_root = 1"
        )))
        .bind(item.as_uuid())
        .bind(CommentStatus::Approved.as_str())
        .bind(comment.as_uuid())
        .fetch_optional(e))?;
        Ok(pos.and_then(|p| u64::try_from(p).ok()))
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
