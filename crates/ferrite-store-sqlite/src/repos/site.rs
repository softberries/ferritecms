//! Site-level repositories.

use async_trait::async_trait;
use ferrite_domain::{OptionEntry, Timestamp};
use ferrite_store::{OptionRepo, SchemaMetaRepo, StoreError};

use super::SqliteRepos;
use crate::{codec, exec::run};

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
