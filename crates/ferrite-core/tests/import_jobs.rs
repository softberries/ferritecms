//! Import jobs, run reads, the importer check and report artifacts (M3-2 Task 2, P25,
//! P26-1, P27, P28, P29), on both backends.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use bytes::Bytes;
use ferrite_core::{
    Actor, CoreError, PermalinkRules, Scopes,
    import::{ArtifactBlobs, ImportJobView, ImportService, ReportFormat},
    jobs::{
        IMPORT_WORDPRESS_KIND, IMPORT_WORDPRESS_VERSION, ImportJobOptions, ImportJobPayload,
        ImportJobRequest,
    },
};
use ferrite_domain::{
    Artifact, ArtifactHash, ArtifactKind, ImportRun, ImportRunId, ImportRunState, JobId, JobStatus,
    NewUser, NodeId, Role, Timestamp, UserId, UserStatus, to_hex,
};
use ferrite_media::{Backend, MediaConfig, MediaStore, PublicUrlMode};
use ferrite_store::{LOCK_IMPORT, Store};
use ferrite_store_tests::{TestDb, backend_tests};
use serde_json::json;

const SOURCE: &str = "https://wp.example";

async fn user(store: &dyn Store, role: Role) -> UserId {
    let id = UserId::new();
    let tx = store.begin().await.unwrap();
    tx.users()
        .insert(&NewUser {
            id,
            login: format!("u-{id}"),
            email: format!("{id}@example.test"),
            display_name: "Test".into(),
            password_hash: None,
            role,
            status: UserStatus::Active,
            must_reset: false,
        })
        .await
        .unwrap();
    tx.commit().await.unwrap();
    id
}

async fn actor(store: &dyn Store, role: Role) -> Actor {
    Actor::new(user(store, role).await, role)
}

/// The `MediaStore` behind [`ArtifactBlobs`], as the binary wires it.
struct Media(MediaStore);

#[async_trait]
impl ArtifactBlobs for Media {
    async fn put(&self, data: Bytes) -> Result<ArtifactHash, CoreError> {
        let stored = self.0.put_artifact(data).await;
        stored
            .map(|r| r.hash)
            .map_err(|_| CoreError::Internal("media store"))
    }

    async fn get(&self, hash: &ArtifactHash, max: u64) -> Result<Bytes, CoreError> {
        let size = self.0.head(hash).await;
        match size.map_err(|_| CoreError::Internal("media store"))? {
            None => Err(CoreError::NotFound),
            Some(size) if size > max => Err(CoreError::Internal("artifact too large")),
            Some(_) => {
                let read = self.0.get(hash).await;
                read.map_err(|_| CoreError::Internal("media store"))
            }
        }
    }
}

struct Fixture {
    svc: ImportService,
    blobs: Arc<Media>,
    _media: tempfile::TempDir,
}

fn service(db: &TestDb) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let media = MediaStore::open(MediaConfig {
        backend: Backend::Local {
            root: dir.path().join("media"),
        },
        prefix: String::new(),
        public: PublicUrlMode::ServeViaApp {
            path_prefix: "/_media".into(),
        },
    })
    .unwrap();
    let blobs = Arc::new(Media(media));
    let svc = ImportService::new(db.shared_store(), Arc::new(PermalinkRules::default()))
        .with_artifacts(blobs.clone());
    Fixture {
        svc,
        blobs,
        _media: dir,
    }
}

/// What the API hands `enqueue`: no user, core binds the actor's.
fn request() -> ImportJobRequest {
    ImportJobRequest {
        wxr: input('a', "application/xml"),
        uploads: Some(input('b', "application/zip")),
        options: ImportJobOptions::default(),
        source_name: "export.xml".into(),
    }
}

/// An uploaded input (P23): an `import` artifact whose hash is `c` repeated.
fn input(c: char, mime: &str) -> Artifact {
    Artifact {
        hash: ArtifactHash::parse(&c.to_string().repeat(64)).unwrap(),
        kind: ArtifactKind::Import,
        bytes: 100,
        mime: mime.into(),
        created_at: Timestamp::now(),
    }
}

/// A `running` run written straight to the store (no lock held, as after a crash) with
/// its heartbeat `secs` ago.
async fn running_run(store: &dyn Store, by: UserId, secs: i64) -> ImportRunId {
    let at = Timestamp::from(Timestamp::now().as_datetime() - chrono::Duration::seconds(secs));
    let run = ImportRun {
        id: ImportRunId::new(),
        source: SOURCE.into(),
        token: [7; 16],
        state: ImportRunState::Running,
        started_by: by,
        started_at: at,
        heartbeat_at: at,
        finished_at: None,
        stats: json!({}),
        dry_run: false,
        job_id: None,
        step: None,
        report_hash: None,
    };
    store.read().imports().create_run(&run).await.unwrap();
    run.id
}

