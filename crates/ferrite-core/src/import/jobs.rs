//! Import jobs and what reads runs (M3-2 P25, P26-1, P27, P29): the enqueue with its
//! conflict rule, the job's importer check, the run and job reads, and report artifacts.
//!
//! **Reports.** `report.json` and `report.html` are stored as `import` artifacts, and
//! the run's `report_hash` names a third one, a small JSON manifest
//! `{"v":1,"json":"<sha256>","html":"<sha256>"}` (one column, no migration). A later
//! attempt stores a new manifest; the files it replaces stay until the store is cleaned.

use bytes::Bytes;
use ferrite_domain::{
    Artifact, ArtifactHash, ArtifactKind, ImportRun, ImportRunId, Job, JobId, JobQuery, JobStatus,
    Role, Timestamp, UserId, UserStatus,
};
use ferrite_store::LOCK_IMPORT;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{ArtifactBlobs, ImportService, release};
use crate::{
    Actor, CoreError,
    actor::{Capability, require, require_reader},
    jobs::{
        IMPORT_WORDPRESS_KIND, IMPORT_WORDPRESS_VERSION, ImportJobPayload, import_wordpress_job,
    },
    tx::finish,
};

/// Refused enqueue (P25): another import is queued or running.
const ALREADY: &str = "an import is already queued or running";
/// The importer check's refusal (P27).
const ACTIVE_ADMIN: &str = "an active admin";
/// Current version of the report manifest.
const MANIFEST_VERSION: u32 = 1;

/// An import job and the run it started, if it got that far (`GET /import/jobs/{id}`).
#[derive(Debug, Clone, PartialEq)]
pub struct ImportJobView {
    pub job: Job,
    pub run: Option<ImportRun>,
}

/// Which report file to read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportFormat {
    Json,
    Html,
}

/// The artifact `report_hash` names.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    v: u32,
    json: String,
    html: String,
}

impl ImportService {
    /// Queues an `import.wordpress` job for `payload` (P25, P27). Needs
    /// [`Capability::Import`] with `import:write`. While an `import.wordpress` job is
    /// queued or running, or a run is `running`, this is [`CoreError::Changed`] (the
    /// API's `409`). The check and the insert are one transaction; the import lock is
    /// taken around it so two enqueues cannot both pass the check (held elsewhere: the
    /// same refusal).
    pub async fn enqueue(
        &self,
        actor: &Actor,
        payload: ImportJobPayload,
    ) -> Result<JobId, CoreError> {
        require(actor, Capability::Import)?;
        if payload.v != IMPORT_WORDPRESS_VERSION {
            return Err(CoreError::Invalid(format!(
                "unknown import job version {}",
                payload.v
            )));
        }
        let lock = match self.store.try_lock(LOCK_IMPORT).await {
            Ok(Some(guard)) => Some(guard),
            Ok(None) => return Err(CoreError::Changed(ALREADY)),
            Err(e) => {
                tracing::warn!(error = %e, "taking the import lock failed; relying on the check");
                None
            }
        };
        let done = self.enqueue_checked(&payload).await;
        release(lock).await;
        done
    }

    async fn enqueue_checked(&self, payload: &ImportJobPayload) -> Result<JobId, CoreError> {
        let tx = self.store.begin().await?;
        let done = async {
            if tx.imports().running().await?.is_some() {
                return Err(CoreError::Changed(ALREADY));
            }
            for status in [JobStatus::Queued, JobStatus::Running] {
                let query = JobQuery {
                    status: Some(status),
                    kind: Some(IMPORT_WORDPRESS_KIND.to_owned()),
                    limit: 1,
                    cursor: None,
                };
                if !tx.jobs().list(&query).await?.items.is_empty() {
                    return Err(CoreError::Changed(ALREADY));
                }
            }
            tx.jobs()
                .enqueue(&import_wordpress_job(payload))
                .await?
                .ok_or(CoreError::Internal(
                    "an import job without a unique key was refused",
                ))
        }
        .await;
        finish(tx, done).await
    }

    /// The actor an import job runs as: `user` while it is an active admin, else
    /// [`CoreError::Forbidden`] (P27; also the CLI's `--as`).
    pub async fn importer(&self, user: UserId) -> Result<Actor, CoreError> {
        match self.store.read().users().get(user).await? {
            Some(u) if u.role == Role::Admin && u.status == UserStatus::Active => {
                Ok(Actor::new(u.id, u.role))
            }
            _ => Err(CoreError::Forbidden(ACTIVE_ADMIN)),
        }
    }

