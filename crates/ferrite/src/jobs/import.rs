//! `import.wordpress` (M3-2 P27–P29): runs an import the admin queued through the API.
//!
//! One attempt: fetch the input artifacts to `<data>/import/job-<id>/`, run the import
//! as the payload's user (still an active admin), store the report (a failed run's
//! partial one too), and remove the job folder. The run's own work folder stays under
//! `<data>/import/` when it fails, so a retry on the same node resumes it; a retry on
//! another node parses again (P27). The inputs are deleted once the run is `done` or the
//! job fails for good: a `Permanent` error, or a `Retry` of the last attempt. A lease
//! that expires on the last attempt is turned `dead` by the runner's reaper without this
//! handler, so those inputs stay until the M4 orphan sweep (accepted, P23).
//!
//! Before it fetches anything an attempt checks that it could start a run
//! ([`ImportService::job_may_start`]), so a busy job does not copy its inputs every few
//! seconds. Each attempt also removes the `job-*` folders of jobs that are over (a
//! crashed node's leftovers), and the run work folders `<data>/import/<run-id>/` of runs
//! whose job is over. When the job fails for good, its run's work folder goes at once
//! (final review BE-m3). A CLI run's work folder (no job) is never swept: the next CLI
//! run of the same file resumes from it (M3-1 R27).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use ferrite_cluster::{JobError, JobHandler};
use ferrite_core::{
    CoreError, SwappablePermalinks,
    import::ImportService,
    jobs::{IMPORT_WORDPRESS_KIND, IMPORT_WORDPRESS_VERSION, ImportJobPayload},
};
use ferrite_domain::{
    ArtifactHash, ArtifactKind, ImportRunId, ImportRunState, Job, JobId, JobStatus,
};
use ferrite_http::import_fetch::HttpSource;
use ferrite_import_wp::ImportError;
use ferrite_import_wp::report::Report;
use ferrite_import_wp::run::{ImportContext, ImportOptions, MediaSource, RunFailed, run};
use ferrite_import_wp::zip_source::ZipSource;
use ferrite_media::{MediaError, MediaStore};
use ferrite_store::{Store, StoreError};
use futures::StreamExt;
use tokio::io::AsyncWriteExt;

use crate::import_cmd::Sources;

/// The longest one attempt may take (P27).
pub const IMPORT_TIMEOUT: Duration = Duration::from_secs(6 * 60 * 60);

/// `import.wordpress`: see the module docs. Errors map as P27 says: another live run is
/// [`JobError::Busy`]; a payload that does not parse, a user who is no longer an active
/// admin, a missing artifact, input the importer refuses (bad XML, a bad archive) and a
/// run taken over are `Permanent`; anything else is retried.
pub struct ImportWordpressHandler {
    pub store: Arc<dyn Store>,
    /// The node's service: the importer check and report storage.
    pub import: Arc<ImportService>,
    pub media: Arc<MediaStore>,
    /// The node's data folder (`<data>/import` holds the job and run folders).
    pub data_dir: PathBuf,
    /// The site's URL, without a trailing slash.
    pub site_url: String,
    /// `media.max_upload`.
    pub max_upload: u64,
    /// `media.strip_metadata`.
    pub strip_metadata: bool,
    /// The binary's version, for the `--fetch-media` user agent.
    pub version: String,
    /// Test seam: every attempt fails after this many batches.
    #[cfg(feature = "test-seams")]
    pub fail_after_batches: Option<u32>,
}