fn payload(user: UserId) -> ImportJobPayload {
    ImportJobPayload {
        v: IMPORT_WORDPRESS_VERSION,
        wxr: "a".repeat(64),
        uploads: Some("b".repeat(64)),
        options: ImportJobOptions::default(),
        user,
        source_name: "export.xml".into(),
    }
}

fn scoped(actor: Actor, scopes: &[&str]) -> Actor {
    let names: Vec<String> = scopes.iter().map(|s| (*s).to_owned()).collect();
    actor.with_scopes(Scopes::from_strs(&names).unwrap())
}

backend_tests!(
    enqueue_requires_import_capability,
    enqueue_conflicts_while_an_import_job_is_queued,
    importer_refuses_a_demoted_or_suspended_admin,
    set_step_is_fenced,
    job_status_links_the_run,
    runs_list_and_get_need_import_read,
    report_round_trips_and_needs_import_read,
    enqueue_takes_over_a_stale_run,
    enqueue_refuses_while_the_import_lock_is_held,
    reports_need_an_artifact_store,
    enqueue_checks_the_bases,
);

/// The payload keeps the CLI's defaults and refuses unknown fields.
#[test]
fn payload_defaults_and_unknown_fields() {
    let options: ImportJobOptions = serde_json::from_value(json!({})).unwrap();
    assert_eq!(
        options,
        ImportJobOptions {
            dry_run: false,
            keep_permalinks: true,
            site_options: true,
            fetch_media: false,
            category_base: "category".into(),
            tag_base: "tag".into(),
        }
    );
    assert_eq!(options, ImportJobOptions::default());
    assert!(serde_json::from_value::<ImportJobOptions>(json!({"takeover": true})).is_err());
    let p = payload(UserId::new());
    let mut value = serde_json::to_value(&p).unwrap();
    assert_eq!(
        serde_json::from_value::<ImportJobPayload>(value.clone()).unwrap(),
        p
    );
    value["extra"] = json!(1);
    assert!(serde_json::from_value::<ImportJobPayload>(value).is_err());
}

/// Only an actor with `import` (and the `import:write` scope) enqueues; the job is an
/// `import.wordpress` job carrying the payload, with three attempts (P27).
pub async fn enqueue_requires_import_capability(db: &TestDb) {
    let f = service(db);
    let store = db.store();
    let editor = actor(store, Role::Editor).await;
    let admin = actor(store, Role::Admin).await;

    for who in [editor, scoped(admin, &["import:read"])] {
        let err = f.svc.check_start(&who).await.unwrap_err();
        assert!(matches!(err, CoreError::Forbidden(_)), "{err:?}");
        let err = f.svc.enqueue(&who, request()).await.unwrap_err();
        assert!(matches!(err, CoreError::Forbidden(_)), "{err:?}");
    }
    f.svc.check_start(&admin).await.unwrap();
    // An input that is not an `import` artifact is refused.
    let mut wrong = request();
    wrong.wxr.kind = ArtifactKind::Media;
    let err = f.svc.enqueue(&admin, wrong).await.unwrap_err();
    assert!(matches!(err, CoreError::Invalid(_)), "{err:?}");
    let id = f
        .svc
        .enqueue(&scoped(admin, &["import:write"]), request())
        .await
        .unwrap();
    let job = store.read().jobs().get(id).await.unwrap().unwrap();
    assert_eq!(job.kind, IMPORT_WORDPRESS_KIND);
    assert_eq!(job.status, JobStatus::Queued);
    assert_eq!(job.max_attempts, 3);
    assert_eq!(job.unique_key, None);
    let back: ImportJobPayload = serde_json::from_value(job.payload).unwrap();
    assert_eq!(back, payload(admin.user));
    // The inputs are recorded as `import` artifacts with the job.
    for a in [request().wxr, request().uploads.unwrap()] {
        let row = store
            .read()
            .artifacts()
            .get(&a.hash)
            .await
            .unwrap()
            .unwrap();
        assert_eq!((row.kind, row.bytes, row.mime), (a.kind, a.bytes, a.mime));
    }

    // The job runs as whoever asked: another admin's request carries their id.
    let other = actor(store, Role::Admin).await;
    let job = store.read().jobs().get(id).await.unwrap().unwrap();
    let lease = store
        .read()
        .jobs()
        .claim(
            NodeId::new(),
            &[IMPORT_WORDPRESS_KIND.to_owned()],
            1,
            Duration::from_secs(60),
        )
        .await
        .unwrap();
    assert_eq!(lease[0].id, job.id);
    assert!(
        store
            .read()
            .jobs()
            .complete(&lease[0].lease().unwrap())
            .await
            .unwrap()
    );
    let second = f.svc.enqueue(&other, request()).await.unwrap();
    let job = store.read().jobs().get(second).await.unwrap().unwrap();
    let back: ImportJobPayload = serde_json::from_value(job.payload).unwrap();
    assert_eq!(back.user, other.user);
}

