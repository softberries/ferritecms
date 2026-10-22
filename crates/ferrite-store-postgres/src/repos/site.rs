//! Site-level repositories.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use ferrite_domain::{OptionEntry, Page, Redirect, RedirectId, Timestamp};
use ferrite_store::{
    OptionRepo, RedirectRepo, SchemaMetaRepo, StoreError, check_limit, check_redirect_code,
    encode_text_cursor, text_cursor, validate_json, validate_text,
};
use sqlx::types::Json;

use super::PgRepos;
use crate::{
    codec::{self, REDIRECT_COLUMNS},
    exec::run,
};

#[async_trait]
impl SchemaMetaRepo for PgRepos {
    async fn get(&self, key: &str) -> Result<Option<String>, StoreError> {
        validate_text("schema_meta key", key)?;
        run!(&self.exec, |e| sqlx::query_scalar(
            "SELECT value FROM schema_meta WHERE key = $1"
        )
        .bind(key)
        .fetch_optional(e))
    }

    async fn set(&self, key: &str, value: &str) -> Result<(), StoreError> {
        validate_text("schema_meta key", key)?;
        validate_text("schema_meta value", value)?;
        run!(&self.exec, |e| sqlx::query(
            "INSERT INTO schema_meta (key, value) VALUES ($1, $2) \
             ON CONFLICT (key) DO UPDATE SET value = excluded.value"
        )
        .bind(key)
        .bind(value)
        .execute(e))
        .map(|_| ())
    }
}

#[async_trait]
impl OptionRepo for PgRepos {
    async fn get(&self, name: &str) -> Result<Option<OptionEntry>, StoreError> {
        validate_text("option key", name)?;
        let row = run!(&self.exec, |e| sqlx::query(
            "SELECT key, value, autoload, updated_at FROM options WHERE key = $1"
        )
        .bind(name)
        .fetch_optional(e))?;
        row.as_ref().map(codec::option_entry).transpose()
    }

    async fn set(
        &self,
        name: &str,
        value: &serde_json::Value,
        autoload: bool,
    ) -> Result<(), StoreError> {
        validate_text("option key", name)?;
        validate_json(value)?;
        run!(&self.exec, |e| sqlx::query(
            "INSERT INTO options (key, value, autoload, updated_at) VALUES ($1, $2, $3, $4) \
             ON CONFLICT (key) DO UPDATE SET value = excluded.value, \
             autoload = excluded.autoload, updated_at = excluded.updated_at"
        )
        .bind(name)
        .bind(Json(value))
        .bind(autoload)
        .bind(DateTime::<Utc>::from(Timestamp::now()))
        .execute(e))
        .map(|_| ())
    }

    async fn delete(&self, name: &str) -> Result<(), StoreError> {
        validate_text("option key", name)?;
        run!(&self.exec, |e| sqlx::query(
            "DELETE FROM options WHERE key = $1"
        )
        .bind(name)
        .execute(e))
        .map(|_| ())
    }

    async fn autoloaded(&self) -> Result<Vec<OptionEntry>, StoreError> {
        let rows = run!(&self.exec, |e| {
            sqlx::query(
            "SELECT key, value, autoload, updated_at FROM options WHERE autoload ORDER BY key COLLATE \"C\""
        )
        .fetch_all(e)
        })?;
        rows.iter().map(codec::option_entry).collect()
    }
}

#[async_trait]
impl RedirectRepo for PgRepos {
    async fn get_by_source(&self, source: &str) -> Result<Option<Redirect>, StoreError> {
        validate_text("redirect source_path", source)?;
        // The unique index is on the plain column: exact, case-sensitive.
        let row = run!(&self.exec, |e| sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {REDIRECT_COLUMNS} FROM redirects WHERE source_path = $1"
        )))
        .bind(source)
        .fetch_optional(e))?;
        row.as_ref().map(codec::redirect).transpose()
    }

    async fn upsert(&self, redirect: &Redirect) -> Result<(), StoreError> {
        check_redirect_code(redirect.code)?;
        validate_text("redirect source_path", &redirect.source_path)?;
        validate_text("redirect target", &redirect.target)?;
        // An existing row keeps its id, hits and creation time.
        run!(&self.exec, |e| sqlx::query(
            "INSERT INTO redirects (id, source_path, target, code, hits, origin, created_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7) \
             ON CONFLICT (source_path) DO UPDATE SET target = excluded.target, \
             code = excluded.code, origin = excluded.origin"
        )
        .bind(redirect.id.as_uuid())
        .bind(&redirect.source_path)
        .bind(&redirect.target)
        .bind(i32::from(redirect.code))
        .bind(redirect.hits)
        .bind(redirect.origin.as_str())
        .bind(DateTime::<Utc>::from(redirect.created_at))
        .execute(e))
        .map(|_| ())
    }

    async fn delete(&self, id: RedirectId) -> Result<(), StoreError> {
        run!(&self.exec, |e| sqlx::query(
            "DELETE FROM redirects WHERE id = $1"
        )
        .bind(id.as_uuid())
        .execute(e))
        .map(|_| ())
    }

    async fn list(&self, limit: u32, cursor: Option<&str>) -> Result<Page<Redirect>, StoreError> {
        let limit = check_limit(limit)?;
        let after = cursor.map(text_cursor).transpose()?;
        let rows = run!(&self.exec, |e| sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {REDIRECT_COLUMNS} FROM redirects \
             WHERE $1::text IS NULL OR (source_path COLLATE \"C\", id) > ($1::text, $2::uuid) \
             ORDER BY source_path COLLATE \"C\", id LIMIT $3"
        )))
        .bind(after.as_ref().map(|(k, _)| k.as_str()))
        .bind(after.as_ref().map(|(_, id)| *id))
        .bind(i64::from(limit) + 1)
        .fetch_all(e))?;
        let mut items = rows
            .iter()
            .map(codec::redirect)
            .collect::<Result<Vec<_>, _>>()?;
        let next_cursor = if items.len() > limit as usize {
            items.truncate(limit as usize);
            items
                .last()
                .map(|r| encode_text_cursor(&r.source_path, r.id.as_uuid()))
        } else {
            None
        };
        Ok(Page { items, next_cursor })
    }

    async fn hit(&self, id: RedirectId) -> Result<(), StoreError> {
        let done = run!(&self.exec, |e| sqlx::query(
            "UPDATE redirects SET hits = hits + 1, last_hit_at = $1 WHERE id = $2"
        )
        .bind(DateTime::<Utc>::from(Timestamp::now()))
        .bind(id.as_uuid())
        .execute(e))?;
        if done.rows_affected() == 0 {
            return Err(StoreError::NotFound);
        }
        Ok(())
    }
}
