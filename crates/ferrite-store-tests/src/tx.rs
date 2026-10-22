//! Transaction semantics shared by both backends: poisoning (R18), nested transactions
//! (savepoints) and the SQLite single-writer timeout (I2).

use std::time::{Duration, Instant};

use ferrite_domain::ContentId;
use ferrite_store::{Backend, StoreError};
use serde_json::json;

use crate::{
    TestDb,
    fixtures::{fixture_user, new_post},
};

/// A constraint error inside a `Tx` aborts it: later calls give `Aborted`, `commit`
/// rolls back and returns `Err(Aborted)`, and nothing is persisted.
pub async fn tx_unique_violation_poisons(db: &TestDb) {
    let store = db.store();
    let author = fixture_user(store).await;
    let tx = store.begin().await.unwrap();
    tx.options().set("before", &json!(1), false).await.unwrap();
    let first = tx
        .content()
        .insert(&new_post(&author, "dup"))
        .await
        .unwrap();
    let err = tx
        .content()
        .insert(&new_post(&author, "dup"))
        .await
        .unwrap_err();
    assert!(matches!(err, StoreError::UniqueViolation { .. }), "{err:?}");
    let next = tx.options().set("after", &json!(2), false).await;
    assert!(matches!(next, Err(StoreError::Aborted)), "{next:?}");
    let read = tx.content().get(first.id).await;
    assert!(matches!(read, Err(StoreError::Aborted)), "{read:?}");
    let commit = tx.commit().await;
    assert!(matches!(commit, Err(StoreError::Aborted)), "{commit:?}");

    let r = store.read();
    assert!(r.content().get(first.id).await.unwrap().is_none());
    assert!(r.options().get("before").await.unwrap().is_none());
    assert!(r.options().get("after").await.unwrap().is_none());

    // The connection is clean again afterwards.
    let tx = store.begin().await.unwrap();
    tx.options().set("later", &json!(3), false).await.unwrap();
    tx.commit().await.unwrap();
    assert!(r.options().get("later").await.unwrap().is_some());
}

/// A foreign-key error poisons the `Tx` the same way.
pub async fn tx_foreign_key_poisons(db: &TestDb) {
    let store = db.store();
    let author = fixture_user(store).await;
    let tx = store.begin().await.unwrap();
    tx.options().set("before", &json!(1), false).await.unwrap();
    let mut orphan = new_post(&author, "orphan");
    orphan.parent_id = Some(ContentId::new());
    let err = tx.content().insert(&orphan).await.unwrap_err();
    assert!(matches!(err, StoreError::ForeignKey { .. }), "{err:?}");
    let next = tx.options().get("before").await;
    assert!(matches!(next, Err(StoreError::Aborted)), "{next:?}");
    let commit = tx.commit().await;
    assert!(matches!(commit, Err(StoreError::Aborted)), "{commit:?}");
    assert!(
        store
            .read()
            .options()
            .get("before")
            .await
            .unwrap()
            .is_none()
    );
}

/// Validation errors never reach the database, so they do not poison the `Tx`.
pub async fn tx_validation_error_does_not_poison(db: &TestDb) {
    let store = db.store();
    let tx = store.begin().await.unwrap();
    tx.options().set("kept", &json!(1), false).await.unwrap();
    let bad = tx.options().set("nul\u{0}key", &json!(1), false).await;
    assert!(matches!(bad, Err(StoreError::Invalid(_))), "{bad:?}");
    let bad = tx.options().set("k", &json!("nul\u{0}value"), false).await;
    assert!(matches!(bad, Err(StoreError::Invalid(_))), "{bad:?}");
    tx.options().set("also", &json!(2), false).await.unwrap();
    tx.commit().await.unwrap();
    let r = store.read();
    assert!(r.options().get("kept").await.unwrap().is_some());
    assert!(r.options().get("also").await.unwrap().is_some());
}