/// A run whose heartbeat is older than 120 s no longer blocks an enqueue: the admin's
/// request takes it over (it ends `abandoned`). A fresh one still refuses.
pub async fn enqueue_takes_over_a_stale_run(db: &TestDb) {
    let f = service(db);
    let store = db.store();
    let admin = actor(store, Role::Admin).await;

    let fresh = running_run(store, admin.user, 5).await;
    let err = f.svc.check_start(&admin).await.unwrap_err();
    assert!(matches!(err, CoreError::Changed(_)), "{err:?}");
    let err = f.svc.enqueue(&admin, request()).await.unwrap_err();
    assert!(matches!(err, CoreError::Changed(_)), "{err:?}");
    let run = store
        .read()
        .imports()
        .get_run(fresh)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(run.state, ImportRunState::Running);
    store
        .read()
        .imports()
        .set_state(fresh, ImportRunState::Failed, &json!({}), Timestamp::now())
        .await
        .unwrap();

    let stale = running_run(store, admin.user, 300).await;
    // The pre-check lets a stale run pass and leaves it alone.
    f.svc.check_start(&admin).await.unwrap();
    let run = store
        .read()
        .imports()
        .get_run(stale)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(run.state, ImportRunState::Running);
    f.svc.enqueue(&admin, request()).await.unwrap();
    let run = store
        .read()
        .imports()
        .get_run(stale)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(run.state, ImportRunState::Abandoned);
    assert!(run.finished_at.is_some());
}

/// The import lock held elsewhere (a CLI import starting) refuses the enqueue.
pub async fn enqueue_refuses_while_the_import_lock_is_held(db: &TestDb) {
    let f = service(db);
    let store = db.store();
    let admin = actor(store, Role::Admin).await;
    let guard = db
        .shared_store()
        .try_lock(LOCK_IMPORT)
        .await
        .unwrap()
        .unwrap();
    let err = f.svc.enqueue(&admin, request()).await.unwrap_err();
    assert!(matches!(err, CoreError::Changed(_)), "{err:?}");
    guard.release().await.unwrap();
    f.svc.enqueue(&admin, request()).await.unwrap();
}

/// Without an artifact store the report methods are `Unavailable`.
pub async fn reports_need_an_artifact_store(db: &TestDb) {
    let store = db.store();
    let admin = actor(store, Role::Admin).await;
    let svc = ImportService::new(db.shared_store(), Arc::new(PermalinkRules::default()));
    let run = svc.start(&admin, SOURCE, false, false, None).await.unwrap();
    let id = run.run().id;
    let err = svc
        .store_report(id, Bytes::from_static(b"{}"), Bytes::from_static(b"<p>"))
        .await
        .unwrap_err();
    assert!(matches!(err, CoreError::Unavailable(_)), "{err:?}");
    let err = svc
        .report(&admin, id, ReportFormat::Json)
        .await
        .unwrap_err();
    assert!(matches!(err, CoreError::Unavailable(_)), "{err:?}");
}

