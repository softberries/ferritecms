//! PostgreSQL storage backend: one pool, optional per-site schema (02-storage §6).

mod codec;
mod connect;
mod exec;
mod migrate;
mod repos;

use async_trait::async_trait;
use ferrite_store::{Backend, MigrationStatus, Repos, Store, StoreError, Tx};
use sqlx::{PgPool, postgres::PgConnectOptions};

pub use connect::PgOptions;
use exec::map_db_err;
use repos::{PgRepos, PgTx};

/// A PostgreSQL-backed [`Store`].
pub struct PgStore {
    pool: PgPool,
    /// Options for dedicated migration connections (no `statement_timeout`).
    connect: PgConnectOptions,
    /// `SET search_path ...` for dedicated connections when a schema is configured.
    search_path_sql: Option<String>,
    read: PgRepos,
}

#[async_trait]
impl Store for PgStore {
    fn backend(&self) -> Backend {
        Backend::Postgres
    }

    fn read(&self) -> &dyn Repos {
        &self.read
    }

    async fn begin(&self) -> Result<Box<dyn Tx>, StoreError> {
        let tx = self.pool.begin().await.map_err(map_db_err)?;
        Ok(Box::new(PgTx::new(tx)))
    }

    async fn migration_status(
        &self,
        binary: &semver::Version,
    ) -> Result<MigrationStatus, StoreError> {
        self.status(binary).await
    }

    async fn migrate_up(&self, to: Option<i64>) -> Result<Vec<i64>, StoreError> {
        self.up(to).await
    }

    async fn migrate_down(&self, to: i64, allow_lossy: bool) -> Result<Vec<i64>, StoreError> {
        self.down(to, allow_lossy).await
    }

    async fn verify_migrations(&self) -> Result<(), StoreError> {
        self.verify().await
    }

    async fn snapshot(&self, _dest: &std::path::Path) -> Result<(), StoreError> {
        // Would need pg_dump on the host (02 §7); operators use provider snapshots.
        Err(StoreError::Unsupported("snapshot"))
    }
}
