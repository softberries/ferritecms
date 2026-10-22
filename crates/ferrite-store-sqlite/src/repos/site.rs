//! Site-level repositories.

use async_trait::async_trait;
use ferrite_domain::{OptionEntry, Page, Redirect, RedirectId, Timestamp};
use ferrite_store::{
    OptionRepo, RedirectRepo, SchemaMetaRepo, StoreError, check_limit, check_redirect_code,
    encode_text_cursor, text_cursor, validate_json, validate_text,
};

use super::SqliteRepos;
use crate::{
    codec::{self, REDIRECT_COLUMNS},
    exec::run,
};

#[async_trait]
impl SchemaMetaRepo for SqliteRepos {
    async fn get(&self, key: &str) -> Result<Option<String>, StoreError> {
        run!(&self.reader, |e| sqlx::query_scalar(
            "SELECT value FROM schema_meta WHERE key = ?"
        )
        .bind(key)
        .fetch_optional(e))
    }

    async fn set(&self, key: &str, value: &str) -> Result<(), StoreError> {
        validate_text("schema_meta key", key)?;
        validate_text("schema_meta value", value)?;
        run!(&self.writer, |e| sqlx::query(
            "INSERT INTO schema_meta (key, value) VALUES (?, ?) \
             ON CONFLICT (key) DO UPDATE SET value = excluded.value"
        )
        .bind(key)
        .bind(value)
        .execute(e))
        .map(|_| ())
    }
}

#[async_trait]
impl OptionRepo for SqliteRepos {
    async fn get(&self, name: &str) -> Result<Option<OptionEntry>, StoreError> {
        let row = run!(&self.reader, |e| sqlx::query(
            "SELECT key, value, autoload, updated_at FROM options WHERE key = ?"
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
        run!(&self.writer, |e| sqlx::query(
            "INSERT INTO options (key, value, autoload, updated_at) VALUES (?, ?, ?, ?) \
             ON CONFLICT (key) DO UPDATE SET value = excluded.value, \
             autoload = excluded.autoload, updated_at = excluded.updated_at"
        )
        .bind(name)
        .bind(codec::json_to_db(value)?)
        .bind(autoload)
        .bind(Timestamp::now().as_micros())
        .execute(e))
        .map(|_| ())
    }

    async fn delete(&self, name: &str) -> Result<(), StoreError> {
        run!(&self.writer, |e| sqlx::query(
            "DELETE FROM options WHERE key = ?"
        )
        .bind(name)
        .execute(e))
        .map(|_| ())
    }

    async fn autoloaded(&self) -> Result<Vec<OptionEntry>, StoreError> {
        let rows = run!(&self.reader, |e| {
            sqlx::query(
            "SELECT key, value, autoload, updated_at FROM options WHERE autoload = 1 ORDER BY key"
        )
        .fetch_all(e)
        })?;
        rows.iter().map(codec::option_entry).collect()
    }
}

#[async_trait]
impl RedirectRepo for SqliteRepos {
    async fn get_by_source(&self, source: &str) -> Result<Option<Redirect>, StoreError> {
        // `source_path` has BINARY collation: the match is exact and case-sensitive.
        let row = run!(&self.reader, |e| sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {REDIRECT_COLUMNS} FROM redirects WHERE source_path = ?"
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
        run!(&self.writer, |e| sqlx::query(
            "INSERT INTO redirects (id, source_path, target, code, hits, origin, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT (source_path) DO UPDATE SET target = excluded.target, \
             code = excluded.code, origin = excluded.origin"
        )
        .bind(redirect.id.as_uuid())
        .bind(&redirect.source_path)
        .bind(&redirect.target)
        .bind(i64::from(redirect.code))
        .bind(redirect.hits)
        .bind(redirect.origin.as_str())
        .bind(redirect.created_at.as_micros())
        .execute(e))
        .map(|_| ())
    }

    async fn delete(&self, id: RedirectId) -> Result<(), StoreError> {
        run!(&self.writer, |e| sqlx::query(
            "DELETE FROM redirects WHERE id = ?"
        )
        .bind(id.as_uuid())
        .execute(e))
        .map(|_| ())
    }

    async fn list(&self, limit: u32, cursor: Option<&str>) -> Result<Page<Redirect>, StoreError> {
        let limit = check_limit(limit)?;
        let after = cursor.map(text_cursor).transpose()?;
        let rows = run!(&self.reader, |e| sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {REDIRECT_COLUMNS} FROM redirects \
             WHERE ?1 IS NULL OR (source_path, id) > (?1, ?2) \
             ORDER BY source_path, id LIMIT ?3"
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
        let done = run!(&self.writer, |e| sqlx::query(
            "UPDATE redirects SET hits = hits + 1, last_hit_at = ? WHERE id = ?"
        )
        .bind(Timestamp::now().as_micros())
        .bind(id.as_uuid())
        .execute(e))?;
        if done.rows_affected() == 0 {
            return Err(StoreError::NotFound);
        }
        Ok(())
    }
}
