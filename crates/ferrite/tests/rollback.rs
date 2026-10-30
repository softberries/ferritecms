//! `ferrite rollback` (M4-2 Task 5, P32): the binary swap with `ferrite.prev`, `--schema`
//! (down migrations to the `schema_from` of `upgrade.json`) and `--restore-snapshot`.
//!
//! No older ferrite binary can be built here, so the binaries being swapped are small shell
//! scripts in a temp folder (they answer `--version` and `migrate status` with a chosen exit
//! code), and the running binary is the test binary with the hidden `--test-current-exe`
//! seam naming the script it stands for. The `upgrade.json` records are real: written by
//! `ferrite migrate up` after a `migrate down`.
//!
//! Schema numbers are derived from the latest embedded version, so the tests need no edit
//! when a migration is added. [`LOSSY_FROM`] is below 0010 (`import_jobs`, whose down is
//! lossy), so a rollback to it always crosses a lossy step.
#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use assert_cmd::{Command, cargo::cargo_bin_cmd};
use ferrite::{
    binary_version,
    upgrade::{self, UPGRADE_FILE, UpgradeRecord},
};
use ferrite_store::{SCHEMA_META_INSTALL_ID, SCHEMA_META_LAST_BINARY, Store};
use ferrite_store_sqlite::{SqliteOptions, SqliteStore, latest_schema_version};
use sqlx::{AssertSqlSafe, Connection, PgConnection};

const PG_URL_ENV: &str = "FERRITE_TEST_PG_URL";
/// A `schema_from` whose down crosses a lossy migration (0010).
const LOSSY_FROM: i64 = 9;
/// The "previous binary" the records name.
const OLD: &str = "0.0.1";

fn ferrite() -> Command {
    let mut cmd = cargo_bin_cmd!("ferrite");
    cmd.env_clear();
    cmd
}

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

/// A stand-in binary: `--version` prints `ferrite <version>`, `migrate status` exits with
/// `status_exit`.
fn script(version: &str, status_exit: u8) -> String {
    format!(
        "#!/bin/sh\n\
         if [ \"$1\" = \"--version\" ]; then echo \"ferrite {version}\"; exit 0; fi\n\
         if [ \"$1\" = \"migrate\" ] && [ \"$2\" = \"status\" ]; then\n\
           echo \"FAKE MIGRATE STATUS {version}\"; exit {status_exit}\n\
         fi\n\
         exit 1\n"
    )
}

