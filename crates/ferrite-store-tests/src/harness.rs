//! Per-test databases: a fresh SQLite file or a fresh Postgres schema, migrated up.

use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

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

/// A Postgres store in its own schema; the schema is dropped when this is dropped.
pub struct PgFixture {
    pub store: PgStore,
    url: String,
    schema: String,
}

impl Drop for PgFixture {
    fn drop(&mut self) {
        // Best effort. The test's runtime is ending, so run the DROP on a fresh runtime in
        // its own thread with its own connection; `lock_timeout` keeps a stuck lock from
        // hanging the test.
        let url = self.url.clone();
        let sql = format!(
            "SET lock_timeout = '5s'; DROP SCHEMA IF EXISTS {} CASCADE",
            quote_ident(&self.schema)
        );
        let handle = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            rt.block_on(async move {
                let mut conn = PgConnection::connect(&url).await?;
                sqlx::raw_sql(AssertSqlSafe(sql)).execute(&mut conn).await?;
                conn.close().await
            })
            .map_err(std::io::Error::other)
        });
        if let Ok(Err(e)) = handle.join() {
            eprintln!("warning: could not drop test schema {}: {e}", self.schema);
        }
    }
}

/// A Postgres store in a fresh `t_<uuid>` schema, migrated up. Returns `None` (after a
/// `SKIP` line on stderr) when `FERRITE_TEST_PG_URL` is unset; panics if CI=true.
pub async fn pg_store() -> Option<PgFixture> {
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
    let opened = PgStore::open(PgOptions {
        url: url.clone(),
        max_connections: 4,
        schema: Some(schema.clone()),
    })
    .await;
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
    let fixture = PgFixture { store, url, schema };
    fixture
        .store
        .migrate_up(None)
        .await
        .expect("migrate postgres up");
    Some(fixture)
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
        store: SqliteStore,
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
                store,
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

    pub fn store(&self) -> &dyn Store {
        match &self.backing {
            Backing::Sqlite { store, .. } => store,
            Backing::Postgres(f) => &f.store,
        }
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
            }

            #[tokio::test]
            async fn postgres() {
                let Some(db) = $crate::TestDb::postgres().await else {
                    return;
                };
                super::$name(&db).await;
            }
        }
    )*};
}
