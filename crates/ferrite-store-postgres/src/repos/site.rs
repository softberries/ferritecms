//! Site-level repositories.

use async_trait::async_trait;
use ferrite_store::{SchemaMetaRepo, StoreError};

use super::PgRepos;
use crate::exec::run;

#[async_trait]
impl SchemaMetaRepo for PgRepos {
    async fn get(&self, key: &str) -> Result<Option<String>, StoreError> {
        run!(&self.exec, |e| sqlx::query_scalar(
            "SELECT value FROM schema_meta WHERE key = $1"
        )
        .bind(key)
        .fetch_optional(e))
    }

    async fn set(&self, key: &str, value: &str) -> Result<(), StoreError> {
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
