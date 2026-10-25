//! Comments (`comments` since `0001`, indexes from `0006_media_comments`) on SQLite.

use async_trait::async_trait;
use ferrite_domain::{Comment, CommentId, CommentQuery, CommentStatus, ContentId, Page, UserId};
use ferrite_store::{
    CommentRepo, StoreError, check_comment, check_item_comments_limit, check_limit, comment_cursor,
    email_key, encode_comment_cursor, validate_text,
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
