//! Per-test databases: a fresh SQLite file or a fresh Postgres schema, migrated up.

use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

use ferrite_domain::JobId;
use ferrite_store::Store;
use ferrite_store_postgres::{PgOptions, PgStore};
use ferrite_store_sqlite::{SqliteOptions, SqliteStore};
use sqlx::{AssertSqlSafe, Connection, PgConnection, SqliteConnection};
use tempfile::TempDir;

/// Env var with the Postgres server URL used for contract tests.
pub const PG_URL_ENV: &str = "FERRITE_TEST_PG_URL";

/// Fresh file-backed SQLite store (WAL + two pools, not `:memory:`), migrated up.
pub async fn sqlite_store() -> (SqliteStore, TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::open(SqliteOptions::new(dir.path().join("ferrite.db")))
        .await
        .expect("open sqlite store");
    store.migrate_up(None).await.expect("migrate sqlite up");
    (store, dir)
}

/// A Postgres store in its own schema. Call [`teardown`](Self::teardown) at the end of
/// the test (the `backend_tests!` macros do); `Drop` is only a quick fallback.
pub struct PgFixture {
    pub store: Arc<PgStore>,
    url: String,
    schema: String,
    /// Extra stores on the same schema ([`TestDb::peer`]), closed by teardown.
    peers: Mutex<Vec<Arc<PgStore>>>,
    /// Peers opened so far; numbers each peer's `application_name`.
    peer_count: AtomicUsize,
    torn_down: AtomicBool,
}

/// How long teardown waits for the pool to close (a `Tx` still alive somewhere keeps
/// its connection checked out).
const CLOSE_TIMEOUT: Duration = Duration::from_secs(5);

impl PgFixture {
    /// Closes the store's pool, then drops the schema. Closing first matters: it waits
    /// for every connection to come back, which on the still-running test runtime lets
    /// sqlx finish the rollback of a dropped `Tx`, so `DROP SCHEMA` finds no locks held.
    /// Idempotent.
    pub async fn teardown(&self) {
        if self.torn_down.swap(true, Ordering::SeqCst) {
            return;
        }
        let peers: Vec<Arc<PgStore>> = self
            .peers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .drain(..)
            .collect();
        for peer in peers {
            if tokio::time::timeout(CLOSE_TIMEOUT, peer.close())
                .await
                .is_err()
            {
                eprintln!("warning: peer pool for {} not closed in time", self.schema);
            }
        }
        if tokio::time::timeout(CLOSE_TIMEOUT, self.store.close())
            .await
            .is_err()
        {
            eprintln!(
                "warning: pool for test schema {} not closed within {CLOSE_TIMEOUT:?} \
                 (a Tx is still alive?)",
                self.schema
            );
        }
        if let Err(e) = drop_schema(&self.url, &self.schema, "5s").await {
            eprintln!("warning: could not drop test schema {}: {e}", self.schema);
        }
    }
}

impl Drop for PgFixture {
    /// Fallback when `teardown` did not run (the test panicked, or the fixture was used
    /// without the macros). It cannot close the pool: that needs the test runtime, which
    /// this blocking call would stall. So it tries one `DROP SCHEMA` with a short lock
    /// timeout on its own connection and runtime, and if a lock is still held (a dropped
    /// `Tx` whose rollback could not run yet) it reports the leaked schema instead of
    /// waiting.
    fn drop(&mut self) {
        if self.torn_down.load(Ordering::SeqCst) {
            return;
        }
        let (url, schema) = (self.url.clone(), self.schema.clone());
        let handle = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            rt.block_on(drop_schema(&url, &schema, "200ms"))
                .map_err(std::io::Error::other)
        });
        if let Ok(Err(e)) = handle.join() {
            eprintln!(
                "warning: leaked test schema {} (teardown did not run): {e}",
                self.schema
            );
        }
    }
}

/// `DROP SCHEMA ... CASCADE` on a dedicated connection, giving up after `lock_timeout`.
async fn drop_schema(url: &str, schema: &str, lock_timeout: &str) -> Result<(), sqlx::Error> {
    let sql = format!(
        "SET lock_timeout = '{lock_timeout}'; DROP SCHEMA IF EXISTS {} CASCADE",
        quote_ident(schema)
    );
    let mut conn = PgConnection::connect(url).await?;
    sqlx::raw_sql(AssertSqlSafe(sql)).execute(&mut conn).await?;
    conn.close().await
}

