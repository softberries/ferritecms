//! Comments (`comments` since `0001`, indexes from `0006_media_comments`) on SQLite.

use std::collections::BTreeMap;

use async_trait::async_trait;
use ferrite_domain::{Comment, CommentId, CommentQuery, CommentStatus, ContentId, Page, UserId};
use ferrite_store::{
    ApprovedTotals, CommentRepo, StoreError, check_comment, check_comment_page, check_limit,
    comment_counts, comment_cursor, email_key, encode_comment_cursor, validate_text,
};
use sqlx::AssertSqlSafe;

use super::SqliteRepos;
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
    if status {
        sql.push_str(" AND status = ?");
    }
    if item {
        sql.push_str(" AND item_id = ?");
    }
    if cursor {
        sql.push_str(" AND (created_at, id) < (?, ?)");
    }
    sql.push_str(" ORDER BY created_at DESC, id DESC LIMIT ?");
    sql
}

/// `approved`: the approved comments of item `?1` (status `?2`), each with `is_root`
/// (1 when its parent is null, itself, or not an approved comment of the item; P34).
macro_rules! approved_cte {
    () => {
        "approved AS ( \
           SELECT c.id, c.parent_id, c.created_at, \
             CASE WHEN c.parent_id IS NULL OR c.parent_id = c.id OR NOT EXISTS ( \
               SELECT 1 FROM comments p \
               WHERE p.id = c.parent_id AND p.item_id = ?1 AND p.status = ?2) \
             THEN 1 ELSE 0 END AS is_root \
           FROM comments c WHERE c.item_id = ?1 AND c.status = ?2)"
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
impl CommentRepo for SqliteRepos {
    async fn insert(&self, comment: &Comment) -> Result<(), StoreError> {
        check_comment(comment)?;
        run!(&self.writer, |e| sqlx::query(
            "INSERT INTO comments (id, item_id, parent_id, user_id, author_name, author_email, \
             author_url, body, status, ip_hash, created_at, by_account) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
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
        .bind(comment.user_id.is_some())
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
            q = q.bind(at).bind(id);
        }
        let q = q.bind(i64::from(limit) + 1);
        let rows = run!(&self.reader, |e| q.fetch_all(e))?;
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
            Some(item) => run!(&self.reader, |e| sqlx::query_as(
                "SELECT status, COUNT(*) FROM comments WHERE item_id = ? GROUP BY status"
            )
            .bind(item.as_uuid())
            .fetch_all(e))?,
            None => run!(&self.reader, |e| sqlx::query_as(
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
        let rows = run!(&self.reader, |e| sqlx::query(AssertSqlSafe(format!(
            "WITH RECURSIVE {THREAD_CTES}, \
             kin AS (SELECT id, parent_id, \
               ROW_NUMBER() OVER (PARTITION BY parent_id ORDER BY created_at, id) AS sib \
               FROM approved WHERE is_root = 0), \
             thread_rows(id, pos, tpath, d) AS ( \
               SELECT id, pos, '', 0 FROM roots WHERE pos >= ?3 AND pos < ?3 + ?4 \
               UNION ALL \
               SELECT k.id, t.pos, t.tpath || printf('%010d', k.sib), t.d + 1 \
               FROM kin k JOIN thread_rows t ON k.parent_id = t.id) \
             SELECT {COMMENT_COLUMNS} FROM ( \
               SELECT c.*, t.pos AS root_pos, t.tpath AS tpath, t.d AS d, \
                 ROW_NUMBER() OVER (PARTITION BY t.d = 0 ORDER BY t.pos, t.tpath) AS n \
               FROM thread_rows t JOIN comments c ON c.id = t.id) \
             WHERE d = 0 OR n <= ?5 - (SELECT COUNT(*) FROM thread_rows WHERE d = 0) \
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
        let (comments, roots): (i64, i64) = run!(&self.reader, |e| sqlx::query_as(AssertSqlSafe(
            format!("WITH {APPROVED_CTE} SELECT COUNT(*), COALESCE(SUM(is_root), 0) FROM approved")
        ))
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
        let pos: Option<i64> = run!(&self.reader, |e| sqlx::query_scalar(AssertSqlSafe(
            format!(
                "WITH RECURSIVE {THREAD_CTES}, \
                 up(id, parent_id, is_root, n) AS ( \
                   SELECT id, parent_id, is_root, 0 FROM approved WHERE id = ?3 \
                   UNION ALL \
                   SELECT a.id, a.parent_id, a.is_root, up.n + 1 \
                   FROM approved a JOIN up ON a.id = up.parent_id \
                   WHERE up.is_root = 0 AND up.n < {MAX_ANCESTORS}) \
                 SELECT roots.pos FROM up JOIN roots ON roots.id = up.id WHERE up.is_root = 1"
            )
        ))
        .bind(item.as_uuid())
        .bind(CommentStatus::Approved.as_str())
        .bind(comment.as_uuid())
        .fetch_optional(e))?;
        Ok(pos.and_then(|p| u64::try_from(p).ok()))
    }

    async fn has_approved_from(&self, email: &str, name: &str) -> Result<bool, StoreError> {
        validate_text("comment author_email", email)?;
        validate_text("comment author_name", name)?;
        let found: bool = run!(&self.reader, |e| sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM comments WHERE author_email = ? AND status = ? \
             AND author_name = ? AND user_id IS NULL AND by_account = 0)"
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

#[cfg(test)]
mod tests {
    use ferrite_store::Store;

    use super::*;
    use crate::{SqliteOptions, SqliteStore};

    /// One of the 50 seeded items (16 bytes, like a UUID).
    const ITEM: &str = "0000000000000007";

    async fn plan(store: &SqliteStore, status: bool, item: bool, cursor: bool) -> String {
        let sql = comment_list_sql(status, item, cursor);
        let mut q = sqlx::query_as::<_, (i64, i64, i64, String)>(AssertSqlSafe(format!(
            "EXPLAIN QUERY PLAN {sql}"
        )));
        if status {
            q = q.bind("pending");
        }
        if item {
            q = q.bind(ITEM.as_bytes());
        }
        if cursor {
            q = q.bind(10_000_i64).bind(uuid::Uuid::nil());
        }
        let rows = q.bind(51_i64).fetch_all(&store.reader).await.unwrap();
        rows.into_iter().map(|r| r.3).collect::<Vec<_>>().join("\n")
    }

    /// The moderation list (`CommentRepo::list`): every filter combination on its index,
    /// never a table scan, and no sort except the `id` tiebreak within one item's
    /// comments; before and after ANALYZE on 20k rows over 50 items.
    #[tokio::test]
    async fn list_uses_indexes() {
        let dir = tempfile::tempdir().unwrap();
        let store = SqliteStore::open(SqliteOptions::new(dir.path().join("ferrite.db")))
            .await
            .unwrap();
        store.migrate_up(None).await.unwrap();
        // The plan does not depend on the referenced items existing.
        sqlx::raw_sql(
            "PRAGMA foreign_keys = OFF; \
             WITH RECURSIVE g(n) AS (SELECT 1 UNION ALL SELECT n + 1 FROM g WHERE n < 20000) \
             INSERT INTO comments (id, item_id, parent_id, user_id, author_name, author_email, \
               author_url, body, status, ip_hash, created_at, by_account) \
             SELECT randomblob(16), CAST(printf('%016d', n % 50) AS BLOB), NULL, NULL, 'a', \
               'a@example.com', '', 'b', \
               CASE WHEN n % 10 = 0 THEN 'pending' WHEN n % 25 = 0 THEN 'spam' \
                 ELSE 'approved' END, \
               NULL, n, 0 \
             FROM g; \
             PRAGMA foreign_keys = ON",
        )
        .execute(&store.writer)
        .await
        .unwrap();
        for analyzed in [false, true] {
            if analyzed {
                sqlx::raw_sql("ANALYZE")
                    .execute(&store.writer)
                    .await
                    .unwrap();
            }
            for (status, item, index) in [
                (false, false, "comments_created_idx"),
                (true, false, "comments_status_created_idx"),
                (true, true, "comments_item_status_idx"),
                (false, true, "comments_item_created_idx"),
            ] {
                for cursor in [false, true] {
                    let p = plan(&store, status, item, cursor).await;
                    eprintln!(
                        "-- analyzed={analyzed} status={status} item={item} cursor={cursor}\n{p}"
                    );
                    assert!(p.contains(index), "{status} {item} {cursor}: {p}");
                    assert!(
                        !p.contains("SCAN comments\n") && !p.ends_with("SCAN comments"),
                        "{p}"
                    );
                    if item && !status {
                        // Bounded by one item's comments.
                        assert!(!p.contains("TEMP B-TREE FOR ORDER BY"), "{p}");
                    } else {
                        assert!(!p.contains("TEMP B-TREE"), "list sorts: {p}");
                    }
                }
            }
        }
    }
}
