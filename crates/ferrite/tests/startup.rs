#![allow(clippy::unwrap_used, clippy::expect_used)]

//! `startup_check` against a real SQLite store (the pure decision table is unit-tested
//! in `src/open.rs`).

use ferrite::{binary_version, startup_check};
use ferrite_config::DatabaseConfig;
use ferrite_domain::Secret;
use ferrite_store::{Compat, Store};
use ferrite_store_sqlite::{SqliteOptions, SqliteStore};

fn dbconf(auto_migrate: bool) -> DatabaseConfig {
    DatabaseConfig {
        url: Secret::new("sqlite://ferrite.db"),
        max_connections: 4,
        auto_migrate,
        allow_contract: false,
        schema: None,
    }
}

async fn store(dir: &tempfile::TempDir) -> SqliteStore {
    SqliteStore::open(SqliteOptions::new(dir.path().join("ferrite.db")))
        .await
        .expect("open")
}

#[tokio::test]
async fn auto_migrates_an_old_schema() {
    let dir = tempfile::tempdir().expect("tempdir");
    let s = store(&dir).await;
    let bin = binary_version().expect("version");
    s.migrate_up(None).await.expect("up");
    s.migrate_down(1, true).await.expect("down");
    startup_check(&s, &bin, &dbconf(true)).await.expect("check");
    let st = s.migration_status(&bin).await.expect("status");
    assert_eq!(st.verdict, Compat::UpToDate);
}

#[tokio::test]
async fn refuses_old_schema_without_auto_migrate() {
    let dir = tempfile::tempdir().expect("tempdir");
    let s = store(&dir).await;
    let bin = binary_version().expect("version");
    s.migrate_up(None).await.expect("up");
    s.migrate_down(1, true).await.expect("down");
    let e = startup_check(&s, &bin, &dbconf(false)).await.unwrap_err();
    assert!(format!("{e:#}").contains("ferrite migrate up"));
    let st = s.migration_status(&bin).await.expect("status");
    assert_eq!(st.verdict, Compat::NeedsUp);
}

#[tokio::test]
async fn up_to_date_is_ok() {
    let dir = tempfile::tempdir().expect("tempdir");
    let s = store(&dir).await;
    let bin = binary_version().expect("version");
    s.migrate_up(None).await.expect("up");
    startup_check(&s, &bin, &dbconf(false))
        .await
        .expect("check");
}