/// P25: a queued or running `import.wordpress` job, or a `running` run, refuses the
/// next enqueue (`Changed`, the API's 409); once both are over it is accepted again.
pub async fn enqueue_conflicts_while_an_import_job_is_queued(db: &TestDb) {
    let f = service(db);
    let store = db.store();
    let admin = actor(store, Role::Admin).await;
    let conflict = |r: Result<JobId, CoreError>| {
        assert!(matches!(r, Err(CoreError::Changed(_))), "{r:?}");
    };

    let first = f.svc.enqueue(&admin, request()).await.unwrap();
    conflict(f.svc.enqueue(&admin, request()).await);
    // The read-only pre-check agrees.
    let err = f.svc.check_start(&admin).await.unwrap_err();
    assert!(matches!(err, CoreError::Changed(_)), "{err:?}");

    // Claimed: running.
    let kinds = vec![IMPORT_WORDPRESS_KIND.to_owned()];
    let claimed = store
        .read()
        .jobs()
        .claim(NodeId::new(), &kinds, 1, Duration::from_secs(60))
        .await
        .unwrap();
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].id, first);
    conflict(f.svc.enqueue(&admin, request()).await);
    let lease = claimed[0].lease().unwrap();
    assert!(store.read().jobs().complete(&lease).await.unwrap());

    // A running run (the CLI's, here) blocks too, until it ends.
    let run = f
        .svc
        .start(&admin, SOURCE, false, false, None)
        .await
        .unwrap();
    conflict(f.svc.enqueue(&admin, request()).await);
    run.finish(&f.svc, ImportRunState::Done, json!({}))
        .await
        .unwrap();

    // Jobs of other kinds do not count.
    let mut other = ferrite_domain::NewJob::new("media.variants", json!({"v": 1}));
    other.unique_key = Some("other".into());
    store.read().jobs().enqueue(&other).await.unwrap();
    f.svc.enqueue(&admin, request()).await.unwrap();
}

/// The job's importer must still be an active admin (P27): a demoted, disabled or
/// unknown user is `Forbidden`.
pub async fn importer_refuses_a_demoted_or_suspended_admin(db: &TestDb) {
    let f = service(db);
    let store = db.store();
    let admin = user(store, Role::Admin).await;
    let got = f.svc.importer(admin).await.unwrap();
    assert_eq!((got.user, got.role), (admin, Role::Admin));
    assert_eq!(got.scopes, Scopes::ALL);

    let demoted = user(store, Role::Admin).await;
    let mut row = store.read().users().get(demoted).await.unwrap().unwrap();
    row.role = Role::Editor;
    store.read().users().update(&row).await.unwrap();
    let disabled = user(store, Role::Admin).await;
    let mut row = store.read().users().get(disabled).await.unwrap().unwrap();
    row.status = UserStatus::Disabled;
    store.read().users().update(&row).await.unwrap();

    for who in [demoted, disabled, UserId::new()] {
        let err = f.svc.importer(who).await.unwrap_err();
        assert!(matches!(err, CoreError::Forbidden(_)), "{err:?}");
    }
}

/// `set_step` takes only P28's steps and is fenced: after a takeover it is `Changed`
/// and the stored step stays.
pub async fn set_step_is_fenced(db: &TestDb) {
    let f = service(db);
    let store = db.store();
    let admin = actor(store, Role::Admin).await;
    let run = f
        .svc
        .start(&admin, SOURCE, false, false, None)
        .await
        .unwrap();
    for step in [
        "parse", "pass1", "pass2", "pass3", "pass4", "fixup", "report",
    ] {
        run.set_step(&f.svc, step).await.unwrap();
        let got = store.read().imports().get_run(run.run().id).await.unwrap();
        assert_eq!(got.unwrap().step.as_deref(), Some(step));
    }
    for bad in ["", "pass5", "Parse", "done"] {
        let err = run.set_step(&f.svc, bad).await.unwrap_err();
        assert!(matches!(err, CoreError::Invalid(_)), "{bad}: {err:?}");
    }

    store
        .read()
        .imports()
        .set_state(
            run.run().id,
            ImportRunState::Abandoned,
            &json!({}),
            ferrite_domain::Timestamp::now(),
        )
        .await
        .unwrap();
    let err = run.set_step(&f.svc, "pass1").await.unwrap_err();
    assert!(matches!(err, CoreError::Changed(_)), "{err:?}");
    let got = store.read().imports().get_run(run.run().id).await.unwrap();
    assert_eq!(got.unwrap().step.as_deref(), Some("report"));
}

