//! Contract tests for change notifications (04 §4, P1–P3): Postgres `LISTEN/NOTIFY`
//! from the writing statement, and SQLite's "no subscription, poll instead".

use std::time::Duration;

use ferrite_domain::{NewEvent, NewJob};
use ferrite_store::{Channel, Notice, Store, Subscription};

use crate::TestDb;

fn event(key: &str) -> NewEvent {
    NewEvent {
        kind: "invalidate".into(),
        keys: vec![key.to_owned()],
        origin_node: None,
    }
}

/// The next notice within `within`, or `None` if none arrived. A `recv` error fails the
/// test.
async fn next_notice(sub: &mut Box<dyn Subscription>, within: Duration) -> Option<Notice> {
    match tokio::time::timeout(within, sub.recv()).await {
        Ok(notice) => Some(notice.expect("subscription recv")),
        Err(_) => None,
    }
}

/// Subscribes on Postgres; `None` on SQLite (pinned by `subscribe_is_none_on_sqlite`).
async fn subscribe(db: &TestDb, channel: Channel) -> Option<Box<dyn Subscription>> {
    let sub = db.store().subscribe(channel).await.expect("subscribe");
    assert_eq!(
        sub.is_some(),
        !db.is_sqlite(),
        "subscribe follows the backend (P1)"
    );
    sub
}

/// A writer on another store (another node) where there is one, else the fixture store.
async fn writer(db: &TestDb) -> std::sync::Arc<dyn Store> {
    match db.peer().await {
        Some(peer) => peer,
        None => db.shared_store(),
    }
}

pub async fn subscribe_is_none_on_sqlite(db: &TestDb) {
    for channel in [Channel::Events, Channel::Jobs] {
        let sub = db.store().subscribe(channel).await.unwrap();
        assert_eq!(sub.is_none(), db.is_sqlite(), "{channel:?}");
    }
}

pub async fn notify_events_only_after_commit(db: &TestDb) {
    let Some(mut sub) = subscribe(db, Channel::Events).await else {
        return;
    };
    let Some(mut jobs) = subscribe(db, Channel::Jobs).await else {
        return;
    };
    let writer = writer(db).await;

    let tx = writer.begin().await.unwrap();
    tx.events().append(&event("post:1")).await.unwrap();
    assert_eq!(
        next_notice(&mut sub, Duration::from_millis(300)).await,
        None,
        "notified before commit"
    );
    tx.commit().await.unwrap();
    assert_eq!(
        next_notice(&mut sub, Duration::from_secs(1)).await,
        Some(Notice::Notified)
    );

    let tx = writer.begin().await.unwrap();
    tx.events().append(&event("post:2")).await.unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(
        next_notice(&mut sub, Duration::from_millis(500)).await,
        None,
        "notified for a rolled-back append"
    );

    // Autocommit appends notify too; the jobs channel hears none of this.
    writer
        .read()
        .events()
        .append(&event("post:3"))
        .await
        .unwrap();
    assert_eq!(
        next_notice(&mut sub, Duration::from_secs(1)).await,
        Some(Notice::Notified)
    );
    assert_eq!(
        next_notice(&mut jobs, Duration::from_millis(200)).await,
        None
    );
}

pub async fn notify_jobs_on_enqueue(db: &TestDb) {
    let Some(mut sub) = subscribe(db, Channel::Jobs).await else {
        return;
    };
    let Some(mut events) = subscribe(db, Channel::Events).await else {
        return;
    };
    let writer = writer(db).await;
    let job = NewJob {
        unique_key: Some("notify:k".into()),
        ..NewJob::new("test.notify", serde_json::json!({"v": 1}))
    };

    assert!(writer.read().jobs().enqueue(&job).await.unwrap().is_some());
    assert_eq!(
        next_notice(&mut sub, Duration::from_secs(1)).await,
        Some(Notice::Notified)
    );
    // A duplicate unique key inserts nothing, so nothing is announced.
    assert!(writer.read().jobs().enqueue(&job).await.unwrap().is_none());
    assert_eq!(
        next_notice(&mut sub, Duration::from_millis(500)).await,
        None
    );
    assert_eq!(
        next_notice(&mut events, Duration::from_millis(200)).await,
        None
    );

    // Inside a Tx: announced at commit only.
    let tx = writer.begin().await.unwrap();
    tx.jobs()
        .enqueue(&NewJob::new("test.notify", serde_json::json!({"v": 1})))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        next_notice(&mut sub, Duration::from_millis(300)).await,
        None
    );
    tx.commit().await.unwrap();
    assert_eq!(
        next_notice(&mut sub, Duration::from_secs(1)).await,
        Some(Notice::Notified)
    );
}

pub async fn subscribe_reports_reconnect(db: &TestDb) {
    let Some(mut sub) = subscribe(db, Channel::Events).await else {
        return;
    };
    let (_, schema) = db.pg_location().expect("postgres");
    assert_eq!(db.terminate_backends(&format!("{schema}:listen")).await, 1);
    assert_eq!(
        next_notice(&mut sub, Duration::from_secs(5)).await,
        Some(Notice::Reconnected)
    );

    let tx = db.store().begin().await.unwrap();
    tx.events().append(&event("post:1")).await.unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        next_notice(&mut sub, Duration::from_secs(1)).await,
        Some(Notice::Notified)
    );

    // Dropping the subscription closes its connection (no leak across resubscribes).
    drop(sub);
    let listen = format!("{schema}:listen");
    let mut left = u64::MAX;
    for _ in 0..20 {
        left = db.active_backends(&listen).await;
        if left == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(left, 0, "listen connection still open after drop");
}

pub async fn events_min_seq(db: &TestDb) {
    let events = db.store().read().events();
    assert_eq!(events.min_seq().await.unwrap(), 0);

    let mut seqs = Vec::new();
    for i in 0..3 {
        seqs.push(events.append(&event(&format!("k{i}"))).await.unwrap());
        // Distinct created_at, so pruning can take the oldest alone.
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(events.min_seq().await.unwrap(), seqs[0]);

    let stored = events.since(0, 10).await.unwrap();
    assert_eq!(events.prune(stored[1].created_at).await.unwrap(), 1);
    assert_eq!(events.min_seq().await.unwrap(), seqs[1]);
    assert_eq!(events.max_seq().await.unwrap(), seqs[2]);

    // Inside a Tx it sees the Tx's own state.
    let tx = db.store().begin().await.unwrap();
    assert_eq!(tx.events().min_seq().await.unwrap(), seqs[1]);
    tx.rollback().await.unwrap();

    let future = ferrite_domain::Timestamp::from_micros(
        ferrite_domain::Timestamp::now().as_micros() + 1_000_000,
    )
    .unwrap();
    assert_eq!(events.prune(future).await.unwrap(), 2);
    assert_eq!(events.min_seq().await.unwrap(), 0);
}
