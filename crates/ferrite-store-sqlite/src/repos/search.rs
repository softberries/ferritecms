//! Full-text search on the FTS5 table `search_fts(item_id UNINDEXED, title, body_text)`.
//!
//! The user query is never FTS5 syntax: it is split by `ferrite_store::search_terms` (the
//! same terms Postgres sees), each term becomes a quoted FTS5 string with a prefix `*`
//! (see [`match_expression`]), and the expression is bound as a parameter. FTS5 has no foreign keys, so `index` checks the item exists (matching
//! Postgres) and `ContentRepo::delete` removes the item's row.

use async_trait::async_trait;
use ferrite_domain::ContentId;
use ferrite_store::{SearchRepo, StoreError, check_limit, search_terms, validate_text};
use uuid::Uuid;

use super::SqliteRepos;
use crate::exec::{atomic, map_db_err, run};

/// Turns a user query into an FTS5 MATCH expression that cannot contain operators:
/// `foo -"bar` becomes `"foo"* "bar"*` (implicit AND of prefix strings). Terms are
/// alphanumeric, so the quoting is only a second line of defence. `None` when the query
/// has no terms.
fn match_expression(q: &str) -> Option<String> {
    let terms: Vec<String> = search_terms(q)
        .iter()
        .map(|t| format!("\"{}\"*", t.replace('"', "\"\"")))
        .collect();
    (!terms.is_empty()).then(|| terms.join(" "))
}

#[async_trait]
impl SearchRepo for SqliteRepos {
    async fn index(&self, id: ContentId, title: &str, body_text: &str) -> Result<(), StoreError> {
        validate_text("search title", title)?;
        validate_text("search body_text", body_text)?;
        atomic!(&self.writer, |conn| async {
            let exists: Option<i64> =
                sqlx::query_scalar("SELECT 1 FROM content_items WHERE id = ?")
                    .bind(id.as_uuid())
                    .fetch_optional(&mut *conn)
                    .await
                    .map_err(map_db_err)?;
            if exists.is_none() {
                return Err(StoreError::ForeignKey {
                    constraint: "search_fts.item_id".into(),
                });
            }
            sqlx::query("DELETE FROM search_fts WHERE item_id = ?")
                .bind(id.as_uuid())
                .execute(&mut *conn)
                .await
                .map_err(map_db_err)?;
            sqlx::query("INSERT INTO search_fts (item_id, title, body_text) VALUES (?, ?, ?)")
                .bind(id.as_uuid())
                .bind(title)
                .bind(body_text)
                .execute(&mut *conn)
                .await
                .map_err(map_db_err)?;
            Ok(())
        })
    }

    async fn remove(&self, id: ContentId) -> Result<(), StoreError> {
        run!(&self.writer, |e| sqlx::query(
            "DELETE FROM search_fts WHERE item_id = ?"
        )
        .bind(id.as_uuid())
        .execute(e))
        .map(|_| ())
    }

    async fn query(&self, q: &str, limit: u32) -> Result<Vec<ContentId>, StoreError> {
        validate_text("search query", q)?;
        let limit = check_limit(limit)?;
        let Some(expr) = match_expression(q) else {
            return Ok(Vec::new());
        };
        // bm25 takes one weight per column, item_id included; lower scores rank higher.
        let ids: Vec<Uuid> = run!(&self.reader, |e| sqlx::query_scalar(
            "SELECT item_id FROM search_fts WHERE search_fts MATCH ? \
             ORDER BY bm25(search_fts, 0.0, 10.0, 1.0), item_id LIMIT ?"
        )
        .bind(expr)
        .bind(i64::from(limit))
        .fetch_all(e))?;
        Ok(ids.into_iter().map(ContentId::from_uuid).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::match_expression;

    #[test]
    fn match_expression_quotes_every_term() {
        assert_eq!(match_expression(""), None);
        assert_eq!(match_expression(" \t\n"), None);
        assert_eq!(match_expression("foo"), Some(r#""foo"*"#.into()));
        assert_eq!(
            match_expression("  a OR  -x "),
            Some(r#""a"* "OR"* "x"*"#.into())
        );
        assert_eq!(
            match_expression(r#"say"hi""#),
            Some(r#""say"* "hi"*"#.into())
        );
        // Punctuation only: no terms, no statement.
        assert_eq!(match_expression("\" * ( -"), None);
    }
}
