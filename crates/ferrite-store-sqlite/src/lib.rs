//! SQLite storage backend: one writer connection, a reader pool, WAL, and a data-folder
//! lock (02-storage §6).

mod connect;
mod exec;
mod migrate;
mod repos;

use std::{fs::File, path::PathBuf};

use async_trait::async_trait;
use ferrite_store::{Backend, MigrationStatus, Repos, Store, StoreError, Tx};
use sqlx::SqlitePool;

pub use connect::SqliteOptions;
use exec::db_err;
use repos::{SqliteRepos, SqliteTx};

/// A SQLite-backed [`Store`]. Holds the data-folder lock until dropped.
pub struct SqliteStore {
    /// Max one connection; every write and every `Tx` goes through it.
    writer: SqlitePool,
    /// Read-only pool; also used for migration bookkeeping reads and snapshots.
    reader: SqlitePool,
    /// Autocommit repositories: reads on `reader`, writes on `writer`.
    read: SqliteRepos,
    backups_dir: PathBuf,
    _lock: File,
}

#[async_trait]
impl Store for SqliteStore {
    fn backend(&self) -> Backend {
        Backend::Sqlite
    }

    fn read(&self) -> &dyn Repos {
        &self.read
    }

    async fn begin(&self) -> Result<Box<dyn Tx>, StoreError> {
        let tx = self.writer.begin().await.map_err(db_err)?;
        Ok(Box::new(SqliteTx::new(tx)))
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

    async fn snapshot(&self, dest: &std::path::Path) -> Result<(), StoreError> {
        self.vacuum_into(dest).await
    }
}