fn put_script(path: &Path, body: &str) {
    std::fs::write(path, body).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// `<tmp>/bin/ferrite` (the newer binary "B") and, when `prev_status` is given,
/// `<tmp>/bin/ferrite.prev` (the older "A", whose `migrate status` exits with it).
struct Bins {
    current: PathBuf,
    prev: PathBuf,
    new_body: String,
    old_body: String,
}

impl Bins {
    fn new(root: &Path, prev_status: Option<u8>) -> Self {
        let dir = root.join("bin");
        std::fs::create_dir_all(&dir).unwrap();
        let current = dir.join("ferrite");
        let prev = dir.join("ferrite.prev");
        let new_body = script("9.0.0", 0);
        let old_body = script(OLD, prev_status.unwrap_or(0));
        put_script(&current, &new_body);
        if prev_status.is_some() {
            put_script(&prev, &old_body);
        }
        Bins {
            current,
            prev,
            new_body,
            old_body,
        }
    }

    fn current_body(&self) -> String {
        std::fs::read_to_string(&self.current).unwrap()
    }

    fn prev_body(&self) -> String {
        std::fs::read_to_string(&self.prev).unwrap()
    }

    fn assert_unswapped(&self) {
        assert_eq!(self.current_body(), self.new_body, "ferrite is still B");
        assert_eq!(self.prev_body(), self.old_body, "ferrite.prev is still A");
    }

    fn assert_swapped(&self) {
        assert_eq!(self.current_body(), self.old_body, "ferrite is A");
        assert_eq!(self.prev_body(), self.new_body, "ferrite.prev is B");
    }
}

/// `ferrite rollback` as the binary at `running` (the test seam).
fn rollback(running: &Path) -> Command {
    let mut cmd = ferrite();
    cmd.args(["rollback", "--test-current-exe"]).arg(running);
    cmd.write_stdin("");
    cmd
}

fn init(data: &Path) {
    ferrite()
        .args(["init", "--url", "https://example.com", "--data"])
        .arg(data)
        .assert()
        .success();
}

fn init_pg(data: &Path, pg: &PgSchema) {
    ferrite()
        .args(["init", "--url", "https://example.com", "--db", &pg.url])
        .args(["--schema", &pg.schema, "--data"])
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

/// The schema version `migrate status` reports.
fn db_version(data: &Path) -> i64 {
    let out = ferrite()
        .args(["migrate", "status", "--data"])
        .arg(data)
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    text.lines()
        .find_map(|l| l.strip_prefix("database version: "))
        .unwrap_or_else(|| panic!("no version in {text}"))
        .trim()
        .parse()
        .unwrap()
}

/// A solo-style store on `data` (it takes the folder lock while it lives). A failed open
/// is retried a few times (with the error shown): a store this test just closed may still
/// be letting go of the file on a loaded machine.
async fn sqlite_in(data: &Path) -> SqliteStore {
    let mut opts = SqliteOptions::new(data.join("ferrite.db"));
    opts.lock_dir = Some(data.to_owned());
    opts.backups_dir = Some(data.join("backups"));
    for _ in 0..10 {
        match SqliteStore::open(opts.clone()).await {
            Ok(store) => return store,
            Err(e) => {
                eprintln!("opening {} failed, retrying: {e}", data.display());
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            }
        }
    }
    SqliteStore::open(opts).await.expect("open")
}

fn meta(data: &Path, key: &str) -> Option<String> {
    rt().block_on(async {
        let store = sqlite_in(data).await;
        let v = store.read().schema_meta().get(key).await.unwrap();
        store.close().await;
        v
    })
}

fn set_meta(data: &Path, key: &str, value: &str) {
    rt().block_on(async {
        let store = sqlite_in(data).await;
        store.read().schema_meta().set(key, value).await.unwrap();
        // Closed before returning, so no late close removes a `-wal` planted afterwards.
        store.close().await;
    });
}

/// A SQLite site upgraded from [`OLD`] at `schema_from` to this binary at the latest
/// schema: `upgrade.json` names the move and its snapshot.
fn upgraded_site(root: &Path, schema_from: i64) -> (PathBuf, UpgradeRecord) {
    let data = root.join("site");
    init(&data);
    migrate_down(&data, schema_from);
    set_meta(&data, SCHEMA_META_LAST_BINARY, OLD);
    migrate_up(&data);
    let rec = upgrade::load(&data).unwrap().expect("upgrade.json");
    assert_eq!(rec.from.as_deref(), Some(OLD));
    assert_eq!(rec.to, binary_version().unwrap().to_string());
    assert_eq!(
        (rec.schema_from, rec.schema_to),
        (schema_from, latest_schema_version())
    );
    assert!(rec.snapshot.is_some());
    (data, rec)
}

fn names(dir: &Path, prefix: &str) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.map(|e| e.unwrap().file_name().into_string().unwrap())
                .filter(|n| n.starts_with(prefix))
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

fn stderr(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn stdout(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

// ---- binary rollback ----

/// `rollback` swaps `ferrite` and `ferrite.prev`; running it again re-upgrades. With
/// `--data` the previous binary's `migrate status` runs first, and `upgrade.json` is kept
/// (a schema rollback afterwards still needs it).
#[test]
fn rollback_swaps_and_back() {
    let tmp = tempfile::tempdir().unwrap();
    let bins = Bins::new(tmp.path(), Some(0));

    let out = rollback(&bins.current).output().unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    bins.assert_swapped();
    assert!(
        stdout(&out).contains("systemctl restart ferrite"),
        "{}",
        stdout(&out)
    );
    assert_eq!(
        names(bins.current.parent().unwrap(), ""),
        ["ferrite", "ferrite.prev"]
    );

    // Again: back to B.
    let out = rollback(&bins.current).output().unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    bins.assert_unswapped();

    // With --data: the check runs, and the record stays.
    let (data, rec) = upgraded_site(tmp.path(), latest_schema_version() - 1);
    let out = rollback(&bins.current)
        .arg("--data")
        .arg(&data)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(
        stdout(&out).contains(&format!("FAKE MIGRATE STATUS {OLD}")),
        "{}",
        stdout(&out)
    );
    bins.assert_swapped();
    assert_eq!(upgrade::load(&data).unwrap(), Some(rec));
    assert!(names(&data, "upgrade-rolled-back-").is_empty());

    // Run as ferrite.prev (either binary works): back to B.
    let out = rollback(&bins.prev).output().unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    bins.assert_unswapped();
}

#[test]
fn rollback_refuses_without_prev() {
    let tmp = tempfile::tempdir().unwrap();
    let bins = Bins::new(tmp.path(), None);
    let out = rollback(&bins.current).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("no previous binary"),
        "{}",
        stderr(&out)
    );
    assert_eq!(bins.current_body(), bins.new_body);
    assert!(!bins.prev.exists());
}

/// The previous binary cannot open this schema (its `migrate status` exits 3): exit 3,
/// pointing at `--schema`, and nothing is swapped.
#[test]
fn rollback_refuses_incompatible_schema_exit_3() {
    let tmp = tempfile::tempdir().unwrap();
    let bins = Bins::new(tmp.path(), Some(3));
    let data = tmp.path().join("site");
    init(&data);
    let out = rollback(&bins.current)
        .arg("--data")
        .arg(&data)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(3), "{}", stderr(&out));
    assert!(stderr(&out).contains("--schema"), "{}", stderr(&out));
    bins.assert_unswapped();

    // Any other failure of the check refuses too (exit 1).
    let bins2 = Bins::new(&tmp.path().join("other"), Some(1));
    let out = rollback(&bins2.current)
        .arg("--data")
        .arg(&data)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    bins2.assert_unswapped();
}

// ---- --schema ----

#[test]
fn rollback_schema_downs_to_schema_from() {
    let tmp = tempfile::tempdir().unwrap();
    let bins = Bins::new(tmp.path(), Some(0));
    let from = latest_schema_version() - 2;
    let (data, _) = upgraded_site(tmp.path(), from);
    let install_id = meta(&data, SCHEMA_META_INSTALL_ID);

    let out = rollback(&bins.current)
        .args(["--schema", "--allow-lossy", "--data"])
        .arg(&data)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(db_version(&data), from);
    bins.assert_swapped();
    assert_eq!(meta(&data, SCHEMA_META_INSTALL_ID), install_id);
    assert!(
        stdout(&out).contains("systemctl restart ferrite"),
        "{}",
        stdout(&out)
    );
}

/// A → B, a binary rollback to A, then `ferrite.prev` (B, the binary `to` names) rolls the
/// schema back: the binaries are already swapped, so they stay as they are.
#[test]
fn rollback_schema_after_binary_rollback_runs_as_prev() {
    let tmp = tempfile::tempdir().unwrap();
    let bins = Bins::new(tmp.path(), Some(0));
    let from = latest_schema_version() - 2;
    let (data, _) = upgraded_site(tmp.path(), from);

    rollback(&bins.current)
        .arg("--data")
        .arg(&data)
        .assert()
        .success();
    bins.assert_swapped();

    // A refuses: it is not the binary the record names (the real A would be older; the
    // stand-in is the test binary run as `ferrite`, so the record's `to` is changed).
    let mut rec = upgrade::load(&data).unwrap().unwrap();
    let real_to = rec.to.clone();
    rec.to = "9.9.9".into();
    upgrade::write(&data, &rec).unwrap();
    let out = rollback(&bins.current)
        .args(["--schema", "--allow-lossy", "--data"])
        .arg(&data)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("ferrite.prev"), "{}", stderr(&out));
    rec.to = real_to;
    upgrade::write(&data, &rec).unwrap();

    let out = rollback(&bins.prev)
        .args(["--schema", "--allow-lossy", "--data"])
        .arg(&data)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(db_version(&data), from);
    bins.assert_swapped();
    assert!(!data.join(UPGRADE_FILE).exists());
    assert_eq!(names(&data, "upgrade-rolled-back-").len(), 1);
}

#[test]
fn rollback_schema_refuses_lossy_without_flag() {
    let tmp = tempfile::tempdir().unwrap();
    let bins = Bins::new(tmp.path(), Some(0));
    let (data, rec) = upgraded_site(tmp.path(), LOSSY_FROM);

    let out = rollback(&bins.current)
        .args(["--schema", "--data"])
        .arg(&data)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let err = stderr(&out);
    assert!(err.contains("--allow-lossy"), "{err}");
    assert!(err.contains("--restore-snapshot"), "{err}");
    assert_eq!(db_version(&data), latest_schema_version());
    bins.assert_unswapped();
    assert_eq!(upgrade::load(&data).unwrap(), Some(rec));
}

/// `--schema` runs only as the binary `upgrade.json` names as `to`, and refuses without a
/// readable record.
#[test]
fn rollback_schema_requires_newer_binary() {
    let tmp = tempfile::tempdir().unwrap();
    let bins = Bins::new(tmp.path(), Some(0));
    let (data, mut rec) = upgraded_site(tmp.path(), latest_schema_version() - 1);
    rec.to = "9.9.9".into();
    upgrade::write(&data, &rec).unwrap();

    let out = rollback(&bins.current)
        .args(["--schema", "--allow-lossy", "--data"])
        .arg(&data)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("9.9.9"), "{}", stderr(&out));
    assert_eq!(db_version(&data), latest_schema_version());
    bins.assert_unswapped();

    // An unreadable record (a newer format) refuses.
    let mut v = serde_json::to_value(&rec).unwrap();
    v["v"] = 2.into();
    std::fs::write(data.join(UPGRADE_FILE), v.to_string()).unwrap();
    let out = rollback(&bins.current)
        .args(["--schema", "--allow-lossy", "--data"])
        .arg(&data)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("format version 2"),
        "{}",
        stderr(&out)
    );

    // No record at all refuses.
    std::fs::remove_file(data.join(UPGRADE_FILE)).unwrap();
    let out = rollback(&bins.current)
        .args(["--schema", "--allow-lossy", "--data"])
        .arg(&data)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("migrate down"), "{}", stderr(&out));
    assert_eq!(db_version(&data), latest_schema_version());
    bins.assert_unswapped();
}

