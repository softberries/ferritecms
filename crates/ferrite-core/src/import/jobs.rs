//! Import jobs and what reads runs (M3-2 P25, P26-1, P27, P29): the enqueue with its
//! conflict rule, the job's importer check, the run and job reads, and report artifacts.
//!
//! **Reports.** `report.json` and `report.html` are stored as `import` artifacts, and
//! the run's `report_hash` names a third one, a small JSON manifest
//! `{"v":1,"json":"<sha256>","html":"<sha256>"}` (one column, no migration). A later
//! attempt stores a new manifest; the files it replaces stay until the store is cleaned.
//! The manifest is read only up to [`MAX_MANIFEST_BYTES`] and each file up to
//! [`MAX_REPORT_BYTES`] (the report's own lists are capped too, M3-1 R42).
//!
//! **Readers never see a run's fencing token**: the reads return [`ImportRunView`].

use bytes::Bytes;
use ferrite_domain::{
    Artifact, ArtifactHash, ArtifactKind, ImportRun, ImportRunId, ImportRunState, Job, JobId,
    JobQuery, JobStatus, Role, Timestamp, UserId, UserStatus,
};
use ferrite_store::{JobRepo, LOCK_IMPORT};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use super::{ArtifactBlobs, ImportService, release, stale_before};
use crate::{
    Actor, CoreError,
    actor::{Capability, require, require_reader},
    jobs::{
        IMPORT_WORDPRESS_KIND, IMPORT_WORDPRESS_VERSION, ImportJobPayload, ImportJobRequest,
        check_import_options, import_wordpress_job,
    },
    tx::finish,
};

/// Refused enqueue (P25): another import is queued or running.
const ALREADY: &str = "an import is already queued or running";
/// The importer check's refusal (P27).
const ACTIVE_ADMIN: &str = "an active admin";
/// The import lock could not be taken (an error, not another holder).
const NO_LOCK: &str = "the import lock is unavailable";
/// Current version of the report manifest.
const MANIFEST_VERSION: u32 = 1;
/// The largest report manifest read (it holds two hashes).
const MAX_MANIFEST_BYTES: u64 = 1024;
/// The largest report file stored or read.
pub const MAX_REPORT_BYTES: u64 = 16 * 1024 * 1024;

/// A run as readers see it: [`ImportRun`] without its fencing token.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportRunView {
    pub id: ImportRunId,
    pub source: String,
    pub state: ImportRunState,
    pub started_by: UserId,
    pub started_at: Timestamp,
    pub heartbeat_at: Timestamp,
    pub finished_at: Option<Timestamp>,
    pub stats: Value,
    pub dry_run: bool,
    pub job_id: Option<JobId>,
    pub step: Option<String>,
    pub report_hash: Option<String>,
}

impl From<ImportRun> for ImportRunView {
    fn from(r: ImportRun) -> Self {
        Self {
            id: r.id,
            source: r.source,
            state: r.state,
            started_by: r.started_by,
            started_at: r.started_at,
            heartbeat_at: r.heartbeat_at,
            finished_at: r.finished_at,
            stats: r.stats,
            dry_run: r.dry_run,
            job_id: r.job_id,
            step: r.step,
            report_hash: r.report_hash,
        }
    }
}

/// An import job and the run it started, if it got that far (`GET /import/jobs/{id}`).
#[derive(Debug, Clone, PartialEq)]
pub struct ImportJobView {
    pub job: Job,
    pub run: Option<ImportRunView>,
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
    /// `Ok` if `actor` may start an import ([`Capability::Import`] with `import:write`):
    /// what the API checks before it claims an `Idempotency-Key`.
    pub fn authorize_import(&self, actor: &Actor) -> Result<(), CoreError> {
        require(actor, Capability::Import)
    }

    /// What the API checks before it reads an upload: `actor` may start an import
    /// ([`Capability::Import`] with `import:write`), and no `import.wordpress` job is
    /// queued or running and no run is `running` with a fresh heartbeat
    /// ([`CoreError::Changed`]). Read-only: a stale run passes and is left alone;
    /// [`enqueue`](Self::enqueue) decides for good.
    pub async fn check_start(&self, actor: &Actor) -> Result<(), CoreError> {
        require(actor, Capability::Import)?;
        let repos = self.store.read();
        if let Some(run) = repos.imports().running().await?
            && run.heartbeat_at >= stale_before(Timestamp::now())
        {
            return Err(CoreError::Changed(ALREADY));
        }
        if import_job_active(repos.jobs()).await? {
            return Err(CoreError::Changed(ALREADY));
        }
        Ok(())
    }

