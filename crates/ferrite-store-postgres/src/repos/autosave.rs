//! The per-user autosave slot (`0007_autosaves`, `seo` from `0008_autosave_seo`) on Postgres.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use ferrite_domain::{Autosave, ContentId, UserId};
use ferrite_store::{AutosaveRepo, StoreError, autosave_terms, check_autosave};
use sqlx::{Row, postgres::PgRow, types::Json};
use uuid::Uuid;

use super::PgRepos;
use crate::exec::{map_db_err, run};

const COLUMNS: &str = "item_id, user_id, base_version, title, excerpt, slug, parent_id, \
     menu_order, terms, body, fields, seo, saved_at";

fn autosave(row: &PgRow) -> Result<Autosave, StoreError> {
    let item: Uuid = row.try_get("item_id").map_err(map_db_err)?;
    let user: Uuid = row.try_get("user_id").map_err(map_db_err)?;
    let parent: Option<Uuid> = row.try_get("parent_id").map_err(map_db_err)?;
    let terms: Option<Json<serde_json::Value>> = row.try_get("terms").map_err(map_db_err)?;
    let body: Json<serde_json::Value> = row.try_get("body").map_err(map_db_err)?;
    let fields: Json<serde_json::Value> = row.try_get("fields").map_err(map_db_err)?;
    let seo: Option<Json<serde_json::Value>> = row.try_get("seo").map_err(map_db_err)?;
    let saved_at: DateTime<Utc> = row.try_get("saved_at").map_err(map_db_err)?;
    Ok(Autosave {
        item_id: ContentId::from_uuid(item),
        user_id: UserId::from_uuid(user),
        base_version: row.try_get("base_version").map_err(map_db_err)?,
        title: row.try_get("title").map_err(map_db_err)?,
        excerpt: row.try_get("excerpt").map_err(map_db_err)?,
        slug: row.try_get("slug").map_err(map_db_err)?,
        parent_id: parent.map(ContentId::from_uuid),
        menu_order: row.try_get("menu_order").map_err(map_db_err)?,
        terms: autosave_terms(terms.map(|t| t.0))?,
        body: body.0,
        fields: fields.0,
        seo: seo.map(|s| s.0),
        saved_at: saved_at.into(),
    })
}

#[async_trait]
impl AutosaveRepo for PgRepos {
    async fn put(&self, a: &Autosave) -> Result<(), StoreError> {
        let terms = check_autosave(a)?;
        run!(&self.exec, |e| sqlx::query(
            "INSERT INTO autosaves (item_id, user_id, base_version, title, excerpt, slug, \
               parent_id, menu_order, terms, body, fields, seo, saved_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13) \
             ON CONFLICT (item_id, user_id) DO UPDATE SET \
               base_version = excluded.base_version, title = excluded.title, \
               excerpt = excluded.excerpt, slug = excluded.slug, \
               parent_id = excluded.parent_id, menu_order = excluded.menu_order, \
               terms = excluded.terms, body = excluded.body, fields = excluded.fields, \
               seo = excluded.seo, saved_at = excluded.saved_at"
        )
        .bind(a.item_id.as_uuid())
        .bind(a.user_id.as_uuid())
        .bind(a.base_version)
        .bind(&a.title)
        .bind(&a.excerpt)
        .bind(&a.slug)
        .bind(a.parent_id.map(|p| p.as_uuid()))
        .bind(a.menu_order)
        .bind(terms.as_ref().map(Json))
        .bind(Json(&a.body))
        .bind(Json(&a.fields))
        .bind(a.seo.as_ref().map(Json))
        .bind(DateTime::<Utc>::from(a.saved_at))
        .execute(e))
        .map(|_| ())
    }

    async fn get(&self, item: ContentId, user: UserId) -> Result<Option<Autosave>, StoreError> {
        let row = run!(&self.exec, |e| sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {COLUMNS} FROM autosaves WHERE item_id = $1 AND user_id = $2"
        )))
        .bind(item.as_uuid())
        .bind(user.as_uuid())
        .fetch_optional(e))?;
        row.as_ref().map(autosave).transpose()
    }

    async fn delete(&self, item: ContentId, user: UserId) -> Result<bool, StoreError> {
        let done = run!(&self.exec, |e| sqlx::query(
            "DELETE FROM autosaves WHERE item_id = $1 AND user_id = $2"
        )
        .bind(item.as_uuid())
        .bind(user.as_uuid())
        .execute(e))?;
        Ok(done.rows_affected() > 0)
    }
}
