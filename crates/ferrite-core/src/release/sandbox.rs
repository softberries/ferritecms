//! [`SandboxStore`]: a read-only [`Store`] over one sandbox transaction (design 15
//! §5.2), so the real `SiteReader`, theme and router render the site as a release will
//! make it. Dropping it (or [`SandboxStore::finish`]) rolls the transaction back.

use std::{path::Path, sync::Arc, time::Duration};

use async_trait::async_trait;
use ferrite_domain::ContentId;
use ferrite_store::{
    Backend, Channel, ExportReader, ImportWriter, LockGuard, MigrateReport, MigrationStatus,
    PoolStats, Repos, Store, StoreError, Subscription, Tx,
};

use super::tx::{ApplyTx, Mode};

const READ_ONLY: StoreError = StoreError::Unsupported("preview is read-only");

/// What a sandbox applied and what it skipped (with the reason), for the preview bar.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SandboxReport {
    /// Entries applied.
    pub applied: u32,
    /// Entries not shown, with why.
    pub skipped: Vec<(ContentId, String)>,
    /// Entries applied over a newer version of their item.
    pub conflicts: Vec<ContentId>,
}

/// A [`Store`] whose `read()` is a sandbox [`ApplyTx`]; every write entry point is
/// refused.
pub struct SandboxStore {
    tx: ApplyTx,
    backend: Backend,
    origin: Arc<dyn Store>,
}

impl SandboxStore {
    /// Opens a sandbox transaction on `origin`.
    pub async fn begin(origin: Arc<dyn Store>) -> Result<Self, StoreError> {
        let inner = origin.begin().await?;
        Ok(Self {
            tx: ApplyTx::new(inner, Mode::Sandbox),
            backend: origin.backend(),
            origin,
        })
    }

    /// The sandbox transaction, for applying entries (savepoints through
    /// [`Tx::begin_nested`]).
    pub fn tx(&self) -> &ApplyTx {
        &self.tx
    }

    /// Rolls the sandbox back.
    pub async fn finish(self) -> Result<(), StoreError> {
        Box::new(self.tx).rollback().await
    }
}

#[async_trait]
impl Store for SandboxStore {
    fn backend(&self) -> Backend {
        self.backend
    }

    fn read(&self) -> &dyn Repos {
        &self.tx
    }

    fn pool_stats(&self) -> PoolStats {
        self.origin.pool_stats()
    }

    async fn begin(&self) -> Result<Box<dyn Tx>, StoreError> {
        Err(READ_ONLY)
    }

    async fn subscribe(
        &self,
        _channel: Channel,
    ) -> Result<Option<Box<dyn Subscription>>, StoreError> {
        Ok(None)
    }

    async fn try_lock(&self, _name: &str) -> Result<Option<Box<dyn LockGuard>>, StoreError> {
        Err(READ_ONLY)
    }

    async fn lock_wait(
        &self,
        _name: &str,
        _wait: Duration,
    ) -> Result<Option<Box<dyn LockGuard>>, StoreError> {
        Err(READ_ONLY)
    }

    async fn migration_status(
        &self,
        binary: &semver::Version,
    ) -> Result<MigrationStatus, StoreError> {
        self.origin.migration_status(binary).await
    }

    async fn migrate_up(&self, _to: Option<i64>) -> Result<MigrateReport, StoreError> {
        Err(READ_ONLY)
    }

    async fn migrate_down(&self, _to: i64, _allow_lossy: bool) -> Result<Vec<i64>, StoreError> {
        Err(READ_ONLY)
    }

    async fn verify_migrations(&self) -> Result<(), StoreError> {
        Err(READ_ONLY)
    }

    async fn export_reader(&self) -> Result<Box<dyn ExportReader>, StoreError> {
        Err(READ_ONLY)
    }

    async fn import_writer(&self) -> Result<Box<dyn ImportWriter>, StoreError> {
        Err(READ_ONLY)
    }

    async fn snapshot(&self, _dest: &Path) -> Result<(), StoreError> {
        Err(READ_ONLY)
    }

    async fn close(&self) {}
}
