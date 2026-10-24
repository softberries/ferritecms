//! Contract tests for the job listing and the dead-job retry (M2a-2b P19), and the
//! hourly failure buckets of the plugin metrics (P20, F26).

use std::time::Duration;

use ferrite_domain::{Job, JobId, JobQuery, JobStatus, NewJob, NodeId, Timestamp};
use ferrite_store::{JobRepo, MAX_PRUNE_BATCH, StoreError, encode_audit_cursor};
use serde_json::json;

use crate::TestDb;

const LEASE: Duration = Duration::from_secs(60);
const HOUR: i64 = 3_600_000_000;
const MINUTE: i64 = 60_000_000;

fn at(micros: i64) -> Timestamp {
    Timestamp::from_micros(micros).unwrap()
}

fn query(status: Option<JobStatus>, kind: Option<&str>, limit: u32) -> JobQuery {
    JobQuery {
        status,
        kind: kind.map(str::to_owned),
        limit,
        cursor: None,
    }
}

async fn enqueue(jobs: &dyn JobRepo, kind: &str, max_attempts: i32) -> JobId {
    let mut job = NewJob::new(kind, json!({"v": 1, "secret": "payload"}));
    job.max_attempts = max_attempts;
    jobs.enqueue(&job).await.unwrap().expect("inserted")
}

/// Claims the one due job of `kind` and fails it permanently (`dead`).
async fn kill(jobs: &dyn JobRepo, kind: &str, error: &str) -> Job {
    let claimed = jobs
        .claim(NodeId::new(), &[kind.to_owned()], 1, LEASE)
        .await
        .unwrap();
    let lease = claimed[0].lease().unwrap();
    assert_eq!(
        jobs.fail(&lease, error, true).await.unwrap(),
        Some(JobStatus::Dead)
    );
    jobs.get(lease.id).await.unwrap().unwrap()
}

/// Every page of `q`, following `next_cursor`.
async fn all_pages(jobs: &dyn JobRepo, mut q: JobQuery) -> Vec<JobId> {
    let mut ids = Vec::new();
    loop {
        let page = jobs.list(&q).await.unwrap();
        assert!(page.items.len() <= q.limit as usize);
        ids.extend(page.items.iter().map(|j| j.id));
        match page.next_cursor {
            Some(c) => q.cursor = Some(c),
            None => return ids,
        }
    }
}

