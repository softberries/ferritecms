//! Contract tests for `Idempotency-Key` records (M2a-2b Task 1, P10, F5).

use std::{sync::Arc, time::Duration};

use ferrite_domain::{IDEMPOTENCY_STALE, IdemBegin, IdempotencyKey, Timestamp, UserId};
use ferrite_store::{MAX_IDEMPOTENCY_RESPONSE, Store, StoreError};
use serde_json::json;

use crate::{TestDb, fixtures::fixture_user};

fn key(user: UserId, c: char) -> IdempotencyKey {
    IdempotencyKey {
        user,
        key_hash: c.to_string().repeat(64),
        method: "POST".into(),
        path: "/api/v1/media".into(),
    }
}

fn token(begin: IdemBegin) -> Timestamp {
    match begin {
        IdemBegin::Started { token } => token,
        other => panic!("expected Started, got {other:?}"),
    }
}

fn is_invalid<T: std::fmt::Debug>(r: Result<T, StoreError>) -> bool {
    matches!(r, Err(StoreError::Invalid(_)))
}

/// Moves every idempotency row's `created_at` two hours into the past (database side).
async fn age_rows(db: &TestDb) {
    if db.is_sqlite() {
        db.exec_raw("UPDATE idempotency_keys SET created_at = created_at - 7200000000")
            .await;
    } else {
        db.exec_raw("UPDATE idempotency_keys SET created_at = created_at - interval '2 hours'")
            .await;
    }
}

pub async fn idempotency_begin_states(db: Arc<TestDb>) {
    let store = db.shared_store();
    let repo = store.read().idempotency();
    let user = fixture_user(store.as_ref()).await;
    let other_user = fixture_user(store.as_ref()).await;
    let k = key(user.id, 'a');

    // First use starts; a second one while pending is in progress.
    let t = token(repo.begin(&k, IDEMPOTENCY_STALE).await.unwrap());
    assert_eq!(
        repo.begin(&k, IDEMPOTENCY_STALE).await.unwrap(),
        IdemBegin::InProgress
    );
    // The same key of another user is independent.
    let theirs = key(other_user.id, 'a');
    let their_token = token(repo.begin(&theirs, IDEMPOTENCY_STALE).await.unwrap());

    // An oversized response is refused and the key stays pending.
    let fits = json!("x".repeat(MAX_IDEMPOTENCY_RESPONSE - 2));
    let over = json!("x".repeat(MAX_IDEMPOTENCY_RESPONSE - 1));
    assert!(is_invalid(
        repo.finish(user.id, &k.key_hash, t, 201, &over).await
    ));
    assert_eq!(
        repo.begin(&k, IDEMPOTENCY_STALE).await.unwrap(),
        IdemBegin::InProgress
    );

    // Finishing needs the matching token; then every retry replays.
    let wrong = Timestamp::from_micros(t.as_micros() - 1).unwrap();
    let response = json!({"body": {"id": "m1", "title": "ü"}, "headers": {"location": "/x"}});
    assert!(
        !repo
            .finish(user.id, &k.key_hash, wrong, 201, &response)
            .await
            .unwrap()
    );
    assert!(
        repo.finish(user.id, &k.key_hash, t, 201, &response)
            .await
            .unwrap()
    );
    assert!(
        !repo
            .finish(user.id, &k.key_hash, t, 200, &response)
            .await
            .unwrap(),
        "a finished row is final"
    );
    assert_eq!(
        repo.begin(&k, IDEMPOTENCY_STALE).await.unwrap(),
        IdemBegin::Replay {
            status: 201,
            response: response.clone()
        }
    );
    // Another path or method is a mismatch, finished or not.
    let mut other_path = k.clone();
    other_path.path = "/api/v1/media?x=1".into();
    assert_eq!(
        repo.begin(&other_path, IDEMPOTENCY_STALE).await.unwrap(),
        IdemBegin::Mismatch
    );
    let mut other_method = theirs.clone();
    other_method.method = "PUT".into();
    assert_eq!(
        repo.begin(&other_method, IDEMPOTENCY_STALE).await.unwrap(),
        IdemBegin::Mismatch
    );
    // A boundary-sized response fits.
    assert!(
        repo.finish(other_user.id, &theirs.key_hash, their_token, 201, &fits)
            .await
            .unwrap()
    );

    // Release frees a pending key (with its token only), never a finished one.
    let k2 = key(user.id, 'b');
    let t2 = token(repo.begin(&k2, IDEMPOTENCY_STALE).await.unwrap());
    assert!(!repo.release(user.id, &k2.key_hash, wrong).await.unwrap());
    assert!(repo.release(user.id, &k2.key_hash, t2).await.unwrap());
    assert!(!repo.release(user.id, &k2.key_hash, t2).await.unwrap());
    token(repo.begin(&k2, IDEMPOTENCY_STALE).await.unwrap());
    assert!(!repo.release(user.id, &k.key_hash, t).await.unwrap());
    assert!(matches!(
        repo.begin(&k, IDEMPOTENCY_STALE).await.unwrap(),
        IdemBegin::Replay { status: 201, .. }
    ));

    // A stale pending key is taken over exactly once by two concurrent takers; the
    // original holder can then neither finish nor release it.
    let k3 = key(user.id, 'c');
    let original = token(repo.begin(&k3, IDEMPOTENCY_STALE).await.unwrap());
    age_rows(&db).await;
    assert!(matches!(
        repo.begin(&k, IDEMPOTENCY_STALE).await.unwrap(),
        IdemBegin::Replay { .. }
    ));
    let mut takers = Vec::new();
    for _ in 0..2 {
        let node: Arc<dyn Store> = db.peer().await.unwrap_or_else(|| store.clone());
        let k3 = k3.clone();
        takers.push(tokio::spawn(async move {
            node.read()
                .idempotency()
                .begin(&k3, IDEMPOTENCY_STALE)
                .await
                .unwrap()
        }));
    }
    let mut results = Vec::new();
    for t in takers {
        results.push(t.await.unwrap());
    }
    let started: Vec<_> = results
        .iter()
        .filter_map(|r| match r {
            IdemBegin::Started { token } => Some(*token),
            _ => None,
        })
        .collect();
    assert_eq!(started.len(), 1, "{results:?}");
    assert!(results.contains(&IdemBegin::InProgress), "{results:?}");
    let taken = started[0];
    assert!(taken > original);
    assert!(
        !repo
            .finish(user.id, &k3.key_hash, original, 201, &response)
            .await
            .unwrap()
    );
    assert!(!repo.release(user.id, &k3.key_hash, original).await.unwrap());
    assert!(
        repo.finish(user.id, &k3.key_hash, taken, 201, &response)
            .await
            .unwrap()
    );

    // Input validation happens before any SQL.
    let mut bad = k.clone();
    bad.key_hash = "A".repeat(64);
    assert!(is_invalid(repo.begin(&bad, IDEMPOTENCY_STALE).await));
    bad.key_hash = "a".repeat(63);
    assert!(is_invalid(repo.begin(&bad, IDEMPOTENCY_STALE).await));
    let mut bad = k.clone();
    bad.path = "/a\0".into();
    assert!(is_invalid(repo.begin(&bad, IDEMPOTENCY_STALE).await));
    let mut bad = k.clone();
    bad.method = "PO\0ST".into();
    assert!(is_invalid(repo.begin(&bad, IDEMPOTENCY_STALE).await));
    assert!(is_invalid(repo.begin(&k, Duration::ZERO).await));
    assert!(is_invalid(
        repo.begin(&k, Duration::from_secs(2 * 24 * 3600)).await
    ));
    assert!(is_invalid(
        repo.finish(user.id, &k.key_hash, t, 99, &json!({})).await
    ));
    assert!(is_invalid(
        repo.finish(user.id, &k.key_hash, t, 201, &json!({"a": "\0"}))
            .await
    ));
    assert!(is_invalid(repo.release(user.id, "x", t).await));
}