/// `job_status` returns the job and, once the job started it, its run; another kind's
/// job or an unknown id is `NotFound`. It needs `import:read`.
pub async fn job_status_links_the_run(db: &TestDb) {
    let f = service(db);
    let store = db.store();
    let admin = actor(store, Role::Admin).await;
    let job = f.svc.enqueue(&admin, request()).await.unwrap();

    let ImportJobView { job: row, run } = f.svc.job_status(&admin, job).await.unwrap();
    assert_eq!((row.id, row.kind.as_str()), (job, IMPORT_WORDPRESS_KIND));
    assert_eq!(run, None);

    let handle = f
        .svc
        .start(&admin, SOURCE, false, false, Some(job))
        .await
        .unwrap();
    let view = f
        .svc
        .job_status(&scoped(admin, &["import:read"]), job)
        .await
        .unwrap();
    let run = view.run.unwrap();
    assert_eq!((run.id, run.job_id), (handle.run().id, Some(job)));
    // Readers never see the run's fencing token.
    let token = to_hex(&handle.run().token);
    assert!(!format!("{run:?}").contains(&token), "{run:?}");
    let listed = f.svc.list_runs(&admin, 10, None).await.unwrap();
    assert!(!format!("{listed:?}").contains(&token));
    let got = f.svc.get_run(&admin, run.id).await.unwrap();
    assert!(!format!("{got:?}").contains(&token));

    let editor = actor(store, Role::Editor).await;
    for who in [editor, scoped(admin, &["content:read"])] {
        let err = f.svc.job_status(&who, job).await.unwrap_err();
        assert!(matches!(err, CoreError::Forbidden(_)), "{err:?}");
    }
    let mut other = ferrite_domain::NewJob::new("media.variants", json!({"v": 1}));
    other.unique_key = Some("other".into());
    let other = store.read().jobs().enqueue(&other).await.unwrap().unwrap();
    for id in [other, JobId::new()] {
        let err = f.svc.job_status(&admin, id).await.unwrap_err();
        assert!(matches!(err, CoreError::NotFound), "{err:?}");
    }
}

/// `list_runs` pages newest first; `get_run` finds one; both need `import:read`.
pub async fn runs_list_and_get_need_import_read(db: &TestDb) {
    let f = service(db);
    let store = db.store();
    let admin = actor(store, Role::Admin).await;
    let mut ids = Vec::new();
    for _ in 0..3 {
        let run = f
            .svc
            .start(&admin, SOURCE, true, false, None)
            .await
            .unwrap();
        ids.push(run.run().id);
        run.finish(&f.svc, ImportRunState::Done, json!({}))
            .await
            .unwrap();
    }
    ids.reverse();
    let reader = scoped(admin, &["import:read"]);
    let first = f.svc.list_runs(&reader, 2, None).await.unwrap();
    assert_eq!(first.iter().map(|r| r.id).collect::<Vec<_>>(), ids[..2]);
    let last = first.last().unwrap();
    let rest = f
        .svc
        .list_runs(&reader, 2, Some((last.started_at, last.id.as_uuid())))
        .await
        .unwrap();
    assert_eq!(rest.iter().map(|r| r.id).collect::<Vec<_>>(), ids[2..]);
    assert_eq!(f.svc.get_run(&reader, ids[0]).await.unwrap().id, ids[0]);
    assert!(matches!(
        f.svc
            .get_run(&reader, ferrite_domain::ImportRunId::new())
            .await,
        Err(CoreError::NotFound)
    ));

    let editor = actor(store, Role::Editor).await;
    for who in [editor, scoped(admin, &["content:read"])] {
        assert!(matches!(
            f.svc.list_runs(&who, 10, None).await,
            Err(CoreError::Forbidden(_))
        ));
        assert!(matches!(
            f.svc.get_run(&who, ids[0]).await,
            Err(CoreError::Forbidden(_))
        ));
    }
}