#[async_trait]
impl JobHandler for ImportWordpressHandler {
    fn kind(&self) -> &'static str {
        IMPORT_WORDPRESS_KIND
    }

    fn versions(&self) -> &'static [u32] {
        &[IMPORT_WORDPRESS_VERSION]
    }

    fn timeout(&self) -> Duration {
        IMPORT_TIMEOUT
    }

    async fn run(&self, job: &Job) -> Result<(), JobError> {
        let payload: ImportJobPayload = serde_json::from_value(job.payload.clone())
            .map_err(|e| JobError::Permanent(format!("invalid payload: {e}")))?;
        let import = self.data_dir.join("import");
        self.sweep_job_folders(&import, job.id).await;
        self.sweep_run_folders(&import).await;
        let folder = import.join(format!("job-{}", job.id));
        let outcome = self.attempt(job.id, &payload, &folder).await;
        if let Err(e) = tokio::fs::remove_dir_all(&folder).await
            && e.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(error = %e, dir = %folder.display(), "removing the import job's folder failed");
        }
        // The inputs are spent once the run is done or the job fails for good (P23): the
        // runner turns a `Retry` of the last attempt (the claim counted it) `dead`.
        let last = job.attempts >= job.max_attempts;
        let spent = match &outcome {
            Ok(()) | Err(JobError::Permanent(_)) => true,
            Err(JobError::Retry(_)) => last,
            Err(JobError::Busy(_)) => false,
        };
        if spent {
            self.delete_inputs(&payload).await;
            if outcome.is_err() {
                self.remove_run_folder(&import, job.id).await;
            }
        }
        outcome
    }
}

