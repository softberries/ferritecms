#![allow(clippy::unwrap_used, clippy::expect_used)]

//! The `import.wordpress` job (M3-2 P27–P29) on in-process nodes, both backends: a job
//! imports site A from its artifacts, takes over only its own stale run, is `Busy` while
//! another run is live, goes `dead` when its user is no longer an active admin or an
//! artifact is missing, and a retry on a node without the first attempt's work folder
//! parses again and ends with a clean run's rows.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use ferrite::jobs::ImportWordpressHandler;
use ferrite::serve::RunnerOverrides;
use ferrite::{NodeOptions, RunningNode, start};
use ferrite_cluster::{JobError, JobHandler};
use ferrite_config::Config;
use ferrite_core::import::{ImportService, ReportFormat};
use ferrite_core::jobs::{ImportJobOptions, ImportJobRequest};
use ferrite_core::{Actor, PermalinkRules};
use ferrite_domain::{
    Artifact, ArtifactHash, ArtifactKind, ContentQuery, ImportRun, ImportRunId, ImportRunState,
    Job, JobId, JobStatus, MEDIA_KIND, Media, NewJob, NewUser, NodeId, Role, Timestamp, UserId,
    UserStatus,
};
use ferrite_http::import_blobs::MediaArtifacts;
use ferrite_media::{Backend, MediaConfig, MediaStore, PublicUrlMode};
use ferrite_store::Store;
use ferrite_store_tests::{TestDb, backend_tests_spawning};
use serde_json::Value;

const SITE_URL: &str = "http://127.0.0.1:8080";
/// How long a job may take to reach the state a test waits for (an import of site A in
/// an unoptimised build, plus a retry's 20 s backoff).
const WAIT: Duration = Duration::from_secs(90);

fn corpus(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../ferrite-import-wp/tests/corpus")
        .join(name)
}

/// One test's site: the database, one shared media folder, and the data folders of the
/// nodes started on it.
struct Site {
    db: Arc<TestDb>,
    media_dir: tempfile::TempDir,
    admin: UserId,
}

/// A node's data folder and its config.
struct Folder {
    dir: tempfile::TempDir,
    config: Config,
}

impl Site {
    async fn new(db: Arc<TestDb>) -> Self {
        let admin = UserId::new();
        let tx = db.store().begin().await.unwrap();
        tx.users()
            .insert(&NewUser {
                id: admin,
                login: "importer".into(),
                email: "a@example.test".into(),
                display_name: "Importer".into(),
                password_hash: None,
                role: Role::Admin,
                status: UserStatus::Active,
                must_reset: false,
            })
            .await
            .unwrap();
        tx.commit().await.unwrap();
        Self {
            db,
            media_dir: tempfile::tempdir().unwrap(),
            admin,
        }
    }

    fn store(&self) -> Arc<dyn Store> {
        self.db.shared_store()
    }

    fn media(&self) -> Arc<MediaStore> {
        Arc::new(
            MediaStore::open(MediaConfig {
                backend: Backend::Local {
                    root: self.media_dir.path().to_owned(),
                },
                prefix: String::new(),
                public: PublicUrlMode::ServeViaApp {
                    path_prefix: "/_media".into(),
                },
            })
            .unwrap(),
        )
    }

    fn import(&self) -> ImportService {
        ImportService::new(self.store(), Arc::new(PermalinkRules::default()))
            .with_artifacts(Arc::new(MediaArtifacts::new(self.media())))
    }

    fn actor(&self) -> Actor {
        Actor::new(self.admin, Role::Admin)
    }

