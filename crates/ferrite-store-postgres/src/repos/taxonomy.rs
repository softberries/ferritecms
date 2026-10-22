//! Taxonomies, terms and term links.
//!
//! `TermRepo::set_for_item` is a delete plus inserts and must be atomic. Inside a `Tx`
//! it runs in a savepoint on the transaction (R11), so a failure undoes only the
//! replacement. Through `Store::read()` the executor is the pool, so it
//! opens its own transaction with `pool.begin()` and commits it; any failure drops the
//! transaction and rolls back.

use std::collections::HashMap;

use async_trait::async_trait;
use ferrite_domain::{ContentId, Taxonomy, Term, TermId};
use ferrite_store::{
    StoreError, TaxonomyRepo, TermRepo, check_id_list, validate_json, validate_text,
};
use sqlx::{PgConnection, Postgres, QueryBuilder, Row, types::Json};
use uuid::Uuid;

use super::PgRepos;
use crate::{
    codec::{self, TAXONOMY_COLUMNS, TERM_COLUMNS},
    exec::{atomic, map_db_err, run},
};

#[async_trait]
impl TaxonomyRepo for PgRepos {
    async fn upsert(&self, taxonomy: &Taxonomy) -> Result<(), StoreError> {
        validate_text("taxonomy name", &taxonomy.name)?;
        validate_text("taxonomy label", &taxonomy.label)?;
        let types = serde_json::to_value(&taxonomy.content_types)
            .map_err(|e| StoreError::Invalid(e.to_string()))?;
        validate_json(&types)?;
        // `built_in` is fixed when the taxonomy is first inserted.
        run!(&self.exec, |e| sqlx::query(
            "INSERT INTO taxonomies (name, label, hierarchical, content_types, built_in) \
             VALUES ($1, $2, $3, $4, $5) \
             ON CONFLICT (name) DO UPDATE SET label = excluded.label, \
             hierarchical = excluded.hierarchical, content_types = excluded.content_types"
        )
        .bind(&taxonomy.name)
        .bind(&taxonomy.label)
        .bind(taxonomy.hierarchical)
        .bind(Json(&types))
        .bind(taxonomy.built_in)
        .execute(e))
        .map(|_| ())
    }

    async fn get(&self, name: &str) -> Result<Option<Taxonomy>, StoreError> {
        validate_text("taxonomy name", name)?;
        let row = run!(&self.exec, |e| sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {TAXONOMY_COLUMNS} FROM taxonomies WHERE name = $1"
        )))
        .bind(name)
        .fetch_optional(e))?;
        row.as_ref().map(codec::taxonomy).transpose()
    }

    async fn list(&self) -> Result<Vec<Taxonomy>, StoreError> {
        let rows = run!(&self.exec, |e| sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {TAXONOMY_COLUMNS} FROM taxonomies ORDER BY name COLLATE \"C\""
        )))
        .fetch_all(e))?;
        rows.iter().map(codec::taxonomy).collect()
    }
}

/// Replaces the links of `item` on `conn` (the caller supplies the transaction).
async fn replace_links(
    conn: &mut PgConnection,
    item: ContentId,
    terms: &[TermId],
) -> Result<(), StoreError> {
    sqlx::query("DELETE FROM term_links WHERE item_id = $1")
        .bind(item.as_uuid())
        .execute(&mut *conn)
        .await
        .map_err(map_db_err)?;
    for (position, term) in (0_i32..).zip(terms) {
        sqlx::query("INSERT INTO term_links (item_id, term_id, position) VALUES ($1, $2, $3)")
            .bind(item.as_uuid())
            .bind(term.as_uuid())
            .bind(position)
            .execute(&mut *conn)
            .await
            .map_err(map_db_err)?;
    }
    Ok(())
}

#[async_trait]
impl TermRepo for PgRepos {
    async fn insert(&self, term: &Term) -> Result<(), StoreError> {
        validate_text("term taxonomy", &term.taxonomy)?;
        validate_text("term slug", &term.slug)?;
        validate_text("term name", &term.name)?;
        validate_text("term description", &term.description)?;
        run!(&self.exec, |e| sqlx::query(
            "INSERT INTO terms (id, taxonomy, slug, name, description, parent_id) \
             VALUES ($1, $2, $3, $4, $5, $6)"
        )
        .bind(term.id.as_uuid())
        .bind(&term.taxonomy)
        .bind(&term.slug)
        .bind(&term.name)
        .bind(&term.description)
        .bind(term.parent_id.map(|p| p.as_uuid()))
        .execute(e))
        .map(|_| ())
    }

