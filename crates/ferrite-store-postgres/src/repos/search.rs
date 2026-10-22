//! Full-text search (R17) on `search_docs(item_id, tsv)` with a GIN index. All
//! normalization happens in `ferrite_store::{index_terms, search_terms}`, identically to
//! SQLite; Postgres only receives `tsvector` / `tsquery` literals built here and bound as
//! parameters, so no text search parser, configuration or locale is involved.

use std::fmt::Write;

use async_trait::async_trait;
use ferrite_domain::{ContentId, Page, SearchQuery};
use ferrite_store::{
    IndexedTerm, SearchRepo, StoreError, encode_rank_cursor, index_terms, plan_search,
    validate_text,
};
use sqlx::{Postgres, QueryBuilder};
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

    async fn search(&self, query: &SearchQuery) -> Result<Page<ContentId>, StoreError> {
        let Some(plan) = plan_search(query)? else {
            return Ok(Page {
                items: Vec::new(),
                next_cursor: None,
            });
        };
        // The rank is a function of the item's own tsvector and the query only, so it is
        // reproducible for a fixed index. It is cast to float8 (ts_rank returns float4,
        // an exact widening) and compared with the bound cursor rank as float8.
        let mut qb = QueryBuilder::<Postgres>::new(
            "SELECT h.id, h.rank FROM (SELECT s.item_id AS id, ts_rank(s.tsv, q)::float8 AS rank \
             FROM search_docs s, CAST(",
        );
        qb.push_bind(tsquery(&plan.terms));
        qb.push(
            " AS tsquery) q WHERE s.tsv @@ q) h \
             JOIN content_items c ON c.id = h.id WHERE TRUE",
        );
        if !query.kinds.is_empty() {
            qb.push(" AND c.type IN (");
            let mut list = qb.separated(", ");
            for kind in &query.kinds {
                list.push_bind(kind.as_str());
            }
            list.push_unseparated(")");
        }
        if !query.statuses.is_empty() {
            qb.push(" AND c.status IN (");
            let mut list = qb.separated(", ");
            for status in &query.statuses {
                list.push_bind(status.as_str());
            }
            list.push_unseparated(")");
        }
        if let Some((rank, id)) = plan.after {
            qb.push(" AND (h.rank < ").push_bind(rank);
            qb.push(" OR (h.rank = ").push_bind(rank);
            qb.push(" AND h.id < ").push_bind(id).push("))");
        }
        qb.push(" ORDER BY h.rank DESC, h.id DESC LIMIT ")
            .push_bind(i64::from(plan.limit) + 1);
        let mut hits: Vec<(Uuid, f64)> = run!(&self.exec, |e| qb.build_query_as().fetch_all(e))?;
        let next_cursor = if hits.len() > plan.limit as usize {
            hits.truncate(plan.limit as usize);
            hits.last().map(|(id, rank)| encode_rank_cursor(*rank, *id))
        } else {
            None
        };
        Ok(Page {
            items: hits
                .into_iter()
                .map(|(id, _)| ContentId::from_uuid(id))
                .collect(),
            next_cursor,
        })
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
