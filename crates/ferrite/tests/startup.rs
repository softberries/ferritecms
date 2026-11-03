#![allow(clippy::unwrap_used, clippy::expect_used)]

//! `startup_check` against real stores (the pure decision table is unit-tested in
//! `src/open.rs`).

use std::{sync::Arc, time::Duration};

use ferrite::{binary_version, startup_check};
use ferrite_config::DatabaseConfig;
use ferrite_domain::Secret;
use ferrite_store::{Compat, LOCK_MIGRATE, Store};
use ferrite_store_sqlite::{SqliteOptions, SqliteStore};
use ferrite_store_tests::{TestDb, backend_tests_spawning};
use tokio::time::Instant;

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
    startup_check(&s, &bin, &dbconf(true), dir.path(), "en")
        .await
        .expect("check");
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
    let e = startup_check(&s, &bin, &dbconf(false), dir.path(), "en")
        .await
        .unwrap_err();
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
    startup_check(&s, &bin, &dbconf(false), dir.path(), "en")
        .await
        .expect("check");
}

/// A database migrated by 0014 whose `site.locale` is unset (the backfill guessed `en`)
/// takes the config's locale at the first start, once (design 13 §2.2).
#[tokio::test]
async fn startup_settles_from_config_when_option_unset() {
    let dir = tempfile::tempdir().expect("tempdir");
    let s = store(&dir).await;
    let bin = binary_version().expect("version");
    s.migrate_up(None).await.expect("up");
    // A row as the backfill left it: the guess `en`.
    let author = ferrite_store_tests::fixtures::fixture_user(&s).await;
    let post = ferrite_store_tests::fixtures::new_post(&author, "old");
    s.read().content().insert(&post).await.expect("insert");
    startup_check(&s, &bin, &dbconf(true), dir.path(), "de")
        .await
        .expect("check");
    let locales = s
        .read()
        .options()
        .get("site.locales")
        .await
        .expect("read")
        .expect("site.locales written");
    assert_eq!(locales.value, serde_json::json!([{"tag": "de"}]));
    let item = s
        .read()
        .content()
        .get(post.id)
        .await
        .expect("get")
        .expect("item");
    assert_eq!(item.locale, "de");
    // A second start with another config changes nothing.
    startup_check(&s, &bin, &dbconf(true), dir.path(), "fr")
        .await
        .expect("check");
    let item = s
        .read()
        .content()
        .get(post.id)
        .await
        .expect("get")
        .expect("item");
    assert_eq!(item.locale, "de");
}

/// 02 §7, 04 §7: a node starting while `ferrite migrate` (or another node) holds the
/// migrate lock waits for it, then checks and migrates.
async fn startup_check_serialises_on_migrate_lock(db: Arc<TestDb>) {
    let store = db.shared_store();
    let bin = binary_version().expect("version");
    store.migrate_down(1, true).await.expect("down");
    let guard = store
        .try_lock(LOCK_MIGRATE)
        .await
        .expect("try_lock")
        .expect("lock free");

    let start = Instant::now();
    let check = {
        let store = store.clone();
        let bin = bin.clone();
        tokio::spawn(async move {
            let data = tempfile::tempdir().expect("tempdir");
            startup_check(store.as_ref(), &bin, &dbconf(true), data.path(), "en").await
        })
    };
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(!check.is_finished(), "startup_check must wait for the lock");
    let st = store.migration_status(&bin).await.expect("status");
    assert_eq!(st.verdict, Compat::NeedsUp, "nothing migrated while locked");
    guard.release().await.expect("release");

    tokio::time::timeout(Duration::from_secs(5), check)
        .await
        .expect("proceeds after release")
        .expect("join")
        .expect("check");
    assert!(start.elapsed() >= Duration::from_millis(500));
    let st = store.migration_status(&bin).await.expect("status");
    assert_eq!(st.verdict, Compat::UpToDate);
    // The check released the lock again.
    let again = store.try_lock(LOCK_MIGRATE).await.expect("try_lock");
    assert!(again.is_some(), "lock released after startup_check");
    again.expect("held").release().await.expect("release");
}

backend_tests_spawning!(startup_check_serialises_on_migrate_lock);