    async fn update(&self, term: &Term) -> Result<(), StoreError> {
        validate_text("term taxonomy", &term.taxonomy)?;
        validate_text("term slug", &term.slug)?;
        validate_text("term name", &term.name)?;
        validate_text("term description", &term.description)?;
        let done = run!(&self.exec, |e| sqlx::query(
            "UPDATE terms SET taxonomy = $1, slug = $2, name = $3, description = $4, \
             parent_id = $5 WHERE id = $6"
        )
        .bind(&term.taxonomy)
        .bind(&term.slug)
        .bind(&term.name)
        .bind(&term.description)
        .bind(term.parent_id.map(|p| p.as_uuid()))
        .bind(term.id.as_uuid())
        .execute(e))?;
        if done.rows_affected() == 0 {
            return Err(StoreError::NotFound);
        }
        Ok(())
    }

    async fn get(&self, id: TermId) -> Result<Option<Term>, StoreError> {
        let row = run!(&self.exec, |e| sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {TERM_COLUMNS} FROM terms t WHERE t.id = $1"
        )))
        .bind(id.as_uuid())
        .fetch_optional(e))?;
        row.as_ref().map(codec::term).transpose()
    }

    async fn get_by_slug(&self, tax: &str, slug: &str) -> Result<Option<Term>, StoreError> {
        validate_text("term taxonomy", tax)?;
        validate_text("term slug", slug)?;
        let row = run!(&self.exec, |e| sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {TERM_COLUMNS} FROM terms t WHERE t.taxonomy = $1 AND t.slug = $2"
        )))
        .bind(tax)
        .bind(slug)
        .fetch_optional(e))?;
        row.as_ref().map(codec::term).transpose()
    }

    async fn list(&self, tax: &str) -> Result<Vec<Term>, StoreError> {
        validate_text("term taxonomy", tax)?;
        let rows = run!(&self.exec, |e| sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {TERM_COLUMNS} FROM terms t WHERE t.taxonomy = $1 \
             ORDER BY t.name COLLATE \"C\", t.id"
        )))
        .bind(tax)
        .fetch_all(e))?;
        rows.iter().map(codec::term).collect()
    }

    async fn delete(&self, id: TermId) -> Result<(), StoreError> {
        // Links cascade; child terms RESTRICT (R9) and surface as `ForeignKey`.
        run!(&self.exec, |e| sqlx::query(
            "DELETE FROM terms WHERE id = $1"
        )
        .bind(id.as_uuid())
        .execute(e))
        .map(|_| ())
    }

    async fn set_for_item(&self, item: ContentId, terms: &[TermId]) -> Result<(), StoreError> {
        atomic!(&self.exec, |conn| replace_links(conn, item, terms))
    }

    async fn for_item(&self, item: ContentId) -> Result<Vec<Term>, StoreError> {
        let rows = run!(&self.exec, |e| sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {TERM_COLUMNS} FROM terms t \
             JOIN term_links l ON l.term_id = t.id \
             WHERE l.item_id = $1 ORDER BY l.position"
        )))
        .bind(item.as_uuid())
        .fetch_all(e))?;
        rows.iter().map(codec::term).collect()
    }

    async fn for_items(
        &self,
        items: &[ContentId],
    ) -> Result<HashMap<ContentId, Vec<Term>>, StoreError> {
        check_id_list("items", items)?;
        let mut out: HashMap<ContentId, Vec<Term>> = HashMap::new();
        if items.is_empty() {
            return Ok(out);
        }
        let mut qb = QueryBuilder::<Postgres>::new(format!(
            "SELECT l.item_id AS item_id, {TERM_COLUMNS} FROM terms t \
             JOIN term_links l ON l.term_id = t.id WHERE l.item_id IN ("
        ));
        let mut list = qb.separated(", ");
        for id in items {
            list.push_bind(id.as_uuid());
        }
        list.push_unseparated(") ORDER BY l.item_id, l.position");
        let rows = run!(&self.exec, |e| qb.build().fetch_all(e))?;
        for row in &rows {
            let item: Uuid = row.try_get("item_id").map_err(map_db_err)?;
            out.entry(ContentId::from_uuid(item))
                .or_default()
                .push(codec::term(row)?);
        }
        Ok(out)
    }
}
