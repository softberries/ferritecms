//! Full-text search on `search_docs` (a generated, weighted `tsvector` with a GIN index).
//!
//! The user query is split by `ferrite_store::search_terms` (the same terms SQLite sees)
//! and bound as a `text[]`; SQL turns each term into a quoted prefix lexeme
//! (`quote_literal(t) || ':*'`), AND-joins them and parses that with
//! `to_tsquery('simple', ..)`. No user text is formatted into SQL. Rows cascade on
//! content delete.

use async_trait::async_trait;
use ferrite_domain::ContentId;
use ferrite_store::{SearchRepo, StoreError, check_limit, search_terms, validate_text};
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
        let terms = search_terms(q);
        if terms.is_empty() {
            return Ok(Vec::new());
        }
        let ids: Vec<Uuid> = run!(&self.exec, |e| {
            sqlx::query_scalar(
            "SELECT d.item_id FROM search_docs d, to_tsquery('simple', \
             (SELECT string_agg(quote_literal(t) || ':*', ' & ') FROM unnest($1::text[]) AS u(t))) q \
             WHERE d.tsv @@ q ORDER BY ts_rank(d.tsv, q) DESC, d.item_id LIMIT $2"
        )
        .bind(&terms)
        .bind(i64::from(limit))
        .fetch_all(e)
        })?;
        Ok(ids.into_iter().map(ContentId::from_uuid).collect())
    }
}