/// After a rollback that used `upgrade.json` it is renamed (never deleted), so the next
/// start of the older binary writes no stale move.
#[test]
fn rollback_renames_upgrade_json() {
    let tmp = tempfile::tempdir().unwrap();
    let bins = Bins::new(tmp.path(), Some(0));
    let (data, _) = upgraded_site(tmp.path(), latest_schema_version() - 1);
    let raw = std::fs::read(data.join(UPGRADE_FILE)).unwrap();

    rollback(&bins.current)
        .args(["--schema", "--allow-lossy", "--data"])
        .arg(&data)
        .assert()
        .success();
    assert!(!data.join(UPGRADE_FILE).exists());
    let renamed = names(&data, "upgrade-rolled-back-");
    assert_eq!(renamed.len(), 1, "{renamed:?}");
    assert!(renamed[0].ends_with(".json"), "{renamed:?}");
    assert_eq!(std::fs::read(data.join(&renamed[0])).unwrap(), raw);
}

// ---- --restore-snapshot ----

/// The snapshot replaces the database; the current database and its `-wal`/`-shm` move to
/// `backups/rolled-back-<ts>.db[-wal|-shm]` unchanged, and the install id stays.
#[test]
fn rollback_restore_snapshot_moves_current_db_aside() {
    let tmp = tempfile::tempdir().unwrap();
    let bins = Bins::new(tmp.path(), Some(0));
    let (data, rec) = upgraded_site(tmp.path(), LOSSY_FROM);
    let install_id = meta(&data, SCHEMA_META_INSTALL_ID);
    assert!(install_id.is_some());
    // Content written after the upgrade.
    set_meta(&data, "rollback_test_marker", "written after the upgrade");
    let db = data.join("ferrite.db");
    let db_bytes = std::fs::read(&db).unwrap();
    std::fs::write(data.join("ferrite.db-wal"), b"wal bytes").unwrap();
    std::fs::write(data.join("ferrite.db-shm"), b"shm bytes").unwrap();
    let snapshot = data.join(rec.snapshot.as_ref().unwrap());
    let snapshot_bytes = std::fs::read(&snapshot).unwrap();

    let out = rollback(&bins.current)
        .args(["--schema", "--restore-snapshot", "--yes", "--data"])
        .arg(&data)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let text = format!("{}{}", stdout(&out), stderr(&out));
    assert!(text.contains("lost"), "{text}");
    assert!(text.contains(&rec.at.to_rfc3339()), "{text}");

    let backups = data.join("backups");
    let aside = names(&backups, "rolled-back-");
    assert_eq!(aside.len(), 3, "{aside:?}");
    let base = aside
        .iter()
        .find(|n| n.ends_with(".db"))
        .expect("rolled-back db");
    assert_eq!(std::fs::read(backups.join(base)).unwrap(), db_bytes);
    assert_eq!(
        std::fs::read(backups.join(format!("{base}-wal"))).unwrap(),
        b"wal bytes"
    );
    assert_eq!(
        std::fs::read(backups.join(format!("{base}-shm"))).unwrap(),
        b"shm bytes"
    );
    assert!(!data.join("ferrite.db-wal").exists());
    assert!(!data.join("ferrite.db-shm").exists());
    // The snapshot is copied, not moved.
    assert_eq!(std::fs::read(&snapshot).unwrap(), snapshot_bytes);

    assert_eq!(db_version(&data), LOSSY_FROM);
    assert_eq!(meta(&data, "rollback_test_marker"), None);
    assert_eq!(meta(&data, SCHEMA_META_INSTALL_ID), install_id);
    bins.assert_swapped();
    assert!(!data.join(UPGRADE_FILE).exists());
    assert_eq!(names(&data, "upgrade-rolled-back-").len(), 1);
}

