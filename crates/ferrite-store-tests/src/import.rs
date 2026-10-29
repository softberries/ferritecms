//! Contract tests for the WordPress import tables (`0009_import`, M3-1 Task 1, P3, P5)
//! and for explicit content timestamps.

use std::{sync::Arc, time::Duration};

use ferrite_domain::{
    Artifact, ArtifactHash, ArtifactKind, ImportMapKind, ImportMapRow, ImportRun, ImportRunId,
    ImportRunState, JobId, Timestamp, UserId,
};
use ferrite_store::{Backend, StoreError};
use serde_json::json;
use uuid::Uuid;

use crate::{
    TestDb,
    fixtures::{fixture_user, new_post},
};

/// A `running` run of `source` by `user` with a random token.
pub fn new_run(user: UserId, source: &str) -> ImportRun {
    let now = Timestamp::from_micros(1_700_000_000_123_456).unwrap();
    ImportRun {
        id: ImportRunId::new(),
        source: source.into(),
        token: *Uuid::now_v7().as_bytes(),
        state: ImportRunState::Running,
        started_by: user,
        started_at: now,
        heartbeat_at: now,
        finished_at: None,
        stats: json!({"items": {"created": 0}}),
        dry_run: false,
        job_id: None,
        step: None,
        report_hash: None,
    }
}

/// A map row of (`source`, `kind`, `wp_id`) with a fresh target.
pub fn map_row(source: &str, kind: ImportMapKind, wp_id: i64) -> ImportMapRow {
    ImportMapRow {
        source: source.into(),
        kind,
        wp_id,
        ferrite_id: Uuid::now_v7(),
        hash: "a".repeat(64),
        version: 1,
    }
}

fn later(micros: i64) -> Timestamp {
    Timestamp::from_micros(1_700_000_000_123_456 + micros).unwrap()
}

/// A run created for a job is found by [`run_by_job`](ferrite_store::ImportRepo::run_by_job)
/// (the newest of that job's runs; none for another job). `set_step` is fenced like a
/// batch: the wrong token, or a run that is no longer `running`, changes nothing and
/// returns `false`.
pub async fn import_run_job_link_and_step(db: &TestDb) {
    let store = db.store();
    let repos = store.read();
    let imports = repos.imports();
    let admin = fixture_user(store).await;
    let job = JobId::new();

    let mut first = new_run(admin.id, "https://wp.example");
    first.job_id = Some(job);
    imports.create_run(&first).await.unwrap();
    assert_eq!(
        imports.get_run(first.id).await.unwrap().as_ref(),
        Some(&first)
    );
    assert_eq!(
        imports.run_by_job(job).await.unwrap().as_ref(),
        Some(&first)
    );
    assert_eq!(imports.run_by_job(JobId::new()).await.unwrap(), None);

    assert!(!imports.set_step(first.id, &[0; 16], "parse").await.unwrap());
    assert_eq!(imports.get_run(first.id).await.unwrap().unwrap().step, None);
    assert!(
        imports
            .set_step(first.id, &first.token, "parse")
            .await
            .unwrap()
    );
    assert!(
        imports
            .set_step(first.id, &first.token, "pass1")
            .await
            .unwrap()
    );
    assert_eq!(
        imports
            .get_run(first.id)
            .await
            .unwrap()
            .unwrap()
            .step
            .as_deref(),
        Some("pass1")
    );
    assert!(
        !imports
            .set_step(ImportRunId::new(), &first.token, "pass1")
            .await
            .unwrap()
    );
    assert!(matches!(
        imports.set_step(first.id, &first.token, "a\0b").await,
        Err(StoreError::Invalid(_))
    ));

    // A retry of the same job abandons the first run and starts a newer one.
    imports
        .set_state(first.id, ImportRunState::Abandoned, &json!({}), later(5))
        .await
        .unwrap();
    assert!(
        !imports
            .set_step(first.id, &first.token, "pass2")
            .await
            .unwrap()
    );
    let mut second = new_run(admin.id, "https://wp.example");
    second.job_id = Some(job);
    second.started_at = later(10);
    second.heartbeat_at = later(10);
    imports.create_run(&second).await.unwrap();
    assert_eq!(
        imports.run_by_job(job).await.unwrap().map(|r| r.id),
        Some(second.id)
    );
}