/// A Postgres store in a fresh `t_<uuid>` schema, migrated up. Returns `None` (after a
/// `SKIP` line on stderr) when `FERRITE_TEST_PG_URL` is unset; panics if CI=true.
pub async fn pg_store() -> Option<PgFixture> {
    pg_store_with(true).await
}

/// [`pg_store`], migrated up only when `migrate` is set (an empty schema otherwise).
async fn pg_store_with(migrate: bool) -> Option<PgFixture> {
    let Ok(url) = std::env::var(PG_URL_ENV) else {
        if std::env::var("CI").as_deref() == Ok("true") {
            panic!("{PG_URL_ENV} must be set in CI");
        }
        eprintln!("SKIP: Postgres contract test skipped ({PG_URL_ENV} is not set)");
        return None;
    };
    let schema = format!("t_{}", uuid::Uuid::now_v7().simple());
    let mut admin = PgConnection::connect(&url).await.expect("connect admin");
    sqlx::raw_sql(AssertSqlSafe(format!(
        "CREATE SCHEMA {}",
        quote_ident(&schema)
    )))
    .execute(&mut admin)
    .await
    .expect("create test schema");
    // The schema doubles as `application_name`, so `terminate_backends` stays inside
    // this test.
    let opened = PgStore::open(pg_options(&url, &schema, schema.clone())).await;
    let store = match opened {
        Ok(store) => store,
        Err(e) => {
            // No fixture exists yet to drop the schema, so do it here.
            let drop_sql = format!("DROP SCHEMA IF EXISTS {} CASCADE", quote_ident(&schema));
            if let Err(de) = sqlx::raw_sql(AssertSqlSafe(drop_sql))
                .execute(&mut admin)
                .await
            {
                eprintln!("warning: could not drop test schema {schema}: {de}");
            }
            panic!("open postgres store: {e}");
        }
    };
    admin.close().await.expect("close admin connection");
    let fixture = PgFixture {
        store: Arc::new(store),
        url,
        schema,
        peers: Mutex::new(Vec::new()),
        peer_count: AtomicUsize::new(0),
        torn_down: AtomicBool::new(false),
    };
    if migrate {
        fixture
            .store
            .migrate_up(None)
            .await
            .expect("migrate postgres up");
    }
    Some(fixture)
}

/// Options for a test store on `schema` (`max_connections` 4).
fn pg_options(url: &str, schema: &str, application_name: String) -> PgOptions {
    let mut opts = PgOptions::new(url);
    opts.max_connections = 4;
    opts.schema = Some(schema.to_owned());
    opts.application_name = application_name;
    opts
}