/// A failed nested transaction (savepoint) is contained: the outer `Tx` stays usable and
/// commits; the nested work is gone.
pub async fn tx_nested_failure_keeps_outer(db: &TestDb) {
    let store = db.store();
    let author = fixture_user(store).await;
    let tx = store.begin().await.unwrap();
    let first = tx
        .content()
        .insert(&new_post(&author, "dup"))
        .await
        .unwrap();

    let nested = tx.begin_nested().await.unwrap();
    nested
        .options()
        .set("inner", &json!(1), false)
        .await
        .unwrap();
    let err = nested
        .content()
        .insert(&new_post(&author, "dup"))
        .await
        .unwrap_err();
    assert!(matches!(err, StoreError::UniqueViolation { .. }), "{err:?}");
    let next = nested.options().get("inner").await;
    assert!(matches!(next, Err(StoreError::Aborted)), "{next:?}");
    let commit = nested.commit().await;
    assert!(matches!(commit, Err(StoreError::Aborted)), "{commit:?}");

    // The outer transaction is untouched by the nested failure.
    assert!(tx.options().get("inner").await.unwrap().is_none());
    tx.options().set("outer", &json!(2), false).await.unwrap();
    tx.commit().await.unwrap();

    let r = store.read();
    assert!(r.content().get(first.id).await.unwrap().is_some());
    assert!(r.options().get("outer").await.unwrap().is_some());
    assert!(r.options().get("inner").await.unwrap().is_none());
}

/// Nested transactions commit into, roll back from, and (when dropped) roll back from the
/// outer one; the outer `Tx` refuses calls while a nested one is open.
pub async fn tx_nested_commit_rollback_drop(db: &TestDb) {
    let store = db.store();
    let tx = store.begin().await.unwrap();

    let kept = tx.begin_nested().await.unwrap();
    kept.options().set("kept", &json!(1), false).await.unwrap();
    // Two levels deep.
    let deep = kept.begin_nested().await.unwrap();
    deep.options().set("deep", &json!(1), false).await.unwrap();
    deep.commit().await.unwrap();
    kept.commit().await.unwrap();

    let undone = tx.begin_nested().await.unwrap();
    undone
        .options()
        .set("undone", &json!(1), false)
        .await
        .unwrap();
    let busy = tx.options().get("kept").await;
    assert!(matches!(busy, Err(StoreError::NestedOpen)), "{busy:?}");
    undone.rollback().await.unwrap();

    let dropped = tx.begin_nested().await.unwrap();
    dropped
        .options()
        .set("dropped", &json!(1), false)
        .await
        .unwrap();
    drop(dropped);

    // The outer transaction is usable and not poisoned by any of this.
    assert!(tx.options().get("kept").await.unwrap().is_some());
    assert!(tx.options().get("undone").await.unwrap().is_none());
    assert!(tx.options().get("dropped").await.unwrap().is_none());
    tx.options().set("outer", &json!(1), false).await.unwrap();
    tx.commit().await.unwrap();

    let r = store.read();
    for (key, present) in [
        ("kept", true),
        ("deep", true),
        ("outer", true),
        ("undone", false),
        ("dropped", false),
    ] {
        assert_eq!(
            r.options().get(key).await.unwrap().is_some(),
            present,
            "{key}"
        );
    }
}

/// SQLite has one writer: `begin()` while this task holds a `Tx` cannot succeed. It must
/// fail with `Busy` after the writer acquire timeout (5 s) instead of hanging.
pub async fn sqlite_begin_while_tx_open_is_busy(db: &TestDb) {
    let store = db.store();
    if store.backend() != Backend::Sqlite {
        return;
    }
    let tx = store.begin().await.unwrap();
    let started = Instant::now();
    let second = tokio::time::timeout(Duration::from_secs(10), store.begin())
        .await
        .expect("begin() must give up on its own");
    let waited = started.elapsed();
    assert!(
        matches!(second, Err(StoreError::Busy)),
        "{:?}",
        second.err()
    );
    assert!(
        waited >= Duration::from_secs(4) && waited <= Duration::from_secs(6),
        "{waited:?}"
    );
    tx.rollback().await.unwrap();
}

/// Using the outer level while a nested one is open gives `NestedOpen` (not `Invalid`),
/// for calls and for commit; the nested level is unaffected.
pub async fn nested_open_error_variant(db: &TestDb) {
    let store = db.store();
    let tx = store.begin().await.unwrap();
    let nested = tx.begin_nested().await.unwrap();
    let r = tx.options().get("k").await;
    assert!(matches!(r, Err(StoreError::NestedOpen)), "{r:?}");
    let r = tx.begin_nested().await.err();
    assert!(matches!(r, Some(StoreError::NestedOpen)), "{r:?}");
    assert!(
        StoreError::NestedOpen.to_string().contains("nested"),
        "{}",
        StoreError::NestedOpen
    );
    nested.options().set("n", &json!(1), false).await.unwrap();
    nested.commit().await.unwrap();
    tx.commit().await.unwrap();
    assert!(store.read().options().get("n").await.unwrap().is_some());
}

