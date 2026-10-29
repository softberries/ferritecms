//! PostgreSQL storage backend: one pool, optional per-site schema (02-storage §6).

mod codec;
mod connect;
mod exec;
mod export;
mod import_writer;
mod listen;
mod lock;
mod migrate;
mod repos;

use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use ferrite_store::{
    Backend, Channel, LockGuard, MigrationStatus, PoolStats, Repos, Store, StoreError,
    Subscription, Tx, check_lock_name,
};
use sqlx::{PgPool, postgres::PgConnectOptions};

pub use connect::PgOptions;
use exec::map_db_err;
use listen::Channels;
pub use repos::{FIND_MEDIA_BY_ARTIFACT_SQL, PRUNE_IDEMPOTENCY_SQL, comment_list_sql};
use repos::{PgRepos, PgTx};

/// A PostgreSQL-backed [`Store`].
pub struct PgStore {
    pool: PgPool,
    /// Options for dedicated migration connections (no `statement_timeout`).
    connect: PgConnectOptions,
    /// Options for subscriptions: `<application_name>:listen`, no session settings.
    listen: PgConnectOptions,
    channels: Arc<Channels>,
    /// Dedicated connections for named locks (`<application_name>:lock:<name>`).
    locks: lock::Locks,
    /// `SET search_path ...` for dedicated connections when a schema is configured.
    search_path_sql: Option<String>,
    read: PgRepos,
}

#[async_trait]
impl Store for PgStore {
    fn backend(&self) -> Backend {
        Backend::Postgres
    }

    fn pool_stats(&self) -> PoolStats {
        let idle = self.pool.num_idle() as u64;
        PoolStats {
            idle,
            used: u64::from(self.pool.size()).saturating_sub(idle),
        }
    }

    fn read(&self) -> &dyn Repos {
        &self.read
    }

    async fn begin(&self) -> Result<Box<dyn Tx>, StoreError> {
        let tx = self.pool.begin().await.map_err(map_db_err)?;
        Ok(Box::new(PgTx::new(
            exec::TxHandle::new(tx),
            self.channels.clone(),
        )))
    }

    async fn subscribe(
        &self,
        channel: Channel,
    ) -> Result<Option<Box<dyn Subscription>>, StoreError> {
        listen::subscribe(self.listen.clone(), self.channels.name(channel))
            .await
            .map(Some)
    }

    async fn try_lock(&self, name: &str) -> Result<Option<Box<dyn LockGuard>>, StoreError> {
        check_lock_name(name)?;
        // The lock connection is not from the pool; keep `close` final for it too.
        if self.pool.is_closed() {
            return Err(map_db_err(sqlx::Error::PoolClosed));
        }
        self.locks.try_lock(name).await
    }

    async fn lock_wait(
        &self,
        name: &str,
        wait: Duration,
    ) -> Result<Option<Box<dyn LockGuard>>, StoreError> {
        check_lock_name(name)?;
        if self.pool.is_closed() {
            return Err(map_db_err(sqlx::Error::PoolClosed));
        }
        if wait.is_zero() {
            return self.locks.try_lock(name).await;
        }
        self.locks.lock_wait(name, wait).await
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

    async fn export_reader(&self) -> Result<Box<dyn ferrite_store::ExportReader>, StoreError> {
        if self.pool.is_closed() {
            return Err(map_db_err(sqlx::Error::PoolClosed));
        }
        let pool = self.export_pool().await?;
        Ok(Box::new(export::PgExportReader::open(pool).await?))
    }

    /// One transaction on a dedicated connection (`<application_name>:import`).
    async fn import_writer(&self) -> Result<Box<dyn ferrite_store::ImportWriter>, StoreError> {
        if self.pool.is_closed() {
            return Err(map_db_err(sqlx::Error::PoolClosed));
        }
        let pool = self.import_pool().await?;
        Ok(Box::new(ferrite_store::Loader::new(
            import_writer::PgLoadTarget::open(pool).await?,
        )))
    }

    async fn snapshot(&self, _dest: &std::path::Path) -> Result<(), StoreError> {
        // Would need pg_dump on the host (02 §7); operators use provider snapshots.
        Err(StoreError::Unsupported("snapshot"))
    }

    async fn close(&self) {
        self.pool.close().await;
    }
}