fn quote_ident(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

/// A migrated database for one contract test, plus raw backend access for assertions
/// the `Store` API deliberately cannot express (schema dumps, tampering).
pub struct TestDb {
    backing: Backing,
}

enum Backing {
    Sqlite {
        store: Arc<SqliteStore>,
        path: PathBuf,
        _dir: TempDir,
    },
    Postgres(Box<PgFixture>),
}

impl TestDb {
    pub async fn sqlite() -> Self {
        let (store, dir) = sqlite_store().await;
        let path = dir.path().join("ferrite.db");
        Self {
            backing: Backing::Sqlite {
                store: Arc::new(store),
                path,
                _dir: dir,
            },
        }
    }

    /// `None` when Postgres tests are skipped (see [`pg_store`]).
    pub async fn postgres() -> Option<Self> {
        pg_store().await.map(|f| Self {
            backing: Backing::Postgres(Box::new(f)),
        })
    }

    /// [`Self::postgres`] on an empty schema: no migration applied, so the code under
    /// test migrates it (first start of a site).
    pub async fn postgres_unmigrated() -> Option<Self> {
        pg_store_with(false).await.map(|f| Self {
            backing: Backing::Postgres(Box::new(f)),
        })
    }

    /// Ends the test's use of the database: on Postgres closes the pool and drops the
    /// schema (see [`PgFixture::teardown`]); SQLite's temp dir goes on drop.
    pub async fn teardown(&self) {
        if let Backing::Postgres(f) = &self.backing {
            f.teardown().await;
        }
    }

    pub fn store(&self) -> &dyn Store {
        match &self.backing {
            Backing::Sqlite { store, .. } => store.as_ref(),
            Backing::Postgres(f) => f.store.as_ref(),
        }
    }

    /// An owned handle to the same store, for code under test that keeps an
    /// `Arc<dyn Store>` (core services).
    pub fn shared_store(&self) -> Arc<dyn Store> {
        match &self.backing {
            Backing::Sqlite { store, .. } => store.clone(),
            Backing::Postgres(f) => f.store.clone(),
        }
    }

    /// A second store on the same database, as another node would open it. Postgres: a
    /// new `PgStore` (own pool, `max_connections` 4) on the same schema, with
    /// `application_name` `<schema>-peer<n>` (n from 1), closed by teardown. SQLite:
    /// `None` (one process owns the data folder; share the store).
    pub async fn peer(&self) -> Option<Arc<dyn Store>> {
        let Backing::Postgres(f) = &self.backing else {
            return None;
        };
        let n = f.peer_count.fetch_add(1, Ordering::SeqCst) + 1;
        let peer = Arc::new(
            PgStore::open(pg_options(
                &f.url,
                &f.schema,
                format!("{}-peer{n}", f.schema),
            ))
            .await
            .expect("open peer store"),
        );
        f.peers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(peer.clone());
        Some(peer)
    }

    /// Terminates every server connection whose `application_name` starts with
    /// `application_name_prefix` (as a crash or a network cut would), returning how many
    /// were terminated. The fixture store's name is the schema (see [`Self::pg_location`]),
    /// its subscriptions use `<schema>:listen`, peers `<schema>-peer<n>`. SQLite: 0.
    pub async fn terminate_backends(&self, application_name_prefix: &str) -> u64 {
        let Backing::Postgres(f) = &self.backing else {
            return 0;
        };
        let mut c = PgConnection::connect(&f.url).await.expect("connect");
        let killed: Vec<bool> = sqlx::query_scalar(
            "SELECT pg_terminate_backend(pid) FROM pg_stat_activity \
             WHERE starts_with(application_name, $1) AND pid <> pg_backend_pid()",
        )
        .bind(application_name_prefix)
        .fetch_all(&mut c)
        .await
        .expect("terminate backends");
        c.close().await.expect("close admin connection");
        killed.into_iter().filter(|k| *k).count() as u64
    }

    /// How many server connections have an `application_name` starting with
    /// `application_name_prefix` (see [`Self::terminate_backends`]). SQLite: 0.
    pub async fn active_backends(&self, application_name_prefix: &str) -> u64 {
        let Backing::Postgres(f) = &self.backing else {
            return 0;
        };
        let mut c = PgConnection::connect(&f.url).await.expect("connect");
        let n: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM pg_stat_activity \
             WHERE starts_with(application_name, $1) AND pid <> pg_backend_pid()",
        )
        .bind(application_name_prefix)
        .fetch_one(&mut c)
        .await
        .expect("count backends");
        c.close().await.expect("close admin connection");
        u64::try_from(n).expect("non-negative count")
    }

    /// `(url, schema)` of the Postgres backing; `None` on SQLite.
    pub fn pg_location(&self) -> Option<(String, String)> {
        match &self.backing {
            Backing::Postgres(f) => Some((f.url.clone(), f.schema.clone())),
            Backing::Sqlite { .. } => None,
        }
    }

    /// Runs raw SQL on a dedicated connection (Postgres: with the test schema as
    /// `search_path`), outside the store under test.
    pub async fn exec_raw(&self, sql: &str) {
        match &self.backing {
            Backing::Sqlite { path, .. } => {
                let mut c = sqlite_conn(path).await;
                sqlx::raw_sql(AssertSqlSafe(sql.to_owned()))
                    .execute(&mut c)
                    .await
                    .expect("exec raw sqlite");
                c.close().await.expect("close raw sqlite connection");
            }
            Backing::Postgres(f) => {
                let mut c = PgConnection::connect(&f.url).await.expect("connect");
                sqlx::raw_sql(AssertSqlSafe(format!(
                    "SET search_path TO {}; {sql}",
                    quote_ident(&f.schema)
                )))
                .execute(&mut c)
                .await
                .expect("exec raw postgres");
                c.close().await.expect("close raw postgres connection");
            }
        }
    }

    /// Makes a job due: sets its `run_at` one second into the past (database clock).
    pub async fn force_job_due(&self, id: JobId) {
        let done = match &self.backing {
            Backing::Sqlite { path, .. } => {
                let mut c = sqlite_conn(path).await;
                let past = ferrite_domain::Timestamp::now().as_micros() - 1_000_000;
                sqlx::query("UPDATE jobs SET run_at = ? WHERE id = ?")
                    .bind(past)
                    .bind(id.as_uuid())
                    .execute(&mut c)
                    .await
                    .map(|r| r.rows_affected())
            }
            Backing::Postgres(f) => {
                let mut c = PgConnection::connect(&f.url).await.expect("connect");
                sqlx::query(AssertSqlSafe(format!(
                    "UPDATE {}.jobs SET run_at = now() - interval '1 second' WHERE id = $1",
                    quote_ident(&f.schema)
                )))
                .bind(id.as_uuid())
                .execute(&mut c)
                .await
                .map(|r| r.rows_affected())
            }
        };
        assert_eq!(done.expect("force job due"), 1);
    }

    /// Names of user tables and indexes, excluding sqlx's `_sqlx_migrations`.
    pub async fn user_objects(&self) -> Vec<String> {
        match &self.backing {
            Backing::Sqlite { path, .. } => {
                let mut c = sqlite_conn(path).await;
                sqlx::query_scalar(
                    "SELECT type || ':' || name FROM sqlite_schema \
                     WHERE name NOT LIKE 'sqlite_%' AND name <> '_sqlx_migrations' ORDER BY name",
                )
                .fetch_all(&mut c)
                .await
                .expect("list sqlite objects")
            }
            Backing::Postgres(f) => {
                let mut c = PgConnection::connect(&f.url).await.expect("connect");
                sqlx::query_scalar(
                    "SELECT c.relkind::text || ':' || c.relname::text FROM pg_class c \
                     JOIN pg_namespace n ON n.oid = c.relnamespace \
                     WHERE n.nspname = $1 AND c.relname NOT LIKE '_sqlx_migrations%' \
                     ORDER BY c.relname",
                )
                .bind(&f.schema)
                .fetch_all(&mut c)
                .await
                .expect("list postgres objects")
            }
        }
    }

    /// Records `version` in `_sqlx_migrations` as if a newer binary had applied it.
    pub async fn insert_unknown_applied(&self, version: i64) {
        let sql = "INSERT INTO {t}_sqlx_migrations \
                   (version, description, success, checksum, execution_time) \
                   VALUES ({p}, 'from a newer binary', TRUE, {c}, 0)";
        match &self.backing {
            Backing::Sqlite { path, .. } => {
                let mut c = sqlite_conn(path).await;
                let sql = sql
                    .replace("{t}", "")
                    .replace("{p}", "?")
                    .replace("{c}", "x'00'");
                sqlx::query(AssertSqlSafe(sql))
                    .bind(version)
                    .execute(&mut c)
                    .await
                    .expect("insert unknown migration");
            }
            Backing::Postgres(f) => {
                let mut c = PgConnection::connect(&f.url).await.expect("connect");
                let sql = sql
                    .replace("{t}", &format!("{}.", quote_ident(&f.schema)))
                    .replace("{p}", "$1")
                    .replace("{c}", "'\\x00'::bytea");
                sqlx::query(AssertSqlSafe(sql))
                    .bind(version)
                    .execute(&mut c)
                    .await
                    .expect("insert unknown migration");
            }
        }
    }

    /// The query plan of `MediaRepo::find_by_artifact` for `hash`, one line per plan
    /// node. Postgres plans with `enable_seqscan = off`, which shows whether indexes
    /// can serve the query (on the tiny test tables the planner would otherwise pick
    /// sequential scans on cost alone).
    pub async fn explain_media_lookup(&self, hash: &str) -> String {
        match &self.backing {
            Backing::Sqlite { path, .. } => {
                let mut c = sqlite_conn(path).await;
                let rows: Vec<(i64, i64, i64, String)> = sqlx::query_as(AssertSqlSafe(format!(
                    "EXPLAIN QUERY PLAN {}",
                    ferrite_store_sqlite::FIND_MEDIA_BY_ARTIFACT_SQL
                )))
                .bind(hash)
                .bind(10_i64)
                .fetch_all(&mut c)
                .await
                .expect("explain sqlite");
                rows.into_iter().map(|r| r.3).collect::<Vec<_>>().join("\n")
            }
            Backing::Postgres(f) => {
                let mut c = PgConnection::connect(&f.url).await.expect("connect");
                sqlx::query(AssertSqlSafe(format!(
                    "SET search_path TO {}",
                    quote_ident(&f.schema)
                )))
                .execute(&mut c)
                .await
                .expect("search_path");
                sqlx::query("SET enable_seqscan = off")
                    .execute(&mut c)
                    .await
                    .expect("enable_seqscan");
                let rows: Vec<String> = sqlx::query_scalar(AssertSqlSafe(format!(
                    "EXPLAIN {}",
                    ferrite_store_postgres::FIND_MEDIA_BY_ARTIFACT_SQL
                )))
                .bind(hash)
                .bind(10_i64)
                .fetch_all(&mut c)
                .await
                .expect("explain postgres");
                rows.join("\n")
            }
        }
    }

    /// The query plan of `CommentRepo::list` with the given filters, one line per plan
    /// node (Postgres with `enable_seqscan = off`, as for
    /// [`explain_media_lookup`](Self::explain_media_lookup)).
    pub async fn explain_comment_list(&self, status: bool, item: bool, cursor: bool) -> String {
        let mut binds = Vec::new();
        if status {
            binds.push(PlanBind::Text("pending"));
        }
        if item {
            binds.push(PlanBind::Uuid(uuid::Uuid::nil()));
        }
        if cursor {
            binds.extend([
                PlanBind::Micros(1_000_000),
                PlanBind::Uuid(uuid::Uuid::nil()),
            ]);
        }
        binds.push(PlanBind::Int(51));
        let sql = match &self.backing {
            Backing::Sqlite { .. } => ferrite_store_sqlite::comment_list_sql(status, item, cursor),
            Backing::Postgres(_) => ferrite_store_postgres::comment_list_sql(status, item, cursor),
        };
        self.explain(&sql, &binds).await
    }

    /// The query plan of `IdempotencyRepo::prune` (as
    /// [`explain_comment_list`](Self::explain_comment_list)).
    pub async fn explain_idempotency_prune(&self) -> String {
        let sql = match &self.backing {
            Backing::Sqlite { .. } => ferrite_store_sqlite::PRUNE_IDEMPOTENCY_SQL,
            Backing::Postgres(_) => ferrite_store_postgres::PRUNE_IDEMPOTENCY_SQL,
        };
        self.explain(sql, &[PlanBind::Micros(1_000_000), PlanBind::Int(500)])
            .await
    }

    async fn explain(&self, sql: &str, binds: &[PlanBind]) -> String {
        match &self.backing {
            Backing::Sqlite { path, .. } => {
                let mut c = sqlite_conn(path).await;
                let mut q = sqlx::query_as::<_, (i64, i64, i64, String)>(AssertSqlSafe(format!(
                    "EXPLAIN QUERY PLAN {sql}"
                )));
                for b in binds {
                    q = match *b {
                        PlanBind::Text(t) => q.bind(t),
                        PlanBind::Uuid(u) => q.bind(u),
                        PlanBind::Micros(n) | PlanBind::Int(n) => q.bind(n),
                    };
                }
                let rows = q.fetch_all(&mut c).await.expect("explain sqlite");
                rows.into_iter().map(|r| r.3).collect::<Vec<_>>().join("\n")
            }
            Backing::Postgres(f) => {
                let mut c = PgConnection::connect(&f.url).await.expect("connect");
                sqlx::query(AssertSqlSafe(format!(
                    "SET search_path TO {}",
                    quote_ident(&f.schema)
                )))
                .execute(&mut c)
                .await
                .expect("search_path");
                sqlx::query("SET enable_seqscan = off")
                    .execute(&mut c)
                    .await
                    .expect("enable_seqscan");
                let mut q =
                    sqlx::query_scalar::<_, String>(AssertSqlSafe(format!("EXPLAIN {sql}")));
                for b in binds {
                    q = match *b {
                        PlanBind::Text(t) => q.bind(t),
                        PlanBind::Uuid(u) => q.bind(u),
                        PlanBind::Micros(n) => q.bind(
                            chrono::DateTime::<chrono::Utc>::from_timestamp_micros(n)
                                .expect("micros in range"),
                        ),
                        PlanBind::Int(n) => q.bind(n),
                    };
                }
                q.fetch_all(&mut c)
                    .await
                    .expect("explain postgres")
                    .join("\n")
            }
        }
    }

    /// Whether this is the SQLite backing.
    pub fn is_sqlite(&self) -> bool {
        matches!(self.backing, Backing::Sqlite { .. })
    }

    /// Overwrites the recorded checksum of an applied migration.
    pub async fn tamper_checksum(&self, version: i64) {
        let done = match &self.backing {
            Backing::Sqlite { path, .. } => {
                let mut c = sqlite_conn(path).await;
                sqlx::query("UPDATE _sqlx_migrations SET checksum = x'00' WHERE version = ?")
                    .bind(version)
                    .execute(&mut c)
                    .await
                    .map(|r| r.rows_affected())
            }
            Backing::Postgres(f) => {
                let mut c = PgConnection::connect(&f.url).await.expect("connect");
                sqlx::query(AssertSqlSafe(format!(
                    "UPDATE {}._sqlx_migrations SET checksum = '\\x00'::bytea WHERE version = $1",
                    quote_ident(&f.schema)
                )))
                .bind(version)
                .execute(&mut c)
                .await
                .map(|r| r.rows_affected())
            }
        };
        assert_eq!(done.expect("tamper checksum"), 1);
    }
}

