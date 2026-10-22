//! Full-text search (R17) on `search_docs(item_id, tsv)` with a GIN index. All
//! normalization happens in `ferrite_store::{index_terms, search_terms}`, identically to
//! SQLite; Postgres only receives `tsvector` / `tsquery` literals built here and bound as
//! parameters, so no text search parser, configuration or locale is involved.

use std::fmt::Write;

use async_trait::async_trait;
use ferrite_domain::ContentId;
use ferrite_store::{
    IndexedTerm, SearchRepo, StoreError, check_limit, index_terms, search_terms, validate_text,
};
use uuid::Uuid;

use super::PgRepos;
use crate::exec::run;

/// Quotes a lexeme for tsvector/tsquery input (`'` doubled, `\` escaped). Terms are
/// alphanumeric runs, so this is a second line of defence.
fn lexeme(term: &str) -> String {
    format!("'{}'", term.replace('\\', "\\\\").replace('\'', "''"))
}

/// The tsvector literal for an item: one entry per distinct term with its positions,
/// weight A for title positions and B for body positions (`'hello':1A,3B 'world':2A`).
fn tsvector(title: &[IndexedTerm], body: &[IndexedTerm]) -> String {
    let mut entries: Vec<(&str, Vec<String>)> = Vec::new();
    let mut slot = std::collections::HashMap::new();
    for (terms, weight) in [(title, 'A'), (body, 'B')] {
        for t in terms {
            let i = *slot.entry(t.term.as_str()).or_insert_with(|| {
                entries.push((t.term.as_str(), Vec::new()));
                entries.len() - 1
            });
            entries[i]
                .1
                .extend(t.positions.iter().map(|p| format!("{p}{weight}")));
        }
    }
    let mut out = String::new();
    for (term, positions) in entries {
        if !out.is_empty() {
            out.push(' ');
        }
        // Writing to a String cannot fail.
        let _ = write!(out, "{}:{}", lexeme(term), positions.join(","));
    }
    out
}

/// The tsquery literal for normalized query terms: AND of prefix lexemes.
fn tsquery(terms: &[String]) -> String {
    terms
        .iter()
        .map(|t| format!("{}:*", lexeme(t)))
        .collect::<Vec<_>>()
        .join(" & ")
}

#[async_trait]
impl SearchRepo for PgRepos {
    async fn index(&self, id: ContentId, title: &str, body_text: &str) -> Result<(), StoreError> {
        validate_text("search title", title)?;
        validate_text("search body_text", body_text)?;
        let terms = index_terms(title, body_text);
        run!(&self.exec, |e| sqlx::query(
            "INSERT INTO search_docs (item_id, tsv) VALUES ($1, $2::tsvector) \
             ON CONFLICT (item_id) DO UPDATE SET tsv = excluded.tsv"
        )
        .bind(id.as_uuid())
        .bind(tsvector(&terms.title, &terms.body))
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
        let ids: Vec<Uuid> = run!(&self.exec, |e| sqlx::query_scalar(
            "SELECT item_id FROM search_docs, CAST($1 AS tsquery) q \
             WHERE tsv @@ q ORDER BY ts_rank(tsv, q) DESC, item_id LIMIT $2"
        )
        .bind(tsquery(&terms))
        .bind(i64::from(limit))
        .fetch_all(e))?;
        Ok(ids.into_iter().map(ContentId::from_uuid).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literals() {
        let t = index_terms("Hello world", "hello again");
        assert_eq!(
            tsvector(&t.title, &t.body),
            "'hello':1A,3B 'world':2A 'again':4B"
        );
        assert_eq!(tsvector(&[], &[]), "");
        assert_eq!(
            tsquery(&["a".into(), "o'k\\".into()]),
            r"'a':* & 'o''k\\':*"
        );
    }
}