pub async fn idempotency_prune_in_batches(db: &TestDb) {
    let store = db.store();
    let repo = store.read().idempotency();
    let user = fixture_user(store).await;
    for (i, c) in ['a', 'b', 'c', 'd', 'e'].into_iter().enumerate() {
        let t = token(
            repo.begin(&key(user.id, c), IDEMPOTENCY_STALE)
                .await
                .unwrap(),
        );
        if i % 2 == 0 {
            assert!(
                repo.finish(user.id, &key(user.id, c).key_hash, t, 201, &json!({}))
                    .await
                    .unwrap()
            );
        }
    }
    age_rows(db).await;
    let fresh = key(user.id, 'f');
    token(repo.begin(&fresh, IDEMPOTENCY_STALE).await.unwrap());
    let cutoff = Timestamp::from_micros(Timestamp::now().as_micros() - 3600 * 1_000_000).unwrap();

    assert!(is_invalid(repo.prune(cutoff, 0).await));
    let mut batches = Vec::new();
    loop {
        let n = repo.prune(cutoff, 2).await.unwrap();
        batches.push(n);
        if n < 2 {
            break;
        }
    }
    assert_eq!(batches, [2, 2, 1]);
    assert_eq!(
        repo.begin(&fresh, IDEMPOTENCY_STALE).await.unwrap(),
        IdemBegin::InProgress
    );
    token(
        repo.begin(&key(user.id, 'a'), IDEMPOTENCY_STALE)
            .await
            .unwrap(),
    );

    // Rows go with their user.
    store.read().users().delete(user.id).await.unwrap();
    let later = Timestamp::from_micros(Timestamp::now().as_micros() + 3600 * 1_000_000).unwrap();
    assert_eq!(repo.prune(later, 100).await.unwrap(), 0);
}

/// The hourly prune finds old rows on `idempotency_keys_created_idx` and never scans the
/// table (final review M1).
pub async fn idempotency_prune_uses_indexes(db: &TestDb) {
    let plan = db.explain_idempotency_prune().await;
    println!("{plan}");
    assert!(plan.contains("idempotency_keys_created_idx"), "{plan}");
    if db.is_sqlite() {
        assert!(!plan.contains("SCAN idempotency_keys"), "{plan}");
    } else {
        assert!(!plan.contains("Seq Scan"), "{plan}");
    }
}