/// Without `--yes` and without a terminal it refuses; a record without a snapshot (a
/// provisional one, or nothing migrated) refuses, even when a snapshot of the move exists
/// by name; and a running site (the folder lock held) refuses.
#[test]
fn rollback_restore_snapshot_needs_confirmation() {
    let tmp = tempfile::tempdir().unwrap();
    let bins = Bins::new(tmp.path(), Some(0));
    let (data, mut rec) = upgraded_site(tmp.path(), LOSSY_FROM);
    set_meta(&data, "rollback_test_marker", "kept");
    // Not compared byte for byte: opening the store in this test checkpoints the WAL.
    let unchanged = |rec: &UpgradeRecord| {
        assert_eq!(db_version(&data), latest_schema_version());
        assert_eq!(meta(&data, "rollback_test_marker").as_deref(), Some("kept"));
        assert!(names(&data.join("backups"), "rolled-back-").is_empty());
        assert_eq!(upgrade::load(&data).unwrap().as_ref(), Some(rec));
        bins.assert_unswapped();
    };

    let out = rollback(&bins.current)
        .args(["--schema", "--restore-snapshot", "--data"])
        .arg(&data)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("--yes"), "{}", stderr(&out));
    unchanged(&rec);

    // The folder lock held (as `serve` holds it).
    let out = rt().block_on(async {
        let store = sqlite_in(&data).await;
        let out = rollback(&bins.current)
            .args(["--schema", "--restore-snapshot", "--yes", "--data"])
            .arg(&data)
            .output()
            .unwrap();
        store.close().await;
        out
    });
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("in use"), "{}", stderr(&out));
    unchanged(&rec);

    // No snapshot recorded: never looked up by name.
    rec.snapshot = None;
    upgrade::write(&data, &rec).unwrap();
    let out = rollback(&bins.current)
        .args(["--schema", "--restore-snapshot", "--yes", "--data"])
        .arg(&data)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("no snapshot"), "{}", stderr(&out));
    unchanged(&rec);
}

