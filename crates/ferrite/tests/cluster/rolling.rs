//! Schema and version changes under running nodes: an old and a new binary on one
//! schema that an N+1 expand migration widened (04 §9, P5, P28), and two nodes started
//! at once on an empty schema (Review Focus 5).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ferrite::{NodeOptions, RunnerOverrides};
use ferrite_core::events::post_key;
use ferrite_domain::{JobId, JobStatus, NewEvent, NewJob};
use ferrite_store::LOCK_MIGRATE;
use ferrite_store_tests::{migration_dirs, migration_files};
use serde_json::json;
use sqlx::Connection;
use tokio::time::Instant;

use crate::harness::{
    CONVERGE, Cluster, Recorder, SETUP, Spec, Then, admin, content, post_draft, published_post,
    runs, wait_until,
};

const OLD: usize = 0;
const NEW: usize = 1;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rolling_version_coexistence() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let Some(mut c) = Cluster::prepare(Spec::new(2)).await else {
        return;
    };
    // What the expand migration of version N+1 does before the new binary rolls out:
    // new columns on the tables both versions read and a new table.
    c.db.exec_raw(
        "ALTER TABLE jobs ADD COLUMN future_col text; \
             ALTER TABLE nodes ADD COLUMN future_col text DEFAULT 'x'; \
             ALTER TABLE events ADD COLUMN future_col jsonb; \
             CREATE TABLE future_table (id bigint PRIMARY KEY, note text)",
    )
    .await;
    let old = NodeOptions {
        version: Some(semver::Version::new(0, 1, 0)),
        extra_handlers: vec![Recorder::handler("k1", &[1], OLD, &log, Then::Done)],
        periodics: Some(Vec::new()),
        ..NodeOptions::default()
    };
    // The new node polls every 50 ms, so a `k1 v2` the old node handed back (with a
    // 10 s backoff, P5) usually goes to the new node when it is due (if the old node
    // wins the race, it hands it back again; the wait below allows for that).
    let new = NodeOptions {
        version: Some(semver::Version::new(0, 2, 0)),
        extra_handlers: vec![
            Recorder::handler("k1", &[1, 2], NEW, &log, Then::Done),
            Recorder::handler("k2", &[1], NEW, &log, Then::Done),
        ],
        periodics: Some(Vec::new()),
        runner: RunnerOverrides {
            idle_poll: Some(Duration::from_millis(50)),
            ..RunnerOverrides::default()
        },
        ..NodeOptions::default()
    };
    // The old node runs alone first, so it sees every job: it must run `k1 v1`, hand
    // `k1 v2` back unrun without using an attempt, and never claim `k2`.
    c.start(OLD, old).await;
    let store = c.db.shared_store();
    let mut ids: BTreeMap<JobId, (&str, u64)> = BTreeMap::new();
    let tx = store.begin().await.unwrap();
    for (kind, v) in [("k1", 1), ("k1", 2), ("k2", 1)] {
        for _ in 0..10 {
            let id = tx
                .jobs()
                .enqueue(&NewJob::new(kind, json!({"v": v})))
                .await
                .unwrap()
                .unwrap();
            ids.insert(id, (kind, v));
        }
    }
    tx.commit().await.unwrap();
    let started = Instant::now();
    let mut pg = c.pg().await;
    loop {
        let (v1_done, v2_back, k2_untouched): (i64, i64, i64) = sqlx::query_as(
            "SELECT \
               count(*) FILTER (WHERE kind = 'k1' AND payload->>'v' = '1' \
                                AND status = 'done'), \
               count(*) FILTER (WHERE kind = 'k1' AND payload->>'v' = '2' \
                                AND status = 'queued' AND attempts = 0 \
                                AND run_at > now()), \
               count(*) FILTER (WHERE kind = 'k2' AND status = 'queued' \
                                AND attempts = 0 AND run_at <= now()) \
             FROM jobs",
        )
        .fetch_one(&mut pg)
        .await
        .unwrap();
        if (v1_done, v2_back, k2_untouched) == (10, 10, 10) {
            break;
        }
        assert!(
            started.elapsed() < SETUP,
            "old node alone: {v1_done} k1 v1 done, {v2_back} k1 v2 handed back, \
             {k2_untouched} k2 untouched"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        runs(&log)
            .iter()
            .all(|r| r.node == OLD && r.v == 1 && r.kind == "k1")
    );
    pg.close().await.unwrap();

    // The new node joins: both versions now run side by side.
    c.start(NEW, new).await;
    let took = wait_until(Duration::from_secs(60), "30 jobs done", || {
        let store = store.clone();
        async move {
            store
                .read()
                .jobs()
                .counts()
                .await
                .unwrap()
                .get(&JobStatus::Done)
                == Some(&30)
        }
    })
    .await;
    let counts = store.read().jobs().counts().await.unwrap();
    assert_eq!(
        counts.get(&JobStatus::Dead).copied().unwrap_or(0),
        0,
        "{counts:?}"
    );
    let runs = runs(&log);
    assert_eq!(runs.len(), 30, "one run per job: {runs:?}");
    let ran: BTreeSet<JobId> = runs.iter().map(|r| r.job).collect();
    assert_eq!(ran.len(), 30);
    for r in &runs {
        let (kind, v) = ids[&r.job];
        assert_eq!((r.kind.as_str(), r.v), (kind, v));
        if (kind, v) != ("k1", 1) {
            assert_eq!(r.node, NEW, "{kind} v{v} ran only on the new node: {r:?}");
        }
    }
    for id in ids.keys() {
        let job = store.read().jobs().get(*id).await.unwrap().unwrap();
        assert_eq!(
            job.attempts, 1,
            "a handed-back job kept its attempt: {job:?}"
        );
    }
    let old_k1 = runs.iter().filter(|r| r.node == OLD).count();
    eprintln!(
        "rolling: 30 jobs in {:?} ({took:?} after the new node joined); old node ran {old_k1}",
        started.elapsed()
    );

    // An event kind neither binary knows invalidates its keys on both.
    let svc = content(c.store(NEW));
    let actor = admin(c.store(NEW).as_ref()).await;
    let item = published_post(&svc, &actor, "rolling").await;
    // Served and cached on both (a repeated GET renders nothing), so a later render
    // can only come from the unknown event.
    for i in [OLD, NEW] {
        wait_until(SETUP, "post served from the cache", || async {
            let renders = c.node(i).state.renders();
            c.get(i, "/rolling/").await.0 == 200 && c.node(i).state.renders() == renders
        })
        .await;
    }
    let renders = [OLD, NEW].map(|i| c.node(i).state.renders());
    store
        .read()
        .events()
        .append(&NewEvent {
            kind: "future.kind".into(),
            keys: vec![post_key(item.id)],
            origin_node: None,
        })
        .await
        .unwrap();
    for i in [OLD, NEW] {
        wait_until(
            CONVERGE,
            "future.kind invalidated the post page",
            || async {
                c.get(i, "/rolling/").await;
                c.node(i).state.renders() > renders[i]
            },
        )
        .await;
    }
    // Both loops are still applying events afterwards.
    svc.update(
        &actor,
        item.id,
        item.version,
        post_draft("After future", "rolling"),
    )
    .await
    .unwrap();
    for i in [OLD, NEW] {
        wait_until(CONVERGE, "edit after the unknown event", || async {
            c.get(i, "/rolling/").await.1.contains("After future")
        })
        .await;
    }

    let versions: BTreeSet<String> = store
        .read()
        .nodes()
        .list()
        .await
        .unwrap()
        .into_iter()
        .map(|n| n.version)
        .collect();
    assert_eq!(
        versions,
        BTreeSet::from(["0.1.0".to_owned(), "0.2.0".to_owned()])
    );
    c.finish().await;
}

