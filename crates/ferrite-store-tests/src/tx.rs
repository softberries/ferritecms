//! Transaction semantics shared by both backends: poisoning (R18), nested transactions
//! (savepoints).

use ferrite_domain::ContentId;
use ferrite_store::StoreError;
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
    assert!(matches!(busy, Err(StoreError::Invalid(_))), "{busy:?}");
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