/// Dropping the outer `Tx` while a nested one is alive finishes the whole transaction:
/// the nested handle gets `Aborted`, its commit fails, nothing persists, and the writer
/// is free again (SQLite) once the last handle is gone.
pub async fn outer_drop_finishes_nested(db: &TestDb) {
    let store = db.store();
    let tx = store.begin().await.unwrap();
    tx.options().set("outer", &json!(1), false).await.unwrap();
    let nested = tx.begin_nested().await.unwrap();
    nested
        .options()
        .set("inner", &json!(1), false)
        .await
        .unwrap();
    drop(tx);

    let r = nested.options().get("inner").await;
    assert!(matches!(r, Err(StoreError::Aborted)), "{r:?}");
    let r = nested.begin_nested().await.err();
    assert!(matches!(r, Some(StoreError::Aborted)), "{r:?}");
    let r = nested.commit().await;
    assert!(matches!(r, Err(StoreError::Aborted)), "{r:?}");

    let tx = tokio::time::timeout(Duration::from_secs(6), store.begin())
        .await
        .expect("begin() must not hang")
        .expect("writer is free after the nested handle is gone");
    tx.commit().await.unwrap();
    let r = store.read();
    assert!(r.options().get("outer").await.unwrap().is_none());
    assert!(r.options().get("inner").await.unwrap().is_none());
}

/// Rolling back the outer `Tx` while a nested one is alive also finishes the nested
/// handle (`Aborted`), and committing the outer one then rolls everything back with
/// `NestedOpen`.
pub async fn outer_rollback_finishes_nested(db: &TestDb) {
    let store = db.store();
    let tx = store.begin().await.unwrap();
    let nested = tx.begin_nested().await.unwrap();
    nested
        .options()
        .set("inner", &json!(1), false)
        .await
        .unwrap();
    tx.rollback().await.unwrap();
    let r = nested.options().get("inner").await;
    assert!(matches!(r, Err(StoreError::Aborted)), "{r:?}");
    let r = nested.commit().await;
    assert!(matches!(r, Err(StoreError::Aborted)), "{r:?}");
    let tx = tokio::time::timeout(Duration::from_secs(6), store.begin())
        .await
        .expect("begin() must not hang")
        .unwrap();
    tx.rollback().await.unwrap();
    assert!(store.read().options().get("inner").await.unwrap().is_none());
}

pub async fn outer_commit_with_nested_open_rolls_back(db: &TestDb) {
    let store = db.store();
    let tx = store.begin().await.unwrap();
    tx.options().set("outer", &json!(1), false).await.unwrap();
    let nested = tx.begin_nested().await.unwrap();
    let r = tx.commit().await;
    assert!(matches!(r, Err(StoreError::NestedOpen)), "{r:?}");
    let r = nested.options().get("outer").await;
    assert!(matches!(r, Err(StoreError::Aborted)), "{r:?}");
    drop(nested);
    assert!(store.read().options().get("outer").await.unwrap().is_none());
    let tx = tokio::time::timeout(Duration::from_secs(6), store.begin())
        .await
        .expect("begin() must not hang")
        .unwrap();
    tx.rollback().await.unwrap();
}

/// The outer `Tx` is dropped while the nested one is parked and never called again; the
/// nested handle is dropped later. The connection must come back and nothing persists.
pub async fn outer_drop_with_parked_nested(db: &TestDb) {
    let store = db.store();
    let tx = store.begin().await.unwrap();
    tx.options().set("outer", &json!(1), false).await.unwrap();
    let nested = tx.begin_nested().await.unwrap();
    nested
        .options()
        .set("inner", &json!(1), false)
        .await
        .unwrap();
    drop(tx);
    drop(nested);
    let tx = tokio::time::timeout(Duration::from_secs(6), store.begin())
        .await
        .expect("begin() must not hang")
        .unwrap();
    tx.rollback().await.unwrap();
    let r = store.read();
    assert!(r.options().get("outer").await.unwrap().is_none());
    assert!(r.options().get("inner").await.unwrap().is_none());
}