/// `set_report` records the report artifact's hash on any run, finished or not; a
/// missing run is `NotFound`.
pub async fn import_run_report_hash(db: &TestDb) {
    let store = db.store();
    let repos = store.read();
    let imports = repos.imports();
    let admin = fixture_user(store).await;
    let run = new_run(admin.id, "https://wp.example");
    imports.create_run(&run).await.unwrap();
    let hash = "b".repeat(64);

    imports.set_report(run.id, &hash).await.unwrap();
    assert_eq!(
        imports
            .get_run(run.id)
            .await
            .unwrap()
            .unwrap()
            .report_hash
            .as_deref(),
        Some(hash.as_str())
    );
    imports
        .set_state(run.id, ImportRunState::Failed, &json!({}), later(5))
        .await
        .unwrap();
    let other = "c".repeat(64);
    imports.set_report(run.id, &other).await.unwrap();
    assert_eq!(
        imports.get_run(run.id).await.unwrap().unwrap().report_hash,
        Some(other)
    );
    assert!(matches!(
        imports.set_report(ImportRunId::new(), &hash).await,
        Err(StoreError::NotFound)
    ));
    assert!(matches!(
        imports.set_report(run.id, "a\0b").await,
        Err(StoreError::Invalid(_))
    ));
}

/// `list_runs_page` is newest first by (`started_at`, `id`); paging with the last row's
/// pair as the cursor visits every run once, even when they share `started_at`.
pub async fn import_runs_page_cursor(db: &TestDb) {
    let store = db.store();
    let repos = store.read();
    let imports = repos.imports();
    let admin = fixture_user(store).await;
    let mut ids = Vec::new();
    for n in 0..5 {
        let mut run = new_run(admin.id, "https://wp.example");
        run.state = ImportRunState::Done;
        run.finished_at = Some(later(1));
        run.source = format!("https://wp{n}.example");
        imports.create_run(&run).await.unwrap();
        ids.push(run.id);
    }
    let mut older = new_run(admin.id, "https://old.example");
    older.state = ImportRunState::Done;
    older.started_at = later(-10);
    older.finished_at = Some(later(-5));
    imports.create_run(&older).await.unwrap();
    // Same `started_at`: the id breaks the tie, highest first.
    ids.sort_by_key(|id| std::cmp::Reverse(id.as_uuid()));
    ids.push(older.id);

    let mut seen = Vec::new();
    let mut cursor = None;
    loop {
        let page = imports.list_runs_page(2, cursor).await.unwrap();
        assert!(page.len() <= 2);
        let Some(last) = page.last() else { break };
        cursor = Some((last.started_at, last.id.as_uuid()));
        seen.extend(page.iter().map(|r| r.id));
    }
    assert_eq!(seen, ids);
    assert_eq!(imports.list_runs_page(10, None).await.unwrap().len(), 6);
    assert!(matches!(
        imports.list_runs_page(0, None).await,
        Err(StoreError::Invalid(_))
    ));
}

/// An artifact of kind `import` (an uploaded WXR or uploads zip, P23) round-trips.
pub async fn artifact_kind_import_round_trips(db: &TestDb) {
    let artifacts = db.store().read().artifacts();
    let artifact = Artifact {
        hash: ArtifactHash::parse(&"e".repeat(64)).unwrap(),
        kind: ArtifactKind::Import,
        bytes: 4096,
        mime: "application/xml".into(),
        created_at: later(0),
    };
    assert!(artifacts.insert_if_absent(&artifact).await.unwrap());
    assert_eq!(
        artifacts.get(&artifact.hash).await.unwrap().as_ref(),
        Some(&artifact)
    );
    assert_eq!(
        "import".parse::<ArtifactKind>().unwrap(),
        ArtifactKind::Import
    );
}

