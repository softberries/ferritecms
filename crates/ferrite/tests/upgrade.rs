#![allow(clippy::unwrap_used, clippy::expect_used)]

//! `<data>/upgrade.json` and `schema_meta.last_binary_version` (M4-2 P30): written by the
//! startup check (`serve` and the other commands that migrate on start) and by
//! `ferrite migrate up` when the binary or the schema changed.

use std::{
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use assert_cmd::{Command, cargo::cargo_bin_cmd};
use ferrite::{
    binary_version, startup_check,
    upgrade::{self, UPGRADE_FILE, UpgradeRecord},
};
use ferrite_config::DatabaseConfig;
use ferrite_domain::{Secret, Timestamp};
use ferrite_store::{SCHEMA_META_LAST_BINARY, Store};
use ferrite_store_sqlite::{SqliteOptions, SqliteStore};
use ferrite_store_tests::{TestDb, backend_tests};

fn dbconf() -> DatabaseConfig {
    DatabaseConfig {
        url: Secret::new("sqlite://ferrite.db"),
        max_connections: 4,
        auto_migrate: true,
        allow_contract: true,
        schema: None,
    }
}

/// A solo-style store whose lock and backups live in `data` (as `open_store` sets them).
async fn sqlite_in(data: &Path) -> SqliteStore {
    let mut opts = SqliteOptions::new(data.join("ferrite.db"));
    opts.lock_dir = Some(data.to_owned());
    opts.backups_dir = Some(data.join("backups"));
    SqliteStore::open(opts).await.expect("open")
}

async fn last_binary(store: &dyn Store) -> Option<String> {
    store
        .read()
        .schema_meta()
        .get(SCHEMA_META_LAST_BINARY)
        .await
        .unwrap()
}

async fn set_last_binary(store: &dyn Store, v: &str) {
    store
        .read()
        .schema_meta()
        .set(SCHEMA_META_LAST_BINARY, v)
        .await
        .unwrap();
}

fn recent(at: Timestamp) {
    let age = Timestamp::now().as_micros() - at.as_micros();
    assert!((0..60_000_000).contains(&age), "at = {}", at.to_rfc3339());
}

/// A start with another binary than the last one writes the record; nothing migrated,
/// so both schema versions are the current one and there is no snapshot.
pub async fn upgrade_json_written_on_version_change(db: &TestDb) {
    let data = tempfile::tempdir().unwrap();
    let store = db.store();
    let bin = binary_version().unwrap();
    set_last_binary(store, "0.0.1").await;

    let written = startup_check(store, &bin, &dbconf(), data.path())
        .await
        .unwrap()
        .expect("a record");
    let rec = upgrade::load(data.path()).unwrap().expect("upgrade.json");
    assert_eq!(rec, written);
    assert_eq!(rec.v, 1);
    assert_eq!(rec.from.as_deref(), Some("0.0.1"));
    assert_eq!(rec.to, bin.to_string());
    assert_eq!((rec.schema_from, rec.schema_to), (11, 11));
    assert_eq!(rec.snapshot, None);
    recent(rec.at);
    assert_eq!(last_binary(store).await, Some(bin.to_string()));

    // The next start of the same binary changes nothing.
    let raw = std::fs::read(data.path().join(UPGRADE_FILE)).unwrap();
    assert!(
        startup_check(store, &bin, &dbconf(), data.path())
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(std::fs::read(data.path().join(UPGRADE_FILE)).unwrap(), raw);
}

/// The same binary applying migrations writes the record too; a database from before
/// `last_binary_version` existed has `from: null`.
pub async fn upgrade_json_written_on_migration(db: &TestDb) {
    let data = tempfile::tempdir().unwrap();
    let store = db.store();
    let bin = binary_version().unwrap();
    store.migrate_down(9, true).await.unwrap();

    startup_check(store, &bin, &dbconf(), data.path())
        .await
        .unwrap()
        .expect("a record");
    let rec = upgrade::load(data.path()).unwrap().expect("upgrade.json");
    assert_eq!(rec.from, None, "no last_binary_version stored yet");
    assert_eq!(rec.to, bin.to_string());
    assert_eq!((rec.schema_from, rec.schema_to), (9, 11));
    // TestDb's SQLite file is outside `data`, so its snapshot path stays absolute there;
    // the relative form is checked in `sqlite_snapshot_is_relative_to_the_data_folder`.
    assert_eq!(rec.snapshot.is_some(), db.is_sqlite());
    if let Some(snap) = &rec.snapshot {
        assert!(data.path().join(snap).is_file(), "{}", snap.display());
    }
    assert_eq!(last_binary(store).await, Some(bin.to_string()));

    // Same binary, migrations again: the record is replaced.
    store.migrate_down(10, true).await.unwrap();
    startup_check(store, &bin, &dbconf(), data.path())
        .await
        .unwrap()
        .expect("a record");
    let rec = upgrade::load(data.path()).unwrap().expect("upgrade.json");
    assert_eq!(rec.from.as_deref(), Some(bin.to_string().as_str()));
    assert_eq!((rec.schema_from, rec.schema_to), (10, 11));
}

/// Same binary, nothing pending: no record, and an old one is left as it is.
pub async fn no_upgrade_json_when_nothing_changed(db: &TestDb) {
    let data = tempfile::tempdir().unwrap();
    let store = db.store();
    let bin = binary_version().unwrap();
    set_last_binary(store, &bin.to_string()).await;
    assert!(
        startup_check(store, &bin, &dbconf(), data.path())
            .await
            .unwrap()
            .is_none()
    );
    assert!(!data.path().join(UPGRADE_FILE).exists());
}

backend_tests!(
    upgrade_json_written_on_version_change,
    upgrade_json_written_on_migration,
    no_upgrade_json_when_nothing_changed,
);

#[tokio::test]
async fn sqlite_snapshot_is_relative_to_the_data_folder() {
    let data = tempfile::tempdir().unwrap();
    let store = sqlite_in(data.path()).await;
    let bin = binary_version().unwrap();
    store.migrate_up(Some(9)).await.unwrap();
    set_last_binary(&store, "0.0.1").await;

    let rec = startup_check(&store, &bin, &dbconf(), data.path())
        .await
        .unwrap()
        .expect("a record");
    let snap = rec.snapshot.expect("snapshot");
    assert!(snap.is_relative(), "{}", snap.display());
    assert!(snap.starts_with("backups"), "{}", snap.display());
    assert!(data.path().join(&snap).is_file());
    let raw = std::fs::read_to_string(data.path().join(UPGRADE_FILE)).unwrap();
    let json: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(json["v"], 1);
    assert_eq!(json["from"], "0.0.1");
    assert_eq!(json["schema_from"], 9);
    assert_eq!(json["snapshot"], snap.to_str().unwrap());
    assert!(json["at"].as_str().unwrap().ends_with('Z'), "{raw}");
}

/// A new database (schema 0 before the start) has nothing to roll back to: no record,
/// but `last_binary_version` is set so the next start does not write one either.
#[tokio::test]
async fn new_database_writes_no_upgrade_json() {
    let data = tempfile::tempdir().unwrap();
    let store = sqlite_in(data.path()).await;
    let bin = binary_version().unwrap();
    assert!(
        startup_check(&store, &bin, &dbconf(), data.path())
            .await
            .unwrap()
            .is_none()
    );
    assert!(!data.path().join(UPGRADE_FILE).exists());
    assert_eq!(last_binary(&store).await, Some(bin.to_string()));
}

/// A start that failed after writing the record but before storing the binary version
/// rewrites nothing on retry: the record already names this move.
#[tokio::test]
async fn retry_keeps_the_record_of_this_move() {
    let data = tempfile::tempdir().unwrap();
    let store = sqlite_in(data.path()).await;
    let bin = binary_version().unwrap();
    store.migrate_up(None).await.unwrap();
    set_last_binary(&store, "0.0.1").await;
    let earlier = UpgradeRecord {
        v: 1,
        from: Some("0.0.1".into()),
        to: bin.to_string(),
        schema_from: 9,
        schema_to: 11,
        snapshot: Some(PathBuf::from("backups/pre-9-to-11-20261030T000000Z.db")),
        at: Timestamp::now(),
    };
    upgrade::write(data.path(), &earlier).unwrap();
    assert!(
        startup_check(&store, &bin, &dbconf(), data.path())
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(upgrade::load(data.path()).unwrap(), Some(earlier));
    assert_eq!(last_binary(&store).await, Some(bin.to_string()));
}

fn ferrite() -> Command {
    let mut cmd = cargo_bin_cmd!("ferrite");
    cmd.env_clear();
    cmd
}

fn pre_snapshots(backups: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(backups)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|n| n.starts_with("pre-") && n.ends_with(".db"))
        .collect();
    names.sort();
    names
}

/// `init` writes no record; `migrate up` with nothing to do writes none either; a
/// `migrate up` that applies migrations does, naming its snapshot.
#[test]
fn migrate_up_writes_upgrade_json() {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("site");
    ferrite()
        .args(["init", "--url", "https://example.com", "--data"])
        .arg(&data)
        .assert()
        .success();
    assert!(!data.join(UPGRADE_FILE).exists(), "init writes no record");
    ferrite()
        .args(["migrate", "up", "--data"])
        .arg(&data)
        .assert()
        .success();
    assert!(!data.join(UPGRADE_FILE).exists(), "nothing changed");

    ferrite()
        .args([
            "migrate",
            "down",
            "--to",
            "9",
            "--allow-lossy",
            "--yes",
            "--data",
        ])
        .arg(&data)
        .assert()
        .success();
    ferrite()
        .args(["migrate", "up", "--data"])
        .arg(&data)
        .assert()
        .success();
    let rec = upgrade::load(&data).unwrap().expect("upgrade.json");
    let bin = binary_version().unwrap().to_string();
    assert_eq!(rec.from.as_deref(), Some(bin.as_str()), "init stored it");
    assert_eq!((rec.schema_from, rec.schema_to), (9, 11));
    let snap = rec.snapshot.expect("snapshot");
    assert!(data.join(&snap).is_file(), "{}", snap.display());
}

/// Pruning keeps the newest five `pre-*.db` plus the one `upgrade.json` names, however old.
#[test]
fn prune_keeps_snapshot_named_in_upgrade_json() {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("site");
    ferrite()
        .args(["init", "--url", "https://example.com", "--data"])
        .arg(&data)
        .assert()
        .success();
    let backups = data.join("backups");
    // Six old snapshots, the oldest first; init wrote one more, newer than all of them.
    let base = SystemTime::now() - Duration::from_secs(30 * 24 * 3600);
    for i in 0..6u64 {
        let p = backups.join(format!("pre-{i}-to-{}-2020010{}T000000Z.db", i + 1, i + 1));
        let f = std::fs::File::create(&p).unwrap();
        f.set_modified(base + Duration::from_secs(i * 60)).unwrap();
    }
    let named = "pre-0-to-1-20200101T000000Z.db";
    upgrade::write(
        &data,
        &UpgradeRecord {
            v: 1,
            from: Some("0.0.1".into()),
            to: binary_version().unwrap().to_string(),
            schema_from: 0,
            schema_to: 1,
            snapshot: Some(Path::new("backups").join(named)),
            at: Timestamp::now(),
        },
    )
    .unwrap();

    ferrite()
        .args([
            "migrate",
            "down",
            "--to",
            "10",
            "--allow-lossy",
            "--yes",
            "--data",
        ])
        .arg(&data)
        .assert()
        .success();
    ferrite()
        .args(["migrate", "up", "--data"])
        .arg(&data)
        .assert()
        .success();

    let names = pre_snapshots(&backups);
    assert_eq!(names.len(), 6, "five newest plus the named one: {names:?}");
    assert!(names.iter().any(|n| n == named), "{names:?}");
    assert!(
        !names.iter().any(|n| n.starts_with("pre-1-to-2-")),
        "{names:?}"
    );
    // The new record names the new snapshot.
    let rec = upgrade::load(&data).unwrap().expect("upgrade.json");
    let snap = rec.snapshot.expect("snapshot");
    assert!(
        snap.file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("pre-10-to-11-"),
        "{}",
        snap.display()
    );
}

/// An unreadable `upgrade.json` stops opening a SQLite site instead of risking the
/// pruning of the snapshot it may name.
#[test]
fn unreadable_upgrade_json_refuses_to_open() {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("site");
    ferrite()
        .args(["init", "--url", "https://example.com", "--data"])
        .arg(&data)
        .assert()
        .success();
    std::fs::write(data.join(UPGRADE_FILE), "{").unwrap();
    let out = ferrite()
        .args(["migrate", "up", "--data"])
        .arg(&data)
        .assert()
        .failure()
        .get_output()
        .stderr
        .clone();
    let err = String::from_utf8_lossy(&out);
    assert!(err.contains("not a valid upgrade record"), "{err}");
}