    /// Runs, newest first, after `cursor` (the last row's `(started_at, id)`); `limit`
    /// is 1..=[`MAX_PAGE`](ferrite_domain::MAX_PAGE). Needs `import:read`.
    pub async fn list_runs(
        &self,
        actor: &Actor,
        limit: u32,
        cursor: Option<(Timestamp, Uuid)>,
    ) -> Result<Vec<ImportRun>, CoreError> {
        require_reader(actor, Capability::Import)?;
        Ok(self
            .store
            .read()
            .imports()
            .list_runs_page(limit, cursor)
            .await?)
    }

    /// Run `id`. Needs `import:read`.
    pub async fn get_run(&self, actor: &Actor, id: ImportRunId) -> Result<ImportRun, CoreError> {
        require_reader(actor, Capability::Import)?;
        self.store
            .read()
            .imports()
            .get_run(id)
            .await?
            .ok_or(CoreError::NotFound)
    }

    /// The `import.wordpress` job `job_id` and its newest run (P26-1); another kind's job
    /// is [`CoreError::NotFound`]. Needs `import:read`.
    pub async fn job_status(
        &self,
        actor: &Actor,
        job_id: JobId,
    ) -> Result<ImportJobView, CoreError> {
        require_reader(actor, Capability::Import)?;
        let repos = self.store.read();
        let job = repos
            .jobs()
            .get(job_id)
            .await?
            .filter(|j| j.kind == IMPORT_WORDPRESS_KIND)
            .ok_or(CoreError::NotFound)?;
        let run = repos.imports().run_by_job(job_id).await?;
        Ok(ImportJobView { job, run })
    }

    /// Stores run `run`'s report files as `import` artifacts and points the run at them
    /// (P29), replacing an earlier attempt's. For the job, so no actor: whatever state
    /// the run is in. A missing run is [`CoreError::NotFound`].
    pub async fn store_report(
        &self,
        run: ImportRunId,
        json: Bytes,
        html: Bytes,
    ) -> Result<(), CoreError> {
        let blobs = self.blobs()?;
        if self.store.read().imports().get_run(run).await?.is_none() {
            return Err(CoreError::NotFound);
        }
        let mut rows = Vec::with_capacity(3);
        let mut put = async |data: Bytes, mime: &str| {
            let bytes = len(&data)?;
            let hash = blobs.put(data).await?;
            rows.push(Artifact {
                hash: hash.clone(),
                kind: ArtifactKind::Import,
                bytes,
                mime: mime.to_owned(),
                created_at: Timestamp::now(),
            });
            Ok::<_, CoreError>(hash)
        };
        let json = put(json, "application/json").await?;
        let html = put(html, "text/html; charset=utf-8").await?;
        let manifest = serde_json::to_vec(&Manifest {
            v: MANIFEST_VERSION,
            json: json.to_string(),
            html: html.to_string(),
        })
        .map_err(|_| CoreError::Internal("the report manifest did not serialise"))?;
        let manifest = put(Bytes::from(manifest), "application/json").await?;

        let tx = self.store.begin().await?;
        let done = async {
            for row in &rows {
                tx.artifacts().insert_if_absent(row).await?;
            }
            tx.imports().set_report(run, manifest.as_str()).await?;
            Ok(())
        }
        .await;
        finish(tx, done).await
    }

    /// One of run `run`'s report files; no report yet is [`CoreError::NotFound`]. Needs
    /// `import:read`.
    pub async fn report(
        &self,
        actor: &Actor,
        run: ImportRunId,
        format: ReportFormat,
    ) -> Result<Bytes, CoreError> {
        require_reader(actor, Capability::Import)?;
        let blobs = self.blobs()?;
        let hash = self
            .store
            .read()
            .imports()
            .get_run(run)
            .await?
            .and_then(|r| r.report_hash)
            .ok_or(CoreError::NotFound)?;
        let manifest: Manifest = serde_json::from_slice(&blobs.get(&parse(&hash)?).await?)
            .map_err(|_| CoreError::Internal("corrupt import report manifest"))?;
        let file = match format {
            ReportFormat::Json => manifest.json,
            ReportFormat::Html => manifest.html,
        };
        blobs.get(&parse(&file)?).await
    }

    fn blobs(&self) -> Result<&dyn ArtifactBlobs, CoreError> {
        self.artifacts.as_deref().ok_or(CoreError::Unavailable(
            "import reports need the media store",
        ))
    }
}

fn parse(hash: &str) -> Result<ArtifactHash, CoreError> {
    ArtifactHash::parse(hash).map_err(|_| CoreError::Internal("corrupt import report hash"))
}

fn len(data: &Bytes) -> Result<i64, CoreError> {
    i64::try_from(data.len()).map_err(|_| CoreError::Invalid("report too large".into()))
}
