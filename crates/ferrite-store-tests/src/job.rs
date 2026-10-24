//! Contract tests for the job queue (design 04 §6, rulings P5–P8).

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    time::Duration,
};

use ferrite_domain::{Job, JobId, JobStatus, NewJob, NodeId, Timestamp};
use ferrite_store::{JobRepo, MAX_JOB_ERROR, Store, StoreError};
use serde_json::json;

use crate::TestDb;

const LEASE: Duration = Duration::from_secs(60);

fn kinds(list: &[&str]) -> Vec<String> {
    list.iter().map(|k| (*k).to_string()).collect()
}

fn job(kind: &str) -> NewJob {
    NewJob::new(kind, json!({"v": 1}))
}

async fn enqueue(jobs: &dyn JobRepo, new: &NewJob) -> JobId {
    jobs.enqueue(new).await.unwrap().expect("inserted")
}

async fn get(jobs: &dyn JobRepo, id: JobId) -> Job {
    jobs.get(id).await.unwrap().expect("job exists")
}

/// `|a - b| <= tolerance` in microseconds.
fn near(a: Timestamp, b: Timestamp, tolerance: Duration) -> bool {
    let tol = i64::try_from(tolerance.as_micros()).unwrap();
    (a.as_micros() - b.as_micros()).abs() <= tol
}

fn plus(t: Timestamp, d: Duration) -> Timestamp {
    Timestamp::from_micros(t.as_micros() + i64::try_from(d.as_micros()).unwrap()).unwrap()
}

pub async fn jobs_enqueue_and_get_roundtrip(db: &TestDb) {
    let jobs = db.store().read().jobs();
    let run_at = Timestamp::from_micros(Timestamp::now().as_micros() + 3_600_000_123).unwrap();
    let full = NewJob {
        kind: "content.publish_scheduled".into(),
        payload: json!({"v": 1, "id": "☃", "n": [1, 2.5, null], "o": {"k": true}}),
        unique_key: Some("publish:ä:1".into()),
        run_at: Some(run_at),
        priority: -7,
        max_attempts: 9,
    };
    let before = Timestamp::now();
    let id = enqueue(jobs, &full).await;
    assert_eq!(id.as_uuid().get_version_num(), 7);
    let got = get(jobs, id).await;
    assert_eq!(got.id, id);
    assert_eq!(got.kind, full.kind);
    assert_eq!(got.payload, full.payload);
    assert_eq!(got.unique_key, full.unique_key);
    assert_eq!(got.run_at, run_at);
    assert_eq!(got.priority, -7);
    assert_eq!(got.status, JobStatus::Queued);
    assert_eq!((got.attempts, got.max_attempts), (0, 9));
    assert_eq!((got.locked_by, got.locked_until), (None, None));
    assert_eq!((&got.last_error, got.finished_at), (&None, None));
    assert!(near(got.created_at, before, Duration::from_secs(2)));
    assert_eq!(got.lease(), None);

    // Defaults: run_at None is "now", no key, priority 0, 5 attempts.
    let id = enqueue(jobs, &job("media.variants")).await;
    let got = get(jobs, id).await;
    assert!(near(got.run_at, Timestamp::now(), Duration::from_secs(2)));
    assert_eq!(got.unique_key, None);
    assert_eq!((got.priority, got.max_attempts), (0, 5));
    assert_eq!(got.payload, json!({"v": 1}));
    assert!(jobs.get(JobId::new()).await.unwrap().is_none());

    let counts = jobs.counts().await.unwrap();
    assert_eq!(
        counts,
        BTreeMap::from([
            (JobStatus::Queued, 2),
            (JobStatus::Running, 0),
            (JobStatus::Done, 0),
            (JobStatus::Dead, 0),
        ])
    );
}

