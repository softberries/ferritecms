//! The per-user autosave slot (`0007_autosaves`) on SQLite.

use async_trait::async_trait;
use ferrite_domain::{Autosave, ContentId, Timestamp, UserId};
use ferrite_store::{AutosaveRepo, StoreError, autosave_terms, check_autosave};
use sqlx::{Row, sqlite::SqliteRow};
use uuid::Uuid;

use super::SqliteRepos;
use crate::{
    codec,
    exec::{map_db_err, run},
};

const COLUMNS: &str = "item_id, user_id, base_version, title, excerpt, slug, parent_id, \
     menu_order, terms, body, fields, saved_at";

fn autosave(row: &SqliteRow) -> Result<Autosave, StoreError> {
    let item: Uuid = row.try_get("item_id").map_err(map_db_err)?;
    let user: Uuid = row.try_get("user_id").map_err(map_db_err)?;
    let parent: Option<Uuid> = row.try_get("parent_id").map_err(map_db_err)?;
    let terms: Option<String> = row.try_get("terms").map_err(map_db_err)?;
    let body: String = row.try_get("body").map_err(map_db_err)?;
    let fields: String = row.try_get("fields").map_err(map_db_err)?;
    let saved_at: i64 = row.try_get("saved_at").map_err(map_db_err)?;
    Ok(Autosave {
        item_id: ContentId::from_uuid(item),
        user_id: UserId::from_uuid(user),
        base_version: row.try_get("base_version").map_err(map_db_err)?,
        title: row.try_get("title").map_err(map_db_err)?,
        excerpt: row.try_get("excerpt").map_err(map_db_err)?,
        slug: row.try_get("slug").map_err(map_db_err)?,
        parent_id: parent.map(ContentId::from_uuid),
        menu_order: row.try_get("menu_order").map_err(map_db_err)?,
        terms: autosave_terms(terms.as_deref().map(codec::json_from_db).transpose()?)?,
        body: codec::json_from_db(&body)?,
        fields: codec::json_from_db(&fields)?,
        saved_at: Timestamp::from_micros(saved_at)?,
    })
}

#[async_trait]
impl AutosaveRepo for SqliteRepos {
    async fn put(&self, a: &Autosave) -> Result<(), StoreError> {
        let terms = check_autosave(a)?
            .as_ref()
            .map(codec::json_to_db)
            .transpose()?;
        run!(&self.writer, |e| sqlx::query(
            "INSERT INTO autosaves (item_id, user_id, base_version, title, excerpt, slug, \
               parent_id, menu_order, terms, body, fields, saved_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT (item_id, user_id) DO UPDATE SET \
               base_version = excluded.base_version, title = excluded.title, \
               excerpt = excluded.excerpt, slug = excluded.slug, \
               parent_id = excluded.parent_id, menu_order = excluded.menu_order, \
               terms = excluded.terms, body = excluded.body, fields = excluded.fields, \
               saved_at = excluded.saved_at"
        )
        .bind(a.item_id.as_uuid())
        .bind(a.user_id.as_uuid())
        .bind(a.base_version)
        .bind(&a.title)
        .bind(&a.excerpt)
        .bind(&a.slug)
        .bind(a.parent_id.map(|p| p.as_uuid()))
        .bind(a.menu_order)
        .bind(terms.as_deref())
        .bind(codec::json_to_db(&a.body)?)
        .bind(codec::json_to_db(&a.fields)?)
        .bind(a.saved_at.as_micros())
        .execute(e))
        .map(|_| ())
    }

    async fn get(&self, item: ContentId, user: UserId) -> Result<Option<Autosave>, StoreError> {
        let row = run!(&self.reader, |e| sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {COLUMNS} FROM autosaves WHERE item_id = ? AND user_id = ?"
        )))
        .bind(item.as_uuid())
        .bind(user.as_uuid())
        .fetch_optional(e))?;
        row.as_ref().map(autosave).transpose()
    }

    async fn delete(&self, item: ContentId, user: UserId) -> Result<bool, StoreError> {
        let done = run!(&self.writer, |e| sqlx::query(
            "DELETE FROM autosaves WHERE item_id = ? AND user_id = ?"
        )
        .bind(item.as_uuid())
        .bind(user.as_uuid())
        .execute(e))?;
        Ok(done.rows_affected() > 0)
    }
}