/// Newest first; `status` and `kind` filter; cursor pages cover every row once; bad
/// input is `Invalid` and does not poison a `Tx`.
pub async fn job_list_filters_and_pages(db: &TestDb) {
    let jobs = db.store().read().jobs();
    let mut created = Vec::new();
    for kind in ["a.one", "b.two", "a.one", "b.two", "a.one"] {
        created.push(enqueue(jobs, kind, 5).await);
        // Distinct `created_at` on both backends (ties are broken by id, too).
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    // The oldest `a.one` dies.
    let dead = kill(jobs, "a.one", "boom").await;
    assert_eq!(dead.id, created[0]);

    let newest_first: Vec<JobId> = created.iter().rev().copied().collect();
    let page = jobs.list(&query(None, None, 200)).await.unwrap();
    assert_eq!(
        page.items.iter().map(|j| j.id).collect::<Vec<_>>(),
        newest_first
    );
    assert_eq!(page.next_cursor, None);
    // Full rows (the API drops the payload).
    let row = page.items.iter().find(|j| j.id == dead.id).unwrap();
    assert_eq!(row, &dead);

    let ids = |list: Vec<Job>| list.into_iter().map(|j| j.id).collect::<Vec<_>>();
    let a = jobs.list(&query(None, Some("a.one"), 200)).await.unwrap();
    assert_eq!(ids(a.items), [created[4], created[2], created[0]]);
    let dead_only = jobs
        .list(&query(Some(JobStatus::Dead), None, 200))
        .await
        .unwrap();
    assert_eq!(ids(dead_only.items), [created[0]]);
    let queued_b = jobs
        .list(&query(Some(JobStatus::Queued), Some("b.two"), 200))
        .await
        .unwrap();
    assert_eq!(ids(queued_b.items), [created[3], created[1]]);
    assert!(
        jobs.list(&query(Some(JobStatus::Running), None, 200))
            .await
            .unwrap()
            .items
            .is_empty()
    );
    assert!(
        jobs.list(&query(None, Some("c.none"), 200))
            .await
            .unwrap()
            .items
            .is_empty()
    );

    // Pages of 2: every row once, in order, with and without a filter.
    assert_eq!(all_pages(jobs, query(None, None, 2)).await, newest_first);
    assert_eq!(all_pages(jobs, query(None, None, 1)).await, newest_first);
    assert_eq!(
        all_pages(jobs, query(None, Some("a.one"), 1)).await,
        [created[4], created[2], created[0]]
    );
    let first = jobs.list(&query(None, None, 2)).await.unwrap();
    assert!(first.next_cursor.as_deref().unwrap().len() > 2);

    // Bounded input.
    for limit in [0, 201] {
        assert!(matches!(
            jobs.list(&query(None, None, limit)).await,
            Err(StoreError::Invalid(_))
        ));
    }
    for kind in ["", "A.one", "a one", "a\0", &"k".repeat(65)] {
        assert!(
            matches!(
                jobs.list(&query(None, Some(kind), 10)).await,
                Err(StoreError::Invalid(_))
            ),
            "{kind:?}"
        );
    }
    let foreign = encode_audit_cursor(Timestamp::now(), uuid::Uuid::nil());
    for cursor in ["garbage", foreign.as_str()] {
        let q = JobQuery {
            cursor: Some(cursor.to_owned()),
            ..query(None, None, 10)
        };
        assert!(
            matches!(jobs.list(&q).await, Err(StoreError::Invalid(_))),
            "{cursor}"
        );
    }
    let tx = db.store().begin().await.unwrap();
    assert!(tx.jobs().list(&query(None, Some("BAD"), 10)).await.is_err());
    assert_eq!(
        tx.jobs()
            .list(&query(None, None, 10))
            .await
            .unwrap()
            .items
            .len(),
        5,
        "not poisoned"
    );
    tx.commit().await.unwrap();
}

/// Only a `dead` job is put back: queued, running and done jobs and unknown ids are
/// `None` and unchanged. A retried job is due now with no attempts, keeps its last
/// error and key, and a claim then takes it.
pub async fn retry_dead_only(db: &TestDb) {
    let jobs = db.store().read().jobs();
    assert_eq!(jobs.retry_dead(JobId::new()).await.unwrap(), None);

    let queued = enqueue(jobs, "q.kind", 5).await;
    let before = jobs.get(queued).await.unwrap().unwrap();
    assert_eq!(jobs.retry_dead(queued).await.unwrap(), None);
    assert_eq!(jobs.get(queued).await.unwrap().unwrap(), before);

    let running = enqueue(jobs, "r.kind", 5).await;
    let claimed = jobs
        .claim(NodeId::new(), &["r.kind".to_owned()], 1, LEASE)
        .await
        .unwrap();
    let before = jobs.get(running).await.unwrap().unwrap();
    assert_eq!(before.status, JobStatus::Running);
    assert_eq!(jobs.retry_dead(running).await.unwrap(), None);
    assert_eq!(jobs.get(running).await.unwrap().unwrap(), before);

    let done = enqueue(jobs, "d.kind", 5).await;
    let lease = jobs
        .claim(NodeId::new(), &["d.kind".to_owned()], 1, LEASE)
        .await
        .unwrap()[0]
        .lease()
        .unwrap();
    assert!(jobs.complete(&lease).await.unwrap());
    let before = jobs.get(done).await.unwrap().unwrap();
    assert_eq!(jobs.retry_dead(done).await.unwrap(), None);
    assert_eq!(jobs.get(done).await.unwrap().unwrap(), before);

    // A dead job with a unique key and a future `run_at` from its last backoff.
    let mut new = NewJob::new("x.kind", json!({"v": 1}));
    new.unique_key = Some("x:1".into());
    new.max_attempts = 1;
    let id = jobs.enqueue(&new).await.unwrap().unwrap();
    let dead = kill(jobs, "x.kind", "boom: /srv/data/x").await;
    assert_eq!(dead.status, JobStatus::Dead);
    assert!(dead.finished_at.is_some());
    let t0 = Timestamp::now();
    let retried = jobs
        .retry_dead(id)
        .await
        .unwrap()
        .expect("dead job retried");
    assert_eq!(retried.id, id);
    assert_eq!(retried.status, JobStatus::Queued);
    assert_eq!(retried.attempts, 0);
    assert_eq!(retried.max_attempts, 1);
    assert_eq!(retried.last_error.as_deref(), Some("boom: /srv/data/x"));
    assert_eq!(retried.unique_key.as_deref(), Some("x:1"));
    assert_eq!(
        (retried.locked_by, retried.locked_until, retried.finished_at),
        (None, None, None)
    );
    assert!(
        (retried.run_at.as_micros() - t0.as_micros()).abs() < 5_000_000,
        "due now: {:?} vs {t0:?}",
        retried.run_at
    );
    assert_eq!(jobs.get(id).await.unwrap().unwrap(), retried);
    // Once retried it is queued: a second retry is `None`; the key still dedupes.
    assert_eq!(jobs.retry_dead(id).await.unwrap(), None);
    assert_eq!(jobs.enqueue(&new).await.unwrap(), None);

    // A claim takes it (attempt 1 of 1 again); the other kinds are untouched.
    let node = NodeId::new();
    let got = jobs
        .claim(node, &["x.kind".to_owned()], 5, LEASE)
        .await
        .unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!((got[0].id, got[0].attempts), (id, 1));
    assert_eq!(got[0].locked_by, Some(node));
    drop(claimed);

    // In a `Tx`: visible after commit only.
    let other = enqueue(jobs, "y.kind", 1).await;
    kill(jobs, "y.kind", "e").await;
    let tx = db.store().begin().await.unwrap();
    assert!(tx.jobs().retry_dead(other).await.unwrap().is_some());
    tx.rollback().await.unwrap();
    assert_eq!(
        jobs.get(other).await.unwrap().unwrap().status,
        JobStatus::Dead,
        "rolled back"
    );
}

/// Hourly sums per hook from `since`, newest hour first then by hook, one plugin only,
/// bounded `limit`.
pub async fn failure_buckets_since_newest_first(db: &TestDb) {
    let f = db.store().read().plugin_failures();
    let h0 = Timestamp::now().as_micros() / HOUR * HOUR;
    assert!(
        f.buckets("p", at(h0 - 24 * HOUR), 100)
            .await
            .unwrap()
            .is_empty()
    );
    // Hour h0: two minutes of render.head, one of a route; hour h0-1: render.head; an
    // hour before `since`; another plugin.
    f.add("p", "render.head", at(h0), 2).await.unwrap();
    f.add("p", "render.head", at(h0 + 5 * MINUTE), 3)
        .await
        .unwrap();
    f.add("p", "route GET /x", at(h0 + MINUTE), 1)
        .await
        .unwrap();
    f.add("p", "render.head", at(h0 - HOUR + 59 * MINUTE), 4)
        .await
        .unwrap();
    f.add("p", "render.head", at(h0 - 30 * HOUR), 9)
        .await
        .unwrap();
    f.add("q", "render.head", at(h0), 7).await.unwrap();

    let got = f.buckets("p", at(h0 - 24 * HOUR), 100).await.unwrap();
    let rows: Vec<(&str, i64, u64)> = got
        .iter()
        .map(|b| (b.hook.as_str(), b.bucket_start.as_micros(), b.count))
        .collect();
    assert_eq!(
        rows,
        [
            ("render.head", h0, 5),
            ("route GET /x", h0, 1),
            ("render.head", h0 - HOUR, 4),
        ]
    );
    // `since` inside an hour: only the minute buckets from it on count.
    let got = f.buckets("p", at(h0 + MINUTE), 100).await.unwrap();
    let rows: Vec<(&str, i64, u64)> = got
        .iter()
        .map(|b| (b.hook.as_str(), b.bucket_start.as_micros(), b.count))
        .collect();
    assert_eq!(rows, [("render.head", h0, 3), ("route GET /x", h0, 1)]);
    // `limit` keeps the newest rows.
    let got = f.buckets("p", at(h0 - 24 * HOUR), 1).await.unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!((got[0].hook.as_str(), got[0].count), ("render.head", 5));
    // Names compare exactly.
    assert!(f.buckets("P", at(0), 10).await.unwrap().is_empty());
    for bad in [0, MAX_PRUNE_BATCH + 1] {
        assert!(matches!(
            f.buckets("p", at(0), bad).await,
            Err(StoreError::Invalid(_))
        ));
    }
    assert!(matches!(
        f.buckets("p\0", at(0), 10).await,
        Err(StoreError::Invalid(_))
    ));
}
