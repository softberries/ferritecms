//! `ApplyTx` (collected invalidation, sandbox) and `SandboxStore` (M6-3 task 3, design 15
//! §4.4, §5.2) on both backends.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use ferrite_core::{
    events,
    release::{ApplyTx, MAX_RELEASE_KEYS, Mode, SandboxStore},
};
use ferrite_domain::{AuditQuery, NewAudit, NewEvent, NewJob, Revision, RevisionId, Timestamp};
use ferrite_store::{Repos, Store, StoreError, Tx};
use ferrite_store_tests::{
    TestDb, backend_tests,
    fixtures::{fixture_user, new_post},
};
use serde_json::json;

async fn events_after(store: &dyn Store, seq: i64) -> Vec<(String, Vec<String>)> {
    store
        .read()
        .events()
        .since(seq, 100)
        .await
        .unwrap()
        .into_iter()
        .map(|e| (e.kind, e.keys))
        .collect()
}

fn other(kind: &str) -> NewEvent {
    NewEvent {
        kind: kind.into(),
        keys: vec!["option:x".into()],
        origin_node: None,
    }
}

async fn collect_merges_invalidations_into_one_event(db: &TestDb) {
    let store = db.store();
    let before = store.read().events().max_seq().await.unwrap();
    let tx = ApplyTx::new(store.begin().await.unwrap(), Mode::Collect);
    tx.events()
        .append(&events::invalidate([
            "post:a".to_owned(),
            "archive:post".into(),
        ]))
        .await
        .unwrap();
    tx.events()
        .append(&events::invalidate(["post:b".to_owned(), "post:a".into()]))
        .await
        .unwrap();
    assert!(events_after(store, before).await.is_empty(), "nothing yet");
    tx.flush_invalidation().await.unwrap();
    Box::new(tx).commit().await.unwrap();
    assert_eq!(
        events_after(store, before).await,
        vec![(
            events::INVALIDATE.to_owned(),
            vec!["archive:post".into(), "post:a".into(), "post:b".into()]
        )]
    );
}

async fn collect_above_the_cap_is_all(db: &TestDb) {
    let store = db.store();
    let before = store.read().events().max_seq().await.unwrap();
    let tx = ApplyTx::new(store.begin().await.unwrap(), Mode::Collect);
    let keys: Vec<String> = (0..=MAX_RELEASE_KEYS)
        .map(|i| format!("post:{i}"))
        .collect();
    tx.events().append(&events::invalidate(keys)).await.unwrap();
    tx.flush_invalidation().await.unwrap();
    Box::new(tx).commit().await.unwrap();
    assert_eq!(
        events_after(store, before).await,
        vec![(events::INVALIDATE.to_owned(), vec![events::ALL_KEY.into()])]
    );
}

async fn collect_passes_other_kinds(db: &TestDb) {
    let store = db.store();
    let before = store.read().events().max_seq().await.unwrap();
    let tx = ApplyTx::new(store.begin().await.unwrap(), Mode::Collect);
    tx.events().append(&other("options.changed")).await.unwrap();
    tx.flush_invalidation().await.unwrap();
    Box::new(tx).commit().await.unwrap();
    assert_eq!(
        events_after(store, before).await,
        vec![("options.changed".to_owned(), vec!["option:x".into()])]
    );
}

async fn nested_savepoints_share_the_collector(db: &TestDb) {
    let store = db.store();
    let before = store.read().events().max_seq().await.unwrap();
    let tx = ApplyTx::new(store.begin().await.unwrap(), Mode::Collect);
    let nested = tx.begin_nested().await.unwrap();
    nested
        .events()
        .append(&events::invalidate(["post:n".to_owned()]))
        .await
        .unwrap();
    nested.commit().await.unwrap();
    tx.events()
        .append(&events::invalidate(["post:o".to_owned()]))
        .await
        .unwrap();
    tx.flush_invalidation().await.unwrap();
    Box::new(tx).commit().await.unwrap();
    assert_eq!(
        events_after(store, before).await,
        vec![(
            events::INVALIDATE.to_owned(),
            vec!["post:n".into(), "post:o".into()]
        )]
    );
}

