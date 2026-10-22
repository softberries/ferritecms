//! Full-text search on `search_docs` (a generated, weighted `tsvector` with a GIN index).
//!
//! The user query goes through `websearch_to_tsquery('simple', $1)`, which never raises a
//! syntax error, and is always bound as a parameter. Rows cascade on content delete.

use async_trait::async_trait;
use ferrite_domain::ContentId;
use ferrite_store::{SearchRepo, StoreError, check_limit, validate_text};
use uuid::Uuid;

use super::PgRepos;
use crate::exec::run;

#[async_trait]
impl SearchRepo for PgRepos {
    async fn index(&self, id: ContentId, title: &str, body_text: &str) -> Result<(), StoreError> {
        validate_text("search title", title)?;
        validate_text("search body_text", body_text)?;
        run!(&self.exec, |e| sqlx::query(
            "INSERT INTO search_docs (item_id, title, body_text) VALUES ($1, $2, $3) \
             ON CONFLICT (item_id) DO UPDATE SET title = excluded.title, \
             body_text = excluded.body_text"
        )
        .bind(id.as_uuid())
        .bind(title)
        .bind(body_text)
        .execute(e))
        .map(|_| ())
    }

    async fn remove(&self, id: ContentId) -> Result<(), StoreError> {
        run!(&self.exec, |e| sqlx::query(
            "DELETE FROM search_docs WHERE item_id = $1"
        )
        .bind(id.as_uuid())
        .execute(e))
        .map(|_| ())
    }

    async fn query(&self, q: &str, limit: u32) -> Result<Vec<ContentId>, StoreError> {
        validate_text("search query", q)?;
        let limit = check_limit(limit)?;
        // Same short-circuit as SQLite: no terms, no statement.
        if q.split_whitespace().next().is_none() {
            return Ok(Vec::new());
        }
        let ids: Vec<Uuid> = run!(&self.exec, |e| sqlx::query_scalar(
            "SELECT d.item_id FROM search_docs d, websearch_to_tsquery('simple', $1) q \
             WHERE d.tsv @@ q ORDER BY ts_rank(d.tsv, q) DESC, d.item_id LIMIT $2"
        )
        .bind(q)
        .bind(i64::from(limit))
        .fetch_all(e))?;
        Ok(ids.into_iter().map(ContentId::from_uuid).collect())
    }
}