    /// A new data folder whose media root is the site's shared one.
    fn folder(&self) -> Folder {
        let dir = tempfile::tempdir().unwrap();
        let media = self.media_dir.path();
        let mut toml = format!(
            "[site]\nurl = \"{SITE_URL}/\"\n[server]\nlisten = \"127.0.0.1:0\"\n\
             [media]\nbackend = \"local\"\nroot = {media:?}\n"
        );
        if let Some((url, schema)) = self.db.pg_location() {
            toml.push_str(&format!(
                "local_shared = true\n[database]\nurl = \"{url}\"\nschema = \"{schema}\"\n\
                 [cluster]\nmode = \"cluster\"\n"
            ));
        }
        std::fs::write(dir.path().join("ferrite.toml"), toml).unwrap();
        let env = vec![("FERRITE_AUTH__SECRET_KEY".to_owned(), "ab".repeat(32))];
        let config = ferrite_config::load(dir.path(), env).unwrap();
        Folder { dir, config }
    }

    async fn start(&self, folder: &Folder, opts: NodeOptions) -> RunningNode {
        start(&folder.config, self.store(), opts).await.unwrap()
    }

    /// Stores `data`'s blob, as the API does before it enqueues (P23); `enqueue` records
    /// the row.
    async fn artifact(&self, data: Vec<u8>) -> ArtifactHash {
        self.media()
            .put_artifact(Bytes::from(data))
            .await
            .unwrap()
            .hash
    }

    /// The `import` artifact row for `hash` (its size from the blob, 0 when gone).
    async fn input(&self, hash: &ArtifactHash) -> Artifact {
        let bytes = self.media().head(hash).await.unwrap().unwrap_or(0);
        Artifact {
            hash: hash.clone(),
            kind: ArtifactKind::Import,
            bytes: i64::try_from(bytes).unwrap(),
            mime: "application/octet-stream".into(),
            created_at: Timestamp::now(),
        }
    }

    /// Whether the blob or the row of `hash` is still there.
    async fn artifact_exists(&self, hash: &ArtifactHash) -> (bool, bool) {
        let blob = self.media().head(hash).await.unwrap().is_some();
        let row = self
            .store()
            .read()
            .artifacts()
            .get(hash)
            .await
            .unwrap()
            .is_some();
        (blob, row)
    }

    /// Site A's WXR and its uploads zipped under `wp-content/uploads/`, as artifacts.
    async fn site_a(&self) -> (ArtifactHash, ArtifactHash) {
        let wxr = self
            .artifact(std::fs::read(corpus("site-a.xml")).unwrap())
            .await;
        let uploads = self.artifact(site_a_zip()).await;
        (wxr, uploads)
    }

    /// Enqueues an import of `wxr` (and `uploads`) as the admin.
    async fn enqueue(&self, wxr: &ArtifactHash, uploads: Option<&ArtifactHash>) -> JobId {
        self.import()
            .enqueue(
                &self.actor(),
                ImportJobRequest {
                    wxr: self.input(wxr).await,
                    uploads: match uploads {
                        Some(hash) => Some(self.input(hash).await),
                        None => None,
                    },
                    options: ImportJobOptions::default(),
                    source_name: "site-a.xml".into(),
                },
            )
            .await
            .unwrap()
    }

    async fn job(&self, id: JobId) -> Job {
        self.store().read().jobs().get(id).await.unwrap().unwrap()
    }