async fn sandbox_writes_no_event_job_audit_revision(db: &TestDb) {
    let store = db.store();
    let user = fixture_user(store).await;
    let item = store
        .read()
        .content()
        .insert(&new_post(&user, "kept"))
        .await
        .unwrap();
    let before = store.read().events().max_seq().await.unwrap();
    let audit_before = store
        .read()
        .audit()
        .list(&AuditQuery {
            limit: 100,
            ..Default::default()
        })
        .await
        .unwrap()
        .items
        .len();
    let tx = ApplyTx::new(store.begin().await.unwrap(), Mode::Sandbox);
    tx.events().append(&other("options.changed")).await.unwrap();
    tx.events()
        .append(&events::invalidate(["post:x".to_owned()]))
        .await
        .unwrap();
    assert_eq!(
        tx.jobs()
            .enqueue(&NewJob::new("x.kind", json!({})))
            .await
            .unwrap(),
        None
    );
    tx.audit()
        .append(&NewAudit {
            actor_id: None,
            action: "x.action".into(),
            target_kind: "system".into(),
            target_id: None,
            details: json!({}),
        })
        .await
        .unwrap();
    tx.revisions()
        .insert(&Revision {
            id: RevisionId::new(),
            item_id: item.id,
            author_id: user.id,
            created_at: Timestamp::now(),
            title: "t".into(),
            excerpt: String::new(),
            body: json!([]),
            fields: json!({}),
            meta: json!({}),
            note: None,
        })
        .await
        .unwrap();
    // Inside the sandbox nothing is visible either.
    assert!(tx.revisions().list(item.id, 10).await.unwrap().is_empty());
    tx.flush_invalidation().await.unwrap();
    // A sandbox's commit rolls back.
    Box::new(tx).commit().await.unwrap();
    assert!(events_after(store, before).await.is_empty());
    assert_eq!(
        store
            .read()
            .audit()
            .list(&AuditQuery {
                limit: 100,
                ..Default::default()
            })
            .await
            .unwrap()
            .items
            .len(),
        audit_before
    );
    let jobs = store
        .read()
        .jobs()
        .list(&ferrite_domain::JobQuery {
            limit: 100,
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(jobs.items.iter().all(|j| j.kind != "x.kind"));
}

async fn sandbox_sees_its_own_writes(db: &TestDb) {
    let store = db.shared_store();
    let user = fixture_user(store.as_ref()).await;
    let sandbox = SandboxStore::begin(store.clone()).await.unwrap();
    let item = sandbox
        .tx()
        .content()
        .insert(&new_post(&user, "inside"))
        .await
        .unwrap();
    assert_eq!(
        sandbox
            .read()
            .content()
            .get(item.id)
            .await
            .unwrap()
            .map(|i| i.slug),
        Some("inside".to_owned())
    );
    sandbox.finish().await.unwrap();
    assert_eq!(store.read().content().get(item.id).await.unwrap(), None);
}

async fn sandbox_rollback_leaves_no_rows(db: &TestDb) {
    let store = db.shared_store();
    let user = fixture_user(store.as_ref()).await;
    let id = {
        let sandbox = SandboxStore::begin(store.clone()).await.unwrap();
        let item = sandbox
            .read()
            .content()
            .insert(&new_post(&user, "dropped"))
            .await
            .unwrap();
        item.id
        // Dropped here: rolls back.
    };
    // The writer is free again at once (SQLite) and the row is gone.
    let tx = store.begin().await.unwrap();
    assert_eq!(tx.content().get(id).await.unwrap(), None);
    tx.rollback().await.unwrap();
}

async fn postgres_sandbox_leaves_no_sequence_gap(db: &TestDb) {
    let store = db.shared_store();
    let before = store.read().events().max_seq().await.unwrap();
    let sandbox = SandboxStore::begin(store.clone()).await.unwrap();
    sandbox
        .read()
        .events()
        .append(&other("options.changed"))
        .await
        .unwrap();
    sandbox.finish().await.unwrap();
    assert_eq!(store.read().events().max_seq().await.unwrap(), before);
    let seq = store
        .read()
        .events()
        .append(&other("options.changed"))
        .await
        .unwrap();
    assert_eq!(seq, before + 1, "no hole in the sequence");
}

async fn sandbox_store_refuses_begin(db: &TestDb) {
    let store = db.shared_store();
    let sandbox: Arc<dyn Store> = Arc::new(SandboxStore::begin(store.clone()).await.unwrap());
    assert!(matches!(
        sandbox.begin().await.err(),
        Some(StoreError::Unsupported(_))
    ));
    assert!(matches!(
        sandbox.try_lock("x").await.err(),
        Some(StoreError::Unsupported(_))
    ));
    assert_eq!(sandbox.backend(), store.backend());
}

backend_tests!(
    collect_merges_invalidations_into_one_event,
    collect_above_the_cap_is_all,
    collect_passes_other_kinds,
    nested_savepoints_share_the_collector,
    sandbox_writes_no_event_job_audit_revision,
    sandbox_sees_its_own_writes,
    sandbox_rollback_leaves_no_rows,
    postgres_sandbox_leaves_no_sequence_gap,
    sandbox_store_refuses_begin,
);
