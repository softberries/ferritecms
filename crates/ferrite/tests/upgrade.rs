#![allow(clippy::unwrap_used, clippy::expect_used)]

//! `<data>/upgrade.json` and `schema_meta.last_binary_version` (M4-2 P30): written by the
//! startup check (`serve` and the other commands that migrate on start) and by
//! `ferrite migrate up` when the binary or the schema changed.
//!
//! Schema numbers are derived from the latest embedded version (`latest`), so the tests
//! need no edit when a migration is added.

use std::{
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use assert_cmd::{Command, cargo::cargo_bin_cmd};
use ferrite::{
    binary_version, startup_check,
    upgrade::{self, KeepSnapshot, Migrate, UPGRADE_FILE, UpgradeRecord},
};
use ferrite_config::DatabaseConfig;
use ferrite_domain::{Secret, Timestamp};
use ferrite_store::{SCHEMA_META_LAST_BINARY, Store};
use ferrite_store_sqlite::{SqliteOptions, SqliteStore, latest_schema_version};
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

/// The newest schema version this binary embeds.
async fn latest(store: &dyn Store) -> i64 {
    store
        .migration_status(&binary_version().unwrap())
        .await
        .unwrap()
        .binary_version
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

fn snapshot_name(rec: &UpgradeRecord) -> String {
    rec.snapshot
        .as_ref()
        .expect("snapshot")
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned()
}

/// A start with another binary than the last one writes the record; nothing migrated,
/// so both schema versions are the current one and there is no snapshot.
pub async fn upgrade_json_written_on_version_change(db: &TestDb) {
    let data = tempfile::tempdir().unwrap();
    let store = db.store();
    let bin = binary_version().unwrap();
    let l = latest(store).await;
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
    assert_eq!((rec.schema_from, rec.schema_to), (l, l));
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
    let l = latest(store).await;
    store.migrate_down(l - 2, true).await.unwrap();

    startup_check(store, &bin, &dbconf(), data.path())
        .await
        .unwrap()
        .expect("a record");
    let rec = upgrade::load(data.path()).unwrap().expect("upgrade.json");
    assert_eq!(rec.from, None, "no last_binary_version stored yet");
    assert_eq!(rec.to, bin.to_string());
    assert_eq!((rec.schema_from, rec.schema_to), (l - 2, l));
    // TestDb's SQLite file is outside `data`, so its snapshot path stays absolute there;
    // the relative form is checked in `sqlite_snapshot_is_relative_to_the_data_folder`.
    assert_eq!(rec.snapshot.is_some(), db.is_sqlite());
    if let Some(snap) = &rec.snapshot {
        assert!(data.path().join(snap).is_file(), "{}", snap.display());
    }
    assert_eq!(last_binary(store).await, Some(bin.to_string()));

    // Same binary, migrations again: the record is replaced.
    store.migrate_down(l - 1, true).await.unwrap();
    startup_check(store, &bin, &dbconf(), data.path())
        .await
        .unwrap()
        .expect("a record");
    let rec = upgrade::load(data.path()).unwrap().expect("upgrade.json");
    assert_eq!(rec.from.as_deref(), Some(bin.to_string().as_str()));
    assert_eq!((rec.schema_from, rec.schema_to), (l - 1, l));
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

/// A→B with a migration, a binary rollback to A, then B again: the record of the A→B move
/// (with its snapshot) survives both starts, so a schema rollback still finds it.
pub async fn binary_rollback_round_trip_keeps_the_record(db: &TestDb) {
    let data = tempfile::tempdir().unwrap();
    let store = db.store();
    let b = binary_version().unwrap();
    let a = semver::Version::new(0, 0, 1);
    let l = latest(store).await;
    store.migrate_down(l - 1, true).await.unwrap();
    set_last_binary(store, &a.to_string()).await;

    let moved = startup_check(store, &b, &dbconf(), data.path())
        .await
        .unwrap()
        .expect("A→B recorded");
    assert_eq!(moved.from.as_deref(), Some("0.0.1"));
    assert_eq!((moved.schema_from, moved.schema_to), (l - 1, l));
    assert_eq!(moved.snapshot.is_some(), db.is_sqlite());

    // A (older) starts on the newer schema: the file is kept, only the key changes.
    assert!(
        startup_check(store, &a, &dbconf(), data.path())
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(upgrade::load(data.path()).unwrap(), Some(moved.clone()));
    assert_eq!(last_binary(store).await, Some(a.to_string()));

    // B again: the record already names A→B and is kept.
    assert!(
        startup_check(store, &b, &dbconf(), data.path())
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(upgrade::load(data.path()).unwrap(), Some(moved));
    assert_eq!(last_binary(store).await, Some(b.to_string()));
}

backend_tests!(
    upgrade_json_written_on_version_change,
    upgrade_json_written_on_migration,
    no_upgrade_json_when_nothing_changed,
    binary_rollback_round_trip_keeps_the_record,
);

#[tokio::test]
async fn sqlite_snapshot_is_relative_to_the_data_folder() {
    let data = tempfile::tempdir().unwrap();
    let store = sqlite_in(data.path()).await;
    let bin = binary_version().unwrap();
    let l = latest(&store).await;
    store.migrate_up(Some(l - 2)).await.unwrap();
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
    assert_eq!(json["schema_from"], l - 2);
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
    let l = latest(&store).await;
    set_last_binary(&store, "0.0.1").await;
    let earlier = UpgradeRecord {
        v: 1,
        from: Some("0.0.1".into()),
        to: bin.to_string(),
        schema_from: l - 2,
        schema_to: l,
        snapshot: Some(PathBuf::from(format!(
            "backups/pre-{}-to-{l}-20261030T000000Z.db",
            l - 2
        ))),
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

/// The crash window: a start dies after `migrate_up`, before the record is finalised.
/// The provisional record written before migrating names the move; the retry (nothing
/// left to apply) finalises it with the snapshot, which pruning then protects.
#[tokio::test]
async fn crash_after_migrate_keeps_the_move_and_its_snapshot() {
    let data = tempfile::tempdir().unwrap();
    let store = sqlite_in(data.path()).await;
    let bin = binary_version().unwrap();
    store.migrate_up(None).await.unwrap();
    let l = latest(&store).await;
    store.migrate_down(l - 1, true).await.unwrap();
    set_last_binary(&store, "0.0.1").await;

    let e = upgrade::seams::fail_after_migrate(startup_check(&store, &bin, &dbconf(), data.path()))
        .await
        .unwrap_err();
    assert!(format!("{e:#}").contains("test seam"), "{e:#}");
    // Migrated, but only the provisional record and the old key.
    assert_eq!(latest(&store).await, l);
    let provisional = upgrade::load(data.path()).unwrap().expect("provisional");
    assert_eq!(provisional.from.as_deref(), Some("0.0.1"));
    assert_eq!((provisional.schema_from, provisional.schema_to), (l - 1, l));
    assert_eq!(provisional.snapshot, None);
    assert_eq!(last_binary(&store).await.as_deref(), Some("0.0.1"));

    let rec = startup_check(&store, &bin, &dbconf(), data.path())
        .await
        .unwrap()
        .expect("finalised");
    assert_eq!(rec.from.as_deref(), Some("0.0.1"));
    assert_eq!((rec.schema_from, rec.schema_to), (l - 1, l));
    assert!(
        snapshot_name(&rec).starts_with(&format!("pre-{}-to-{l}-", l - 1)),
        "{rec:?}"
    );
    let snap = data.path().join(rec.snapshot.as_ref().unwrap());
    assert!(snap.is_file());
    assert_eq!(
        upgrade::keep_snapshot(data.path()),
        Some(KeepSnapshot::Keep(snap))
    );
    assert_eq!(last_binary(&store).await, Some(bin.to_string()));
}

/// An earlier attempt that stopped partway (the schema between its start and target):
/// the next start carries the attempt's start and snapshot forward.
#[tokio::test]
async fn incomplete_attempt_keeps_its_start() {
    let data = tempfile::tempdir().unwrap();
    let store = sqlite_in(data.path()).await;
    let bin = binary_version().unwrap();
    store.migrate_up(None).await.unwrap();
    let l = latest(&store).await;
    store.migrate_down(l - 1, true).await.unwrap();
    set_last_binary(&store, "0.0.1").await;
    let first = PathBuf::from(format!("backups/pre-{}-to-{l}-20261030T000000Z.db", l - 2));
    upgrade::write(
        data.path(),
        &UpgradeRecord {
            v: 1,
            from: Some("0.0.1".into()),
            to: bin.to_string(),
            schema_from: l - 2,
            schema_to: l,
            snapshot: Some(first.clone()),
            at: Timestamp::now(),
        },
    )
    .unwrap();

    let rec = startup_check(&store, &bin, &dbconf(), data.path())
        .await
        .unwrap()
        .expect("a record");
    assert_eq!((rec.schema_from, rec.schema_to), (l - 2, l));
    assert_eq!(rec.snapshot, Some(first));
}

/// A `migrate_up` that fails without moving the schema changes nothing: the record that
/// was there is put back, and with none there stays none.
#[cfg(unix)]
#[tokio::test]
async fn failed_migration_restores_the_previous_record() {
    use std::os::unix::fs::PermissionsExt;

    let data = tempfile::tempdir().unwrap();
    let store = sqlite_in(data.path()).await;
    let bin = binary_version().unwrap();
    store.migrate_up(None).await.unwrap();
    let l = latest(&store).await;
    store.migrate_down(l - 1, true).await.unwrap();
    set_last_binary(&store, "0.0.1").await;
    // The pre-migration snapshot cannot be written: migrate_up fails before applying.
    let backups = data.path().join("backups");
    let perms = |mode| std::fs::Permissions::from_mode(mode);
    std::fs::set_permissions(&backups, perms(0o500)).unwrap();

    // No record before: none after.
    let e = startup_check(&store, &bin, &dbconf(), data.path())
        .await
        .unwrap_err();
    assert!(format!("{e:#}").contains("backups/pre-"), "{e:#}");
    assert!(!data.path().join(UPGRADE_FILE).exists());

    // A record before: the same record after.
    let previous = UpgradeRecord {
        v: 1,
        from: Some("0.0.0".into()),
        to: "0.0.1".into(),
        schema_from: l - 2,
        schema_to: l - 1,
        snapshot: Some(PathBuf::from(format!(
            "backups/pre-{}-to-{}-20261030T000000Z.db",
            l - 2,
            l - 1
        ))),
        at: Timestamp::now(),
    };
    upgrade::write(data.path(), &previous).unwrap();
    startup_check(&store, &bin, &dbconf(), data.path())
        .await
        .unwrap_err();
    assert_eq!(upgrade::load(data.path()).unwrap(), Some(previous));
    assert_eq!(latest_applied(&store).await, l - 1);
    assert_eq!(last_binary(&store).await.as_deref(), Some("0.0.1"));
    std::fs::set_permissions(&backups, perms(0o700)).unwrap();
}

async fn latest_applied(store: &dyn Store) -> i64 {
    store
        .migration_status(&binary_version().unwrap())
        .await
        .unwrap()
        .db_version
}

/// A→B dies after migrating (the key still says A), then C starts and migrates further:
/// C's record carries the unfinished A→B move's start and snapshot.
#[tokio::test]
async fn unfinished_earlier_move_is_carried_into_the_next() {
    let data = tempfile::tempdir().unwrap();
    let store = sqlite_in(data.path()).await;
    store.migrate_up(None).await.unwrap();
    let l = latest(&store).await;
    store.migrate_down(l - 2, true).await.unwrap();
    set_last_binary(&store, "0.0.1").await;
    let b = semver::Version::new(0, 0, 2);
    let c = semver::Version::new(0, 0, 3);

    upgrade::seams::fail_after_migrate(upgrade::migrate_and_record(
        &store,
        data.path(),
        &b,
        Migrate::UpTo(Some(l - 1)),
    ))
    .await
    .unwrap_err();
    assert_eq!(latest_applied(&store).await, l - 1);
    assert_eq!(last_binary(&store).await.as_deref(), Some("0.0.1"));

    let rec = startup_check(&store, &c, &dbconf(), data.path())
        .await
        .unwrap()
        .expect("a record");
    assert_eq!(rec.from.as_deref(), Some("0.0.1"));
    assert_eq!(rec.to, "0.0.3");
    assert_eq!((rec.schema_from, rec.schema_to), (l - 2, l));
    assert!(
        snapshot_name(&rec).starts_with(&format!("pre-{}-to-{}-", l - 2, l - 1)),
        "{rec:?}"
    );
    assert!(data.path().join(rec.snapshot.unwrap()).is_file());
}

/// A→B finished (target reached, snapshot named), a binary rollback to A, then A→C: C's
/// record is its own move, with its own snapshot, not A→B's older start (restoring that
/// snapshot would lose more than needed).
#[tokio::test]
async fn finished_move_kept_by_a_rollback_is_not_carried() {
    let data = tempfile::tempdir().unwrap();
    let store = sqlite_in(data.path()).await;
    store.migrate_up(None).await.unwrap();
    let l = latest(&store).await;
    store.migrate_down(l - 2, true).await.unwrap();
    let a = semver::Version::new(0, 0, 1);
    let b = semver::Version::new(0, 0, 2);
    let c = semver::Version::new(0, 0, 3);
    set_last_binary(&store, &a.to_string()).await;

    // A→B to l-1, finished.
    let (_, ab) = upgrade::migrate_and_record(&store, data.path(), &b, Migrate::UpTo(Some(l - 1)))
        .await
        .unwrap();
    let ab = ab.expect("A→B recorded");
    assert_eq!((ab.schema_from, ab.schema_to), (l - 2, l - 1));
    assert!(ab.snapshot.is_some());
    // Binary rollback: A starts on l-1, which is its own or a newer schema, so it migrates
    // nothing (this test binary embeds l, so the start is driven without migrating); the
    // file is kept, the key says A.
    let (_, kept) = upgrade::migrate_and_record(&store, data.path(), &a, Migrate::No)
        .await
        .unwrap();
    assert!(kept.is_none());
    assert_eq!(upgrade::load(data.path()).unwrap(), Some(ab));
    assert_eq!(last_binary(&store).await.as_deref(), Some("0.0.1"));

    // A→C to l.
    let rec = startup_check(&store, &c, &dbconf(), data.path())
        .await
        .unwrap()
        .expect("A→C recorded");
    assert_eq!(rec.from.as_deref(), Some("0.0.1"));
    assert_eq!(rec.to, "0.0.3");
    assert_eq!((rec.schema_from, rec.schema_to), (l - 1, l));
    assert!(
        snapshot_name(&rec).starts_with(&format!("pre-{}-to-{l}-", l - 1)),
        "{rec:?}"
    );
}

fn ferrite() -> Command {
    let mut cmd = cargo_bin_cmd!("ferrite");
    cmd.env_clear();
    cmd
}

fn init(data: &Path) {
    ferrite()
        .args(["init", "--url", "https://example.com", "--data"])
        .arg(data)
        .assert()
        .success();
}

fn migrate_down(data: &Path, to: i64) {
    ferrite()
        .args(["migrate", "down", "--to"])
        .arg(to.to_string())
        .args(["--allow-lossy", "--yes", "--data"])
        .arg(data)
        .assert()
        .success();
}

fn migrate_up(data: &Path) {
    ferrite()
        .args(["migrate", "up", "--data"])
        .arg(data)
        .assert()
        .success();
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

/// Six old `pre-*.db`, the oldest first (`pre-0-to-1-…` … `pre-5-to-6-…`).
fn plant_old_snapshots(backups: &Path) {
    let base = SystemTime::now() - Duration::from_secs(30 * 24 * 3600);
    for i in 0..6u64 {
        let p = backups.join(format!("pre-{i}-to-{}-2020010{}T000000Z.db", i + 1, i + 1));
        let f = std::fs::File::create(&p).unwrap();
        f.set_modified(base + Duration::from_secs(i * 60)).unwrap();
    }
}

/// `init` writes no record; `migrate up` with nothing to do writes none either; a
/// `migrate up` that applies migrations does, naming its snapshot.
#[test]
fn migrate_up_writes_upgrade_json() {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("site");
    let l = latest_schema_version();
    init(&data);
    assert!(!data.join(UPGRADE_FILE).exists(), "init writes no record");
    migrate_up(&data);
    assert!(!data.join(UPGRADE_FILE).exists(), "nothing changed");

    migrate_down(&data, l - 2);
    migrate_up(&data);
    let rec = upgrade::load(&data).unwrap().expect("upgrade.json");
    let bin = binary_version().unwrap().to_string();
    assert_eq!(rec.from.as_deref(), Some(bin.as_str()), "init stored it");
    assert_eq!((rec.schema_from, rec.schema_to), (l - 2, l));
    let snap = rec.snapshot.expect("snapshot");
    assert!(data.join(&snap).is_file(), "{}", snap.display());
}

/// Pruning keeps the newest five `pre-*.db` plus the one `upgrade.json` names, however old.
#[test]
fn prune_keeps_snapshot_named_in_upgrade_json() {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("site");
    let l = latest_schema_version();
    init(&data);
    let backups = data.join("backups");
    // init wrote one more, newer than all of them.
    plant_old_snapshots(&backups);
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

    migrate_down(&data, l - 1);
    migrate_up(&data);

    let names = pre_snapshots(&backups);
    assert_eq!(names.len(), 6, "five newest plus the named one: {names:?}");
    assert!(names.iter().any(|n| n == named), "{names:?}");
    assert!(
        !names.iter().any(|n| n.starts_with("pre-1-to-2-")),
        "{names:?}"
    );
    // The new record names the new snapshot.
    let rec = upgrade::load(&data).unwrap().expect("upgrade.json");
    assert!(
        snapshot_name(&rec).starts_with(&format!("pre-{}-to-{l}-", l - 1)),
        "{rec:?}"
    );
}

/// An unreadable `upgrade.json` does not stop the site: opening keeps every snapshot (it
/// cannot know which one the file names), and the next record moves the bad file aside.
#[test]
fn unreadable_upgrade_json_keeps_all_snapshots_and_is_moved_aside() {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("site");
    let l = latest_schema_version();
    init(&data);
    let backups = data.join("backups");
    plant_old_snapshots(&backups);
    let before = pre_snapshots(&backups).len();
    std::fs::write(data.join(UPGRADE_FILE), "{").unwrap();

    ferrite()
        .args(["migrate", "status", "--data"])
        .arg(&data)
        .assert()
        .success();
    migrate_down(&data, l - 1);
    migrate_up(&data);

    assert_eq!(pre_snapshots(&backups).len(), before + 1, "nothing pruned");
    let rec = upgrade::load(&data).unwrap().expect("a new valid record");
    assert_eq!((rec.schema_from, rec.schema_to), (l - 1, l));
    let aside: Vec<PathBuf> = std::fs::read_dir(&data)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("upgrade.json.invalid-")
        })
        .collect();
    assert_eq!(aside.len(), 1, "{aside:?}");
    assert_eq!(std::fs::read_to_string(&aside[0]).unwrap(), "{");
}
