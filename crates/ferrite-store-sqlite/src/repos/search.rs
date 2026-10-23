//! Full-text search (R17): `search_docs` holds each item's normalized tokens and the
//! external-content FTS5 table `search_fts(title, body_text)` indexes them with the
//! `ascii` tokenizer (sync triggers in 0002). All normalization happens in
//! `ferrite_store::{index_terms, search_terms}`, identically to Postgres; FTS5 only sees
//! opaque tokens. Rows cascade with the content item through the foreign key.

use async_trait::async_trait;
use ferrite_domain::{ContentId, Page, SearchQuery};
use ferrite_store::{
    IndexTerms, SearchRepo, StoreError, encode_rank_cursor, index_terms, plan_search, validate_text,
};
use sqlx::{QueryBuilder, Sqlite};
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

    async fn search(&self, query: &SearchQuery) -> Result<Page<ContentId>, StoreError> {
        let Some(plan) = plan_search(query)? else {
            return Ok(Page {
                items: Vec::new(),
                next_cursor: None,
            });
        };
        // bm25 only works directly under the MATCH query, so the rank is materialized in
        // a CTE (flattening would move bm25() into the outer WHERE, where it is
        // rejected) and the keyset predicate compares that stored value. Weights are per
        // column (title, body_text); lower bm25 is better, so the rank is its negation
        // (exact). The bound cursor rank carries the same 8 bytes, so `=` is exact.
        let mut qb = QueryBuilder::<Sqlite>::new(
            "WITH hits AS MATERIALIZED (SELECT d.item_id AS id, \
             -bm25(search_fts, 10.0, 1.0) AS rank \
             FROM search_fts JOIN search_docs d ON d.id = search_fts.rowid \
             WHERE search_fts MATCH ",
        );
        qb.push_bind(match_expression(&plan.terms));
        qb.push(
            ") SELECT h.id, h.rank FROM hits h JOIN content_items c ON c.id = h.id WHERE 1 = 1",
        );
        if !query.kinds.is_empty() {
            qb.push(" AND c.type IN (");
            let mut list = qb.separated(", ");
            for kind in &query.kinds {
                list.push_bind(kind.as_str());
            }
            list.push_unseparated(")");
        }
        if !plan.statuses.is_empty() {
            qb.push(" AND c.status IN (");
            let mut list = qb.separated(", ");
            for status in &plan.statuses {
                list.push_bind(status.as_str());
            }
            list.push_unseparated(")");
        }
        if let Some(reader) = query.readable_by {
            qb.push(" AND (c.status = 'published' OR c.author_id = ")
                .push_bind(reader.as_uuid())
                .push(")");
        }
        if let Some((rank, id)) = plan.after {
            qb.push(" AND (h.rank < ").push_bind(rank);
            qb.push(" OR (h.rank = ").push_bind(rank);
            qb.push(" AND h.id < ").push_bind(id).push("))");
        }
        qb.push(" ORDER BY h.rank DESC, h.id DESC LIMIT ")
            .push_bind(i64::from(plan.limit) + 1);
        let mut hits: Vec<(Uuid, f64)> = run!(&self.reader, |e| qb.build_query_as().fetch_all(e))?;
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
