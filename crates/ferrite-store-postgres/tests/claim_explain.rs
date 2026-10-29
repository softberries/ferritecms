#![allow(clippy::unwrap_used, clippy::expect_used)]

//! P31: the job claim must read a bounded number of rows even with tens of thousands of
//! future-dated jobs queued (the WordPress import schedules one per future post).
//!
//! Gated on `FERRITE_PG_EXPLAIN` (plus `FERRITE_TEST_PG_URL`); prints `SKIP` otherwise.
//! Run with `--nocapture` to see the plan.

use ferrite_store::Store;
use ferrite_store_postgres::{PgOptions, PgStore};
use sqlx::{AssertSqlSafe, Connection, PgConnection, Row};

const KIND: &str = "content.publish_scheduled";
const LIMIT: i64 = 8;
const DUE: i64 = 3;
const EXPIRED: i64 = 2;

/// The selection inside `claim`'s `next` CTE in `src/repos/job.rs`; keep them identical.
const SELECTION: &str = "SELECT id FROM ( \
      SELECT id, priority, run_at FROM ( \
        SELECT id, priority, run_at FROM jobs \
        WHERE kind = ANY($1) AND status = 'queued' AND run_at <= now() \
        ORDER BY priority DESC, run_at, id \
        LIMIT $2 \
        FOR UPDATE SKIP LOCKED) due \
      UNION ALL \
      SELECT id, priority, run_at FROM ( \
        SELECT id, priority, run_at FROM jobs \
        WHERE kind = ANY($1) AND status = 'running' AND locked_until < now() \
          AND attempts < max_attempts \
        ORDER BY priority DESC, run_at, id \
        LIMIT $2 \
        FOR UPDATE SKIP LOCKED) expired) u \
    ORDER BY priority DESC, run_at, id \
    LIMIT $2";

/// Sums the rows read (returned plus filtered out, times loops) of every scan node.
fn scanned_rows(plan: &serde_json::Value) -> i64 {
    let mut total = 0;
    let node_type = plan["Node Type"].as_str().unwrap_or("");
    if node_type.ends_with("Scan") {
        // Rows read = rows returned + rows the node discarded: `Actual Rows` alone hides
        // a scan that reads the whole index and filters it away.
        let read = [
            "Actual Rows",
            "Rows Removed by Filter",
            "Rows Removed by Index Recheck",
        ]
        .iter()
        .map(|k| plan[*k].as_f64().unwrap_or(0.0))
        .sum::<f64>();
        let loops = plan["Actual Loops"].as_f64().unwrap_or(1.0);
        total += (read * loops) as i64;
    }
    if let Some(children) = plan["Plans"].as_array() {
        total += children.iter().map(scanned_rows).sum::<i64>();
    }
    total
}

#[test]
fn claim_reads_a_bounded_number_of_rows() {
    if std::env::var("FERRITE_PG_EXPLAIN").is_err() {
        eprintln!("SKIP: claim EXPLAIN skipped (FERRITE_PG_EXPLAIN is not set)");
        return;
    }
    let Ok(url) = std::env::var("FERRITE_TEST_PG_URL") else {
        if std::env::var("CI").as_deref() == Ok("true") {
            panic!("FERRITE_TEST_PG_URL must be set in CI");
        }
        eprintln!("SKIP: claim EXPLAIN skipped (FERRITE_TEST_PG_URL is not set)");
        return;
    };
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
        .block_on(run(&url));
}

async fn run(url: &str) {
    let schema = format!("t_{}", uuid::Uuid::now_v7().simple());
    let mut admin = PgConnection::connect(url).await.expect("connect");
    sqlx::raw_sql(AssertSqlSafe(format!("CREATE SCHEMA \"{schema}\"")))
        .execute(&mut admin)
        .await
        .expect("create schema");
    let (rows, text) = measure(url, &schema, &mut admin).await;
    sqlx::raw_sql(AssertSqlSafe(format!("DROP SCHEMA \"{schema}\" CASCADE")))
        .execute(&mut admin)
        .await
        .expect("drop schema");
    println!("{text}");
    println!(
        "scan rows read: {rows} (bound {})",
        10 * LIMIT + DUE + EXPIRED
    );
    assert!(
        rows <= 10 * LIMIT + DUE + EXPIRED,
        "claim read {rows} rows; plan:\n{text}"
    );
}

async fn measure(url: &str, schema: &str, admin: &mut PgConnection) -> (i64, String) {
    let mut opts = PgOptions::new(url);
    opts.max_connections = 2;
    opts.schema = Some(schema.to_owned());
    let store = PgStore::open(opts).await.expect("open store");
    store.migrate_up(None).await.expect("migrate");
    store.close().await;

    sqlx::raw_sql(AssertSqlSafe(format!("SET search_path TO \"{schema}\"")))
        .execute(&mut *admin)
        .await
        .expect("search_path");
    for sql in [
        "INSERT INTO jobs (id, kind, payload, run_at, status, created_at) \
         SELECT gen_random_uuid(), $1, '{}'::jsonb, \
                now() + make_interval(mins => g), 'queued', now() \
         FROM generate_series(1, 50000) g",
        "INSERT INTO jobs (id, kind, payload, run_at, status, created_at) \
         SELECT gen_random_uuid(), $1, '{}'::jsonb, now() - interval '1 minute', \
                'queued', now() FROM generate_series(1, 3)",
        "INSERT INTO jobs (id, kind, payload, run_at, status, attempts, locked_by, \
                           locked_until, created_at) \
         SELECT gen_random_uuid(), $1, '{}'::jsonb, now() - interval '1 hour', \
                'running', 1, 'n', now() - interval '1 minute', now() \
         FROM generate_series(1, 2)",
    ] {
        sqlx::query(sql)
            .bind(KIND)
            .execute(&mut *admin)
            .await
            .expect("seed");
    }
    sqlx::raw_sql("ANALYZE jobs")
        .execute(&mut *admin)
        .await
        .expect("analyze");

    let mut tx = admin.begin().await.expect("begin");
    let kinds = vec![KIND.to_owned()];
    let json: serde_json::Value = sqlx::query(AssertSqlSafe(format!(
        "EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) {SELECTION}"
    )))
    .bind(&kinds)
    .bind(LIMIT)
    .fetch_one(&mut *tx)
    .await
    .map(|r| r.get::<serde_json::Value, _>(0))
    .expect("explain json");
    let text: Vec<String> = sqlx::query(AssertSqlSafe(format!(
        "EXPLAIN (ANALYZE, BUFFERS) {SELECTION}"
    )))
    .bind(&kinds)
    .bind(LIMIT)
    .fetch_all(&mut *tx)
    .await
    .expect("explain text")
    .iter()
    .map(|r| r.get::<String, _>(0))
    .collect();
    tx.rollback().await.expect("rollback");
    (scanned_rows(&json[0]["Plan"]), text.join("\n"))
}