/// `store_report` keeps both files as `import` artifacts and points the run at them; a
/// later attempt replaces them. `report` returns either file to an `import:read`
/// reader; a run without a report is `NotFound`.
pub async fn report_round_trips_and_needs_import_read(db: &TestDb) {
    let f = service(db);
    let store = db.store();
    let admin = actor(store, Role::Admin).await;
    let handle = f
        .svc
        .start(&admin, SOURCE, false, false, None)
        .await
        .unwrap();
    let id = handle.run().id;
    assert!(matches!(
        f.svc.report(&admin, id, ReportFormat::Json).await,
        Err(CoreError::NotFound)
    ));

    let json_v1 = Bytes::from_static(br#"{"attempt":1}"#);
    let html_v1 = Bytes::from_static(b"<!doctype html><p>attempt 1</p>");
    f.svc
        .store_report(id, json_v1.clone(), html_v1.clone())
        .await
        .unwrap();
    assert_eq!(
        f.svc.report(&admin, id, ReportFormat::Json).await.unwrap(),
        json_v1
    );
    // A failed run keeps its report; a later attempt replaces it.
    handle
        .finish(&f.svc, ImportRunState::Failed, json!({}))
        .await
        .unwrap();
    let json_v2 = Bytes::from_static(br#"{"attempt":2}"#);
    let html_v2 = Bytes::from_static(b"<!doctype html><p>attempt 2</p>");
    f.svc
        .store_report(id, json_v2.clone(), html_v2.clone())
        .await
        .unwrap();
    let reader = scoped(admin, &["import:read"]);
    assert_eq!(
        f.svc.report(&reader, id, ReportFormat::Json).await.unwrap(),
        json_v2
    );
    assert_eq!(
        f.svc.report(&reader, id, ReportFormat::Html).await.unwrap(),
        html_v2
    );

    // Every file is an `import` artifact row.
    let manifest = store.read().imports().get_run(id).await.unwrap().unwrap();
    let manifest = ArtifactHash::parse(&manifest.report_hash.unwrap()).unwrap();
    let artifacts = store.read().artifacts();
    let row = artifacts.get(&manifest).await.unwrap().unwrap();
    assert_eq!(
        (row.kind, row.mime.as_str()),
        (ArtifactKind::Import, "application/json")
    );
    for (body, mime) in [(&json_v2, "application/json"), (&html_v2, "text/html")] {
        let hash = ArtifactHash::parse(&hex_sha256(body)).unwrap();
        let row = artifacts.get(&hash).await.unwrap().unwrap();
        assert_eq!(row.kind, ArtifactKind::Import);
        assert!(row.mime.starts_with(mime), "{}", row.mime);
        assert_eq!(row.bytes, body.len() as i64);
    }

    let editor = actor(store, Role::Editor).await;
    for who in [editor, scoped(admin, &["content:read"])] {
        assert!(matches!(
            f.svc.report(&who, id, ReportFormat::Html).await,
            Err(CoreError::Forbidden(_))
        ));
    }

    // A manifest of another version, or over 1 KiB, is refused.
    let json_hash = hex_sha256(&json_v2);
    let other_version = format!(r#"{{"v":2,"json":"{json_hash}","html":"{json_hash}"}}"#);
    let padded = format!(
        r#"{{"v":1,"json":"{json_hash}","html":"{json_hash}"{}}}"#,
        " ".repeat(1100)
    );
    for manifest in [other_version, padded] {
        let hash = f.blobs.put(Bytes::from(manifest)).await.unwrap();
        store
            .read()
            .imports()
            .set_report(id, hash.as_str())
            .await
            .unwrap();
        let err = f
            .svc
            .report(&admin, id, ReportFormat::Json)
            .await
            .unwrap_err();
        assert!(matches!(err, CoreError::Internal(_)), "{err:?}");
    }
    assert!(matches!(
        f.svc
            .store_report(
                ferrite_domain::ImportRunId::new(),
                json_v2.clone(),
                html_v2.clone()
            )
            .await,
        Err(CoreError::NotFound)
    ));
}

fn hex_sha256(data: &[u8]) -> String {
    use sha2::Digest;
    ferrite_domain::to_hex(&sha2::Sha256::digest(data))
}

/// The category and tag bases are paths of URL-safe segments (surrounding slashes
/// ignored); anything else is `InvalidField` and nothing is queued.
pub async fn enqueue_checks_the_bases(db: &TestDb) {
    let f = service(db);
    let store = db.store();
    let admin = actor(store, Role::Admin).await;
    let with = |category: &str, tag: &str| {
        let mut r = request();
        r.options.category_base = category.into();
        r.options.tag_base = tag.into();
        r
    };
    for (category, tag, field) in [
        ("", "tag", "category_base"),
        ("/", "tag", "category_base"),
        ("a//b", "tag", "category_base"),
        ("category", "..", "tag_base"),
        ("category", "a b", "tag_base"),
        ("category", "x?y", "tag_base"),
        ("category", &"t".repeat(101), "tag_base"),
    ] {
        let err = f
            .svc
            .enqueue(&admin, with(category, tag))
            .await
            .unwrap_err();
        assert!(
            matches!(&err, CoreError::InvalidField { field: f, .. } if *f == field),
            "{category:?}/{tag:?}: {err:?}"
        );
    }
    let jobs = store.read().jobs();
    let query = ferrite_domain::JobQuery {
        status: None,
        kind: Some(IMPORT_WORDPRESS_KIND.into()),
        limit: 10,
        cursor: None,
    };
    assert!(jobs.list(&query).await.unwrap().items.is_empty());
    f.svc
        .enqueue(&admin, with("/topics/", "blog/tags-2"))
        .await
        .unwrap();
}
