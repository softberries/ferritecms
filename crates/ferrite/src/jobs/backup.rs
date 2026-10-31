//! `site.backup` (M4-1 P15): the scheduled backup `serve` runs when `[backup] schedule` is
//! not `off`. On SQLite it is the P13 repository backup ([`crate::backup::run_backup`],
//! next to the live writers); on Postgres a P7 export archive uploaded to
//! `<to>/exports/` ([`crate::backup::run_export_backup`]). One run at a time holds the
//! `site.backup` store lock; the period's unique job key makes one node run it per period.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, bail};
use async_trait::async_trait;
use ferrite_cluster::{JobError, JobHandler, Periodic};
use ferrite_config::{ClusterMode, Config};
use ferrite_domain::Job;
use ferrite_media::MediaStore;
use ferrite_store::{Backend, LOCK_BACKUP, Store};
use serde_json::json;

use crate::backup::target::{RepoLocation, Target};
use crate::backup::{
    BACKUP_GC_WAIT, BackupOptions, GcBusy, check_repo_location, run_backup, run_export_backup,
};

/// The job kind.
pub const SITE_BACKUP_KIND: &str = "site.backup";
/// Longest one run may take.
pub const SITE_BACKUP_TIMEOUT: Duration = Duration::from_secs(21_600);
/// Attempts each period's run gets (a failure is retried).
pub const SITE_BACKUP_MAX_ATTEMPTS: i32 = 3;
/// Missing artifacts named in the log line of a run that lacks some (R10); the count is
/// always logged.
const MISSING_LOGGED: usize = 20;

/// `site.backup`: one scheduled backup of the site to `[backup] to`, keeping the newest
/// `keep`. Another run holding the `site.backup` lock, or an artifacts GC pass holding
/// `artifacts.gc` past the wait (R17), is [`JobError::Busy`]; any other failure is
/// `Retry` (up to [`SITE_BACKUP_MAX_ATTEMPTS`]). A run that completes without some blobs
/// (missing from the media store, R10) succeeds with a warning.
pub struct SiteBackupHandler {
    config: Config,
    store: Arc<dyn Store>,
    /// `None`: `with_media = false`.
    media: Option<Arc<MediaStore>>,
    repo: RepoLocation,
    keep: usize,
    /// How long a run waits for a running artifacts GC (R17).
    gc_wait: Duration,
}

/// The `site.backup` handler and its periodic for `config`, or `None` when `[backup]
/// schedule` is `off`. Fails when `backup.to` is not a repository location this site may
/// use (an archive path, or a folder inside the data or media folder, symlinks resolved).
pub fn site_backup(
    config: &Config,
    store: Arc<dyn Store>,
    media: Arc<MediaStore>,
) -> anyhow::Result<Option<(SiteBackupHandler, Periodic)>> {
    let backup = &config.backup;
    let Some(every) = backup.schedule.every_secs() else {
        return Ok(None);
    };
    let repo = backup_location(config)
        .context("backup.to")?
        .context("backup.to is required unless backup.schedule is \"off\"")?;
    crate::backup::check_backup_database(config).context("database.url")?;
    if let RepoLocation::Dir(dir) = &repo
        && config.effective_cluster_mode() == ClusterMode::Cluster
    {
        tracing::warn!(
            to = %dir.display(),
            "backup.to is a local folder in cluster mode: each backup lands on whichever node \
             runs it; use a shared folder or s3://"
        );
    }
    let handler = SiteBackupHandler {
        config: config.clone(),
        store,
        media: backup.with_media.then_some(media),
        repo,
        keep: backup.keep as usize,
        gc_wait: BACKUP_GC_WAIT,
    };
    let periodic = Periodic {
        kind: SITE_BACKUP_KIND,
        every: Duration::from_secs(every),
        payload: json!({"v": 1}),
        max_attempts: SITE_BACKUP_MAX_ATTEMPTS,
        // At the period start: 00:00 UTC (Thursdays for weekly); `artifacts.gc` runs at
        // 03:00 (M4-4 P75).
        offset: Duration::ZERO,
    };
    Ok(Some((handler, periodic)))
}

