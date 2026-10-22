//! Full-text search (R17): `search_docs` holds each item's normalized tokens and the
//! external-content FTS5 table `search_fts(title, body_text)` indexes them with the
//! `ascii` tokenizer (sync triggers in 0002). All normalization happens in
//! `ferrite_store::{index_terms, search_terms}`, identically to Postgres; FTS5 only sees
//! opaque tokens. Rows cascade with the content item through the foreign key.

use async_trait::async_trait;
use ferrite_domain::ContentId;
use ferrite_store::{
    IndexTerms, SearchRepo, StoreError, check_limit, index_terms, search_terms, validate_text,
};
use uuid::Uuid;

use super::SqliteRepos;
use crate::exec::run;

/// The FTS5 MATCH expression for normalized query terms: each a quoted prefix string,
/// implicitly AND-ed (`"foo"* "bar"*`). Terms never contain `"` (they are alphanumeric
/// runs); the doubling is a second line of defence.
fn match_expression(terms: &[String]) -> String {
    terms
        .iter()
        .map(|t| format!("\"{}\"*", t.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" ")
}

#[async_trait]
impl SearchRepo for SqliteRepos {
    async fn index(&self, id: ContentId, title: &str, body_text: &str) -> Result<(), StoreError> {
        validate_text("search title", title)?;
        validate_text("search body_text", body_text)?;
        let terms = index_terms(title, body_text);
        // One statement; the update trigger re-indexes an existing row. An unknown item
        // fails the foreign key.
        run!(&self.writer, |e| sqlx::query(
            "INSERT INTO search_docs (item_id, title, body_text) VALUES (?, ?, ?) \
             ON CONFLICT (item_id) DO UPDATE SET title = excluded.title, \
             body_text = excluded.body_text"
        )
        .bind(id.as_uuid())
        .bind(IndexTerms::joined(&terms.title))
        .bind(IndexTerms::joined(&terms.body))
        .execute(e))
        .map(|_| ())
    }

    async fn remove(&self, id: ContentId) -> Result<(), StoreError> {
        run!(&self.writer, |e| sqlx::query(
            "DELETE FROM search_docs WHERE item_id = ?"
        )
        .bind(id.as_uuid())
        .execute(e))
        .map(|_| ())
    }

    async fn query(&self, q: &str, limit: u32) -> Result<Vec<ContentId>, StoreError> {
        validate_text("search query", q)?;
        let limit = check_limit(limit)?;
        let terms = search_terms(q);
        if terms.is_empty() {
            return Ok(Vec::new());
        }
        // bm25 weights are per column (title, body_text); lower scores rank higher.
        let ids: Vec<Uuid> = run!(&self.reader, |e| sqlx::query_scalar(
            "SELECT d.item_id FROM search_fts JOIN search_docs d ON d.id = search_fts.rowid \
             WHERE search_fts MATCH ? \
             ORDER BY bm25(search_fts, 10.0, 1.0), d.item_id LIMIT ?"
        )
        .bind(match_expression(&terms))
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
        let terms = |v: &[&str]| v.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        assert_eq!(match_expression(&terms(&["foo"])), r#""foo"*"#);
        assert_eq!(
            match_expression(&terms(&["a", "or", "x"])),
            r#""a"* "or"* "x"*"#
        );
        assert_eq!(match_expression(&terms(&["a\"b"])), r#""a""b"*"#);
    }
}