impl ImportWordpressHandler {
    async fn attempt(
        &self,
        job: JobId,
        payload: &ImportJobPayload,
        folder: &Path,
    ) -> Result<(), JobError> {
        let actor = self
            .import
            .importer(payload.user)
            .await
            .map_err(core_error)?;
        if !self.import.job_may_start(job).await.map_err(core_error)? {
            return Err(JobError::Busy("another import is running".into()));
        }
        // In the runner's `job` span, which names the job and the node.
        tracing::info!(user = %payload.user, "import.wordpress started");
        // Fresh copies: an earlier attempt's may be partial.
        match tokio::fs::remove_dir_all(folder).await {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                return Err(JobError::Retry(format!("{}: {e}", folder.display())));
            }
            _ => {}
        }
        tokio::fs::create_dir_all(folder)
            .await
            .map_err(|e| JobError::Retry(format!("{}: {e}", folder.display())))?;
        let wxr = folder.join("export.xml");
        if !self.fetch(&payload.wxr, &wxr).await? {
            return self.missing(job, &payload.wxr).await;
        }
        let mut sources: Vec<Arc<dyn MediaSource>> = Vec::new();
        if let Some(uploads) = &payload.uploads {
            let archive = folder.join("uploads.zip");
            if !self.fetch(uploads, &archive).await? {
                return self.missing(job, uploads).await;
            }
            let zip = tokio::task::spawn_blocking(move || ZipSource::open(&archive))
                .await
                .map_err(|e| JobError::Retry(format!("reading the uploads archive: {e}")))?
                .map_err(import_error)?;
            sources.push(Arc::new(zip));
        }
        let o = &payload.options;
        if o.fetch_media {
            sources.push(Arc::new(
                HttpSource::new(&self.version).map_err(JobError::Retry)?,
            ));
        }
        // The run's own rules, never the node's: a dry run swaps in rolled-back ones.
        let rules = ferrite_core::permalink::rules_from(self.store.read(), &[])
            .await
            .map_err(core_error)?;
        let permalinks = Arc::new(SwappablePermalinks::new(rules));
        let ctx = ImportContext {
            import: Arc::new(ImportService::new(self.store.clone(), permalinks.clone())),
            permalinks,
            media_store: self.media.clone(),
            actor,
            work_dir: self.data_dir.join("import"),
            media_source: Arc::new(Sources(sources)),
            site_url: self.site_url.clone(),
        };
        let opts = ImportOptions {
            dry_run: o.dry_run,
            keep_permalinks: o.keep_permalinks,
            site_options: o.site_options,
            category_base: o.category_base.clone(),
            tag_base: o.tag_base.clone(),
            // A job's start decides the takeover itself (P27 as built).
            takeover: false,
            max_upload: self.max_upload,
            strip_metadata: self.strip_metadata,
            job_id: Some(job),
            #[cfg(feature = "test-seams")]
            fail_after_batches: self.fail_after_batches,
            ..ImportOptions::default()
        };
        match run(ctx, &wxr, opts).await {
            Ok(report) => {
                self.store_report(&report).await;
                // A failed attempt is logged by the runner (`job attempt failed`).
                tracing::info!(run = %report.run_id, "import.wordpress done");
                Ok(())
            }
            Err(RunFailed { error, report }) => {
                if let Some(report) = &report {
                    self.store_report(report).await;
                }
                // A work file that does not read back would fail every resume: drop it,
                // so the next attempt parses again.
                if let (ImportError::Record(_), Some(report)) = (&error, &report) {
                    let dir = self.data_dir.join("import").join(&report.run_id);
                    if let Err(e) = tokio::fs::remove_dir_all(&dir).await {
                        tracing::warn!(error = %e, dir = %dir.display(), "removing a corrupt import work folder failed");
                    }
                }
                Err(import_error(error))
            }
        }
    }

    /// An input is missing: `done` when this job's run already is (the job was dropped
    /// while deleting its inputs), else `Permanent`.
    async fn missing(&self, job: JobId, hash: &str) -> Result<(), JobError> {
        let run = self
            .store
            .read()
            .imports()
            .run_by_job(job)
            .await
            .map_err(|e| JobError::Retry(e.to_string()))?;
        if run.is_some_and(|r| r.state == ImportRunState::Done) {
            tracing::info!(%job, "the import's run is done and its inputs gone");
            return Ok(());
        }
        Err(JobError::Permanent(format!(
            "the import artifact {hash} is missing"
        )))
    }

    /// Removes the `job-*` folders under `import` of jobs that are over (not queued or
    /// running, or gone); this job's own is left to it.
    async fn sweep_job_folders(&self, import: &Path, current: JobId) {
        let Ok(mut entries) = tokio::fs::read_dir(import).await else {
            return;
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let name = entry.file_name();
            let Some(id) = name
                .to_str()
                .and_then(|n| n.strip_prefix("job-"))
                .and_then(|id| id.parse::<JobId>().ok())
            else {
                continue;
            };
            if id == current {
                continue;
            }
            let over = match self.store.read().jobs().get(id).await {
                Ok(job) => {
                    job.is_none_or(|j| matches!(j.status, JobStatus::Done | JobStatus::Dead))
                }
                Err(e) => {
                    tracing::warn!(error = %e, %id, "reading an import job failed; its folder stays");
                    false
                }
            };
            if over && let Err(e) = tokio::fs::remove_dir_all(entry.path()).await {
                tracing::warn!(error = %e, dir = %entry.path().display(), "removing a leftover import job folder failed");
            }
        }
    }

    /// Removes the work folders under `import` of runs that are not `running` and whose
    /// job is over (not queued or running, or gone). Folders of CLI runs (no job) and of
    /// runs whose job may retry stay; so does any folder whose run cannot be read.
    async fn sweep_run_folders(&self, import: &Path) {
        let Ok(mut entries) = tokio::fs::read_dir(import).await else {
            return;
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let Some(id) = entry
                .file_name()
                .to_str()
                .and_then(|n| n.parse::<ImportRunId>().ok())
            else {
                continue;
            };
            let run = match self.store.read().imports().get_run(id).await {
                Ok(Some(run)) => run,
                Ok(None) => continue,
                Err(e) => {
                    tracing::warn!(error = %e, run = %id, "reading an import run failed; its folder stays");
                    continue;
                }
            };
            let Some(job) = run.job_id.filter(|_| run.state != ImportRunState::Running) else {
                continue;
            };
            let over = match self.store.read().jobs().get(job).await {
                Ok(job) => {
                    job.is_none_or(|j| matches!(j.status, JobStatus::Done | JobStatus::Dead))
                }
                Err(e) => {
                    tracing::warn!(error = %e, %job, "reading an import job failed; its run's folder stays");
                    false
                }
            };
            if over && let Err(e) = tokio::fs::remove_dir_all(entry.path()).await {
                tracing::warn!(error = %e, dir = %entry.path().display(), "removing an import run's work folder failed");
            }
        }
    }

    /// Removes the work folder of job `job`'s newest run (the job failed for good, so no
    /// attempt resumes it).
    async fn remove_run_folder(&self, import: &Path, job: JobId) {
        let run = match self.store.read().imports().run_by_job(job).await {
            Ok(Some(run)) => run,
            Ok(None) => return,
            Err(e) => {
                tracing::warn!(error = %e, %job, "reading the import job's run failed; its folder stays");
                return;
            }
        };
        let dir = import.join(run.id.to_string());
        if let Err(e) = tokio::fs::remove_dir_all(&dir).await
            && e.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(error = %e, dir = %dir.display(), "removing an import run's work folder failed");
        }
    }

    /// Copies artifact `hash` to `to`, streaming; `false` when it is missing.
    async fn fetch(&self, hash: &str, to: &Path) -> Result<bool, JobError> {
        let hash = ArtifactHash::parse(hash)
            .map_err(|_| JobError::Permanent(format!("invalid artifact hash {hash:?}")))?;
        let mut stream = match self.media.open_read(&hash, None).await {
            Ok(read) => read.stream,
            Err(MediaError::NotFound) => return Ok(false),
            Err(e) => return Err(JobError::Retry(format!("reading artifact {hash}: {e}"))),
        };
        let write_err = |e: std::io::Error| JobError::Retry(format!("{}: {e}", to.display()));
        let mut file = tokio::fs::File::create(to).await.map_err(write_err)?;
        while let Some(chunk) = stream.next().await {
            let chunk =
                chunk.map_err(|e| JobError::Retry(format!("reading artifact {hash}: {e}")))?;
            file.write_all(&chunk).await.map_err(write_err)?;
        }
        file.flush().await.map_err(write_err)?;
        Ok(true)
    }

    /// Stores `report` as the run's report artifacts; a failure is logged (the import
    /// itself is not redone for its report).
    async fn store_report(&self, report: &Report) {
        let Ok(run) = report.run_id.parse() else {
            tracing::warn!(run = %report.run_id, "an import report without a run id");
            return;
        };
        let json = Bytes::from(report.to_json());
        let html = Bytes::from(report.to_html());
        if let Err(e) = self.import.store_report(run, json, html).await {
            tracing::warn!(error = %e, %run, "storing the import report failed");
        }
    }

    /// Deletes the payload's input artifacts, the row first and the blob only once that
    /// succeeded: a media upload of the same bytes shares the row (`insert_if_absent`
    /// kept kind `import`), and the foreign keys refuse to delete a row a media item,
    /// plugin or theme references, so its file stays. A blob whose delete fails after its
    /// row went is left for the M4 orphan sweep. Never one of another kind.
    async fn delete_inputs(&self, payload: &ImportJobPayload) {
        for hash in std::iter::once(&payload.wxr).chain(&payload.uploads) {
            let Ok(hash) = ArtifactHash::parse(hash) else {
                continue;
            };
            let repos = self.store.read();
            let artifacts = repos.artifacts();
            match artifacts.get(&hash).await {
                Ok(Some(row)) if row.kind != ArtifactKind::Import => {
                    tracing::warn!(%hash, kind = ?row.kind, "an import input of another kind is kept");
                    continue;
                }
                Ok(_) => {}
                Err(e) => {
                    tracing::warn!(error = %e, %hash, "reading an import input failed; kept");
                    continue;
                }
            }
            if let Err(e) = artifacts.delete(&hash).await {
                input_row_kept(&e, &hash);
                continue;
            }
            if let Err(e) = self.media.delete(&hash).await {
                tracing::warn!(error = %e, %hash, "deleting an import input's file failed; it waits for the orphan sweep");
            }
        }
    }
}

