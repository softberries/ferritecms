//! Site-level repositories.

use async_trait::async_trait;
use ferrite_store::{SchemaMetaRepo, StoreError};

use super::SqliteRepos;
use crate::exec::run;

#[async_trait]
impl SchemaMetaRepo for SqliteRepos {
    async fn get(&self, key: &str) -> Result<Option<String>, StoreError> {
        run!(&self.exec, |e| sqlx::query_scalar(
            "SELECT value FROM schema_meta WHERE key = ?"
        )
        .bind(key)
        .fetch_optional(e))
    }

    async fn set(&self, key: &str, value: &str) -> Result<(), StoreError> {
        run!(&self.exec, |e| sqlx::query(
            "INSERT INTO schema_meta (key, value) VALUES (?, ?) \
             ON CONFLICT (key) DO UPDATE SET value = excluded.value"
        )
        .bind(key)
        .bind(value)
        .execute(e))
        .map(|_| ())
    }
}