pub async fn jobs_unique_key_dedupes_in_any_status(db: &TestDb) {
    let store = db.store();
    let jobs = store.read().jobs();
    let mut keyed = job("k");
    keyed.unique_key = Some("once".into());
    let id = enqueue(jobs, &keyed).await;
    assert_eq!(jobs.enqueue(&keyed).await.unwrap(), None, "queued");
    // Unkeyed jobs never collide (NULLs are distinct).
    enqueue(jobs, &job("k")).await;
    enqueue(jobs, &job("k")).await;

    let node = NodeId::new();
    let claimed = jobs.claim(node, &kinds(&["k"]), 10, LEASE).await.unwrap();
    let mine = claimed.iter().find(|j| j.id == id).expect("claimed");
    assert_eq!(jobs.enqueue(&keyed).await.unwrap(), None, "running");
    assert!(jobs.complete(&mine.lease().unwrap()).await.unwrap());
    assert_eq!(get(jobs, id).await.status, JobStatus::Done);
    assert_eq!(jobs.enqueue(&keyed).await.unwrap(), None, "done");

    // A dead row keeps its key too.
    let mut doomed = job("k");
    doomed.unique_key = Some("doomed".into());
    let doomed_id = enqueue(jobs, &doomed).await;
    let lease = jobs.claim(node, &kinds(&["k"]), 10, LEASE).await.unwrap();
    let lease = lease
        .iter()
        .find(|j| j.id == doomed_id)
        .expect("claimed")
        .lease()
        .unwrap();
    assert_eq!(
        jobs.fail(&lease, "bad", true).await.unwrap(),
        Some(JobStatus::Dead)
    );
    assert_eq!(jobs.enqueue(&doomed).await.unwrap(), None, "dead");

    // Inside a Tx the duplicate does not poison it.
    let tx = store.begin().await.unwrap();
    assert_eq!(tx.jobs().enqueue(&keyed).await.unwrap(), None);
    let mut other = job("k");
    other.unique_key = Some("other".into());
    let other_id = tx.jobs().enqueue(&other).await.unwrap().expect("inserted");
    tx.options().set("after", &json!(1), false).await.unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        get(jobs, other_id).await.unique_key.as_deref(),
        Some("other")
    );
    assert!(store.read().options().get("after").await.unwrap().is_some());
}

