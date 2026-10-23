//! Contract tests for the event outbox.

use std::{collections::BTreeSet, sync::Arc};

use ferrite_domain::{NewEvent, Timestamp};
use ferrite_store::StoreError;

use crate::TestDb;

fn event(kind: &str, keys: &[&str]) -> NewEvent {
    NewEvent {
        kind: kind.into(),
        keys: keys.iter().map(|k| (*k).to_string()).collect(),
        origin_node: Some("node-ä".into()),
    }
}

pub async fn events_append_since_max(db: &TestDb) {
    let events = db.store().read().events();
    assert_eq!(events.max_seq().await.unwrap(), 0);
    assert!(events.since(0, 10).await.unwrap().is_empty());

    let before = Timestamp::now();
    let a = events
        .append(&event("content.changed", &["post:1"]))
        .await
        .unwrap();
    let b = events
        .append(&event("term.changed", &["term:☃", "term:2"]))
        .await
        .unwrap();
    let c = events
        .append(&NewEvent {
            kind: "options.changed".into(),
            keys: vec![],
            origin_node: None,
        })
        .await
        .unwrap();
    assert!(a < b && b < c);
    assert_eq!(events.max_seq().await.unwrap(), c);

    let all = events.since(0, 10).await.unwrap();
    assert_eq!(all.iter().map(|e| e.seq).collect::<Vec<_>>(), [a, b, c]);
    assert!(all[0].created_at >= before);

    let later = events.since(a, 10).await.unwrap();
    assert_eq!(later.len(), 2);
    assert_eq!(later[0].seq, b);
    assert_eq!(later[0].kind, "term.changed");
    assert_eq!(later[0].keys, ["term:☃", "term:2"]);
    assert_eq!(later[0].origin_node.as_deref(), Some("node-ä"));
    assert_eq!(later[1].seq, c);
    assert!(later[1].keys.is_empty());
    assert_eq!(later[1].origin_node, None);

    assert_eq!(events.since(0, 2).await.unwrap().len(), 2);
    assert!(events.since(c, 10).await.unwrap().is_empty());
    assert!(matches!(
        events.since(0, 0).await,
        Err(StoreError::Invalid(_))
    ));
    assert!(matches!(
        events.since(0, 1001).await,
        Err(StoreError::Invalid(_))
    ));
    assert_eq!(events.since(0, 1000).await.unwrap().len(), 3);
}

pub async fn events_reject_nul(db: &TestDb) {
    let events = db.store().read().events();
    for bad in [
        event("a\0b", &[]),
        event("k", &["x\0"]),
        NewEvent {
            origin_node: Some("n\0".into()),
            ..event("k", &[])
        },
    ] {
        assert!(matches!(
            events.append(&bad).await,
            Err(StoreError::Invalid(_))
        ));
    }
    assert_eq!(events.max_seq().await.unwrap(), 0);
}

pub async fn events_in_tx_only_visible_after_commit(db: &TestDb) {
    let store = db.store();
    let tx = store.begin().await.unwrap();
    let seq = tx.events().append(&event("a", &["k"])).await.unwrap();
    assert_eq!(tx.events().since(0, 10).await.unwrap().len(), 1);
    assert!(store.read().events().since(0, 10).await.unwrap().is_empty());
    tx.commit().await.unwrap();
    let got = store.read().events().since(0, 10).await.unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].seq, seq);

    let tx = store.begin().await.unwrap();
    tx.events().append(&event("b", &["k"])).await.unwrap();
    tx.rollback().await.unwrap();
    let got = store.read().events().since(0, 10).await.unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(store.read().events().max_seq().await.unwrap(), seq);
}

pub async fn events_prune(db: &TestDb) {
    let events = db.store().read().events();
    let first = Timestamp::now();
    for i in 0..3 {
        events
            .append(&event("a", &[&format!("k{i}")]))
            .await
            .unwrap();
    }
    let oldest = events.since(0, 10).await.unwrap()[0].created_at;
    assert_eq!(events.prune(oldest, 100).await.unwrap(), 0);
    assert_eq!(events.prune(first, 100).await.unwrap(), 0);
    assert_eq!(events.since(0, 10).await.unwrap().len(), 3);

    let last = events.max_seq().await.unwrap();
    // One second ahead, in microseconds.
    let future = Timestamp::from_micros(Timestamp::now().as_micros() + 1_000_000).unwrap();
    assert_eq!(events.prune(future, 100).await.unwrap(), 3);
    assert!(events.since(0, 10).await.unwrap().is_empty());
    assert_eq!(events.prune(future, 100).await.unwrap(), 0);

    // Sequence numbers are never reused, even after the outbox is emptied.
    let next = events.append(&event("a", &["after"])).await.unwrap();
    assert!(next > last, "seq {next} reused (previous max {last})");
    assert_eq!(events.since(last, 10).await.unwrap()[0].seq, next);
}

pub async fn events_concurrent_appends_unique_seq(db: Arc<TestDb>) {
    const TASKS: usize = 8;
    const APPENDS: usize = 25;
    let handles: Vec<_> = (0..TASKS)
        .map(|t| {
            let db = db.clone();
            tokio::spawn(async move {
                let mut seqs = Vec::new();
                for i in 0..APPENDS {
                    let tx = db.store().begin().await.unwrap();
                    let seq = tx
                        .events()
                        .append(&event("a", &[&format!("t{t}-{i}")]))
                        .await
                        .unwrap();
                    tx.commit().await.unwrap();
                    seqs.push(seq);
                }
                seqs
            })
        })
        .collect();
    let mut seen = BTreeSet::new();
    for h in handles {
        for seq in h.await.unwrap() {
            assert!(seen.insert(seq), "duplicate seq {seq}");
        }
    }
    assert_eq!(seen.len(), TASKS * APPENDS);
    let stored = db.store().read().events().since(0, 1000).await.unwrap();
    assert_eq!(stored.len(), TASKS * APPENDS);
    assert_eq!(stored.iter().map(|e| e.seq).collect::<BTreeSet<_>>(), seen);
}

/// R5: `prune` deletes at most `limit` rows per call, oldest first; the limit is checked.
pub async fn events_prune_in_batches(db: &TestDb) {
    let events = db.store().read().events();
    let mut seqs = Vec::new();
    for i in 0..5 {
        seqs.push(
            events
                .append(&event("a", &[&format!("k{i}")]))
                .await
                .unwrap(),
        );
    }
    let future = Timestamp::from_micros(Timestamp::now().as_micros() + 1_000_000).unwrap();
    for bad in [0, ferrite_store::MAX_PRUNE_BATCH + 1] {
        assert!(matches!(
            events.prune(future, bad).await,
            Err(StoreError::Invalid(_))
        ));
    }
    assert_eq!(events.prune(future, 2).await.unwrap(), 2);
    assert_eq!(events.min_seq().await.unwrap(), seqs[2]);
    assert_eq!(events.prune(future, 2).await.unwrap(), 2);
    assert_eq!(events.prune(future, 2).await.unwrap(), 1);
    assert_eq!(
        events
            .prune(future, ferrite_store::MAX_PRUNE_BATCH)
            .await
            .unwrap(),
        0
    );
}