/// A run round-trips exactly (the JSON stats and the token included); `running()` finds
/// it until `set_state(Done)`, which sets `finished_at` and replaces the stats. A second
/// `running` run is a unique violation, `set_state` on a missing run is `NotFound`, and
/// `list_runs` is newest first.
pub async fn import_run_roundtrip_and_running(db: &TestDb) {
    let store = db.store();
    let repos = store.read();
    let imports = repos.imports();
    let admin = fixture_user(store).await;
    let run = new_run(admin.id, "https://wp.example");

    assert_eq!(imports.running().await.unwrap(), None);
    imports.create_run(&run).await.unwrap();
    assert_eq!(imports.get_run(run.id).await.unwrap().as_ref(), Some(&run));
    assert_eq!(imports.running().await.unwrap().as_ref(), Some(&run));

    let mut second = new_run(admin.id, "https://other.example");
    second.started_at = later(10);
    let err = imports.create_run(&second).await.unwrap_err();
    assert!(
        matches!(&err, StoreError::UniqueViolation { constraint } if constraint == "import_runs_running_uq"),
        "{err:?}"
    );

    let stats = json!({"items": {"created": 5, "skipped": 1}});
    imports
        .set_state(run.id, ImportRunState::Done, &stats, later(5))
        .await
        .unwrap();
    assert_eq!(imports.running().await.unwrap(), None);
    let done = imports.get_run(run.id).await.unwrap().unwrap();
    assert_eq!(done.state, ImportRunState::Done);
    assert_eq!(done.finished_at, Some(later(5)));
    assert_eq!(done.stats, stats);

    // A finished run does not block the next one; the list is newest first.
    imports.create_run(&second).await.unwrap();
    let listed = imports.list_runs(10).await.unwrap();
    assert_eq!(
        listed.iter().map(|r| r.id).collect::<Vec<_>>(),
        [second.id, run.id]
    );
    assert_eq!(imports.list_runs(1).await.unwrap().len(), 1);
    assert!(matches!(
        imports.list_runs(0).await,
        Err(StoreError::Invalid(_))
    ));

    // `running` has no finish time.
    imports
        .set_state(second.id, ImportRunState::Running, &json!({}), later(20))
        .await
        .unwrap();
    assert_eq!(
        imports
            .get_run(second.id)
            .await
            .unwrap()
            .unwrap()
            .finished_at,
        None
    );
    assert!(matches!(
        imports
            .set_state(
                ImportRunId::new(),
                ImportRunState::Failed,
                &json!({}),
                later(1)
            )
            .await,
        Err(StoreError::NotFound)
    ));
    assert!(matches!(
        imports
            .set_state(
                second.id,
                ImportRunState::Failed,
                &json!({"a\0": 1}),
                later(1)
            )
            .await,
        Err(StoreError::Invalid(_))
    ));
}

/// The fence is true only for the matching token of a `running` run, and moves
/// `heartbeat_at`; a wrong token, an unknown run and a finished run are false and leave
/// the row alone.
pub async fn fence_true_only_for_matching_running_token(db: &TestDb) {
    let store = db.store();
    let repos = store.read();
    let imports = repos.imports();
    let admin = fixture_user(store).await;
    let run = new_run(admin.id, "https://wp.example");
    imports.create_run(&run).await.unwrap();

    let mut wrong = run.token;
    wrong[0] ^= 0xff;
    assert!(!imports.fence(run.id, &wrong, later(1)).await.unwrap());
    assert!(
        !imports
            .fence(ImportRunId::new(), &run.token, later(1))
            .await
            .unwrap()
    );
    let untouched = imports.get_run(run.id).await.unwrap().unwrap();
    assert_eq!(untouched.heartbeat_at, run.heartbeat_at);

    assert!(imports.fence(run.id, &run.token, later(2)).await.unwrap());
    let beat = imports.get_run(run.id).await.unwrap().unwrap();
    assert_eq!(beat.heartbeat_at, later(2));
    assert_eq!(beat.state, ImportRunState::Running);

    imports
        .set_state(run.id, ImportRunState::Abandoned, &json!({}), later(3))
        .await
        .unwrap();
    assert!(!imports.fence(run.id, &run.token, later(4)).await.unwrap());
    assert_eq!(
        imports.get_run(run.id).await.unwrap().unwrap().heartbeat_at,
        later(2)
    );
}

