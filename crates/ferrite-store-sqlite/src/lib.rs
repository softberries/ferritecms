//! SQLite storage backend: one writer connection, a reader pool, WAL, and a data-folder
//! lock (02-storage §6).

mod codec;
mod connect;
mod constraint;
mod exec;
mod export;
mod import_writer;
mod lock;
mod migrate;
mod repos;

use std::{fs::File, path::PathBuf};

use async_trait::async_trait;
use ferrite_store::{
    Backend, Channel, LockGuard, MigrationStatus, PoolStats, Repos, Store, StoreError,
    Subscription, Tx, check_lock_name,
};
use sqlx::SqlitePool;

pub use connect::{KeepSnapshot, SqliteOptions};
use exec::map_db_err;
pub use migrate::latest_schema_version;
pub use repos::{FIND_MEDIA_BY_ARTIFACT_SQL, PRUNE_IDEMPOTENCY_SQL, comment_list_sql};
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
    /// Never pruned (see [`SqliteOptions::keep_snapshot`]).
    keep_snapshot: Option<KeepSnapshot>,
    /// Named locks (`Store::try_lock`).
    locks: lock::Locks,
    _lock: File,
}

#[async_trait]
impl Store for SqliteStore {
    fn backend(&self) -> Backend {
        Backend::Sqlite
    }

    fn pool_stats(&self) -> PoolStats {
        let (idle, size) = [&self.reader, &self.writer]
            .into_iter()
            .fold((0u64, 0u64), |(idle, size), pool| {
                (idle + pool.num_idle() as u64, size + u64::from(pool.size()))
            });
        PoolStats {
            idle,
            used: size.saturating_sub(idle),
        }
    }

    fn read(&self) -> &dyn Repos {
        &self.read
    }

    async fn begin(&self) -> Result<Box<dyn Tx>, StoreError> {
        let tx = self.writer.begin().await.map_err(map_db_err)?;
        Ok(Box::new(SqliteTx::new(exec::TxHandle::new(tx))))
    }

    /// No subscription on SQLite (P1): one process owns the database, so it polls.
    async fn subscribe(
        &self,
        _channel: Channel,
    ) -> Result<Option<Box<dyn Subscription>>, StoreError> {
        Ok(None)
    }

    /// In-process mutex per name (P1): this process owns the data folder.
    async fn try_lock(&self, name: &str) -> Result<Option<Box<dyn LockGuard>>, StoreError> {
        check_lock_name(name)?;
        if self.writer.is_closed() {
            return Err(map_db_err(sqlx::Error::PoolClosed));
        }
        Ok(self.locks.try_lock(name))
    }

    async fn migration_status(
        &self,
        binary: &semver::Version,
    ) -> Result<MigrationStatus, StoreError> {
        self.status(binary).await
    }

    async fn migrate_up(
        &self,
        to: Option<i64>,
    ) -> Result<ferrite_store::MigrateReport, StoreError> {
        self.up(to).await
    }

    async fn migrate_down(&self, to: i64, allow_lossy: bool) -> Result<Vec<i64>, StoreError> {
        self.down(to, allow_lossy).await
    }

    async fn verify_migrations(&self) -> Result<(), StoreError> {
        self.verify().await
    }

    async fn export_reader(&self) -> Result<Box<dyn ferrite_store::ExportReader>, StoreError> {
        Ok(Box::new(
            export::SqliteExportReader::open(&self.reader).await?,
        ))
    }

    /// One transaction on the writer connection: other writes wait until the load ends.
    async fn import_writer(&self) -> Result<Box<dyn ferrite_store::ImportWriter>, StoreError> {
        let tx = self.writer.begin().await.map_err(map_db_err)?;
        Ok(Box::new(ferrite_store::Loader::new(
            import_writer::SqliteLoadTarget::new(tx),
        )))
    }

    async fn snapshot(&self, dest: &std::path::Path) -> Result<(), StoreError> {
        self.vacuum_into(dest).await
    }

    async fn close(&self) {
        // Readers first: the writer's close checkpoints the WAL as the last connection.
        self.reader.close().await;
        self.writer.close().await;
    }
}