// ---- Postgres ----

struct PgSchema {
    url: String,
    schema: String,
}

impl PgSchema {
    /// `None` (after a SKIP line) when `FERRITE_TEST_PG_URL` is unset; panics in CI.
    fn new() -> Option<Self> {
        let Ok(url) = std::env::var(PG_URL_ENV) else {
            if std::env::var("CI").as_deref() == Ok("true") {
                panic!("{PG_URL_ENV} must be set in CI");
            }
            eprintln!("SKIP: Postgres rollback test skipped ({PG_URL_ENV} is not set)");
            return None;
        };
        let schema = format!("t_{}", uuid::Uuid::now_v7().simple());
        Some(Self { url, schema })
    }
}

impl Drop for PgSchema {
    fn drop(&mut self) {
        let url = self.url.clone();
        let sql = format!("DROP SCHEMA IF EXISTS \"{}\" CASCADE", self.schema);
        let handle = std::thread::spawn(move || {
            rt().block_on(async move {
                let mut conn = PgConnection::connect(&url).await?;
                sqlx::raw_sql(AssertSqlSafe(sql)).execute(&mut conn).await?;
                conn.close().await
            })
        });
        if let Ok(Err(e)) = handle.join() {
            eprintln!("warning: could not drop test schema {}: {e}", self.schema);
        }
    }
}