    /// Queues an `import.wordpress` job for `request` (P25, P27), run as `actor` (the
    /// payload's `user` is always the actor's). Needs [`Capability::Import`] with
    /// `import:write`. While an `import.wordpress` job is queued or running, or a run is
    /// `running` with a fresh heartbeat, this is [`CoreError::Changed`] (the API's
    /// `409`); the inputs must be `import` artifacts (else `Invalid`) and their rows are
    /// recorded in the same transaction; a run whose heartbeat is older than [`STALE_AFTER`](super::STALE_AFTER)
    /// is abandoned instead, since the admin asked to import.
    ///
    /// The check and the insert are one transaction, with the import lock taken around
    /// it so two enqueues cannot both pass the check; the lock held elsewhere is the same
    /// refusal, and failing to take it is [`CoreError::Unavailable`]. A CLI import that
    /// starts between this check and the job's run is still caught: the run's unique
    /// `running` index refuses one of the two, and the job retries a `Busy` start.
    pub async fn enqueue(
        &self,
        actor: &Actor,
        request: ImportJobRequest,
    ) -> Result<JobId, CoreError> {
        require(actor, Capability::Import)?;
        check_import_options(&request.options)?;
        let inputs: Vec<Artifact> = std::iter::once(request.wxr)
            .chain(request.uploads)
            .collect();
        if inputs.iter().any(|a| a.kind != ArtifactKind::Import) {
            return Err(CoreError::Invalid(
                "import inputs must be import artifacts".into(),
            ));
        }
        let payload = ImportJobPayload {
            v: IMPORT_WORDPRESS_VERSION,
            wxr: inputs[0].hash.to_string(),
            uploads: inputs.get(1).map(|a| a.hash.to_string()),
            options: request.options,
            user: actor.user,
            source_name: request.source_name,
        };
        let lock = match self.store.try_lock(LOCK_IMPORT).await {
            Ok(Some(guard)) => guard,
            Ok(None) => return Err(CoreError::Changed(ALREADY)),
            Err(e) => {
                tracing::warn!(error = %e, "taking the import lock failed");
                return Err(CoreError::Unavailable(NO_LOCK));
            }
        };
        let done = self.enqueue_checked(&payload, &inputs).await;
        release(Some(lock)).await;
        done
    }

    async fn enqueue_checked(
        &self,
        payload: &ImportJobPayload,
        inputs: &[Artifact],
    ) -> Result<JobId, CoreError> {
        let tx = self.store.begin().await?;
        let done = async {
            let imports = tx.imports();
            if let Some(run) = imports.running().await? {
                let now = Timestamp::now();
                let stale_before = stale_before(now);
                // One statement re-checks the heartbeat, so a run that fenced since the
                // read stays and refuses.
                if run.heartbeat_at >= stale_before
                    || !imports.abandon_stale(run.id, stale_before, now).await?
                {
                    return Err(CoreError::Changed(ALREADY));
                }
                tracing::warn!(run = %run.id, "an import request took over a stale run");
            }
            if import_job_active(tx.jobs()).await? {
                return Err(CoreError::Changed(ALREADY));
            }
            for input in inputs {
                tx.artifacts().insert_if_absent(input).await?;
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
    ) -> Result<Vec<ImportRunView>, CoreError> {
        require_reader(actor, Capability::Import)?;
        let runs = self
            .store
            .read()
            .imports()
            .list_runs_page(limit, cursor)
            .await?;
        Ok(runs.into_iter().map(ImportRunView::from).collect())
    }

    /// Run `id`. Needs `import:read`.
    pub async fn get_run(
        &self,
        actor: &Actor,
        id: ImportRunId,
    ) -> Result<ImportRunView, CoreError> {
        require_reader(actor, Capability::Import)?;
        self.store
            .read()
            .imports()
            .get_run(id)
            .await?
            .map(ImportRunView::from)
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
        Ok(ImportJobView {
            job,
            run: run.map(ImportRunView::from),
        })
    }

    /// Stores run `run`'s report files as `import` artifacts and points the run at them
    /// (P29), replacing an earlier attempt's. For the job, so no actor: whatever state
    /// the run is in. A missing run is [`CoreError::NotFound`]; a file over
    /// [`MAX_REPORT_BYTES`] is `Invalid`.
    pub async fn store_report(
        &self,
        run: ImportRunId,
        json: Bytes,
        html: Bytes,
    ) -> Result<(), CoreError> {
        let blobs = self.blobs()?;
        if [&json, &html]
            .iter()
            .any(|b| b.len() as u64 > MAX_REPORT_BYTES)
        {
            return Err(CoreError::Invalid("an import report over 16 MiB".into()));
        }
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
        let corrupt = || CoreError::Internal("corrupt import report manifest");
        let manifest = blobs.get(&parse(&hash)?, MAX_MANIFEST_BYTES).await?;
        if manifest.len() as u64 > MAX_MANIFEST_BYTES {
            return Err(corrupt());
        }
        let manifest: Manifest = serde_json::from_slice(&manifest).map_err(|_| corrupt())?;
        if manifest.v != MANIFEST_VERSION {
            return Err(corrupt());
        }
        let file = match format {
            ReportFormat::Json => manifest.json,
            ReportFormat::Html => manifest.html,
        };
        let data = blobs.get(&parse(&file)?, MAX_REPORT_BYTES).await?;
        if data.len() as u64 > MAX_REPORT_BYTES {
            return Err(CoreError::Internal("an import report over 16 MiB"));
        }
        Ok(data)
    }

    fn blobs(&self) -> Result<&dyn ArtifactBlobs, CoreError> {
        self.artifacts.as_deref().ok_or(CoreError::Unavailable(
            "import reports need the media store",
        ))
    }
}

/// An `import.wordpress` job is queued or running.
async fn import_job_active(jobs: &dyn JobRepo) -> Result<bool, CoreError> {
    for status in [JobStatus::Queued, JobStatus::Running] {
        let query = JobQuery {
            status: Some(status),
            kind: Some(IMPORT_WORDPRESS_KIND.to_owned()),
            limit: 1,
            cursor: None,
        };
        if !jobs.list(&query).await?.items.is_empty() {
            return Ok(true);
        }
    }
    Ok(false)
}

fn parse(hash: &str) -> Result<ArtifactHash, CoreError> {
    ArtifactHash::parse(hash).map_err(|_| CoreError::Internal("corrupt import report hash"))
}

fn len(data: &Bytes) -> Result<i64, CoreError> {
    i64::try_from(data.len()).map_err(|_| CoreError::Invalid("report too large".into()))
}