/// `abandon_stale` abandons only a `running` run whose heartbeat is older than
/// `stale_before` (strictly), sets `finished_at` and keeps the stats; a fresh heartbeat,
/// a finished run and an unknown run are `false` and leave the row alone.
pub async fn abandon_stale_only_when_still_stale(db: &TestDb) {
    let store = db.store();
    let repos = store.read();
    let imports = repos.imports();
    let admin = fixture_user(store).await;
    let run = new_run(admin.id, "https://wp.example");
    imports.create_run(&run).await.unwrap();

    // Heartbeat == stale_before is not stale.
    assert!(
        !imports
            .abandon_stale(run.id, run.heartbeat_at, later(9))
            .await
            .unwrap()
    );
    assert!(
        !imports
            .abandon_stale(ImportRunId::new(), later(5), later(9))
            .await
            .unwrap()
    );
    assert_eq!(imports.get_run(run.id).await.unwrap().unwrap(), run);

    assert!(
        imports
            .abandon_stale(run.id, later(1), later(9))
            .await
            .unwrap()
    );
    let gone = imports.get_run(run.id).await.unwrap().unwrap();
    assert_eq!(gone.state, ImportRunState::Abandoned);
    assert_eq!(gone.finished_at, Some(later(9)));
    assert_eq!(gone.stats, run.stats);
    assert_eq!(gone.heartbeat_at, run.heartbeat_at);
    // Already abandoned: false, unchanged.
    assert!(
        !imports
            .abandon_stale(run.id, later(5), later(10))
            .await
            .unwrap()
    );
    assert_eq!(
        imports.get_run(run.id).await.unwrap().unwrap().finished_at,
        Some(later(9))
    );
}

/// `abandon_stale_for_job` abandons a stale run only when it has no job, is the
/// starting job's own, or its job is not `running` (no such job either); a stale run of
/// another `running` job, and a fresh one, stay.
pub async fn abandon_stale_for_job_spares_live_jobs(db: &TestDb) {
    let store = db.store();
    let repos = store.read();
    let imports = repos.imports();
    let jobs = repos.jobs();
    let admin = fixture_user(store).await;
    let job = |kind: &str| ferrite_domain::NewJob::new(kind, json!({}));
    let me = jobs.enqueue(&job("t.me")).await.unwrap().unwrap();
    let live = jobs.enqueue(&job("t.live")).await.unwrap().unwrap();
    let queued = jobs.enqueue(&job("t.queued")).await.unwrap().unwrap();
    jobs.claim(
        ferrite_domain::NodeId::new(),
        &["t.live".to_owned()],
        1,
        Duration::from_secs(600),
    )
    .await
    .unwrap();
    for (owner, abandoned) in [
        (None, true),
        (Some(me), true),
        (Some(queued), true),
        (Some(JobId::new()), true),
        (Some(live), false),
    ] {
        let run = ImportRun {
            job_id: owner,
            ..new_run(admin.id, "https://wp.example")
        };
        imports.create_run(&run).await.unwrap();
        // Fresh: never.
        assert!(
            !imports
                .abandon_stale_for_job(run.id, run.heartbeat_at, later(9), me)
                .await
                .unwrap()
        );
        assert_eq!(
            imports
                .abandon_stale_for_job(run.id, later(1), later(9), me)
                .await
                .unwrap(),
            abandoned,
            "{owner:?}"
        );
        let state = imports.get_run(run.id).await.unwrap().unwrap().state;
        if abandoned {
            assert_eq!(state, ImportRunState::Abandoned);
        } else {
            assert_eq!(state, ImportRunState::Running);
            assert!(
                imports
                    .abandon_stale(run.id, later(1), later(9))
                    .await
                    .unwrap()
            );
        }
    }
}

