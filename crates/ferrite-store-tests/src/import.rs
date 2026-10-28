//! Contract tests for the WordPress import tables (`0009_import`, M3-1 Task 1, P3, P5)
//! and for explicit content timestamps.

use std::{sync::Arc, time::Duration};

use ferrite_domain::{
    ImportMapKind, ImportMapRow, ImportRun, ImportRunId, ImportRunState, Timestamp, UserId,
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

/// `map_put` upserts by (source, kind, wp_id); `map_get` and `map_list` filter by
/// source and kind, and the list is ordered by `wp_id`.
pub async fn import_map_upsert_and_list(db: &TestDb) {
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
        imports.map_list(a, ImportMapKind::Term).await.unwrap(),
        std::slice::from_ref(&again)
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
        imports.map_list(a, ImportMapKind::Term).await.unwrap(),
        [low, again]
    );
    assert_eq!(
        imports
            .map_list(a, ImportMapKind::Content)
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        imports
            .map_list(b, ImportMapKind::Term)
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(
        imports
            .map_list(b, ImportMapKind::Menu)
            .await
            .unwrap()
            .is_empty()
    );
    // Every kind is accepted, `option` (site options the import wrote) included.
    let option = map_row(a, ImportMapKind::Option, 3);
    imports.map_put(&option).await.unwrap();
    assert_eq!(
        imports.map_get(a, ImportMapKind::Option, 3).await.unwrap(),
        Some(option)
    );

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
