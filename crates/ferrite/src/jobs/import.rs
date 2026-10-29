//! `import.wordpress` (M3-2 P27–P29): runs an import the admin queued through the API.
//!
//! One attempt: fetch the input artifacts to `<data>/import/job-<id>/`, run the import
//! as the payload's user (still an active admin), store the report (a failed run's
//! partial one too), and remove the job folder. The run's own work folder stays under
//! `<data>/import/` when it fails, so a retry on the same node resumes it; a retry on
//! another node parses again (P27). The inputs are deleted once the run is `done` or the
//! job fails for good.

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
use ferrite_domain::{ArtifactHash, ArtifactKind, Job, JobId};
use ferrite_http::import_fetch::HttpSource;
use ferrite_import_wp::ImportError;
use ferrite_import_wp::report::Report;
use ferrite_import_wp::run::{ImportContext, ImportOptions, MediaSource, RunFailed, run};
use ferrite_import_wp::zip_source::ZipSource;
use ferrite_media::{MediaError, MediaStore};
use ferrite_store::Store;
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
        let folder = self.data_dir.join("import").join(format!("job-{}", job.id));
        let outcome = self.attempt(job.id, &payload, &folder).await;
        if let Err(e) = tokio::fs::remove_dir_all(&folder).await
            && e.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(error = %e, dir = %folder.display(), "removing the import job's folder failed");
        }
        // The inputs are spent once the run is done or the job fails for good (P23).
        if matches!(outcome, Ok(()) | Err(JobError::Permanent(_))) {
            self.delete_inputs(&payload).await;
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
        self.fetch(&payload.wxr, &wxr).await?;
        let mut sources: Vec<Arc<dyn MediaSource>> = Vec::new();
        if let Some(uploads) = &payload.uploads {
            let archive = folder.join("uploads.zip");
            self.fetch(uploads, &archive).await?;
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
            takeover: self.owns_stale_run(job).await?,
            max_upload: self.max_upload,
            job_id: Some(job),
            #[cfg(feature = "test-seams")]
            fail_after_batches: self.fail_after_batches,
            ..ImportOptions::default()
        };
        match run(ctx, &wxr, opts).await {
            Ok(report) => {
                self.store_report(&report).await;
                Ok(())
            }
            Err(RunFailed { error, report }) => {
                if let Some(report) = report {
                    self.store_report(&report).await;
                }
                Err(import_error(error))
            }
        }
    }

    /// Whether the running run is this job's own (an earlier attempt that stopped):
    /// only then may this attempt take it over, and only once it is stale (P27).
    async fn owns_stale_run(&self, job: JobId) -> Result<bool, JobError> {
        let running = self
            .store
            .read()
            .imports()
            .running()
            .await
            .map_err(|e| JobError::Retry(e.to_string()))?;
        Ok(running.is_some_and(|r| r.job_id == Some(job)))
    }

    /// Copies artifact `hash` to `to`, streaming; a missing one is `Permanent`.
    async fn fetch(&self, hash: &str, to: &Path) -> Result<(), JobError> {
        let hash = ArtifactHash::parse(hash)
            .map_err(|_| JobError::Permanent(format!("invalid artifact hash {hash:?}")))?;
        let mut stream = match self.media.open_read(&hash, None).await {
            Ok(read) => read.stream,
            Err(MediaError::NotFound) => {
                return Err(JobError::Permanent(format!(
                    "the import artifact {hash} is missing"
                )));
            }
            Err(e) => return Err(JobError::Retry(format!("reading artifact {hash}: {e}"))),
        };
        let write_err = |e: std::io::Error| JobError::Retry(format!("{}: {e}", to.display()));
        let mut file = tokio::fs::File::create(to).await.map_err(write_err)?;
        while let Some(chunk) = stream.next().await {
            let chunk =
                chunk.map_err(|e| JobError::Retry(format!("reading artifact {hash}: {e}")))?;
            file.write_all(&chunk).await.map_err(write_err)?;
        }
        file.flush().await.map_err(write_err)
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

    /// Deletes the payload's input artifacts (row and blob); never one of another kind.
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
                tracing::warn!(error = %e, %hash, "deleting an import input's row failed");
                continue;
            }
            if let Err(e) = self.media.delete(&hash).await {
                tracing::warn!(error = %e, %hash, "deleting an import input's file failed");
            }
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
}