pub async fn jobs_claim_respects_run_at_priority_and_kinds(db: &TestDb) {
    let jobs = db.store().read().jobs();
    let node = NodeId::new();
    let mut future = job("a");
    future.run_at = Some(plus(Timestamp::now(), Duration::from_secs(600)));
    let future = enqueue(jobs, &future).await;
    let other_kind = enqueue(jobs, &job("b")).await;
    let mut ids = Vec::new();
    for priority in [0, 5, 1, 5] {
        let mut j = job("a");
        j.priority = priority;
        ids.push((priority, enqueue(jobs, &j).await));
    }

    let first = jobs.claim(node, &kinds(&["a"]), 2, LEASE).await.unwrap();
    let now = Timestamp::now();
    // Both priority-5 jobs, in (priority DESC, run_at, id) order.
    assert_eq!(first.len(), 2);
    assert!(first.iter().all(|j| j.priority == 5));
    assert!((first[0].run_at, first[0].id) < (first[1].run_at, first[1].id));
    for j in &first {
        assert_eq!(j.status, JobStatus::Running);
        assert_eq!(j.attempts, 1);
        assert_eq!(j.locked_by, Some(node));
        let until = j.locked_until.expect("locked_until");
        assert!(
            near(until, plus(now, LEASE), Duration::from_secs(2)),
            "{until:?}"
        );
        assert_eq!(get(jobs, j.id).await, *j, "returned row is the stored row");
    }

    let rest = jobs
        .claim(node, &kinds(&["a", "x"]), 10, LEASE)
        .await
        .unwrap();
    let priorities: Vec<i32> = rest.iter().map(|j| j.priority).collect();
    assert_eq!(priorities, [1, 0]);
    assert!(
        jobs.claim(node, &kinds(&["a"]), 10, LEASE)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(get(jobs, future).await.status, JobStatus::Queued);
    assert_eq!(get(jobs, other_kind).await.status, JobStatus::Queued);

    let b = jobs.claim(node, &kinds(&["b"]), 10, LEASE).await.unwrap();
    assert_eq!(b.iter().map(|j| j.id).collect::<Vec<_>>(), [other_kind]);
}

pub async fn jobs_expired_lease_is_reclaimed(db: &TestDb) {
    let jobs = db.store().read().jobs();
    let (a, b) = (NodeId::new(), NodeId::new());
    let id = enqueue(jobs, &job("k")).await;
    let first = jobs
        .claim(a, &kinds(&["k"]), 1, Duration::from_secs(1))
        .await
        .unwrap();
    assert_eq!(first.len(), 1);
    assert!(
        jobs.claim(b, &kinds(&["k"]), 1, LEASE)
            .await
            .unwrap()
            .is_empty(),
        "lease still held"
    );
    tokio::time::sleep(Duration::from_millis(1200)).await;
    let again = jobs.claim(b, &kinds(&["k"]), 1, LEASE).await.unwrap();
    assert_eq!(again.len(), 1);
    assert_eq!(again[0].id, id);
    assert_eq!(again[0].attempts, 2);
    assert_eq!(again[0].locked_by, Some(b));
}

/// Review Focus 2: a job whose node dies on every attempt goes dead at `max_attempts`.
pub async fn jobs_expired_lease_at_max_attempts_goes_dead(db: &TestDb) {
    let jobs = db.store().read().jobs();
    let short = Duration::from_secs(1);
    let mut poison = job("k");
    poison.max_attempts = 2;
    let id = enqueue(jobs, &poison).await;
    let (a, b) = (NodeId::new(), NodeId::new());
    assert_eq!(
        jobs.claim(a, &kinds(&["k"]), 1, short).await.unwrap().len(),
        1
    );
    tokio::time::sleep(Duration::from_millis(1200)).await;
    let second = jobs.claim(b, &kinds(&["k"]), 1, short).await.unwrap();
    assert_eq!(second.len(), 1);
    assert_eq!(second[0].attempts, 2);
    tokio::time::sleep(Duration::from_millis(1200)).await;
    assert!(
        jobs.claim(a, &kinds(&["k"]), 1, short)
            .await
            .unwrap()
            .is_empty()
    );
    let dead = get(jobs, id).await;
    assert_eq!(dead.status, JobStatus::Dead);
    let error = dead.last_error.expect("last_error");
    assert!(error.contains("lease expired after attempt 2"), "{error}");
    assert!(error.contains(&b.to_string()), "{error}");
    assert!(dead.finished_at.is_some());
    assert_eq!((dead.locked_by, dead.locked_until), (None, None));
    assert_eq!(dead.attempts, 2);
}

/// Review Focus 3: a node whose lease lapsed cannot overwrite the new owner's attempt.
pub async fn jobs_stale_lease_holder_cannot_write(db: &TestDb) {
    let jobs = db.store().read().jobs();
    let (a, b) = (NodeId::new(), NodeId::new());
    let id = enqueue(jobs, &job("k")).await;
    let stale = jobs
        .claim(a, &kinds(&["k"]), 1, Duration::from_secs(1))
        .await
        .unwrap()[0]
        .lease()
        .unwrap();
    tokio::time::sleep(Duration::from_millis(1200)).await;
    let current = jobs.claim(b, &kinds(&["k"]), 1, LEASE).await.unwrap()[0]
        .lease()
        .unwrap();
    assert_eq!((current.id, current.attempt), (id, 2));

    assert!(!jobs.extend(&stale, LEASE).await.unwrap());
    assert!(!jobs.complete(&stale).await.unwrap());
    assert_eq!(jobs.fail(&stale, "late", false).await.unwrap(), None);
    assert_eq!(jobs.fail(&stale, "late", true).await.unwrap(), None);
    assert!(!jobs.release(&stale, Duration::ZERO, false).await.unwrap());
    // Same node, old attempt: still not the holder.
    let own_old = ferrite_domain::JobLease { node: b, ..current };
    let own_old = ferrite_domain::JobLease {
        attempt: 1,
        ..own_old
    };
    assert!(!jobs.complete(&own_old).await.unwrap());

    let row = get(jobs, id).await;
    assert_eq!(row.status, JobStatus::Running);
    assert_eq!((row.locked_by, row.attempts), (Some(b), 2));
    assert_eq!(row.last_error, None);

    assert!(jobs.extend(&current, LEASE).await.unwrap());
    assert!(jobs.complete(&current).await.unwrap());
    let done = get(jobs, id).await;
    assert_eq!(done.status, JobStatus::Done);
    assert!(done.finished_at.is_some());
    assert_eq!((done.locked_by, done.locked_until), (None, None));
    // Finished: the token no longer holds anything.
    assert!(!jobs.complete(&current).await.unwrap());
    assert!(!jobs.extend(&current, LEASE).await.unwrap());
}

pub async fn jobs_fail_backoff_then_dead(db: &TestDb) {
    let jobs = db.store().read().jobs();
    let node = NodeId::new();
    let k = kinds(&["k"]);
    let id = enqueue(jobs, &job("k")).await;
    let lease = jobs.claim(node, &k, 1, LEASE).await.unwrap()[0]
        .lease()
        .unwrap();
    assert_eq!(
        jobs.fail(&lease, "boom 1", false).await.unwrap(),
        Some(JobStatus::Queued)
    );
    let row = get(jobs, id).await;
    assert_eq!(row.status, JobStatus::Queued);
    assert!(
        near(
            row.run_at,
            plus(Timestamp::now(), Duration::from_secs(20)),
            Duration::from_secs(2)
        ),
        "{:?}",
        row.run_at
    );
    assert_eq!(row.last_error.as_deref(), Some("boom 1"));
    assert_eq!(
        (row.locked_by, row.locked_until, row.finished_at),
        (None, None, None)
    );
    assert!(
        jobs.claim(node, &k, 1, LEASE).await.unwrap().is_empty(),
        "backing off"
    );

    for attempt in 2..=5 {
        db.force_job_due(id).await;
        let claimed = jobs.claim(node, &k, 1, LEASE).await.unwrap();
        assert_eq!(claimed.len(), 1, "attempt {attempt}");
        assert_eq!(claimed[0].attempts, attempt);
        assert_eq!(
            claimed[0].last_error.as_deref(),
            Some(&*format!("boom {}", attempt - 1))
        );
        let expected = if attempt < 5 {
            JobStatus::Queued
        } else {
            JobStatus::Dead
        };
        assert_eq!(
            jobs.fail(
                &claimed[0].lease().unwrap(),
                &format!("boom {attempt}"),
                false
            )
            .await
            .unwrap(),
            Some(expected)
        );
    }
    let dead = get(jobs, id).await;
    assert_eq!(dead.status, JobStatus::Dead);
    assert_eq!(dead.attempts, 5);
    assert_eq!(dead.last_error.as_deref(), Some("boom 5"));
    assert!(dead.finished_at.is_some());
    db.force_job_due(id).await;
    assert!(jobs.claim(node, &k, 1, LEASE).await.unwrap().is_empty());

    // Permanent failure on the first attempt goes straight to dead.
    let id = enqueue(jobs, &job("k")).await;
    let lease = jobs.claim(node, &k, 1, LEASE).await.unwrap()[0]
        .lease()
        .unwrap();
    assert_eq!(
        jobs.fail(&lease, "bad payload", true).await.unwrap(),
        Some(JobStatus::Dead)
    );
    let dead = get(jobs, id).await;
    assert_eq!((dead.status, dead.attempts), (JobStatus::Dead, 1));
    assert!(dead.finished_at.is_some());
}

pub async fn jobs_release_without_attempt(db: &TestDb) {
    let jobs = db.store().read().jobs();
    let node = NodeId::new();
    let k = kinds(&["k"]);
    let id = enqueue(jobs, &job("k")).await;
    let lease = jobs.claim(node, &k, 1, LEASE).await.unwrap()[0]
        .lease()
        .unwrap();
    assert!(jobs.release(&lease, Duration::ZERO, false).await.unwrap());
    let row = get(jobs, id).await;
    assert_eq!((row.status, row.attempts), (JobStatus::Queued, 0));
    assert_eq!((row.locked_by, row.locked_until), (None, None));
    assert!(
        !jobs.release(&lease, Duration::ZERO, false).await.unwrap(),
        "released"
    );

    let again = jobs.claim(node, &k, 1, LEASE).await.unwrap();
    assert_eq!(again.len(), 1, "claimable at once");
    assert_eq!(again[0].attempts, 1);

    // Counting the attempt, with a delay.
    assert!(
        jobs.release(&again[0].lease().unwrap(), Duration::from_secs(30), true)
            .await
            .unwrap()
    );
    let row = get(jobs, id).await;
    assert_eq!((row.status, row.attempts), (JobStatus::Queued, 1));
    assert!(near(
        row.run_at,
        plus(Timestamp::now(), Duration::from_secs(30)),
        Duration::from_secs(2)
    ));
    assert!(jobs.claim(node, &k, 1, LEASE).await.unwrap().is_empty());
}

/// Counting the final attempt on release ends the job like a failure would; refunding it
/// does not.
pub async fn jobs_release_final_attempt_goes_dead(db: &TestDb) {
    let jobs = db.store().read().jobs();
    let node = NodeId::new();
    let k = kinds(&["k"]);
    let mut last = job("k");
    last.max_attempts = 2;
    let id = enqueue(jobs, &last).await;

    // Attempt 1 of 2, counted: back to queued.
    let lease = jobs.claim(node, &k, 1, LEASE).await.unwrap()[0]
        .lease()
        .unwrap();
    assert!(jobs.release(&lease, Duration::ZERO, true).await.unwrap());
    let row = get(jobs, id).await;
    assert_eq!((row.status, row.attempts), (JobStatus::Queued, 1));
    assert_eq!((row.finished_at, row.last_error), (None, None));

    // Attempt 2 of 2, refunded: still queued, attempt given back.
    let lease = jobs.claim(node, &k, 1, LEASE).await.unwrap()[0]
        .lease()
        .unwrap();
    assert_eq!(lease.attempt, 2);
    assert!(jobs.release(&lease, Duration::ZERO, false).await.unwrap());
    assert_eq!(get(jobs, id).await.status, JobStatus::Queued);

    // Attempt 2 of 2, counted: dead.
    let lease = jobs.claim(node, &k, 1, LEASE).await.unwrap()[0]
        .lease()
        .unwrap();
    assert!(
        jobs.release(&lease, Duration::from_secs(30), true)
            .await
            .unwrap()
    );
    let dead = get(jobs, id).await;
    assert_eq!((dead.status, dead.attempts), (JobStatus::Dead, 2));
    assert!(dead.finished_at.is_some());
    assert_eq!(
        dead.last_error.as_deref(),
        Some("released after final attempt")
    );
    assert_eq!((dead.locked_by, dead.locked_until), (None, None));
    db.force_job_due(id).await;
    assert!(jobs.claim(node, &k, 1, LEASE).await.unwrap().is_empty());
    assert!(!jobs.release(&lease, Duration::ZERO, true).await.unwrap());
}

pub async fn jobs_error_text_is_sanitised(db: &TestDb) {
    let jobs = db.store().read().jobs();
    let node = NodeId::new();
    enqueue(jobs, &job("k")).await;
    let lease = jobs.claim(node, &kinds(&["k"]), 1, LEASE).await.unwrap()[0]
        .lease()
        .unwrap();
    let error = format!("a\0b{}", "é☃".repeat(1100));
    assert!(error.len() > 5 * 1024);
    jobs.fail(&lease, &error, false).await.unwrap();
    let stored = get(jobs, lease.id).await.last_error.expect("stored");
    assert!(stored.len() <= MAX_JOB_ERROR, "{}", stored.len());
    assert!(stored.len() > MAX_JOB_ERROR - 4);
    assert!(!stored.contains('\0'));
    assert!(stored.starts_with("a\u{FFFD}bé☃"));
}

pub async fn jobs_validation_does_not_poison(db: &TestDb) {
    let store = db.store();
    let tx = store.begin().await.unwrap();
    let jobs = tx.jobs();
    let node = NodeId::new();
    let invalid = |r: Result<_, StoreError>| matches!(r, Err(StoreError::Invalid(_)));
    assert!(invalid(jobs.enqueue(&job("Bad Kind")).await.map(|_| ())));
    assert!(invalid(
        jobs.enqueue(&NewJob::new("k", json!("not an object")))
            .await
            .map(|_| ())
    ));
    let mut long = job("k");
    long.unique_key = Some("x".repeat(201));
    assert!(invalid(jobs.enqueue(&long).await.map(|_| ())));
    let mut attempts = job("k");
    attempts.max_attempts = 0;
    assert!(invalid(jobs.enqueue(&attempts).await.map(|_| ())));
    assert!(invalid(jobs.claim(node, &[], 1, LEASE).await.map(|_| ())));
    assert!(invalid(
        jobs.claim(node, &kinds(&["k"]), 0, LEASE).await.map(|_| ())
    ));
    assert!(invalid(
        jobs.claim(node, &kinds(&["k"]), 101, LEASE)
            .await
            .map(|_| ())
    ));
    assert!(invalid(
        jobs.claim(node, &vec!["k".to_string(); 65], 1, LEASE)
            .await
            .map(|_| ())
    ));
    assert!(invalid(
        jobs.claim(node, &kinds(&["k"]), 1, Duration::ZERO)
            .await
            .map(|_| ())
    ));
    // The Tx is still usable and commits.
    let id = jobs.enqueue(&job("k")).await.unwrap().expect("inserted");
    tx.commit().await.unwrap();
    assert!(store.read().jobs().get(id).await.unwrap().is_some());
}

pub async fn jobs_prune_by_status_age(db: &TestDb) {
    let jobs = db.store().read().jobs();
    let node = NodeId::new();
    let k = kinds(&["k"]);
    let mut finish = Vec::new();
    for _ in 0..3 {
        finish.push(enqueue(jobs, &job("k")).await);
    }
    let claimed = jobs.claim(node, &k, 3, LEASE).await.unwrap();
    let by_id = |id| {
        claimed
            .iter()
            .find(|j| j.id == id)
            .unwrap()
            .lease()
            .unwrap()
    };
    assert!(jobs.complete(&by_id(finish[0])).await.unwrap());
    assert!(jobs.complete(&by_id(finish[1])).await.unwrap());
    jobs.fail(&by_id(finish[2]), "x", true).await.unwrap();
    let queued = enqueue(jobs, &job("k")).await;
    let running = enqueue(jobs, &job("k")).await;
    let _ = jobs.claim(node, &kinds(&["k"]), 1, LEASE).await.unwrap();
    assert!(
        matches!(get(jobs, queued).await.status, JobStatus::Running)
            ^ matches!(get(jobs, running).await.status, JobStatus::Running)
    );

    let past = Timestamp::from_micros(Timestamp::now().as_micros() - 3_600_000_000).unwrap();
    let future = plus(Timestamp::now(), Duration::from_secs(3600));
    // Cut-offs before every finish: nothing goes.
    assert_eq!(jobs.prune(past, past, 100).await.unwrap(), 0);
    // Dead uses its own cut-off: done go, dead stays.
    assert_eq!(jobs.prune(future, past, 100).await.unwrap(), 2);
    assert!(jobs.get(finish[0]).await.unwrap().is_none());
    assert!(jobs.get(finish[2]).await.unwrap().is_some());
    assert_eq!(jobs.prune(past, future, 100).await.unwrap(), 1);
    assert!(jobs.get(finish[2]).await.unwrap().is_none());
    // Queued and running are never pruned.
    assert_eq!(jobs.prune(future, future, 100).await.unwrap(), 0);
    assert!(jobs.get(queued).await.unwrap().is_some());
    assert!(jobs.get(running).await.unwrap().is_some());
    let counts = jobs.counts().await.unwrap();
    assert_eq!(counts[&JobStatus::Queued] + counts[&JobStatus::Running], 2);
    assert_eq!(counts[&JobStatus::Done] + counts[&JobStatus::Dead], 0);
}

/// Rolling deploys (04 §9): a newer schema's extra column does not break this binary.
pub async fn jobs_tolerate_unknown_columns(db: &TestDb) {
    db.exec_raw("ALTER TABLE jobs ADD COLUMN future_col text DEFAULT 'x'")
        .await;
    let jobs = db.store().read().jobs();
    let node = NodeId::new();
    let id = enqueue(jobs, &job("k")).await;
    let claimed = jobs.claim(node, &kinds(&["k"]), 1, LEASE).await.unwrap();
    assert_eq!(claimed.len(), 1);
    assert!(jobs.complete(&claimed[0].lease().unwrap()).await.unwrap());
    assert_eq!(get(jobs, id).await.status, JobStatus::Done);
}

/// 500 jobs, 8 workers on distinct nodes (Postgres: each on its own store, so the claim
/// races across connections under `SKIP LOCKED`; SQLite: the shared store, pinning the
/// single-writer equivalent): every job is claimed exactly once and ends `done`.
pub async fn jobs_claim_is_exclusive_under_concurrency(db: Arc<TestDb>) {
    const JOBS: usize = 500;
    const WORKERS: usize = 8;
    let store = db.shared_store();
    for _ in 0..JOBS {
        enqueue(store.read().jobs(), &job("k")).await;
    }
    let mut tasks = Vec::new();
    for _ in 0..WORKERS {
        let worker: Arc<dyn Store> = db.peer().await.unwrap_or_else(|| store.clone());
        tasks.push(tokio::spawn(async move {
            let node = NodeId::new();
            let mut seen = Vec::new();
            loop {
                let batch = worker
                    .read()
                    .jobs()
                    .claim(node, &kinds(&["k"]), 7, LEASE)
                    .await
                    .unwrap();
                if batch.is_empty() {
                    break;
                }
                for j in batch {
                    assert_eq!(j.locked_by, Some(node));
                    assert!(
                        worker
                            .read()
                            .jobs()
                            .complete(&j.lease().unwrap())
                            .await
                            .unwrap()
                    );
                    seen.push(j.id);
                }
            }
            seen
        }));
    }
    let mut all = Vec::new();
    for task in tasks {
        all.extend(task.await.unwrap());
    }
    let unique: BTreeSet<JobId> = all.iter().copied().collect();
    assert_eq!(all.len(), JOBS, "every job claimed exactly once");
    assert_eq!(unique.len(), JOBS);
    let counts = store.read().jobs().counts().await.unwrap();
    assert_eq!(counts[&JobStatus::Done], JOBS as u64);
}

/// Two nodes enqueue the same key at once. Postgres: node A's insert is still
/// uncommitted when node B's arrives; B waits on the unique index and gets `None` once A
/// commits (no error, no duplicate). SQLite pins the single-writer equivalent: the second
/// enqueue can only run after the first `Tx` commits, and it gets `None`.
pub async fn jobs_concurrent_enqueue_same_key(db: Arc<TestDb>) {
    let mut keyed = job("k");
    keyed.unique_key = Some("race".into());
    let store = db.shared_store();
    let (Some(a), Some(b)) = (db.peer().await, db.peer().await) else {
        let tx = store.begin().await.unwrap();
        let first = tx.jobs().enqueue(&keyed).await.unwrap();
        tx.commit().await.unwrap();
        assert!(first.is_some());
        assert_eq!(store.read().jobs().enqueue(&keyed).await.unwrap(), None);
        assert_eq!(
            store.read().jobs().counts().await.unwrap()[&JobStatus::Queued],
            1
        );
        return;
    };
    let tx = a.begin().await.unwrap();
    let first = tx.jobs().enqueue(&keyed).await.unwrap().expect("inserted");
    let second = {
        let keyed = keyed.clone();
        tokio::spawn(async move { b.read().jobs().enqueue(&keyed).await })
    };
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(!second.is_finished(), "B must wait for A's uncommitted key");
    tx.commit().await.unwrap();
    let second = tokio::time::timeout(Duration::from_secs(5), second)
        .await
        .expect("B finishes after A commits")
        .unwrap()
        .unwrap();
    assert_eq!(second, None);
    let jobs = store.read().jobs();
    assert_eq!(get(jobs, first).await.unique_key.as_deref(), Some("race"));
    assert_eq!(jobs.counts().await.unwrap()[&JobStatus::Queued], 1);
}

/// R5: `prune` deletes at most `limit` finished jobs per call; the limit is checked.
pub async fn jobs_prune_in_batches(db: &TestDb) {
    let jobs = db.store().read().jobs();
    let node = NodeId::new();
    for _ in 0..3 {
        enqueue(jobs, &job("k")).await;
    }
    let claimed = jobs.claim(node, &kinds(&["k"]), 3, LEASE).await.unwrap();
    assert!(jobs.complete(&claimed[0].lease().unwrap()).await.unwrap());
    assert!(jobs.complete(&claimed[1].lease().unwrap()).await.unwrap());
    jobs.fail(&claimed[2].lease().unwrap(), "x", true)
        .await
        .unwrap();
    let future = plus(Timestamp::now(), Duration::from_secs(3600));
    for bad in [0, ferrite_store::MAX_PRUNE_BATCH + 1] {
        assert!(matches!(
            jobs.prune(future, future, bad).await,
            Err(StoreError::Invalid(_))
        ));
    }
    assert_eq!(jobs.prune(future, future, 2).await.unwrap(), 2);
    assert_eq!(jobs.prune(future, future, 2).await.unwrap(), 1);
    assert_eq!(jobs.prune(future, future, 2).await.unwrap(), 0);
    let counts = jobs.counts().await.unwrap();
    assert_eq!(counts.values().sum::<u64>(), 0);
}

/// Due queued jobs and expired leases compete in one ordering (`priority DESC, run_at,
/// id`) under one LIMIT across both branches, never per branch: limits 1, 2 and 3 claim
/// a prefix of `[queued p9 (older run_at), expired lease p9, queued p1]`; a job not yet
/// due is never claimed.
pub async fn jobs_claim_orders_queued_and_expired_leases_together(db: &TestDb) {
    let jobs = db.store().read().jobs();
    let (a, b) = (NodeId::new(), NodeId::new());
    let now = Timestamp::now();
    let hour = Duration::from_secs(3600);
    let past = Timestamp::from_micros(now.as_micros() - 3_600_000_000).unwrap();
    let mut leases = Vec::new();
    for limit in 1..=3u32 {
        let kind = format!("race{limit}");
        let mut lease = job(&kind);
        lease.priority = 9;
        let id = enqueue(jobs, &lease).await;
        let got = jobs
            .claim(a, &kinds(&[&kind]), 1, Duration::from_secs(1))
            .await
            .unwrap();
        assert_eq!(got.iter().map(|j| j.id).collect::<Vec<_>>(), [id]);
        leases.push(id);
    }
    tokio::time::sleep(Duration::from_millis(1200)).await;
    for (limit, lease) in (1..=3u32).zip(leases) {
        let kind = format!("race{limit}");
        let mut older = job(&kind);
        older.priority = 9;
        older.run_at = Some(past);
        let older = enqueue(jobs, &older).await;
        let mut low = job(&kind);
        low.priority = 1;
        let low = enqueue(jobs, &low).await;
        let mut future = job(&kind);
        future.priority = 99;
        future.run_at = Some(plus(now, hour));
        let future = enqueue(jobs, &future).await;

        let claimed = jobs.claim(b, &kinds(&[&kind]), limit, LEASE).await.unwrap();
        let order = [older, lease, low];
        assert_eq!(
            claimed.iter().map(|j| j.id).collect::<Vec<_>>(),
            order[..limit as usize],
            "limit {limit}"
        );
        assert!(claimed.iter().all(|j| j.locked_by == Some(b)));
        for id in &order[limit as usize..] {
            let left = get(jobs, *id).await;
            let expected = if *id == lease {
                JobStatus::Running
            } else {
                JobStatus::Queued
            };
            assert_eq!(left.status, expected, "limit {limit}");
            assert_ne!(left.locked_by, Some(b));
        }
        if limit >= 2 {
            assert_eq!(get(jobs, lease).await.attempts, 2);
        }
        assert_eq!(get(jobs, future).await.status, JobStatus::Queued);
        assert!(
            jobs.claim(b, &kinds(&[&kind]), 10, LEASE)
                .await
                .unwrap()
                .iter()
                .all(|j| j.id != future)
        );
    }
}