/// A Postgres site whose `upgrade.json` (from `migrate up`, snapshot null) starts at
/// `schema_from`.
fn upgraded_pg_site(root: &Path, pg: &PgSchema, schema_from: i64) -> (PathBuf, UpgradeRecord) {
    let data = root.join("site");
    init_pg(&data, pg);
    migrate_down(&data, schema_from);
    migrate_up(&data);
    let rec = upgrade::load(&data).unwrap().expect("upgrade.json");
    assert_eq!(rec.schema_from, schema_from);
    assert_eq!(rec.snapshot, None);
    (data, rec)
}

#[test]
fn rollback_schema_downs_on_postgres() {
    let Some(pg) = PgSchema::new() else { return };
    let tmp = tempfile::tempdir().unwrap();
    let bins = Bins::new(tmp.path(), Some(0));
    let (data, _) = upgraded_pg_site(tmp.path(), &pg, LOSSY_FROM);

    let out = rollback(&bins.current)
        .args(["--schema", "--data"])
        .arg(&data)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("--allow-lossy"), "{}", stderr(&out));
    assert_eq!(db_version(&data), latest_schema_version());

    let out = rollback(&bins.current)
        .args(["--schema", "--allow-lossy", "--data"])
        .arg(&data)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(db_version(&data), LOSSY_FROM);
    bins.assert_swapped();
    assert_eq!(names(&data, "upgrade-rolled-back-").len(), 1);
}

#[test]
fn rollback_restore_snapshot_refused_on_postgres() {
    let Some(pg) = PgSchema::new() else { return };
    let tmp = tempfile::tempdir().unwrap();
    let bins = Bins::new(tmp.path(), Some(0));
    let (data, rec) = upgraded_pg_site(tmp.path(), &pg, latest_schema_version() - 1);

    let out = rollback(&bins.current)
        .args(["--schema", "--restore-snapshot", "--yes", "--data"])
        .arg(&data)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let err = stderr(&out);
    assert!(err.contains("PITR"), "{err}");
    assert_eq!(db_version(&data), latest_schema_version());
    bins.assert_unswapped();
    assert_eq!(upgrade::load(&data).unwrap(), Some(rec));
}