    /// Waits until job `id` satisfies `done`.
    async fn wait_job(&self, id: JobId, what: &str, done: impl Fn(&Job) -> bool) -> Job {
        let deadline = tokio::time::Instant::now() + WAIT;
        loop {
            let job = self.job(id).await;
            if done(&job) {
                return job;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "job never {what}: {job:?}"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// Inserts a `running` run, its heartbeat `age` old, started by `job`.
    async fn running_run(&self, age: Duration, job: Option<JobId>) -> ImportRunId {
        self.run_in(ImportRunState::Running, age, job).await
    }

    /// Inserts a run in `state`, its heartbeat `age` old, started by `job`.
    async fn run_in(
        &self,
        state: ImportRunState,
        age: Duration,
        job: Option<JobId>,
    ) -> ImportRunId {
        let age = i64::try_from(age.as_micros()).unwrap();
        let at = Timestamp::from_micros(Timestamp::now().as_micros() - age).unwrap();
        let id = ImportRunId::new();
        self.store()
            .read()
            .imports()
            .create_run(&ImportRun {
                id,
                source: "http://elsewhere.test".into(),
                token: [7; 16],
                state,
                started_by: self.admin,
                started_at: at,
                heartbeat_at: at,
                finished_at: (state != ImportRunState::Running).then_some(at),
                stats: serde_json::json!({}),
                dry_run: false,
                job_id: job,
                step: None,
                report_hash: None,
            })
            .await
            .unwrap();
        id
    }

    /// A job of another kind, `running` with a fresh lease (`dead` when `dead`).
    async fn other_job(&self, dead: bool) -> JobId {
        let store = self.store();
        let jobs = store.read().jobs();
        let id = jobs
            .enqueue(&NewJob::new("test.other", serde_json::json!({})))
            .await
            .unwrap()
            .unwrap();
        let claimed = jobs
            .claim(
                NodeId::new(),
                &["test.other".to_owned()],
                1,
                Duration::from_secs(600),
            )
            .await
            .unwrap();
        if dead {
            jobs.fail(&claimed[0].lease().unwrap(), "gone", true)
                .await
                .unwrap();
        }
        id
    }

    /// The job handler of a node on `folder`, called directly (no runner).
    fn handler(&self, folder: &Folder, fail_after_batches: Option<u32>) -> ImportWordpressHandler {
        ImportWordpressHandler {
            store: self.store(),
            import: Arc::new(self.import()),
            media: self.media(),
            data_dir: folder.dir.path().to_owned(),
            site_url: SITE_URL.into(),
            max_upload: 256 << 20,
            strip_metadata: false,
            version: "0.0.0-test".into(),
            fail_after_batches,
        }
    }

    async fn run(&self, id: ImportRunId) -> ImportRun {
        self.store()
            .read()
            .imports()
            .get_run(id)
            .await
            .unwrap()
            .unwrap()
    }

    async fn run_of(&self, job: JobId) -> ImportRun {
        self.store()
            .read()
            .imports()
            .run_by_job(job)
            .await
            .unwrap()
            .expect("the job started a run")
    }

    /// Content items per kind.
    async fn content(&self) -> Vec<(String, u64)> {
        let mut out = Vec::new();
        for kind in ["post", "page", "book", MEDIA_KIND] {
            let n = self
                .store()
                .read()
                .content()
                .count(&ContentQuery {
                    kind: Some(kind.into()),
                    ..ContentQuery::default()
                })
                .await
                .unwrap();
            out.push((kind.to_owned(), n));
        }
        out
    }
}

/// Short runner timings, and the import seam.
fn opts(fail_after_batches: Option<u32>) -> NodeOptions {
    NodeOptions {
        runner: RunnerOverrides {
            idle_poll: Some(Duration::from_millis(100)),
            ..RunnerOverrides::default()
        },
        import_fail_after_batches: fail_after_batches,
        ..NodeOptions::default()
    }
}

fn site_a_zip() -> Vec<u8> {
    let root = corpus("site-a-uploads/2026/10");
    let mut names: Vec<_> = std::fs::read_dir(&root)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for name in names {
        zip.start_file(
            format!("wp-content/uploads/2026/10/{name}"),
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
        zip.write_all(&std::fs::read(root.join(&name)).unwrap())
            .unwrap();
    }
    zip.finish().unwrap().into_inner()
}

/// Released `Busy`: queued again, due at least the 1 s busy delay after it was enqueued
/// (a `Busy` release records no error).
fn busy_released(job: &Job) -> bool {
    job.status == JobStatus::Queued && job.run_at.as_micros() > job.created_at.as_micros() + 900_000
}

fn terminal(job: &Job) -> bool {
    matches!(job.status, JobStatus::Done | JobStatus::Dead)
}

/// Site A through the job: `done`, the run linked to the job at step `report` with a
/// report, the inputs deleted and the job folder gone.
async fn job_imports_site_a(db: Arc<TestDb>) {
    let site = Site::new(db).await;
    let (wxr, uploads) = site.site_a().await;
    let id = site.enqueue(&wxr, Some(&uploads)).await;
    let job = site.job(id).await;
    assert_eq!(job.max_attempts, 3);
    let folder = site.folder();
    let node = site.start(&folder, opts(None)).await;
    let job = site.wait_job(id, "ended", terminal).await;
    assert_eq!(job.status, JobStatus::Done, "{job:?}");
    let run = site.run_of(id).await;
    assert_eq!(run.state, ImportRunState::Done);
    assert_eq!(run.job_id, Some(id));
    assert_eq!(run.step.as_deref(), Some("report"));
    assert!(run.report_hash.is_some());
    let report = site
        .import()
        .report(&site.actor(), run.id, ReportFormat::Json)
        .await
        .unwrap();
    let report: Value = serde_json::from_slice(&report).unwrap();
    assert_eq!(report["counts"]["post"]["created"], 8, "{report}");
    assert_eq!(report["counts"]["media"]["created"], 4, "{report}");
    assert!(report["failed"].is_null());
    for hash in [&wxr, &uploads] {
        assert_eq!(site.artifact_exists(hash).await, (false, false), "{hash}");
    }
    assert!(!folder.dir.path().join(format!("import/job-{id}")).exists());
    node.shutdown().await.unwrap();
}

/// A stale run of this job is taken over; a stale run of another job that is still
/// `running` is not: the job is `Busy` (its attempt refunded) and that run stays.
async fn job_retry_takes_over_only_its_own_run(db: Arc<TestDb>) {
    let site = Site::new(db).await;
    let (wxr, uploads) = site.site_a().await;
    let id = site.enqueue(&wxr, Some(&uploads)).await;
    let own = site.running_run(Duration::from_secs(600), Some(id)).await;
    let folder = site.folder();
    let node = site.start(&folder, opts(None)).await;
    let job = site.wait_job(id, "ended", terminal).await;
    assert_eq!(job.status, JobStatus::Done, "{job:?}");
    assert_eq!(site.run(own).await.state, ImportRunState::Abandoned);
    node.shutdown().await.unwrap();

    let (wxr, uploads) = site.site_a().await;
    let id = site.enqueue(&wxr, Some(&uploads)).await;
    let live = site.other_job(false).await;
    let other = site.running_run(Duration::from_secs(600), Some(live)).await;
    let node = site.start(&folder, opts(None)).await;
    let job = site.wait_job(id, "released busy", busy_released).await;
    assert_eq!(job.attempts, 0, "{job:?}");
    assert_eq!(site.run(other).await.state, ImportRunState::Running);
    node.shutdown().await.unwrap();
}

/// A stale run of a crashed CLI import (no job), and one of a job that is `dead`, are
/// taken over by the job (P27 as built).
async fn job_takes_over_stale_runs_without_a_live_job(db: Arc<TestDb>) {
    let site = Site::new(db).await;
    let folder = site.folder();
    let dead = site.other_job(true).await;
    for owner in [None, Some(dead)] {
        let (wxr, uploads) = site.site_a().await;
        let id = site.enqueue(&wxr, Some(&uploads)).await;
        let stale = site.running_run(Duration::from_secs(600), owner).await;
        let node = site.start(&folder, opts(None)).await;
        let job = site.wait_job(id, "ended", terminal).await;
        assert_eq!(job.status, JobStatus::Done, "{owner:?}: {job:?}");
        assert_eq!(site.run(stale).await.state, ImportRunState::Abandoned);
        node.shutdown().await.unwrap();
    }
}

/// A busy attempt fetches nothing: not even the import folder is made.
async fn busy_attempt_fetches_nothing(db: Arc<TestDb>) {
    let site = Site::new(db).await;
    let (wxr, uploads) = site.site_a().await;
    let id = site.enqueue(&wxr, Some(&uploads)).await;
    site.running_run(Duration::ZERO, None).await;
    let folder = site.folder();
    let err = site
        .handler(&folder, None)
        .run(&site.job(id).await)
        .await
        .unwrap_err();
    assert!(matches!(err, JobError::Busy(_)), "{err:?}");
    assert!(!folder.dir.path().join("import").exists());
    assert_eq!(site.artifact_exists(&uploads).await, (true, true));
}

/// A `Retry` is final on the last attempt: the inputs go then too (P23), not before.
/// Leftover job folders of jobs that are over are removed; a running job's stays.
async fn last_attempt_retry_deletes_inputs(db: Arc<TestDb>) {
    let site = Site::new(db).await;
    let (wxr, uploads) = site.site_a().await;
    let id = site.enqueue(&wxr, Some(&uploads)).await;
    let folder = site.folder();
    let import = folder.dir.path().join("import");
    let gone = import.join(format!("job-{}", JobId::new()));
    let kept = import.join(format!("job-{}", site.other_job(false).await));
    for dir in [&gone, &kept] {
        std::fs::create_dir_all(dir).unwrap();
    }
    let handler = site.handler(&folder, Some(3));
    let mut job = site.job(id).await;
    job.attempts = 1;
    let err = handler.run(&job).await.unwrap_err();
    assert!(matches!(err, JobError::Retry(_)), "{err:?}");
    assert_eq!(site.artifact_exists(&wxr).await, (true, true));
    assert!(!gone.exists());
    assert!(kept.exists());
    // Not final: the run's work folder stays for the retry to resume.
    let first = site.run_of(id).await;
    assert!(import.join(first.id.to_string()).exists());
    job.attempts = job.max_attempts;
    let err = handler.run(&job).await.unwrap_err();
    assert!(matches!(err, JobError::Retry(_)), "{err:?}");
    for hash in [&wxr, &uploads] {
        assert_eq!(site.artifact_exists(hash).await, (false, false), "{hash}");
    }
    // Final: the work folder goes too (BE-m3).
    let last = site.run_of(id).await;
    assert_ne!(last.id, first.id);
    for run in [first.id, last.id] {
        assert!(!import.join(run.to_string()).exists(), "{run}");
    }
}

/// Each attempt removes the work folders of runs whose job is over (`done` or `dead`);
/// a CLI run's folder (no job) stays for the next CLI run of the same file to resume
/// (M3-1 R27), and so does one whose job may still retry.
async fn attempt_sweeps_work_folders_of_ended_jobs(db: Arc<TestDb>) {
    let site = Site::new(db).await;
    let (wxr, uploads) = site.site_a().await;
    let id = site.enqueue(&wxr, Some(&uploads)).await;
    let folder = site.folder();
    let import = folder.dir.path().join("import");
    let dead = site.other_job(true).await;
    let live = site.other_job(false).await;
    let ended = site
        .run_in(ImportRunState::Failed, Duration::from_secs(600), Some(dead))
        .await;
    let retrying = site
        .run_in(ImportRunState::Failed, Duration::from_secs(600), Some(live))
        .await;
    let cli = site
        .run_in(ImportRunState::Failed, Duration::from_secs(600), None)
        .await;
    for run in [ended, retrying, cli] {
        std::fs::create_dir_all(import.join(run.to_string())).unwrap();
    }
    // A live CLI run makes the attempt busy; the sweep runs first anyway.
    site.running_run(Duration::ZERO, None).await;
    let err = site
        .handler(&folder, None)
        .run(&site.job(id).await)
        .await
        .unwrap_err();
    assert!(matches!(err, JobError::Busy(_)), "{err:?}");
    assert!(!import.join(ended.to_string()).exists());
    assert!(import.join(retrying.to_string()).exists());
    assert!(import.join(cli.to_string()).exists());
}

/// An attempt that finds its input gone after its run was `done` (a drop between the
/// run's end and the job's) ends `done`, not `dead`.
async fn missing_input_of_a_done_run_is_done(db: Arc<TestDb>) {
    let site = Site::new(db).await;
    let (wxr, uploads) = site.site_a().await;
    let id = site.enqueue(&wxr, Some(&uploads)).await;
    site.run_in(ImportRunState::Done, Duration::ZERO, Some(id))
        .await;
    site.media().delete(&wxr).await.unwrap();
    let folder = site.folder();
    site.handler(&folder, None)
        .run(&site.job(id).await)
        .await
        .unwrap();
}

/// Another run with a fresh heartbeat: `Busy`, the attempt refunded, nothing deleted.
async fn job_busy_when_another_run_is_live(db: Arc<TestDb>) {
    let site = Site::new(db).await;
    let (wxr, uploads) = site.site_a().await;
    let id = site.enqueue(&wxr, Some(&uploads)).await;
    let live = site.running_run(Duration::ZERO, None).await;
    let folder = site.folder();
    let node = site.start(&folder, opts(None)).await;
    let job = site.wait_job(id, "released busy", busy_released).await;
    assert_eq!(job.attempts, 0, "{job:?}");
    assert_eq!(site.run(live).await.state, ImportRunState::Running);
    assert_eq!(site.artifact_exists(&wxr).await, (true, true));
    node.shutdown().await.unwrap();
}

/// A user no longer an active admin when the job runs: `dead`, inputs deleted.
async fn job_dead_when_user_demoted(db: Arc<TestDb>) {
    let site = Site::new(db).await;
    let (wxr, uploads) = site.site_a().await;
    let id = site.enqueue(&wxr, Some(&uploads)).await;
    let store = site.store();
    let mut user = store.read().users().get(site.admin).await.unwrap().unwrap();
    user.role = Role::Editor;
    store.read().users().update(&user).await.unwrap();
    let folder = site.folder();
    let node = site.start(&folder, opts(None)).await;
    let job = site.wait_job(id, "ended", terminal).await;
    assert_eq!(job.status, JobStatus::Dead, "{job:?}");
    assert_eq!(job.attempts, 1, "{job:?}");
    assert!(
        job.last_error
            .as_deref()
            .unwrap_or_default()
            .contains("admin"),
        "{job:?}"
    );
    for hash in [&wxr, &uploads] {
        assert_eq!(site.artifact_exists(hash).await, (false, false), "{hash}");
    }
    assert!(
        store
            .read()
            .imports()
            .run_by_job(id)
            .await
            .unwrap()
            .is_none()
    );
    node.shutdown().await.unwrap();
}

/// A media item uploaded with the same bytes as a queued import's uploads zip shares its
/// artifact row (kind `import`, `insert_if_absent`). When the job's inputs are spent, the
/// row is deleted first and the foreign key refuses it, so the media item keeps both its
/// row and its file; the WXR, which nothing else holds, goes.
async fn input_shared_with_media_keeps_its_file(db: Arc<TestDb>) {
    let site = Site::new(db).await;
    let (wxr, uploads) = site.site_a().await;
    let id = site.enqueue(&wxr, Some(&uploads)).await;
    let store = site.store();
    let mut user = store.read().users().get(site.admin).await.unwrap().unwrap();
    let post = ferrite_store_tests::fixtures::new_post(&user, "uploads-zip");
    store.read().content().insert(&post).await.unwrap();
    store
        .read()
        .media()
        .insert(&Media {
            item_id: post.id,
            artifact_hash: uploads.clone(),
            mime: "application/zip".into(),
            bytes: 1,
            width: None,
            height: None,
            alt: String::new(),
            original_name: "uploads.zip".into(),
            variants: serde_json::json!({}),
            sanitized: false,
        })
        .await
        .unwrap();
    // The job is spent at once: its user is no longer an admin (`Permanent`).
    user.role = Role::Editor;
    store.read().users().update(&user).await.unwrap();
    let folder = site.folder();
    let err = site
        .handler(&folder, None)
        .run(&site.job(id).await)
        .await
        .unwrap_err();
    assert!(matches!(err, JobError::Permanent(_)), "{err:?}");
    assert_eq!(site.artifact_exists(&wxr).await, (false, false));
    assert_eq!(site.artifact_exists(&uploads).await, (true, true));
    assert!(store.read().media().get(post.id).await.unwrap().is_some());
}

/// An input artifact that is not in the store: `dead` on the first attempt.
async fn job_dead_when_artifact_missing(db: Arc<TestDb>) {
    let site = Site::new(db).await;
    let wxr = site
        .artifact(std::fs::read(corpus("site-a.xml")).unwrap())
        .await;
    site.media().delete(&wxr).await.unwrap();
    let id = site.enqueue(&wxr, None).await;
    let folder = site.folder();
    let node = site.start(&folder, opts(None)).await;
    let job = site.wait_job(id, "ended", terminal).await;
    assert_eq!(job.status, JobStatus::Dead, "{job:?}");
    assert_eq!(job.attempts, 1, "{job:?}");
    assert!(!folder.dir.path().join(format!("import/job-{id}")).exists());
    node.shutdown().await.unwrap();
}

/// A first attempt that fails part-way keeps its work folder on its node; the retry on
/// another node (another data folder) parses again, and the site ends with the rows of
/// a clean import.
async fn job_retry_on_fresh_node_reparses(db: Arc<TestDb>) {
    let site = Site::new(db).await;
    let (wxr, uploads) = site.site_a().await;
    let id = site.enqueue(&wxr, Some(&uploads)).await;
    let first = site.folder();
    let node = site.start(&first, opts(Some(3))).await;
    let job = site
        .wait_job(id, "failed once", |j| {
            j.attempts == 1 && j.status == JobStatus::Queued
        })
        .await;
    assert!(job.last_error.is_some(), "{job:?}");
    node.shutdown().await.unwrap();
    let failed = site.run_of(id).await;
    assert_eq!(failed.state, ImportRunState::Failed);
    assert!(
        failed.report_hash.is_some(),
        "a failed attempt stores its report"
    );
    assert!(
        first
            .dir
            .path()
            .join(format!("import/{}/ir.sqlite", failed.id))
            .is_file()
    );
    assert_eq!(site.artifact_exists(&wxr).await, (true, true));

    let second = site.folder();
    let node = site.start(&second, opts(None)).await;
    let job = site.wait_job(id, "ended", terminal).await;
    assert_eq!(job.status, JobStatus::Done, "{job:?}");
    let run = site.run_of(id).await;
    assert_ne!(run.id, failed.id);
    assert_eq!(run.state, ImportRunState::Done);
    // Site A's items, each once (see the rerun tests' counts).
    assert_eq!(
        site.content().await,
        [
            ("post".to_owned(), 8),
            ("page".to_owned(), 4),
            ("book".to_owned(), 2),
            (MEDIA_KIND.to_owned(), 4),
        ]
    );
    node.shutdown().await.unwrap();
}

backend_tests_spawning!(
    job_imports_site_a,
    job_retry_takes_over_only_its_own_run,
    job_busy_when_another_run_is_live,
    job_takes_over_stale_runs_without_a_live_job,
    busy_attempt_fetches_nothing,
    last_attempt_retry_deletes_inputs,
    attempt_sweeps_work_folders_of_ended_jobs,
    missing_input_of_a_done_run_is_done,
    job_dead_when_user_demoted,
    input_shared_with_media_keeps_its_file,
    job_dead_when_artifact_missing,
    job_retry_on_fresh_node_reparses,
);