/// Postgres only: a takeover's `abandon_stale` waits for a batch that fenced (fresh
/// heartbeat) and then sees the new heartbeat, so it abandons nothing.
pub async fn abandon_stale_rechecks_after_fence(db: Arc<TestDb>) {
    if db.store().backend() != Backend::Postgres {
        println!("SKIP abandon_stale_rechecks_after_fence: single writer");
        return;
    }
    let admin = fixture_user(db.store()).await;
    let run = new_run(admin.id, "https://wp.example");
    db.store().read().imports().create_run(&run).await.unwrap();

    let batch = db.store().begin().await.unwrap();
    assert!(
        batch
            .imports()
            .fence(run.id, &run.token, later(100))
            .await
            .unwrap()
    );
    let taker = {
        let db = db.clone();
        let id = run.id;
        tokio::spawn(async move {
            let tx = db.store().begin().await.unwrap();
            let abandoned = tx
                .imports()
                .abandon_stale(id, later(50), later(101))
                .await
                .unwrap();
            tx.commit().await.unwrap();
            abandoned
        })
    };
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!taker.is_finished(), "takeover did not wait for the batch");
    batch.commit().await.unwrap();
    let abandoned = tokio::time::timeout(Duration::from_secs(10), taker)
        .await
        .expect("takeover finishes after the batch commits")
        .unwrap();
    assert!(!abandoned, "a run that just fenced is not stale");
    assert_eq!(
        db.store()
            .read()
            .imports()
            .get_run(run.id)
            .await
            .unwrap()
            .unwrap()
            .state,
        ImportRunState::Running
    );
}

/// Postgres only (SQLite has a single writer): a batch that fenced and holds its
/// transaction makes a takeover's `set_state(Abandoned)` wait; after the batch commits
/// the takeover finishes and the old token's next fence is false.
pub async fn fence_serialises_with_takeover(db: Arc<TestDb>) {
    if db.store().backend() != Backend::Postgres {
        println!("SKIP fence_serialises_with_takeover: single writer");
        return;
    }
    let admin = fixture_user(db.store()).await;
    let run = new_run(admin.id, "https://wp.example");
    db.store().read().imports().create_run(&run).await.unwrap();

    let batch = db.store().begin().await.unwrap();
    assert!(
        batch
            .imports()
            .fence(run.id, &run.token, later(1))
            .await
            .unwrap()
    );

    let taker = {
        let db = db.clone();
        let id = run.id;
        tokio::spawn(async move {
            let tx = db.store().begin().await.unwrap();
            tx.imports()
                .set_state(id, ImportRunState::Abandoned, &json!({}), later(2))
                .await
                .unwrap();
            tx.commit().await.unwrap();
        })
    };
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!taker.is_finished(), "takeover did not wait for the batch");

    batch.commit().await.unwrap();
    tokio::time::timeout(Duration::from_secs(10), taker)
        .await
        .expect("takeover finishes after the batch commits")
        .unwrap();

    let tx = db.store().begin().await.unwrap();
    assert!(
        !tx.imports()
            .fence(run.id, &run.token, later(3))
            .await
            .unwrap()
    );
    tx.rollback().await.unwrap();
    assert_eq!(
        db.store()
            .read()
            .imports()
            .get_run(run.id)
            .await
            .unwrap()
            .unwrap()
            .state,
        ImportRunState::Abandoned
    );
}