async fn sqlite_conn(path: &Path) -> SqliteConnection {
    let opts = sqlx::sqlite::SqliteConnectOptions::new().filename(path);
    SqliteConnection::connect_with(&opts)
        .await
        .expect("open raw sqlite connection")
}

/// Both backends' migration directories (SQLite first).
pub fn migration_dirs() -> [PathBuf; 2] {
    let crates = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    [
        crates.join("ferrite-store-sqlite/migrations"),
        crates.join("ferrite-store-postgres/migrations"),
    ]
}

/// File names in a migration directory, sorted.
pub fn migration_files(dir: &Path) -> BTreeSet<String> {
    std::fs::read_dir(dir)
        .expect("read migration dir")
        .map(|e| {
            e.expect("dir entry")
                .file_name()
                .into_string()
                .expect("utf-8 name")
        })
        .collect()
}

/// Emits one `#[tokio::test]` per backend for each contract test `fn(&TestDb)` in scope.
#[macro_export]
macro_rules! backend_tests {
    ($($name:ident),* $(,)?) => {$(
        mod $name {
            #[tokio::test]
            async fn sqlite() {
                let db = $crate::TestDb::sqlite().await;
                super::$name(&db).await;
                db.teardown().await;
            }

            #[tokio::test]
            async fn postgres() {
                let Some(db) = $crate::TestDb::postgres().await else {
                    return;
                };
                super::$name(&db).await;
                db.teardown().await;
            }
        }
    )*};
}