/// The repository `backup.to` names (`None` without one), checked as `serve` checks it:
/// a folder or `s3://`, not inside the data folder or the media store (symlinks resolved
/// where the paths exist), and a local folder absent, empty or already a repository (R11;
/// an S3 prefix is checked when the job runs). No database or network access, so `config
/// check` runs it too.
pub(crate) fn backup_location(config: &Config) -> anyhow::Result<Option<RepoLocation>> {
    let Some(to) = config.backup.to.as_deref() else {
        return Ok(None);
    };
    let Target::Repo(repo) = Target::parse(to, &config.data_dir)? else {
        bail!("must be a folder or s3://bucket[/prefix]");
    };
    check_repo_location(config, &repo)?;
    // A local folder is checked now (B-I3); an S3 prefix only when the job runs (no
    // network here).
    if let RepoLocation::Dir(dir) = &repo {
        crate::backup::repo::check_dir(dir)?;
    }
    Ok(Some(repo))
}

impl SiteBackupHandler {
    /// Waits up to `wait` (instead of [`BACKUP_GC_WAIT`]) for a running artifacts GC.
    pub fn with_gc_wait(mut self, wait: Duration) -> Self {
        self.gc_wait = wait;
        self
    }

    async fn backup(&self) -> anyhow::Result<()> {
        let store = self.store.as_ref();
        match store.backend() {
            Backend::Sqlite => {
                let opts = BackupOptions {
                    secret_key: true,
                    keep: Some(self.keep),
                    gc_wait: self.gc_wait,
                    #[cfg(feature = "test-seams")]
                    corrupt_snapshot: false,
                };
                let target = Target::Repo(self.repo.clone());
                let report =
                    run_backup(&self.config, store, self.media.clone(), &target, &opts).await?;
                let blobs = report.blobs.clone().unwrap_or_default();
                let pruned = report.pruned.clone().unwrap_or_default();
                tracing::info!(
                    location = %report.location,
                    schema_version = report.schema_version,
                    db_bytes = report.db_bytes,
                    files = %report.files.join(","),
                    media = report.blobs.is_some(),
                    blobs_copied = blobs.written,
                    blobs_present = blobs.present,
                    blob_bytes = blobs.bytes,
                    pruned_snapshots = pruned.snapshots.len(),
                    pruned_artifacts = pruned.artifacts,
                    "scheduled backup done"
                );
                let missing: Vec<String> = report.missing.iter().map(ToString::to_string).collect();
                warn_missing(&report.location, &missing);
            }
            Backend::Postgres => {
                let report = run_export_backup(
                    &self.config,
                    store,
                    self.media.clone(),
                    &self.repo,
                    self.keep,
                    self.gc_wait,
                )
                .await?;
                tracing::info!(
                    location = %report.location,
                    schema_version = report.schema_version,
                    bytes = report.bytes,
                    tables = report.tables,
                    rows = report.rows,
                    artifacts = report.artifacts.as_ref().map_or(0, |a| a.count),
                    artifact_bytes = report.artifacts.as_ref().map_or(0, |a| a.bytes),
                    pruned = report.pruned.len(),
                    "scheduled export done"
                );
                warn_missing(&report.location, &report.missing);
            }
        }
        Ok(())
    }
}

/// R10: a backup without some blobs is still a success (retrying would not bring them
/// back); the count and the first hashes are logged.
fn warn_missing(location: &str, missing: &[String]) {
    if missing.is_empty() {
        return;
    }
    let shown = missing[..missing.len().min(MISSING_LOGGED)].join(",");
    tracing::warn!(
        location,
        missing = missing.len(),
        hashes = %shown,
        "the backup lacks artifacts missing from the media store (listed in its manifest's \
         `missing`)"
    );
}

#[async_trait]
impl JobHandler for SiteBackupHandler {
    fn kind(&self) -> &'static str {
        SITE_BACKUP_KIND
    }

    fn timeout(&self) -> Duration {
        SITE_BACKUP_TIMEOUT
    }

    async fn run(&self, _job: &Job) -> Result<(), JobError> {
        let guard = match self.store.try_lock(LOCK_BACKUP).await {
            Ok(Some(guard)) => guard,
            Ok(None) => return Err(JobError::Busy("another backup is running".into())),
            Err(e) => return Err(JobError::Retry(format!("cannot take the backup lock: {e}"))),
        };
        let result = self.backup().await;
        if let Err(e) = guard.release().await {
            tracing::warn!(error = %e, "cannot release the backup lock");
        }
        // The runner logs the failed attempt with this message. A running GC pass (R17)
        // is no failure: the job comes back shortly, its attempt refunded.
        result.map_err(|e| {
            if e.downcast_ref::<GcBusy>().is_some() {
                JobError::Busy(e.to_string())
            } else {
                JobError::Retry(format!("{e:#}"))
            }
        })
    }
}