/// `map_put` upserts by (source, kind, wp_id); `map_get` finds a row by all three.
pub async fn import_map_upsert_and_get(db: &TestDb) {
    let store = db.store();
    let repos = store.read();
    let imports = repos.imports();
    let a = "https://a.example";
    let b = "https://b.example";

    assert_eq!(
        imports.map_get(a, ImportMapKind::Term, 3).await.unwrap(),
        None
    );
    let first = map_row(a, ImportMapKind::Term, 3);
    imports.map_put(&first).await.unwrap();
    assert_eq!(
        imports
            .map_get(a, ImportMapKind::Term, 3)
            .await
            .unwrap()
            .as_ref(),
        Some(&first)
    );

    let again = ImportMapRow {
        ferrite_id: Uuid::now_v7(),
        hash: "b".repeat(64),
        version: 4,
        ..first.clone()
    };
    imports.map_put(&again).await.unwrap();
    assert_eq!(
        imports.map_get(a, ImportMapKind::Term, 3).await.unwrap(),
        Some(again.clone())
    );

    let low = map_row(a, ImportMapKind::Term, 1);
    imports.map_put(&low).await.unwrap();
    // Same wp_id under another kind or source is another row.
    imports
        .map_put(&map_row(a, ImportMapKind::Content, 3))
        .await
        .unwrap();
    imports
        .map_put(&map_row(b, ImportMapKind::Term, 3))
        .await
        .unwrap();
    assert_eq!(
        imports.map_get(a, ImportMapKind::Term, 1).await.unwrap(),
        Some(low)
    );
    assert_eq!(
        imports.map_get(a, ImportMapKind::Term, 3).await.unwrap(),
        Some(again)
    );
    assert!(
        imports
            .map_get(a, ImportMapKind::Content, 3)
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        imports
            .map_get(b, ImportMapKind::Term, 3)
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        imports
            .map_get(b, ImportMapKind::Menu, 3)
            .await
            .unwrap()
            .is_none()
    );
    // Every kind is accepted, `option` and `redirect` (what the import wrote) included.
    for kind in [ImportMapKind::Option, ImportMapKind::Redirect] {
        let row = map_row(a, kind, 3);
        imports.map_put(&row).await.unwrap();
        assert_eq!(imports.map_get(a, kind, 3).await.unwrap(), Some(row));
    }

    let bad = ImportMapRow {
        hash: "x\0".into(),
        ..map_row(a, ImportMapKind::User, 9)
    };
    assert!(matches!(
        imports.map_put(&bad).await,
        Err(StoreError::Invalid(_))
    ));
}

/// `NewContent` timestamps set both columns exactly (micros); `None` means now.
pub async fn content_insert_keeps_explicit_timestamps(db: &TestDb) {
    let store = db.store();
    let repos = store.read();
    let admin = fixture_user(store).await;
    let t1 = Timestamp::from_micros(1_262_304_000_123_456).unwrap();
    let t2 = Timestamp::from_micros(1_262_390_400_654_321).unwrap();

    let mut new = new_post(&admin, "imported");
    new.created_at = Some(t1);
    new.updated_at = Some(t2);
    let item = repos.content().insert(&new).await.unwrap();
    assert_eq!((item.created_at, item.updated_at), (t1, t2));
    let got = repos.content().get(item.id).await.unwrap().unwrap();
    assert_eq!((got.created_at, got.updated_at), (t1, t2));

    // Only one of them: the other is now.
    let before = Timestamp::now();
    let mut half = new_post(&admin, "half-imported");
    half.created_at = Some(t1);
    let item = repos.content().insert(&half).await.unwrap();
    assert_eq!(item.created_at, t1);
    assert!(item.updated_at >= before);

    let plain = repos
        .content()
        .insert(&new_post(&admin, "plain"))
        .await
        .unwrap();
    assert!(plain.created_at >= before && plain.updated_at >= before);
}
