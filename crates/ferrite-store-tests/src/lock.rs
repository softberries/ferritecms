//! Contract tests for named locks (04 §7, P1, P13): Postgres session advisory locks on
//! dedicated connections (across stores, released when the holder dies), SQLite an
//! in-process mutex per store.

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use ferrite_store::{LockGuard, Store, StoreError, lock};
use sqlx::Connection;

use crate::TestDb;

/// `try_lock` until it succeeds or `within` elapses.
async fn lock_within(
    store: &dyn Store,
    name: &str,
    within: Duration,
) -> Option<Box<dyn LockGuard>> {
    let deadline = Instant::now() + within;
    loop {
        if let Some(guard) = store.try_lock(name).await.expect("try_lock") {
            return Some(guard);
        }
        if Instant::now() >= deadline {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// `application_name` prefix of the fixture store's lock connections (Postgres).
fn lock_app_prefix(db: &TestDb) -> Option<String> {
    db.pg_location()
        .map(|(_, schema)| format!("{schema}:lock:"))
}

pub async fn lock_is_exclusive_and_released(db: &TestDb) {
    let store = db.store();
    let one = store.try_lock("t.one").await.unwrap().expect("free lock");
    assert!(store.try_lock("t.one").await.unwrap().is_none(), "held");
    let two = store.try_lock("t.two").await.unwrap().expect("other name");

    one.release().await.unwrap();
    let again = store
        .try_lock("t.one")
        .await
        .unwrap()
        .expect("after release");
    assert!(
        store.try_lock("t.one").await.unwrap().is_none(),
        "held again"
    );

    drop(again);
    let after_drop = if db.is_sqlite() {
        store.try_lock("t.one").await.unwrap()
    } else {
        // The server notices the closed socket asynchronously.
        lock_within(store, "t.one", Duration::from_secs(1)).await
    };
    let after_drop = after_drop.expect("free after drop");
    after_drop.release().await.unwrap();
    two.release().await.unwrap();
}

pub async fn lock_waits_then_busy(db: Arc<TestDb>) {
    let store = db.shared_store();
    let held = store.try_lock("t.wait").await.unwrap().expect("free lock");

    let started = Instant::now();
    let busy = lock(store.as_ref(), "t.wait", Duration::from_millis(600)).await;
    let waited = started.elapsed();
    assert!(matches!(busy, Err(StoreError::Busy)), "{:?}", busy.err());
    assert!(
        waited >= Duration::from_millis(500),
        "gave up after {waited:?}"
    );

    let releaser = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        held.release().await.unwrap();
    });
    let started = Instant::now();
    let got = lock(store.as_ref(), "t.wait", Duration::from_millis(600))
        .await
        .expect("acquired once released");
    assert!(started.elapsed() >= Duration::from_millis(250));
    releaser.await.unwrap();
    got.release().await.unwrap();
}

pub async fn lock_excludes_peer_store(db: &TestDb) {
    let Some(peer) = db.peer().await else {
        return; // SQLite: one store per data folder (in-process lock, see above).
    };
    let prefix = lock_app_prefix(db).expect("postgres");
    let held = db
        .store()
        .try_lock("t.peer")
        .await
        .unwrap()
        .expect("free lock");
    assert_eq!(db.active_backends(&format!("{prefix}t.peer")).await, 1);
    assert!(
        peer.try_lock("t.peer").await.unwrap().is_none(),
        "peer sees it held"
    );
    let other = peer.try_lock("t.other").await.unwrap().expect("other name");

    held.release().await.unwrap();
    let taken = lock_within(peer.as_ref(), "t.peer", Duration::from_secs(1))
        .await
        .expect("peer acquires after release");
    assert!(db.store().try_lock("t.peer").await.unwrap().is_none());
    taken.release().await.unwrap();
    other.release().await.unwrap();
}

pub async fn lock_released_when_holder_dies(db: &TestDb) {
    let Some(peer) = db.peer().await else {
        return;
    };
    let prefix = lock_app_prefix(db).expect("postgres");
    let held = db
        .store()
        .try_lock("t.die")
        .await
        .unwrap()
        .expect("free lock");
    assert!(peer.try_lock("t.die").await.unwrap().is_none());

    assert_eq!(db.terminate_backends(&prefix).await, 1);
    let taken = lock_within(peer.as_ref(), "t.die", Duration::from_secs(2))
        .await
        .expect("peer acquires once the holder's connection is gone");
    // The dead holder's guard: releasing it reports the lost connection, and must not
    // disturb the new holder.
    assert!(held.release().await.is_err());
    assert!(db.store().try_lock("t.die").await.unwrap().is_none());
    taken.release().await.unwrap();
}

pub async fn lock_names_are_validated(db: &TestDb) {
    let store = db.store();
    let long = "x".repeat(65);
    for bad in ["Bad Name", "", long.as_str()] {
        assert!(
            matches!(store.try_lock(bad).await, Err(StoreError::Invalid(_))),
            "{bad:?}"
        );
        assert!(
            matches!(
                lock(store, bad, Duration::from_millis(10)).await,
                Err(StoreError::Invalid(_))
            ),
            "{bad:?}"
        );
    }
    let max = "x".repeat(64);
    store
        .try_lock(&max)
        .await
        .unwrap()
        .expect("64 chars")
        .release()
        .await
        .unwrap();
}

// Wait tests (P19). Postgres only: they pin what `lock_timeout` on one held connection
// does; SQLite keeps the polling default, covered by `lock_waits_then_busy`. Lock names
// are unique per test so concurrent runs on one database do not collide (advisory locks
// are database-wide, the key includes the schema, which is per test).

pub async fn pg_lock_wait_times_out(db: &TestDb) {
    if db.is_sqlite() {
        return;
    }
    let store = db.store();
    let held = store.try_lock("w.timeout").await.unwrap().expect("free");
    let started = Instant::now();
    let got = store
        .lock_wait("w.timeout", Duration::from_millis(300))
        .await
        .unwrap();
    let waited = started.elapsed();
    assert!(got.is_none(), "still held");
    assert!(
        waited >= Duration::from_millis(300) && waited <= Duration::from_millis(1500),
        "waited {waited:?}"
    );
    // A connection that timed out is gone, and the lock is still the holder's.
    assert!(store.try_lock("w.timeout").await.unwrap().is_none());
    held.release().await.unwrap();
}

pub async fn pg_lock_wait_wakes_on_release(db: Arc<TestDb>) {
    if db.is_sqlite() {
        return;
    }
    let store = db.shared_store();
    let held = store.try_lock("w.wake").await.unwrap().expect("free");
    // Connect before the release so only the wake-up is timed.
    let waiter = {
        let store = store.clone();
        tokio::spawn(async move {
            let got = store
                .lock_wait("w.wake", Duration::from_secs(5))
                .await
                .unwrap();
            (got, Instant::now())
        })
    };
    tokio::time::sleep(Duration::from_millis(200)).await;
    let released_at = Instant::now();
    held.release().await.unwrap();
    let (got, got_at) = waiter.await.unwrap();
    let lag = got_at.saturating_duration_since(released_at);
    let got = got.expect("acquired after release");
    assert!(
        lag < Duration::from_millis(300),
        "woke {lag:?} after release"
    );
    // `lock_timeout` was reset: a statement-free hold must outlive the wait budget.
    tokio::time::sleep(Duration::from_millis(50)).await;
    got.release().await.unwrap();
}

pub async fn pg_lock_wait_cancel_does_not_leak(db: &TestDb) {
    if db.is_sqlite() {
        return;
    }
    let store = db.store();
    let held = store.try_lock("w.cancel").await.unwrap().expect("free");
    let cancelled = tokio::time::timeout(
        Duration::from_millis(100),
        store.lock_wait("w.cancel", Duration::from_secs(30)),
    )
    .await;
    assert!(cancelled.is_err(), "the wait was still pending");
    held.release().await.unwrap();
    let again = lock_within(store, "w.cancel", Duration::from_secs(2))
        .await
        .expect("the cancelled wait must not have taken or kept the lock");
    again.release().await.unwrap();
}

pub async fn pg_lock_wait_opens_one_connection(db: Arc<TestDb>) {
    let Some(prefix) = lock_app_prefix(&db) else {
        return;
    };
    let store = db.shared_store();
    let held = store.try_lock("w.conns").await.unwrap().expect("free");
    let name = format!("{prefix}w.conns");
    assert_eq!(db.active_backends(&name).await, 1, "the holder's");
    let waiter = {
        let store = store.clone();
        tokio::spawn(async move { store.lock_wait("w.conns", Duration::from_secs(1)).await })
    };
    // Distinct backends seen with this lock's application name: the holder plus the
    // waiter's one connection. Polling would show a new backend per attempt.
    let (url, _) = db.pg_location().expect("postgres");
    let mut probe = sqlx::PgConnection::connect(&url).await.expect("connect");
    let mut seen = std::collections::BTreeSet::new();
    let end = Instant::now() + Duration::from_millis(900);
    while Instant::now() < end {
        let pids: Vec<i32> = sqlx::query_scalar(
            "SELECT pid FROM pg_stat_activity WHERE application_name = $1 AND pid <> pg_backend_pid()",
        )
        .bind(&name)
        .fetch_all(&mut probe)
        .await
        .expect("sample backends");
        seen.extend(pids);
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        seen.len() <= 2,
        "{} lock connections during one wait",
        seen.len()
    );
    assert!(waiter.await.unwrap().unwrap().is_none());
    held.release().await.unwrap();
}