/// Like [`backend_tests!`] for tests that spawn tasks: they run on a 4-thread runtime and
/// take an `Arc<TestDb>` so the database can move into `tokio::spawn`.
#[macro_export]
macro_rules! backend_tests_spawning {
    ($($name:ident),* $(,)?) => {$(
        mod $name {
            #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
            async fn sqlite() {
                let db = std::sync::Arc::new($crate::TestDb::sqlite().await);
                super::$name(db.clone()).await;
                db.teardown().await;
            }

            #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
            async fn postgres() {
                let Some(db) = $crate::TestDb::postgres().await else {
                    return;
                };
                let db = std::sync::Arc::new(db);
                super::$name(db.clone()).await;
                db.teardown().await;
            }
        }
    )*};
}

/// A bind value for an `EXPLAIN` of a store statement.
enum PlanBind {
    Text(&'static str),
    Uuid(uuid::Uuid),
    /// A timestamp: unix micros on SQLite, `timestamptz` on Postgres.
    Micros(i64),
    Int(i64),
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;
    use crate::fixtures::{fixture_user, new_post};

    /// The teardown stall: a test ending right after dropping an uncommitted `Tx` that holds
    /// locks. Teardown must be quick and must remove the schema.
    #[tokio::test]
    async fn teardown_after_dropped_tx_is_quick_and_drops_schema() {
        let Some(db) = TestDb::postgres().await else {
            return;
        };
        let author = fixture_user(db.store()).await;
        let tx = db.store().begin().await.unwrap();
        tx.content()
            .insert(&new_post(&author, "pending"))
            .await
            .unwrap();
        drop(tx);
        let Backing::Postgres(f) = &db.backing else {
            unreachable!("postgres fixture");
        };
        let (url, schema) = (f.url.clone(), f.schema.clone());
        let started = Instant::now();
        db.teardown().await;
        let took = started.elapsed();
        assert!(took < Duration::from_secs(2), "teardown took {took:?}");
        let mut c = PgConnection::connect(&url).await.unwrap();
        let exists: bool =
            sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM pg_namespace WHERE nspname = $1)")
                .bind(&schema)
                .fetch_one(&mut c)
                .await
                .unwrap();
        assert!(!exists, "schema {schema} still exists");
    }
}