/// Review Focus 5: `docker compose up` on a fresh database. Exactly one node migrates
/// (the other waits on the migrate lock and finds the schema current); both serve.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_first_start_migrates_once() {
    let mut spec = Spec::new(2);
    spec.migrate = false;
    let Some(mut c) = Cluster::prepare(spec).await else {
        return;
    };
    let mut pg = c.pg().await;
    let schema = c.schema.clone();
    async fn tables(pg: &mut sqlx::PgConnection, schema: &str) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM information_schema.tables WHERE table_schema = $1")
            .bind(schema)
            .fetch_one(pg)
            .await
            .unwrap()
    }
    assert_eq!(tables(&mut pg, &schema).await, 0, "empty schema");

    // Hold the migrate lock (as a running `ferrite migrate` would): both nodes must wait
    // on it rather than migrate, then one migrates and the other finds the schema
    // current once it is released.
    let holder = c.db.shared_store();
    let guard = holder
        .try_lock(LOCK_MIGRATE)
        .await
        .unwrap()
        .expect("migrate lock free");
    let gate = async {
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert_eq!(
            tables(&mut pg, &schema).await,
            0,
            "no node migrated while the lock was held"
        );
        guard.release().await.unwrap();
    };
    tokio::join!(c.start_concurrently(|_| NodeOptions::default()), gate);
    for i in 0..c.len() {
        for path in ["/healthz", "/readyz"] {
            let (status, body) = c.get(i, path).await;
            assert_eq!(status, 200, "node {i} {path}: {body}");
        }
        assert_eq!(c.get(i, "/").await.0, 200, "node {i} serves the site");
    }
    let applied: Vec<(i64, i64, bool)> = sqlx::query_as(
        "SELECT version, count(*), bool_and(success) FROM _sqlx_migrations \
         GROUP BY version ORDER BY version",
    )
    .fetch_all(&mut pg)
    .await
    .unwrap();
    let [_, pg_dir] = migration_dirs();
    let expected = migration_files(&pg_dir)
        .iter()
        .filter(|f| f.ends_with(".up.sql"))
        .count();
    assert_eq!(applied.len(), expected, "{applied:?}");
    for (version, n, ok) in &applied {
        assert_eq!(*n, 1, "version {version} applied once");
        assert!(ok, "version {version} succeeded");
    }
    let nodes = c.db.store().read().nodes().list().await.unwrap();
    assert_eq!(nodes.len(), 2, "both registered: {nodes:?}");
    pg.close().await.unwrap();
    c.finish().await;
}