/// Logs why the row of import input `hash` was not deleted (its file stays): the
/// expected foreign-key refusal (a media item, plugin or theme shares the row) at
/// `info`, any other store error at `warn`.
fn input_row_kept(e: &StoreError, hash: &ArtifactHash) {
    match e {
        StoreError::ForeignKey { .. } => {
            tracing::info!(error = %e, %hash, "an import input's row is still referenced; its file stays");
        }
        _ => {
            tracing::warn!(error = %e, %hash, "deleting an import input's row failed; its file stays");
        }
    }
}

/// A core error of the job's own checks (P27).
fn core_error(e: CoreError) -> JobError {
    match e {
        CoreError::Unavailable(_) => JobError::Busy(e.to_string()),
        CoreError::Changed(_)
        | CoreError::Forbidden(_)
        | CoreError::Invalid(_)
        | CoreError::InvalidField { .. } => JobError::Permanent(e.to_string()),
        other => JobError::Retry(other.to_string()),
    }
}

/// An import's error (P27): busy is `Busy`; input the importer refuses and a run taken
/// over are `Permanent`; the rest is retried.
fn import_error(e: ImportError) -> JobError {
    match e {
        ImportError::Core(core) => core_error(core),
        ImportError::Xml { .. }
        | ImportError::Doctype
        | ImportError::TooLarge { .. }
        | ImportError::Utf8 { .. }
        | ImportError::Missing { .. }
        | ImportError::Archive { .. } => JobError::Permanent(e.to_string()),
        other => JobError::Retry(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_map_as_p27_says() {
        let permanent = |e: JobError| matches!(e, JobError::Permanent(_));
        assert!(matches!(
            import_error(ImportError::Core(CoreError::Unavailable("busy"))),
            JobError::Busy(_)
        ));
        assert!(permanent(import_error(ImportError::Core(
            CoreError::Changed("taken over")
        ))));
        assert!(permanent(import_error(ImportError::Xml {
            offset: 0,
            msg: "bad".into()
        })));
        assert!(permanent(import_error(ImportError::Archive {
            path: "u.zip".into(),
            msg: "bad".into()
        })));
        assert!(permanent(core_error(CoreError::Forbidden(
            "an active admin"
        ))));
        assert!(matches!(
            import_error(ImportError::Stopped),
            JobError::Retry(_)
        ));
        assert!(matches!(
            import_error(ImportError::Io {
                offset: 0,
                msg: "disk".into()
            }),
            JobError::Retry(_)
        ));
    }

    #[test]
    fn timeout_is_six_hours() {
        assert_eq!(IMPORT_TIMEOUT, Duration::from_secs(21_600));
    }

    /// The log lines `f` writes, as plain text (a scoped subscriber at `info`).
    fn logs(f: impl FnOnce()) -> String {
        #[derive(Clone, Default)]
        struct Buf(Arc<std::sync::Mutex<Vec<u8>>>);
        impl std::io::Write for Buf {
            fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
                self.0.lock().expect("log buffer").extend_from_slice(data);
                Ok(data.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let buf = Buf::default();
        let writer = buf.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(move || writer.clone())
            .with_ansi(false)
            .with_max_level(tracing::Level::INFO)
            .finish();
        tracing::subscriber::with_default(subscriber, f);
        let bytes = buf.0.lock().expect("log buffer").clone();
        String::from_utf8(bytes).expect("utf-8 logs")
    }

    /// The foreign-key refusal (a media item shares the input's row) is expected and
    /// logged at `info`; any other database error deleting the row is a `warn`.
    #[test]
    fn an_input_row_delete_failure_is_a_warning() {
        let hash = ArtifactHash::parse(&"ab".repeat(32)).expect("hash");
        let fk = logs(|| {
            input_row_kept(
                &StoreError::ForeignKey {
                    constraint: "".into(),
                },
                &hash,
            )
        });
        assert!(fk.contains(" INFO "), "{fk}");
        assert!(fk.contains("still referenced"), "{fk}");
        let db = logs(|| input_row_kept(&StoreError::Database("disk I/O error".into()), &hash));
        assert!(db.contains(" WARN "), "{db}");
        assert!(db.contains("deleting an import input's row failed"), "{db}");
        assert!(db.contains("disk I/O error"), "{db}");
        assert!(db.contains(&hash.to_string()), "{db}");
    }
}
