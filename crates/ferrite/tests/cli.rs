#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;

use assert_cmd::Command;
use assert_cmd::cargo::cargo_bin_cmd;
use sqlx::{AssertSqlSafe, Connection, PgConnection, SqliteConnection};

#[path = "../../ferrite-plugin-host/src/test_support.rs"]
mod test_support;

const PG_URL_ENV: &str = "FERRITE_TEST_PG_URL";

fn ferrite() -> Command {
    let mut cmd = cargo_bin_cmd!("ferrite");
    cmd.env_clear();
    cmd
}

fn data_arg(dir: &Path) -> String {
    dir.to_str().expect("utf-8 temp path").to_owned()
}

fn init_sqlite(dir: &Path) {
    ferrite()
        .args(["init", "--url", "https://example.com", "--data"])
        .arg(dir)
        .assert()
        .success();
}

/// Gives `dir`'s database its `-wal` and `-shm` side files. Whether `init` leaves them
/// depends on the platform (SQLite removes both when the last connection closes cleanly,
/// as it does on Linux); the restore-tmp checks look only at which files exist, and SQLite
/// takes an empty `-wal` or `-shm` as a valid, empty one.
fn ensure_side_files(dir: &Path) {
    for side in ["ferrite.db-wal", "ferrite.db-shm"] {
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join(side))
            .expect("side file");
    }
}

fn stdout(mut cmd: Command) -> String {
    let out = cmd.assert().success().get_output().stdout.clone();
    String::from_utf8(out).expect("utf-8 stdout")
}

/// Runs `cmd`, asserts its exit code and returns stdout.
fn stdout_code(mut cmd: Command, code: i32) -> String {
    let out = cmd.assert().code(code).get_output().stdout.clone();
    String::from_utf8(out).expect("utf-8 stdout")
}

#[test]
fn init_creates_config_and_db() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let data = tmp.path().join("site");
    let out = stdout({
        let mut c = ferrite();
        c.args([
            "init",
            "--url",
            "https://example.com",
            "--title",
            "Hi",
            "--data",
        ])
        .arg(&data);
        c
    });
    assert!(out.contains("ferrite migrate status --data"), "{out}");
    assert!(data.join("ferrite.toml").is_file());
    assert!(data.join("ferrite.db").is_file());
    assert!(data.join("media").is_dir());
    assert!(data.join("backups").is_dir());
    let toml = std::fs::read_to_string(data.join("ferrite.toml")).expect("read toml");
    assert!(toml.contains("title = \"Hi\""), "{toml}");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(data.join("ferrite.toml"))
            .expect("metadata")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        let mode = std::fs::metadata(data.join("secret.key"))
            .expect("secret.key metadata")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    // The solo site key is never printed.
    let key = std::fs::read_to_string(data.join("secret.key")).expect("read secret.key");
    assert_eq!(key.len(), 65, "{key:?}");
    assert!(!out.contains(key.trim()), "{out}");
    let status = stdout({
        let mut c = ferrite();
        c.args(["migrate", "status", "--data"]).arg(&data);
        c
    });
    assert!(status.contains("verdict: up to date"), "{status}");
}

#[test]
fn init_refuses_overwrite() {
    let tmp = tempfile::tempdir().expect("tempdir");
    init_sqlite(tmp.path());
    let before = std::fs::read_to_string(tmp.path().join("ferrite.toml")).expect("read");
    let assert = ferrite()
        .args(["init", "--url", "https://other.example", "--data"])
        .arg(tmp.path())
        .assert()
        .code(1);
    let err = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
    assert!(err.contains("already exists"), "{err}");
    let after = std::fs::read_to_string(tmp.path().join("ferrite.toml")).expect("read");
    assert_eq!(before, after);
}

#[test]
fn init_rejects_bad_media_and_leaves_no_config() {
    let tmp = tempfile::tempdir().expect("tempdir");
    ferrite()
        .args([
            "init",
            "--url",
            "https://example.com",
            "--media",
            "gs://x",
            "--data",
        ])
        .arg(tmp.path())
        .assert()
        .code(1);
    assert!(!tmp.path().join("ferrite.toml").exists());
    // Invalid config (sqlite + schema) is rolled back so a retry works.
    ferrite()
        .args([
            "init",
            "--url",
            "https://example.com",
            "--schema",
            "s",
            "--data",
        ])
        .arg(tmp.path())
        .assert()
        .code(1);
    assert!(!tmp.path().join("ferrite.toml").exists());
}

#[test]
fn init_s3_media_writes_bucket_and_prefix() {
    let tmp = tempfile::tempdir().expect("tempdir");
    // Config validation passes; the database is the default SQLite file.
    ferrite()
        .args([
            "init",
            "--url",
            "https://example.com",
            "--media",
            "s3://my-bucket/uploads",
            "--data",
        ])
        .arg(tmp.path())
        .assert()
        .success();
    let toml = std::fs::read_to_string(tmp.path().join("ferrite.toml")).expect("read");
    assert!(toml.contains("backend = \"s3\""), "{toml}");
    assert!(toml.contains("bucket = \"my-bucket\""), "{toml}");
    assert!(toml.contains("prefix = \"uploads/\""), "{toml}");
    assert!(!tmp.path().join("media").exists());
}

#[test]
fn migrate_status_output() {
    let tmp = tempfile::tempdir().expect("tempdir");
    init_sqlite(tmp.path());
    let out = stdout({
        let mut c = ferrite();
        c.args(["migrate", "status", "--data"]).arg(tmp.path());
        c
    });
    assert!(out.contains("database version: 14"), "{out}");
    assert!(
        out.contains(&format!("(ferrite {})", env!("CARGO_PKG_VERSION"))),
        "{out}"
    );
    assert!(out.contains("pending migrations: none"), "{out}");
    assert!(out.contains("verdict: up to date"), "{out}");

    // 0009's down is lossy: refused without --allow-lossy.
    ferrite()
        .args(["migrate", "down", "--to", "8", "--yes", "--data"])
        .arg(tmp.path())
        .assert()
        .code(1);
    // Revert the newest migrations: they become pending with their directives.
    let mut c = ferrite();
    c.args([
        "migrate",
        "down",
        "--to",
        "2",
        "--allow-lossy",
        "--yes",
        "--data",
    ])
    .arg(tmp.path());
    assert!(stdout(c).contains("Reverted migrations: 14, 13, 12, 11, 10, 9, 8, 7, 6, 5, 4, 3"));
    let out = stdout_code(
        {
            let mut c = ferrite();
            c.args(["migrate", "status", "--data"]).arg(tmp.path());
            c
        },
        2,
    );
    assert!(out.contains("database version: 2"), "{out}");
    assert!(
        out.contains("0003 media variants (phase=expand, min-binary=-, lossy-down=no)"),
        "{out}"
    );
    assert!(
        out.contains("0004 cluster (phase=expand, min-binary=-, lossy-down=yes)"),
        "{out}"
    );
    assert!(
        out.contains("0005 auth (phase=expand, min-binary=-, lossy-down=yes)"),
        "{out}"
    );
    assert!(
        out.contains("0006 media comments (phase=expand, min-binary=-, lossy-down=yes)"),
        "{out}"
    );
    assert!(
        out.contains("0007 autosaves (phase=expand, min-binary=-, lossy-down=yes)"),
        "{out}"
    );
    assert!(
        out.contains("0008 autosave seo (phase=expand, min-binary=-, lossy-down=yes)"),
        "{out}"
    );
    assert!(
        out.contains("0009 import (phase=expand, min-binary=-, lossy-down=yes)"),
        "{out}"
    );
    assert!(out.contains("verdict: needs migrate up"), "{out}");

    let mut c = ferrite();
    c.args(["migrate", "up", "--data"]).arg(tmp.path());
    assert!(stdout(c).contains("Applied migrations: 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14"));
    let mut c = ferrite();
    c.args(["migrate", "verify", "--data"]).arg(tmp.path());
    assert!(stdout(c).contains("checksums OK"));
}

#[test]
fn migrate_down_requires_yes() {
    let tmp = tempfile::tempdir().expect("tempdir");
    init_sqlite(tmp.path());
    let assert = ferrite()
        .args(["migrate", "down", "--to", "1", "--data"])
        .arg(tmp.path())
        .write_stdin("")
        .assert()
        .code(1);
    let err = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
    assert!(err.contains("--yes"), "{err}");
    let out = stdout({
        let mut c = ferrite();
        c.args(["migrate", "status", "--data"]).arg(tmp.path());
        c
    });
    assert!(out.contains("database version: 14"), "{out}");
}

#[test]
fn missing_config_is_an_error() {
    let tmp = tempfile::tempdir().expect("tempdir");
    ferrite()
        .args(["migrate", "status", "--data"])
        .arg(tmp.path())
        .assert()
        .code(1);
}

#[test]
fn status_on_missing_database_creates_nothing() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let data = tmp.path().join("site");
    let assert = ferrite()
        .env("FERRITE_SITE__URL", "https://example.com")
        .args(["migrate", "status", "--data"])
        .arg(&data)
        .assert()
        .code(1);
    let err = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
    assert!(err.contains("database not found"), "{err}");
    assert!(err.contains("ferrite.db"), "{err}");
    assert!(!data.exists(), "no data dir may be created");
    // Also for down/verify with an existing, empty data dir.
    std::fs::create_dir(&data).expect("mkdir");
    for args in [
        vec!["migrate", "verify"],
        vec!["migrate", "down", "--to", "0", "--yes"],
    ] {
        ferrite()
            .env("FERRITE_SITE__URL", "https://example.com")
            .args(args)
            .arg("--data")
            .arg(&data)
            .assert()
            .code(1);
    }
    assert_eq!(std::fs::read_dir(&data).expect("readdir").count(), 0);
}

/// P70: a `rollback --restore-snapshot` killed between moving `ferrite.db` aside and
/// renaming the staged copy into place leaves only `ferrite.db.restore-<ts>.tmp`. Opening
/// the store then refuses (naming the file and the `mv` that finishes the restore) instead
/// of creating an empty database that `serve` would migrate and serve.
#[test]
fn serve_refuses_when_only_a_restore_tmp_is_left() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let data = tmp.path().join("site");
    init_sqlite(&data);
    let db = data.join("ferrite.db");
    let staged = data.join("ferrite.db.restore-20260101T000000Z.tmp");
    std::fs::rename(&db, &staged).expect("stage");
    let aside = data.join("backups").join("rolled-back-20260101T000000Z.db");
    std::fs::write(&aside, b"the database that was current").expect("aside");

    let mut serve = ferrite();
    serve
        .env("FERRITE_SERVER__LISTEN", "127.0.0.1:0")
        .args(["serve", "--data"])
        .arg(&data)
        .timeout(std::time::Duration::from_secs(60));
    let out = serve.assert().code(1).get_output().clone();
    let err = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(err.contains(&staged.display().to_string()), "{err}");
    assert!(
        err.contains(&format!("mv {} {}", staged.display(), db.display())),
        "{err}"
    );
    assert!(err.contains(&aside.display().to_string()), "{err}");
    assert!(!db.exists(), "no empty database may be created");
    // The other commands that may create the database refuse too.
    ferrite()
        .args(["migrate", "up", "--data"])
        .arg(&data)
        .assert()
        .code(1);
    assert!(!db.exists());
    assert!(staged.is_file());
}

/// P70: a staged copy left beside a database that is still in place (a run killed before
/// it moved anything) only warns.
#[test]
fn a_leftover_restore_tmp_beside_the_db_only_warns() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let data = tmp.path().join("site");
    init_sqlite(&data);
    let staged = data.join("ferrite.db.restore-20260101T000000Z.tmp");
    std::fs::copy(data.join("ferrite.db"), &staged).expect("copy");
    let out = ferrite()
        .args(["migrate", "status", "--data"])
        .arg(&data)
        .assert()
        .code(0)
        .get_output()
        .clone();
    let err = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(err.contains("WARN"), "{err}");
    assert!(err.contains(&staged.display().to_string()), "{err}");
    assert!(staged.is_file(), "the leftover is not deleted");
}

/// M4-4 R-c2 (amends P70): a run killed after it moved `ferrite.db-wal` (and `-shm`) into
/// `backups/` but before it moved `ferrite.db` leaves the database in place without its
/// WAL. Opening it would lose the transactions committed there, so it refuses and names the
/// `mv` that puts each file back; likewise when the kill came inside the database's own
/// move (`rolled-back-<ts>.db` is a second name of `ferrite.db`).
#[test]
fn a_restore_tmp_with_the_wal_moved_aside_refuses() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let data = tmp.path().join("site");
    init_sqlite(&data);
    let db = data.join("ferrite.db");
    let staged = data.join("ferrite.db.restore-20260101T000000Z.tmp");
    std::fs::copy(&db, &staged).expect("copy");
    let backups = data.join("backups");
    let wal = backups.join("rolled-back-20260101T000000Z.db-wal");
    let shm = backups.join("rolled-back-20260101T000000Z.db-shm");
    // A WAL holds committed transactions the database file does not yet have, the very
    // state a killed run loses: move both side files as `replace_db` does.
    ensure_side_files(&data);
    let (db_wal, db_shm) = (data.join("ferrite.db-wal"), data.join("ferrite.db-shm"));
    std::fs::rename(&db_wal, &wal).expect("move wal");
    std::fs::rename(&db_shm, &shm).expect("move shm");
    let refused = |extra: &[&str]| {
        let out = ferrite()
            .args(["migrate", "status", "--data"])
            .arg(&data)
            .assert()
            .code(1)
            .get_output()
            .clone();
        let err = String::from_utf8_lossy(&out.stderr).into_owned();
        for want in [
            format!("mv {} {}-wal", wal.display(), db.display()),
            format!("mv {} {}-shm", shm.display(), db.display()),
            staged.display().to_string(),
        ] {
            assert!(err.contains(&want), "{want}: {err}");
        }
        for want in extra {
            assert!(err.contains(want), "{want}: {err}");
        }
        // Nothing was opened: no WAL was created beside the database.
        assert!(!db_wal.exists(), "{err}");
    };
    refused(&[]);

    // Fix round 3: while another process holds the folder lock (a rollback still moving
    // its files), the files are in flight, not leftovers: "in use", and no advice.
    let lock = std::fs::File::options()
        .read(true)
        .write(true)
        .open(data.join("ferrite.lock"))
        .expect("lock file");
    lock.try_lock().expect("lock");
    let out = ferrite()
        .args(["migrate", "status", "--data"])
        .arg(&data)
        .output()
        .expect("run");
    drop(lock);
    let err = String::from_utf8_lossy(&out.stderr).into_owned();
    assert_eq!(out.status.code(), Some(1), "{err}");
    assert!(err.contains("in use") && !err.contains("mv "), "{err}");

    // Killed inside the database's own move: the link exists, the source was not unlinked.
    let aside = backups.join("rolled-back-20260101T000000Z.db");
    std::fs::hard_link(&db, &aside).expect("link");
    let rm = format!("rm {}", aside.display());
    refused(&[&rm]);

    // The steps the message gives: the site opens again with everything its WAL held.
    std::fs::rename(&wal, &db_wal).expect("mv wal back");
    std::fs::rename(&shm, &db_shm).expect("mv shm back");
    std::fs::remove_file(&aside).expect("rm aside");
    std::fs::remove_file(&staged).expect("rm tmp");
    let out = stdout_code(
        {
            let mut c = ferrite();
            c.args(["migrate", "status", "--data"]).arg(&data);
            c
        },
        0,
    );
    assert!(out.contains("database version: 14"), "{out}");
}

/// M4-4 fix round 2 (Minor A): killed inside the `-wal` move (the link made, the source not
/// yet unlinked), nothing is missing beside the database, so the open only warns, and the
/// warning names the `rm` of the second name too.
#[cfg(unix)]
#[test]
fn a_restore_tmp_with_a_linked_wal_warns_with_the_rm() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let data = tmp.path().join("site");
    init_sqlite(&data);
    let staged = data.join("ferrite.db.restore-20260101T000000Z.tmp");
    std::fs::copy(data.join("ferrite.db"), &staged).expect("copy");
    ensure_side_files(&data);
    let link = data
        .join("backups")
        .join("rolled-back-20260101T000000Z.db-wal");
    std::fs::hard_link(data.join("ferrite.db-wal"), &link).expect("link");
    let out = ferrite()
        .args(["migrate", "status", "--data"])
        .arg(&data)
        .assert()
        .code(0)
        .get_output()
        .clone();
    let err = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(err.contains(&staged.display().to_string()), "{err}");
    assert!(err.contains(&format!("rm {}", link.display())), "{err}");
}

#[test]
fn init_refuses_existing_database_untouched() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let db = tmp.path().join("ferrite.db");
    std::fs::write(&db, b"precious bytes").expect("write");
    let assert = ferrite()
        .args(["init", "--url", "https://example.com", "--data"])
        .arg(tmp.path())
        .assert()
        .code(1);
    let err = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
    assert!(err.contains("existing database found"), "{err}");
    assert!(err.contains("ferrite migrate"), "{err}");
    assert_eq!(std::fs::read(&db).expect("read"), b"precious bytes");
    assert!(!tmp.path().join("ferrite.toml").exists());
    assert!(!tmp.path().join("media").exists());
    assert!(!tmp.path().join("backups").exists());
}

#[test]
fn failed_init_removes_what_it_created() {
    let tmp = tempfile::tempdir().expect("tempdir");
    // Postgres URL nobody listens on: config is valid, opening the store fails.
    ferrite()
        .args(["init", "--url", "https://example.com"])
        .args(["--db", "postgres://u:secret@127.0.0.1:1/d", "--data"])
        .arg(tmp.path())
        .assert()
        .code(1);
    assert_eq!(std::fs::read_dir(tmp.path()).expect("readdir").count(), 0);
}

#[test]
fn failed_init_removes_the_secret_key_it_created() {
    let tmp = tempfile::tempdir().expect("tempdir");
    // A file where the backups folder goes: init fails after creating secret.key.
    std::fs::write(tmp.path().join("backups"), b"in the way").expect("write");
    let out = ferrite()
        .args(["init", "--url", "https://example.com", "--data"])
        .arg(tmp.path())
        .assert()
        .code(1)
        .get_output()
        .clone();
    let err = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(err.contains("backups"), "{err}");
    assert!(!tmp.path().join("secret.key").exists());
    assert!(!tmp.path().join("ferrite.toml").exists());
    assert!(!tmp.path().join("media").exists());
}

#[test]
fn init_cluster_config_needs_key_and_writes_no_key_file() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let init = || {
        let mut c = ferrite();
        c.args(["init", "--url", "https://example.com"])
            .args(["--db", "postgres://u:secret@127.0.0.1:1/d"])
            .args(["--media", "s3://bucket", "--data"])
            .arg(tmp.path());
        c
    };
    // auto + Postgres + S3 is cluster mode: the key must be configured.
    let out = init().assert().code(1).get_output().clone();
    let err = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(err.contains("auth.secret_key"), "{err}");
    assert!(err.contains("required in cluster mode"), "{err}");
    assert_eq!(std::fs::read_dir(tmp.path()).expect("readdir").count(), 0);
    // With the key the config is valid (the database is unreachable); no secret.key
    // is written next to a cluster config.
    let key = "cd".repeat(32);
    let out = init()
        .env("FERRITE_AUTH__SECRET_KEY", &key)
        .assert()
        .code(1)
        .get_output()
        .clone();
    let err = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(!err.contains("auth.secret_key"), "{err}");
    assert!(!err.contains(&key), "{err}");
    assert_eq!(std::fs::read_dir(tmp.path()).expect("readdir").count(), 0);
}

#[test]
fn solo_site_key_file_created_once() {
    let tmp = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        tmp.path().join("ferrite.toml"),
        "[site]\nurl = \"https://example.com\"\n",
    )
    .unwrap();
    let config = ferrite_config::load(tmp.path(), Vec::new()).unwrap();
    let path = tmp.path().join("secret.key");
    assert!(!path.exists());
    let first = ferrite::keys::site_key(&config).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert_eq!(text, format!("{}\n", first.to_hex()));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    // The second call reads the same key.
    let second = ferrite::keys::site_key(&config).unwrap();
    assert_eq!(second.to_hex(), first.to_hex());
    // A file without the trailing newline is accepted.
    std::fs::write(&path, first.to_hex()).unwrap();
    assert_eq!(
        ferrite::keys::site_key(&config).unwrap().to_hex(),
        first.to_hex()
    );

    // A malformed file is an error naming it, without echoing its content.
    std::fs::write(&path, "not-a-key-0123456789\n").unwrap();
    let err = format!("{:#}", ferrite::keys::site_key(&config).unwrap_err());
    assert!(err.contains("secret.key"), "{err}");
    assert!(!err.contains("not-a-key-0123456789"), "{err}");
    std::fs::write(&path, "").unwrap();
    let err = format!("{:#}", ferrite::keys::site_key(&config).unwrap_err());
    assert!(err.contains("secret.key"), "{err}");

    // A configured key wins over the file and leaves it alone.
    let configured = "ef".repeat(32);
    let config = ferrite_config::load(
        tmp.path(),
        vec![("FERRITE_AUTH__SECRET_KEY".to_owned(), configured.clone())],
    )
    .unwrap();
    assert_eq!(
        ferrite::keys::site_key(&config).unwrap().to_hex(),
        configured
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "");
}

fn solo_config(dir: &Path) -> ferrite_config::Config {
    std::fs::write(
        dir.join("ferrite.toml"),
        "[site]\nurl = \"https://example.com\"\n",
    )
    .unwrap();
    ferrite_config::load(dir, Vec::new()).unwrap()
}

fn temp_files(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".tmp"))
        .collect();
    names.sort();
    names
}

#[test]
fn site_key_create_ignores_crash_leftovers() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let config = solo_config(tmp.path());
    // Temp files a crashed run may have left: a partial write and an empty file, one
    // with this process's id in the name.
    let pid = std::process::id();
    std::fs::write(tmp.path().join(".secret.key.1.00ff00ff00ff00ff.tmp"), "abc").unwrap();
    std::fs::write(
        tmp.path()
            .join(format!(".secret.key.{pid}.0000000000000000.tmp")),
        "",
    )
    .unwrap();
    let before = temp_files(tmp.path());
    let key = ferrite::keys::site_key(&config).unwrap();
    let text = std::fs::read_to_string(tmp.path().join("secret.key")).unwrap();
    assert_eq!(text, format!("{}\n", key.to_hex()));
    // Leftovers are untouched and the new temp file is gone.
    assert_eq!(temp_files(tmp.path()), before);
    assert_eq!(
        ferrite::keys::site_key(&config).unwrap().to_hex(),
        key.to_hex()
    );
}

/// Collects formatted log output from the current thread's subscriber.
#[derive(Clone, Default)]
struct LogBuf(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for LogBuf {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(unix)]
#[test]
fn site_key_file_with_loose_mode_warns_without_content() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().expect("tempdir");
    let config = solo_config(tmp.path());
    let path = tmp.path().join("secret.key");
    let hex = "ab".repeat(32);
    std::fs::write(&path, format!("{hex}\n")).unwrap();
    let logs = LogBuf::default();
    let writer = logs.clone();
    let dispatch = tracing::Dispatch::new(
        tracing_subscriber::fmt()
            .with_ansi(false)
            .with_writer(move || writer.clone())
            .finish(),
    );
    let read = |mode: u32| {
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
        logs.0.lock().unwrap().clear();
        let key = tracing::dispatcher::with_default(&dispatch, || {
            ferrite::keys::site_key(&config).unwrap()
        });
        assert_eq!(key.to_hex(), hex);
        String::from_utf8(logs.0.lock().unwrap().clone()).unwrap()
    };
    let out = read(0o644);
    assert!(out.contains("WARN"), "{out}");
    assert!(out.contains("secret.key") && out.contains("0644"), "{out}");
    assert!(!out.contains(&hex), "{out}");
    let out = read(0o600);
    assert!(!out.contains("WARN"), "{out}");
}

#[test]
fn non_utf8_ferrite_env_names_the_key_not_the_value() {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let tmp = tempfile::tempdir().expect("tempdir");
        let bad = std::ffi::OsStr::from_bytes(b"sekrit\xff");
        let assert = ferrite()
            .env("FERRITE_SITE__URL", bad)
            .env("OTHER_VAR", bad)
            .args(["migrate", "status", "--data"])
            .arg(tmp.path())
            .assert()
            .code(1);
        let err = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
        assert!(err.contains("FERRITE_SITE__URL"), "{err}");
        assert!(!err.contains("sekrit"), "{err}");
        // A non-FERRITE variable alone is ignored (the failure is then the missing db).
        let assert = ferrite()
            .env("FERRITE_SITE__URL", "https://example.com")
            .env("OTHER_VAR", bad)
            .args(["migrate", "status", "--data"])
            .arg(tmp.path())
            .assert()
            .code(1);
        let err = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
        assert!(err.contains("database not found"), "{err}");
    }
}

#[test]
fn status_exit_codes() {
    let tmp = tempfile::tempdir().expect("tempdir");
    init_sqlite(tmp.path());
    let status = |tmp: &Path| {
        let mut c = ferrite();
        c.args(["migrate", "status", "--data"]).arg(tmp);
        c
    };
    status(tmp.path()).assert().code(0);
    ferrite()
        .args([
            "migrate",
            "down",
            "--to",
            "1",
            "--allow-lossy",
            "--yes",
            "--data",
        ])
        .arg(tmp.path())
        .assert()
        .success();
    status(tmp.path()).assert().code(2);
    ferrite()
        .args(["migrate", "up", "--data"])
        .arg(tmp.path())
        .assert()
        .success();
    // A newer schema that requires a newer binary.
    let db = tmp.path().join("ferrite.db");
    let rt = tokio_rt().expect("runtime");
    rt.block_on(async {
        let mut conn = SqliteConnection::connect(&format!("sqlite://{}", db.display()))
            .await
            .expect("connect");
        sqlx::query(
            "INSERT INTO _sqlx_migrations (version, description, success, checksum, execution_time) \
             VALUES (99, 'future', 1, x'00', 0)",
        )
        .execute(&mut conn)
        .await
        .expect("insert migration");
        sqlx::query("INSERT INTO schema_meta (key, value) VALUES ('min_binary_version', '99.0.0')")
            .execute(&mut conn)
            .await
            .expect("insert min");
        conn.close().await.expect("close");
    });
    status(tmp.path()).assert().code(3);
}

#[test]
fn status_help_documents_exit_codes() {
    let out = stdout({
        let mut c = ferrite();
        c.args(["migrate", "status", "--help"]);
        c
    });
    assert!(
        out.contains("Exit code") && out.contains('2') && out.contains('3'),
        "{out}"
    );
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
            eprintln!("SKIP: Postgres CLI test skipped ({PG_URL_ENV} is not set)");
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
            let rt = tokio_rt()?;
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

/// Names of the tables in `schema`, plus any ferrite table found in `public`.
fn pg_tables(url: &str, schema: &str) -> Vec<String> {
    let (url, schema) = (url.to_owned(), schema.to_owned());
    std::thread::spawn(move || {
        tokio_rt().expect("runtime").block_on(async move {
            let mut conn = PgConnection::connect(&url).await.expect("connect");
            let in_schema: Vec<String> = sqlx::query_scalar(
                "SELECT table_name FROM information_schema.tables WHERE table_schema = $1",
            )
            .bind(&schema)
            .fetch_all(&mut conn)
            .await
            .expect("tables");
            let in_public: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM information_schema.tables \
                 WHERE table_schema = 'public' AND table_name = 'content_items'",
            )
            .fetch_one(&mut conn)
            .await
            .expect("public");
            conn.close().await.expect("close");
            assert_eq!(in_public, 0, "tables must not land in public");
            in_schema
        })
    })
    .join()
    .expect("thread")
}

fn tokio_rt() -> std::io::Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
}

#[test]
fn postgres_init_and_migrate() {
    let Some(pg) = PgSchema::new() else { return };
    let tmp = tempfile::tempdir().expect("tempdir");
    ferrite()
        .args(["init", "--url", "https://example.com", "--db", &pg.url])
        .args(["--schema", &pg.schema, "--data", &data_arg(tmp.path())])
        .assert()
        .success();
    let toml = std::fs::read_to_string(tmp.path().join("ferrite.toml")).expect("read");
    assert!(
        toml.contains(&format!("schema = \"{}\"", pg.schema)),
        "{toml}"
    );

    let tables = pg_tables(&pg.url, &pg.schema);
    assert!(tables.iter().any(|t| t == "content_items"), "{tables:?}");
    assert!(tables.iter().any(|t| t == "_sqlx_migrations"), "{tables:?}");
    // init refuses to adopt the now-migrated schema.
    let tmp2 = tempfile::tempdir().expect("tempdir");
    let assert = ferrite()
        .args(["init", "--url", "https://example.com", "--db", &pg.url])
        .args(["--schema", &pg.schema, "--data", &data_arg(tmp2.path())])
        .assert()
        .code(1);
    let err = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
    assert!(err.contains("existing database found"), "{err}");
    assert!(!tmp2.path().join("ferrite.toml").exists());

    let status = |tmp: &Path| {
        let mut c = ferrite();
        c.args(["migrate", "status", "--data"]).arg(tmp);
        stdout(c)
    };
    let out = status(tmp.path());
    assert!(out.contains("database version: 14"), "{out}");
    assert!(out.contains("verdict: up to date"), "{out}");

    let mut c = ferrite();
    c.args([
        "migrate",
        "down",
        "--to",
        "2",
        "--allow-lossy",
        "--yes",
        "--data",
    ])
    .arg(tmp.path());
    assert!(stdout(c).contains("Reverted migrations: 14, 13, 12, 11, 10, 9, 8, 7, 6, 5, 4, 3"));
    let mut c = ferrite();
    c.args(["migrate", "status", "--data"]).arg(tmp.path());
    assert!(stdout_code(c, 2).contains("verdict: needs migrate up"));
    let mut c = ferrite();
    c.args(["migrate", "up", "--data"]).arg(tmp.path());
    assert!(stdout(c).contains("Applied migrations: 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14"));
    let mut c = ferrite();
    c.args(["migrate", "verify", "--data"]).arg(tmp.path());
    assert!(stdout(c).contains("checksums OK"));
}

#[test]
fn config_check_reports_effective_settings() {
    let tmp = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        tmp.path().join("ferrite.toml"),
        "[site]\nurl = \"https://example.com\"\n[cluster]\nworkers = 3\n",
    )
    .unwrap();
    let out = stdout({
        let mut c = ferrite();
        c.args(["config", "check", "--data"]).arg(tmp.path());
        c
    });
    assert_eq!(
        out.trim(),
        "ok: mode=solo database=sqlite media=local workers=3 mail=off metrics=off tls=off backup=off"
    );
    // `server.metrics_listen` (P34) is reported as its address.
    let out = stdout({
        let mut c = ferrite();
        c.args(["config", "check", "--data"])
            .arg(tmp.path())
            .env("FERRITE_SERVER__METRICS_LISTEN", "127.0.0.1:9464");
        c
    });
    assert!(out.contains(" metrics=127.0.0.1:9464 "), "{out}");
    // `[server.tls]` (P33) is reported as its mode.
    let out = stdout({
        let mut c = ferrite();
        c.args(["config", "check", "--data"])
            .arg(tmp.path())
            .env("FERRITE_SERVER__TLS__MODE", "acme")
            .env("FERRITE_SERVER__TLS__CONTACT", "mailto:ops@example.com");
        c
    });
    assert!(out.contains(" tls=acme "), "{out}");
    // TLS keys without `mode = "acme"` load, with a warning that they are ignored.
    let out = ferrite()
        .args(["config", "check", "--data"])
        .arg(tmp.path())
        .env("FERRITE_SERVER__TLS__REDIRECT_HTTP", "0.0.0.0:80")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("warning: server.tls.redirect_http: ignored: server.tls.mode is \"off\""),
        "{err}"
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains(" tls=off "));
    // Never touches the database or media folder.
    assert!(!tmp.path().join("ferrite.db").exists());
    assert!(!tmp.path().join("media").exists());
}

#[test]
fn config_check_prints_backup() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let data = tmp.path().join("site");
    std::fs::create_dir(&data).unwrap();
    std::fs::write(
        data.join("ferrite.toml"),
        "[site]\nurl = \"https://example.com\"\n[cluster]\nworkers = 2\n\
         [backup]\nschedule = \"weekly\"\nto = \"../repo\"\n",
    )
    .unwrap();
    let out = stdout({
        let mut c = ferrite();
        c.args(["config", "check", "--data"]).arg(&data);
        c
    });
    assert_eq!(
        out.trim(),
        "ok: mode=solo database=sqlite media=local workers=2 mail=off metrics=off tls=off backup=weekly"
    );
    // The environment switches it like `serve`; a missing `to` is a problem (exit 1).
    let out = stdout({
        let mut c = ferrite();
        c.args(["config", "check", "--data"])
            .arg(&data)
            .env("FERRITE_BACKUP__SCHEDULE", "daily");
        c
    });
    assert!(out.trim().ends_with(" backup=daily"), "{out}");
    std::fs::write(
        data.join("ferrite.toml"),
        "[site]\nurl = \"https://example.com\"\n[backup]\nschedule = \"daily\"\n",
    )
    .unwrap();
    let out = ferrite()
        .args(["config", "check", "--data"])
        .arg(&data)
        .assert()
        .code(1)
        .get_output()
        .stderr
        .clone();
    let err = String::from_utf8(out).unwrap();
    assert!(err.contains("backup.to: required"), "{err}");
    // Nothing is created by the check.
    assert!(!tmp.path().join("repo").exists());

    // `--data .` (a relative data folder): an absolute or sibling `to` is fine.
    let elsewhere = tmp.path().join("elsewhere");
    for to in [elsewhere.to_str().unwrap(), "../backups"] {
        std::fs::write(
            data.join("ferrite.toml"),
            format!(
                "[site]\nurl = \"https://example.com\"\n[backup]\nschedule = \"daily\"\nto = \"{to}\"\n"
            ),
        )
        .unwrap();
        let out = stdout({
            let mut c = ferrite();
            c.current_dir(&data)
                .args(["config", "check", "--data", "."]);
            c
        });
        assert!(out.trim().ends_with(" backup=daily"), "{to}: {out}");
    }

    // The same location check as `serve`: a symlink into the data folder and the media
    // root are refused.
    #[cfg(unix)]
    {
        let link = tmp.path().join("link");
        std::os::unix::fs::symlink(&data, &link).unwrap();
        let inside = link.join("repo");
        for (to, media) in [
            (inside.to_str().unwrap().to_owned(), String::new()),
            (
                tmp.path().join("media/repo").to_str().unwrap().to_owned(),
                format!(
                    "[media]\nroot = \"{}\"\n",
                    tmp.path().join("media").display()
                ),
            ),
        ] {
            std::fs::write(
                data.join("ferrite.toml"),
                format!(
                    "[site]\nurl = \"https://example.com\"\n{media}[backup]\nschedule = \"daily\"\nto = \"{to}\"\n"
                ),
            )
            .unwrap();
            let out = ferrite()
                .args(["config", "check", "--data"])
                .arg(&data)
                .assert()
                .code(1)
                .get_output()
                .stderr
                .clone();
            let err = String::from_utf8(out).unwrap();
            assert!(err.contains("backup.to: "), "{to}: {err}");
            assert!(err.contains("inside the"), "{to}: {err}");
        }
    }
}

#[test]
fn config_check_applies_env_like_serve() {
    let tmp = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        tmp.path().join("ferrite.toml"),
        "[site]\nurl = \"https://example.com\"\n",
    )
    .unwrap();
    let out = stdout({
        let mut c = ferrite();
        c.args(["config", "check", "--data"])
            .arg(tmp.path())
            .env("FERRITE_DATABASE__URL", "postgres://u@db.invalid/x")
            .env("FERRITE_MEDIA__BACKEND", "s3")
            .env("FERRITE_MEDIA__S3__BUCKET", "b")
            .env("FERRITE_MEDIA__S3__REGION", "us-east-1")
            .env("FERRITE_CLUSTER__WORKERS", "2")
            .env("FERRITE_AUTH__SECRET_KEY", "ab".repeat(32));
        c
    });
    // auto + Postgres + S3 resolves to cluster.
    assert_eq!(
        out.trim(),
        "ok: mode=cluster database=postgres media=s3 workers=2 mail=off metrics=off tls=off backup=off"
    );
}

#[test]
fn config_check_lists_every_problem_and_exits_1() {
    let tmp = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        tmp.path().join("ferrite.toml"),
        "[site]\nurl = \"ftp://x\"\n[cluster]\nmode = \"cluster\"\nworkers = 0\n",
    )
    .unwrap();
    let out = ferrite()
        .args(["config", "check", "--data"])
        .arg(tmp.path())
        .env("FERRITE_DATABASE__URL", "postgres://u:hunter2@db.invalid/x")
        .assert()
        .code(1)
        .get_output()
        .clone();
    let stderr = String::from_utf8(out.stderr).unwrap();
    for path in ["site.url", "cluster.workers", "media.backend"] {
        assert!(stderr.contains(path), "missing {path}: {stderr}");
    }
    assert!(!stderr.contains("hunter2"), "{stderr}");
    assert!(out.stdout.is_empty());
}

/// Serves one canned HTTP response per connection on a loopback port.
fn canned_http(response: &'static str) -> String {
    canned_http_on("127.0.0.1:0", response)
}

/// [`canned_http`] on `bind`; the URL uses the bound address (IPv6 in brackets).
fn canned_http_on(bind: &str, response: &'static str) -> String {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind(bind).unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for mut s in listener.incoming().flatten() {
            let mut buf = [0u8; 1024];
            let _ = s.read(&mut buf);
            let _ = s.write_all(response.as_bytes());
        }
    });
    format!("http://{addr}/readyz")
}

#[test]
fn healthcheck_exit_code_follows_status() {
    let ok = canned_http("HTTP/1.1 200 OK\r\ncontent-length: 6\r\n\r\nready\n");
    ferrite()
        .args(["healthcheck", "--url", &ok])
        .assert()
        .success();
    let busy =
        canned_http("HTTP/1.1 503 Service Unavailable\r\ncontent-length: 10\r\n\r\nnot ready\n");
    ferrite()
        .args(["healthcheck", "--url", &busy])
        .assert()
        .code(1);
    // Nothing listening.
    let closed = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap()
    };
    ferrite()
        .args(["healthcheck", "--url", &format!("http://{closed}/readyz")])
        .assert()
        .code(1);
    ferrite()
        .args(["healthcheck", "--url", "https://127.0.0.1/readyz"])
        .assert()
        .code(1);
}

#[test]
fn healthcheck_accepts_ipv6_literals() {
    // Skip where the machine has no IPv6 loopback.
    if std::net::TcpListener::bind("[::1]:0").is_err() {
        eprintln!("SKIP: no IPv6 loopback");
        return;
    }
    let ok = canned_http_on(
        "[::1]:0",
        "HTTP/1.1 200 OK\r\ncontent-length: 6\r\n\r\nready\n",
    );
    assert!(ok.starts_with("http://[::1]:"), "{ok}");
    ferrite()
        .args(["healthcheck", "--url", &ok])
        .assert()
        .success();
}

// ---- ferrite user (P17) ------------------------------------------------------------

/// Runs `f` on the site's SQLite store opened in-process, then closes it.
///
/// The store's `ferrite.lock` goes to a scratch folder, not the site's: a lock fd of
/// this process is copied into every `ferrite` child a sibling test spawns meanwhile
/// (the fd table is copied before `O_CLOEXEC` closes it), and with `flock` that copy
/// would keep the site locked for a moment after `close`, so the next command here
/// could fail with "data folder is in use". The steps of a test run one at a time, so
/// nothing needs the site lock in this process.
fn with_site_store<T>(
    dir: &Path,
    f: impl AsyncFnOnce(std::sync::Arc<dyn ferrite_store::Store>) -> T,
) -> T {
    tokio_rt().expect("runtime").block_on(async {
        let config = ferrite_config::load(dir, Vec::new()).unwrap();
        let ferrite_config::DbKind::Sqlite { path } = config.database_kind() else {
            panic!("with_site_store is for SQLite sites");
        };
        assert!(path.is_file(), "no database at {}", path.display());
        let lock_dir = tempfile::tempdir().expect("tempdir");
        let store: std::sync::Arc<dyn ferrite_store::Store> = std::sync::Arc::new(
            ferrite_store_sqlite::SqliteStore::open(ferrite_store_sqlite::SqliteOptions {
                lock_dir: Some(lock_dir.path().to_path_buf()),
                ..ferrite_store_sqlite::SqliteOptions::new(path)
            })
            .await
            .unwrap(),
        );
        let out = f(store.clone()).await;
        store.close().await;
        out
    })
}

/// Logs `login` in through `AuthService` and returns the session (`None`: refused).
async fn login_session(
    store: std::sync::Arc<dyn ferrite_store::Store>,
    login: &str,
    password: &str,
) -> Option<ferrite_core::IssuedSession> {
    let auth =
        ferrite_core::AuthService::new(store, ferrite_core::auth::SiteKey::generate().unwrap());
    match auth
        .login(ferrite_core::LoginAttempt {
            login,
            password,
            ip: None,
            user_agent: None,
        })
        .await
    {
        Ok(ferrite_core::LoginOutcome::Session(s)) => Some(*s),
        Ok(other) => panic!("unexpected outcome {other:?}"),
        Err(ferrite_core::CoreError::Unauthenticated) => None,
        Err(e) => panic!("login failed: {e}"),
    }
}

fn user_cmd(dir: &Path, args: &[&str]) -> Command {
    let mut c = ferrite();
    c.arg("user").args(args).arg("--data").arg(dir);
    c
}

/// The generated password from a `password: <p>` line.
fn printed_password(out: &str) -> String {
    let p = out
        .lines()
        .find_map(|l| l.strip_prefix("password: "))
        .unwrap_or_else(|| panic!("no password line in {out:?}"))
        .to_owned();
    assert_eq!(p.len(), 20, "{p:?}");
    assert!(
        p.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
        "{p:?}"
    );
    p
}

#[test]
fn user_create_generates_password_and_login_works() {
    let tmp = tempfile::tempdir().expect("tempdir");
    init_sqlite(tmp.path());
    let out = stdout(user_cmd(
        tmp.path(),
        &["create", "--login", "Ann", "--email", "ann@example.com"],
    ));
    let password = printed_password(&out);
    let other = printed_password(&stdout(user_cmd(
        tmp.path(),
        &["create", "--login", "ben", "--email", "ben@example.com"],
    )));
    assert_ne!(password, other, "passwords are random");
    with_site_store(tmp.path(), async |store| {
        let ann = store
            .read()
            .users()
            .get_by_login("ann")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(ann.role, ferrite_domain::Role::Admin);
        assert_eq!(ann.display_name, "Ann");
        assert!(ann.must_reset);
        let session = login_session(store.clone(), "ann", &password).await;
        assert!(session.is_some_and(|s| s.user.must_reset));
    });
}

#[test]
fn user_create_password_stdin() {
    let tmp = tempfile::tempdir().expect("tempdir");
    init_sqlite(tmp.path());
    let mut c = user_cmd(
        tmp.path(),
        &[
            "create",
            "--login",
            "ed",
            "--email",
            "ed@example.com",
            "--role",
            "editor",
            "--display-name",
            "Ed Editor",
            "--password-stdin",
        ],
    );
    c.write_stdin("a long enough passphrase\n");
    let out = stdout(c);
    assert!(!out.contains("password:"), "{out}");
    assert!(!out.contains("passphrase"), "{out}");
    with_site_store(tmp.path(), async |store| {
        let ed = store
            .read()
            .users()
            .get_by_login("ed")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(ed.role, ferrite_domain::Role::Editor);
        assert_eq!(ed.display_name, "Ed Editor");
        assert!(!ed.must_reset);
        let s = login_session(store.clone(), "ed", "a long enough passphrase").await;
        assert!(s.is_some_and(|s| !s.user.must_reset));
    });
    // A password under the policy is refused.
    let mut c = user_cmd(
        tmp.path(),
        &[
            "create",
            "--login",
            "fay",
            "--email",
            "fay@example.com",
            "--password-stdin",
        ],
    );
    c.write_stdin("short\n");
    let err = c.assert().code(1).get_output().stderr.clone();
    let err = String::from_utf8(err).unwrap();
    assert!(err.contains("password"), "{err}");
}

#[test]
fn user_reset_password_revokes_sessions() {
    let tmp = tempfile::tempdir().expect("tempdir");
    init_sqlite(tmp.path());
    let mut c = user_cmd(
        tmp.path(),
        &[
            "create",
            "--login",
            "ann",
            "--email",
            "ann@example.com",
            "--password-stdin",
        ],
    );
    c.write_stdin("the first passphrase\r\n");
    c.assert().success();
    let token = with_site_store(tmp.path(), async |store| {
        let s = login_session(store, "ann", "the first passphrase")
            .await
            .expect("logged in");
        s.token.expose().to_owned()
    });

    let password = printed_password(&stdout(user_cmd(
        tmp.path(),
        &["reset-password", "--login", "ANN"],
    )));
    with_site_store(tmp.path(), async |store| {
        let auth = ferrite_core::AuthService::new(
            store.clone(),
            ferrite_core::auth::SiteKey::generate().unwrap(),
        );
        assert!(auth.resolve_session(&token).await.unwrap().is_none());
        assert!(
            login_session(store.clone(), "ann", "the first passphrase")
                .await
                .is_none()
        );
        let s = login_session(store, "ann", &password).await;
        assert!(s.is_some_and(|s| s.user.must_reset));
    });

    // With --password-stdin: must_reset is cleared.
    let mut c = user_cmd(
        tmp.path(),
        &["reset-password", "--login", "ann", "--password-stdin"],
    );
    c.write_stdin("the third passphrase\n");
    let out = stdout(c);
    assert!(!out.contains("password:"), "{out}");
    with_site_store(tmp.path(), async |store| {
        let s = login_session(store, "ann", "the third passphrase").await;
        assert!(s.is_some_and(|s| !s.user.must_reset));
    });

    let err = user_cmd(tmp.path(), &["reset-password", "--login", "nobody"])
        .assert()
        .code(1)
        .get_output()
        .stderr
        .clone();
    assert!(String::from_utf8(err).unwrap().contains("not found"));
}

#[test]
fn user_list_output() {
    let tmp = tempfile::tempdir().expect("tempdir");
    init_sqlite(tmp.path());
    for (login, role) in [("zed", "contributor"), ("amy", "author")] {
        user_cmd(
            tmp.path(),
            &[
                "create",
                "--login",
                login,
                "--email",
                &format!("{login}@example.com"),
                "--role",
                role,
            ],
        )
        .assert()
        .success();
    }
    with_site_store(tmp.path(), async |store| {
        let zed = store
            .read()
            .users()
            .get_by_login("zed")
            .await
            .unwrap()
            .unwrap();
        let admin = ferrite_core::UserService::new(store.clone())
            .operator()
            .create(ferrite_core::NewAccount {
                login: "root".into(),
                email: "root@example.com".into(),
                display_name: "Root".into(),
                role: ferrite_domain::Role::Admin,
                password: None,
                must_reset: true,
            })
            .await
            .unwrap();
        ferrite_core::UserService::new(store)
            .update(
                &ferrite_core::Actor::new(admin.id, admin.role),
                zed.id,
                ferrite_core::UserPatch {
                    status: Some(ferrite_domain::UserStatus::Disabled),
                    ..ferrite_core::UserPatch::default()
                },
            )
            .await
            .unwrap();
    });
    let out = stdout(user_cmd(tmp.path(), &["list"]));
    assert_eq!(
        out,
        "amy  author  active  amy@example.com\n\
         root  admin  active  root@example.com\n\
         zed  contributor  disabled  zed@example.com\n"
    );

    // The SQLite folder lock: refused while another process (here: this one) holds it.
    tokio_rt().expect("runtime").block_on(async {
        let config = ferrite_config::load(tmp.path(), Vec::new()).unwrap();
        let store = ferrite::open_store(&config, ferrite::OpenMode::MustExist)
            .await
            .unwrap();
        user_cmd(tmp.path(), &["list"]).assert().code(1);
        store.close().await;
    });
}

#[test]
fn user_disable_totp_clears_the_secret() {
    let tmp = tempfile::tempdir().expect("tempdir");
    init_sqlite(tmp.path());
    user_cmd(
        tmp.path(),
        &["create", "--login", "ann", "--email", "ann@example.com"],
    )
    .assert()
    .success();
    with_site_store(tmp.path(), async |store| {
        let ann = store
            .read()
            .users()
            .get_by_login("ann")
            .await
            .unwrap()
            .unwrap();
        store
            .read()
            .users()
            .set_totp(ann.id, Some(&ferrite_domain::Secret::new("sealed")))
            .await
            .unwrap();
    });
    let out = stdout(user_cmd(tmp.path(), &["disable-totp", "--login", "ann"]));
    assert!(out.contains("ann"), "{out}");
    with_site_store(tmp.path(), async |store| {
        let ann = store
            .read()
            .users()
            .get_by_login("ann")
            .await
            .unwrap()
            .unwrap();
        assert!(ann.totp_secret.is_none());
    });
}

#[test]
fn user_cli_refuses_invalid_login() {
    let tmp = tempfile::tempdir().expect("tempdir");
    init_sqlite(tmp.path());
    for login in ["-bad", "a b", "é", ""] {
        let out = user_cmd(
            tmp.path(),
            &[
                "create",
                &format!("--login={login}"),
                "--email",
                "x@example.com",
            ],
        )
        .assert()
        .code(1)
        .get_output()
        .clone();
        let err = String::from_utf8(out.stderr).unwrap();
        assert!(err.contains("login must be"), "{err}");
        assert!(out.stdout.is_empty(), "no password printed");
    }
    user_cmd(
        tmp.path(),
        &["create", "--login", "ann", "--email", "ann@example.com"],
    )
    .assert()
    .success();
    let err = user_cmd(
        tmp.path(),
        &["create", "--login", "ANN", "--email", "other@example.com"],
    )
    .assert()
    .code(1)
    .get_output()
    .stderr
    .clone();
    assert!(String::from_utf8(err).unwrap().contains("login taken"));
    // Unknown role: a usage error from the argument parser (exit 2).
    user_cmd(
        tmp.path(),
        &[
            "create",
            "--login",
            "x",
            "--email",
            "x@example.com",
            "--role",
            "root",
        ],
    )
    .assert()
    .code(2);
}

// ---- ferrite theme / ferrite plugin (M2a-2 P26) --------------------------------------

fn ext_cmd(dir: &Path, group: &str, args: &[&str]) -> Command {
    let mut c = ferrite();
    c.arg(group).args(args).arg("--data").arg(dir);
    c
}

fn plugin_cmd(dir: &Path, args: &[&str]) -> Command {
    ext_cmd(dir, "plugin", args)
}

fn theme_cmd(dir: &Path, args: &[&str]) -> Command {
    ext_cmd(dir, "theme", args)
}

fn stderr_code(mut cmd: Command, code: i32) -> String {
    let out = cmd.assert().code(code).get_output().stderr.clone();
    String::from_utf8(out).expect("utf-8 stderr")
}

/// A plugin folder: the manifest of `plugins/examples/<manifest_of>` and the component
/// of the example crate `wasm_of` (they differ for the manifest-only `hello-v2`).
fn plugin_dir(parent: &Path, folder: &str, manifest_of: &str, wasm_of: &str) -> std::path::PathBuf {
    let dir = parent.join(folder);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::copy(
        test_support::repo_root()
            .join("plugins/examples")
            .join(manifest_of)
            .join("ferrite-plugin.toml"),
        dir.join("ferrite-plugin.toml"),
    )
    .unwrap();
    std::fs::write(
        dir.join(format!("{wasm_of}.wasm")),
        test_support::plugin_wasm(wasm_of),
    )
    .unwrap();
    dir
}

fn theme_dir(parent: &Path, name: &str, index: &str) -> std::path::PathBuf {
    let dir = parent.join("src").join(name);
    std::fs::create_dir_all(dir.join("templates")).unwrap();
    std::fs::write(
        dir.join("theme.toml"),
        format!("[theme]\nname = \"{name}\"\nversion = \"1.2.3\"\n"),
    )
    .unwrap();
    std::fs::write(dir.join("templates/index.html"), index).unwrap();
    dir
}

fn dir_arg(dir: &Path) -> &str {
    dir.to_str().expect("utf-8 temp path")
}

#[test]
fn plugin_check_prints_report() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let site = tmp.path().join("site");
    init_sqlite(&site);
    let hello = plugin_dir(tmp.path(), "hello", "hello", "hello");
    let out = stdout(plugin_cmd(&site, &["check", "--dir", dir_arg(&hello)]));
    assert!(out.contains("plugin hello 0.1.0 (new)"), "{out}");
    assert!(out.contains("render.head"), "{out}");
    assert!(out.contains("fingerprint: "), "{out}");
    assert!(out.contains("nothing was stored"), "{out}");
    // `render.head` is high risk (ruling R22): a head script runs with the site's origin.
    assert!(out.contains("\n! render.head: "), "{out}");
    // A wildcard read is high risk: its line starts with `!`.
    let probe = plugin_dir(tmp.path(), "probe", "settings-probe", "settings_probe");
    let out = stdout(plugin_cmd(&site, &["check", "--dir", dir_arg(&probe)]));
    assert!(out.contains("\n! options.read"), "{out}");
    // `request.served` is medium risk (R22, M1): its line starts with `~`.
    let counter = plugin_dir(tmp.path(), "counter", "counter", "counter");
    let out = stdout(plugin_cmd(&site, &["check", "--dir", dir_arg(&counter)]));
    assert!(
        out.contains("\n~ hooks: receives every page view") && out.contains("[request.served]"),
        "{out}"
    );
    // Nothing was stored: no row, no artifact.
    assert_eq!(stdout(plugin_cmd(&site, &["list"])), "");
    assert!(!site.join("media/artifacts").exists());

    // Problems exit 1: a manifest that does not parse, a folder without a component.
    std::fs::write(hello.join("ferrite-plugin.toml"), "[plugin\n").unwrap();
    plugin_cmd(&site, &["check", "--dir", dir_arg(&hello)])
        .assert()
        .code(1);
    let empty = tmp.path().join("empty");
    std::fs::create_dir_all(&empty).unwrap();
    std::fs::write(empty.join("ferrite-plugin.toml"), "").unwrap();
    let err = stderr_code(plugin_cmd(&site, &["check", "--dir", dir_arg(&empty)]), 1);
    assert!(err.contains("no *.wasm"), "{err}");
}

#[test]
fn plugin_install_needs_yes_without_tty() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let site = tmp.path().join("site");
    init_sqlite(&site);
    let hello = plugin_dir(tmp.path(), "hello", "hello", "hello");
    let mut c = plugin_cmd(&site, &["install", "--dir", dir_arg(&hello)]);
    c.write_stdin("y\n");
    let assert = c.assert().code(1);
    let out = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
    let err = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
    assert!(err.contains("--yes"), "{err}");
    assert!(
        out.contains("fingerprint: "),
        "the report is shown first: {out}"
    );
    assert_eq!(stdout(plugin_cmd(&site, &["list"])), "");
    assert!(!site.join("media/artifacts").exists(), "nothing uploaded");
}

#[test]
fn plugin_install_enable_list_remove() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let site = tmp.path().join("site");
    init_sqlite(&site);
    let hello = plugin_dir(tmp.path(), "hello", "hello", "hello");
    let reader = plugin_dir(tmp.path(), "reader", "reader", "reader");
    let out = stdout(plugin_cmd(
        &site,
        &["install", "--dir", dir_arg(&hello), "--yes"],
    ));
    assert!(
        out.contains("installed plugin hello 0.1.0 (disabled)"),
        "{out}"
    );
    let out = stdout(plugin_cmd(
        &site,
        &["install", "--dir", dir_arg(&reader), "--yes", "--enable"],
    ));
    assert!(
        out.contains("installed plugin reader 0.1.0 (enabled)"),
        "{out}"
    );
    assert_eq!(
        stdout(plugin_cmd(&site, &["list"])),
        "hello  0.1.0  disabled\nreader  0.1.0  enabled\n"
    );

    let out = stdout(plugin_cmd(&site, &["enable", "hello"]));
    assert!(out.contains("enabled plugin hello"), "{out}");
    stdout(plugin_cmd(&site, &["disable", "reader"]));
    assert_eq!(
        stdout(plugin_cmd(&site, &["list"])),
        "hello  0.1.0  enabled\nreader  0.1.0  disabled\n"
    );
    // An automatic disable shows its reason.
    with_site_store(&site, async |store| {
        store
            .read()
            .plugins()
            .set_enabled("reader", false, Some("too many failures"))
            .await
            .unwrap();
    });
    assert_eq!(
        stdout(plugin_cmd(&site, &["list"])),
        "hello  0.1.0  enabled\nreader  0.1.0  disabled  too many failures\n"
    );

    let out = stdout(plugin_cmd(&site, &["remove", "reader"]));
    assert!(out.contains("removed plugin reader"), "{out}");
    assert_eq!(
        stdout(plugin_cmd(&site, &["list"])),
        "hello  0.1.0  enabled\n"
    );
    for args in [
        ["enable", "reader"],
        ["disable", "reader"],
        ["remove", "reader"],
    ] {
        let err = stderr_code(plugin_cmd(&site, &args), 1);
        assert!(err.contains("plugin not found"), "{err}");
    }
}

#[test]
fn plugin_widening_update_needs_approval() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let site = tmp.path().join("site");
    init_sqlite(&site);
    let v1 = plugin_dir(tmp.path(), "v1", "hello", "hello");
    stdout(plugin_cmd(
        &site,
        &["install", "--dir", dir_arg(&v1), "--yes", "--enable"],
    ));
    let granted_before = with_site_store(&site, async |store| {
        store
            .read()
            .plugins()
            .get("hello")
            .await
            .unwrap()
            .unwrap()
            .granted
    });
    // 0.2.0 also asks for `kv`: without approval it is refused and 0.1.0 keeps running.
    let v2 = plugin_dir(tmp.path(), "v2", "hello-v2", "hello");
    let mut c = plugin_cmd(&site, &["install", "--dir", dir_arg(&v2)]);
    c.write_stdin("");
    let assert = c.assert().code(1);
    let out = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
    assert!(out.contains("plugin hello 0.2.0 (update)"), "{out}");
    assert!(out.contains("WIDENS"), "{out}");
    assert!(out.contains("kv"), "{out}");
    assert_eq!(
        stdout(plugin_cmd(&site, &["list"])),
        "hello  0.1.0  enabled\n"
    );
    with_site_store(&site, async |store| {
        let row = store.read().plugins().get("hello").await.unwrap().unwrap();
        assert_eq!(row.granted, granted_before);
    });
    // `check` prints the same fingerprint for the same folder.
    let checked = stdout(plugin_cmd(&site, &["check", "--dir", dir_arg(&v2)]));
    let expected = checked
        .lines()
        .find_map(|l| l.strip_prefix("fingerprint: "))
        .expect("fingerprint line")
        .to_owned();
    // --yes approves exactly the report it printed.
    let out = stdout(plugin_cmd(
        &site,
        &["install", "--dir", dir_arg(&v2), "--yes"],
    ));
    assert!(out.contains("WIDENS"), "{out}");
    let fingerprint = out
        .lines()
        .find_map(|l| l.strip_prefix("fingerprint: "))
        .expect("fingerprint line")
        .to_owned();
    assert_eq!(fingerprint.len(), 64, "{fingerprint}");
    assert_eq!(
        fingerprint, expected,
        "the printed report is the approved one"
    );
    assert_eq!(
        stdout(plugin_cmd(&site, &["list"])),
        "hello  0.2.0  enabled\n"
    );
    with_site_store(&site, async |store| {
        let row = store.read().plugins().get("hello").await.unwrap().unwrap();
        assert_eq!(row.granted["capabilities"]["kv"], true, "{}", row.granted);
    });
    // The same version again asks for nothing more.
    let out = stdout(plugin_cmd(
        &site,
        &["install", "--dir", dir_arg(&v2), "--yes"],
    ));
    assert!(out.contains("nothing beyond"), "{out}");
    assert!(!out.contains("WIDENS"), "{out}");
}

#[test]
fn plugin_remove_keeps_data_without_purge() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let site = tmp.path().join("site");
    init_sqlite(&site);
    let probe = plugin_dir(tmp.path(), "probe", "settings-probe", "settings_probe");
    stdout(plugin_cmd(
        &site,
        &["install", "--dir", dir_arg(&probe), "--yes", "--enable"],
    ));
    let title = "plugin.settings-probe.settings.title";
    let secret = "plugin.settings-probe.settings.api_key";
    let marker = "plugin.settings-probe.deleted_at";
    with_site_store(&site, async |store| {
        let options = store.read().options();
        options
            .set(title, &serde_json::json!("Hi"), false)
            .await
            .unwrap();
        options
            .set(secret, &serde_json::json!("SEKRET-VALUE"), false)
            .await
            .unwrap();
    });
    let has = |name: &'static str| {
        with_site_store(&site, async move |store| {
            store.read().options().get(name).await.unwrap().is_some()
        })
    };

    // Without --purge the data stays, with a deletion marker for the purge job.
    let out = stdout(plugin_cmd(&site, &["remove", "settings-probe"]));
    assert!(out.contains("30 days"), "{out}");
    assert!(has(title) && has(secret) && has(marker));
    assert_eq!(stdout(plugin_cmd(&site, &["list"])), "");
    let queued = with_site_store(&site, async |store| {
        store.read().jobs().counts().await.unwrap()[&ferrite_domain::JobStatus::Queued]
    });
    assert_eq!(queued, 1, "the 30-day purge job");
    let day = 24 * 3600;
    let jobs = purge_jobs(&site);
    assert_eq!(jobs.len(), 1, "{jobs:?}");
    let ahead = jobs[0] - now_micros();
    assert!(
        (29 * day * 1_000_000..=31 * day * 1_000_000).contains(&ahead),
        "due at the retention date: {ahead}"
    );

    // A reinstall finds the data (and clears the marker); --purge deletes it at once.
    stdout(plugin_cmd(
        &site,
        &["install", "--dir", dir_arg(&probe), "--yes"],
    ));
    assert!(has(title) && !has(marker));
    let out = stdout(plugin_cmd(&site, &["remove", "settings-probe", "--purge"]));
    assert!(out.contains("purged"), "{out}");
    assert!(!has(title) && !has(secret));
    // R5: the marker stays, and the delete's own job runs again after the grace.
    assert!(has(marker), "the deletion marker stays after --purge");
    let jobs = purge_jobs(&site);
    assert_eq!(jobs.len(), 2, "{jobs:?}");
    assert!(
        jobs.iter()
            .any(|at| (60_000_000..=180_000_000).contains(&(at - now_micros()))),
        "a second purge about 2 minutes out: {jobs:?}"
    );
    // Setting values are never printed.
    assert!(!out.contains("SEKRET-VALUE"), "{out}");
}

#[test]
fn theme_install_activate_list() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let site = tmp.path().join("site");
    init_sqlite(&site);
    // Before any row the embedded default is the active theme.
    let out = stdout(theme_cmd(&site, &["list"]));
    assert!(out.contains("blog  "), "{out}");
    assert!(
        out.lines()
            .any(|l| l.starts_with("blog  ") && l.ends_with("  embedded  active")),
        "{out}"
    );

    let mine = theme_dir(tmp.path(), "mine", "mine index");
    let out = stdout(theme_cmd(&site, &["install", "--dir", dir_arg(&mine)]));
    assert!(out.contains("installed theme mine 1.2.3"), "{out}");
    assert!(!out.contains("(active)"), "{out}");
    let out = stdout(theme_cmd(&site, &["list"]));
    assert!(out.contains("mine  1.2.3  installed\n"), "{out}");
    assert!(
        out.contains("blog  ") && out.contains("embedded  active"),
        "{out}"
    );

    stdout(theme_cmd(&site, &["activate", "mine"]));
    let out = stdout(theme_cmd(&site, &["list"]));
    assert!(out.contains("mine  1.2.3  installed  active\n"), "{out}");
    assert!(!out.contains("embedded  active"), "{out}");

    // An embedded theme is installed from the embedded files and activated.
    stdout(theme_cmd(&site, &["activate", "blog"]));
    let out = stdout(theme_cmd(&site, &["list"]));
    assert!(
        out.lines()
            .any(|l| l.starts_with("blog  ") && l.ends_with("  installed  active")),
        "{out}"
    );
    assert!(out.contains("mine  1.2.3  installed\n"), "{out}");

    // --activate installs and activates in one step.
    let other = theme_dir(tmp.path(), "other", "other index");
    let out = stdout(theme_cmd(
        &site,
        &["install", "--dir", dir_arg(&other), "--activate"],
    ));
    assert!(
        out.contains("installed theme other 1.2.3 (active)"),
        "{out}"
    );

    let err = stderr_code(theme_cmd(&site, &["activate", "nope"]), 1);
    assert!(err.contains("theme not found"), "{err}");
}

#[test]
fn theme_activate_refuses_broken() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let site = tmp.path().join("site");
    init_sqlite(&site);
    stdout(theme_cmd(
        &site,
        &[
            "install",
            "--dir",
            dir_arg(&theme_dir(tmp.path(), "good", "good")),
            "--activate",
        ],
    ));
    stdout(theme_cmd(
        &site,
        &[
            "install",
            "--dir",
            dir_arg(&theme_dir(tmp.path(), "bad", "bad")),
        ],
    ));
    // Tamper with the stored bundle of `bad`.
    let hash = with_site_store(&site, async |store| {
        store
            .read()
            .themes()
            .get("bad")
            .await
            .unwrap()
            .unwrap()
            .artifact_hash
    });
    let h = hash.as_str();
    std::fs::write(
        site.join("media/artifacts").join(&h[..2]).join(h),
        b"not a tar",
    )
    .unwrap();
    let err = stderr_code(theme_cmd(&site, &["activate", "bad"]), 1);
    assert!(err.contains("does not load"), "{err}");
    let out = stdout(theme_cmd(&site, &["list"]));
    assert!(out.contains("good  1.2.3  installed  active\n"), "{out}");
    assert!(out.contains("bad  1.2.3  installed\n"), "{out}");
    // A theme that does not compile is refused at install.
    let broken = theme_dir(tmp.path(), "broken", "{% if %}");
    theme_cmd(&site, &["install", "--dir", dir_arg(&broken)])
        .assert()
        .code(1);
}

#[test]
fn extension_commands_fail_while_the_lock_is_held() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let site = tmp.path().join("site");
    init_sqlite(&site);
    let hello = plugin_dir(tmp.path(), "hello", "hello", "hello");
    let mine = theme_dir(tmp.path(), "mine", "mine");
    tokio_rt().expect("runtime").block_on(async {
        let config = ferrite_config::load(&site, Vec::new()).unwrap();
        let store = ferrite::open_store(&config, ferrite::OpenMode::MustExist)
            .await
            .unwrap();
        for args in [
            vec!["list"],
            vec!["check", "--dir", dir_arg(&hello)],
            vec!["install", "--dir", dir_arg(&hello), "--yes"],
            vec!["enable", "hello"],
            vec!["disable", "hello"],
            vec!["remove", "hello"],
        ] {
            plugin_cmd(&site, &args).assert().code(1);
        }
        for args in [
            vec!["list"],
            vec!["install", "--dir", dir_arg(&mine)],
            vec!["activate", "blog"],
        ] {
            theme_cmd(&site, &args).assert().code(1);
        }
        store.close().await;
    });
    // Nothing was written while locked out; the commands work again afterwards. A
    // `ferrite` child a sibling test spawned while this process held the lock can keep a
    // copy of the lock fd until its exec completes (see `with_site_store`), so allow a
    // short wait for the lock to come free.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let out = loop {
        let out = plugin_cmd(&site, &["list"])
            .output()
            .expect("run plugin list");
        if out.status.success() || std::time::Instant::now() >= deadline {
            break out;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    assert!(out.status.success(), "{out:?}");
    assert_eq!(String::from_utf8(out.stdout).expect("utf-8 stdout"), "");
}

fn now_micros() -> i64 {
    let d = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap();
    i64::try_from(d.as_micros()).unwrap()
}

/// `run_at` (unix micros) of every `plugin.purge` job, read from the SQLite file.
fn purge_jobs(site: &Path) -> Vec<i64> {
    tokio_rt().expect("runtime").block_on(async {
        let mut conn =
            SqliteConnection::connect(&format!("sqlite://{}", site.join("ferrite.db").display()))
                .await
                .unwrap();
        let rows: Vec<(i64,)> =
            sqlx::query_as("SELECT run_at FROM jobs WHERE kind = 'plugin.purge' ORDER BY run_at")
                .fetch_all(&mut conn)
                .await
                .unwrap();
        rows.into_iter().map(|r| r.0).collect()
    })
}

#[test]
fn plugin_install_refuses_bad_packages() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let site = tmp.path().join("site");
    init_sqlite(&site);
    let install = |dir: &Path| {
        stderr_code(
            plugin_cmd(&site, &["install", "--dir", dir_arg(dir), "--yes"]),
            1,
        )
    };
    // Two components.
    let two = plugin_dir(tmp.path(), "two", "hello", "hello");
    std::fs::write(two.join("other.wasm"), b"x").unwrap();
    assert!(install(&two).contains("more than one"));
    // An oversize component is refused before it is read.
    let big = tmp.path().join("big");
    std::fs::create_dir_all(&big).unwrap();
    std::fs::copy(
        two.join("ferrite-plugin.toml"),
        big.join("ferrite-plugin.toml"),
    )
    .unwrap();
    let f = std::fs::File::create(big.join("big.wasm")).unwrap();
    f.set_len(ferrite_http::extensions::MAX_PLUGIN_BYTES + 1)
        .unwrap();
    assert!(install(&big).contains("larger than"));
    // A component importing more than the manifest declares.
    let mismatch = plugin_dir(tmp.path(), "mismatch", "hello", "reader");
    install(&mismatch);
    // A directory that does not exist.
    assert!(install(&tmp.path().join("missing")).contains("cannot read"));
    // Nothing was stored by any of them.
    assert_eq!(stdout(plugin_cmd(&site, &["list"])), "");
    assert!(!site.join("media/artifacts").exists());
}

// ---- seed demo profiles (M4-3b P61), on both backends ----

/// An `init`ed site: SQLite, or Postgres in `pg`'s schema (local media either way).
fn seed_site(dir: &Path, pg: Option<&PgSchema>) {
    let mut c = ferrite();
    c.args(["init", "--url", "https://example.com"]);
    if let Some(pg) = pg {
        c.args(["--db", &pg.url, "--schema", &pg.schema]);
    }
    c.arg("--data").arg(dir).assert().success();
}

/// What `seed demo` prints after its summary while `admin` has no password
/// (`seed_site` uses `https://example.com`).
fn sign_in_hint(dir: &Path) -> String {
    let d = dir.display();
    format!(
        "The admin user has no password yet. To sign in at https://example.com/admin/, set one \
         (stop `ferrite serve` first):\n  \
         ferrite user reset-password --data {d} --login admin\n\
         or create your own account:\n  \
         ferrite user create --data {d} --login me --email me@example.com\n"
    )
}

#[test]
fn seed_hint_goes_once_admin_has_a_password() {
    on_both_backends(|dir, pg| {
        seed_site(dir, pg);
        stdout(seed_cmd(dir, &[]));
        let mut c = user_cmd(
            dir,
            &["reset-password", "--login", "admin", "--password-stdin"],
        );
        c.write_stdin("a demo passphrase\n");
        stdout(c);
        let out = stdout(seed_cmd(dir, &["--force"]));
        assert!(out.starts_with("Seeded demo content:"), "{out}");
        assert!(!out.contains("no password"), "{out}");
    });
}

fn seed_cmd(dir: &Path, extra: &[&str]) -> Command {
    let mut c = ferrite();
    c.args(["seed", "demo"]).args(extra).arg("--data").arg(dir);
    c
}

/// Runs `f` against the site's store (SQLite through [`with_site_store`], else `pg`).
fn with_seeded_store<T>(
    dir: &Path,
    pg: Option<&PgSchema>,
    f: impl AsyncFnOnce(std::sync::Arc<dyn ferrite_store::Store>) -> T,
) -> T {
    let Some(pg) = pg else {
        return with_site_store(dir, f);
    };
    tokio_rt().expect("runtime").block_on(async {
        let store: std::sync::Arc<dyn ferrite_store::Store> = std::sync::Arc::new(
            ferrite_store_postgres::PgStore::open(ferrite_store_postgres::PgOptions {
                max_connections: 2,
                schema: Some(pg.schema.clone()),
                ..ferrite_store_postgres::PgOptions::new(pg.url.clone())
            })
            .await
            .unwrap(),
        );
        let out = f(store.clone()).await;
        store.close().await;
        out
    })
}

/// Each backend: SQLite always, Postgres when `FERRITE_TEST_PG_URL` is set.
fn on_both_backends(check: impl Fn(&Path, Option<&PgSchema>)) {
    let tmp = tempfile::tempdir().expect("tempdir");
    check(&tmp.path().join("sqlite"), None);
    if let Some(pg) = PgSchema::new() {
        check(&tmp.path().join("pg"), Some(&pg));
    }
}

fn content_query(kind: &str) -> ferrite_domain::ContentQuery {
    ferrite_domain::ContentQuery {
        kind: Some(kind.into()),
        limit: 100,
        ..Default::default()
    }
}

#[test]
fn seed_business_profile() {
    on_both_backends(|dir, pg| {
        seed_site(dir, pg);
        let out = stdout(seed_cmd(dir, &["--profile", "business"]));
        assert!(
            out.contains("Seeded business demo content: 3 posts, 6 pages"),
            "{out}"
        );
        let next = format!(
            "next: ferrite theme activate business --data {}\n",
            dir.display()
        );
        assert!(out.contains(&next), "{out}");
        assert!(out.ends_with(&sign_in_hint(dir)), "{out}");

        with_seeded_store(dir, pg, async |store| {
            let read = store.read();
            let home = read
                .content()
                .get_by_slug("page", None, "home")
                .await
                .unwrap()
                .expect("the front page");
            assert_eq!(home.title, "Gardens that grow with you");
            assert_eq!(home.status, ferrite_domain::ContentStatus::Published);
            assert!(
                home.fields["featured_image"].is_string(),
                "Home has a featured image: {}",
                home.fields
            );
            let front = read
                .options()
                .get("reading.front_page")
                .await
                .unwrap()
                .expect("front page set");
            assert_eq!(front.value, serde_json::json!(home.id.to_string()));

            let mut q = content_query("page");
            q.parent = ferrite_domain::ParentFilter::Of(home.id);
            let mut kids = read.content().list(&q).await.unwrap().items;
            kids.sort_by_key(|k| k.menu_order);
            let kids: Vec<(String, i32, bool)> = kids
                .into_iter()
                .map(|k| (k.title, k.menu_order, !k.excerpt.is_empty()))
                .collect();
            assert_eq!(
                kids,
                vec![
                    ("Garden design".to_owned(), 1, true),
                    ("Consulting".to_owned(), 2, true),
                    ("Aftercare".to_owned(), 3, true),
                ]
            );
            for slug in ["about", "contact"] {
                let p = read
                    .content()
                    .get_by_slug("page", None, slug)
                    .await
                    .unwrap();
                assert!(p.is_some(), "page {slug}");
            }
            let pages = read.content().count(&content_query("page")).await.unwrap();
            let posts = read.content().count(&content_query("post")).await.unwrap();
            assert_eq!((pages, posts), (6, 3));

            let mut menus: Vec<String> = read
                .menus()
                .list()
                .await
                .unwrap()
                .into_iter()
                .map(|(n, _)| n)
                .collect();
            menus.sort();
            assert_eq!(menus, ["footer", "primary"]);
            let primary = read.menus().get("primary").await.unwrap().unwrap();
            let labels: Vec<&str> = primary.items.iter().map(|i| i.label.as_str()).collect();
            assert_eq!(labels, ["Home", "About", "News", "Contact"]);
            let footer = read.menus().get("footer").await.unwrap().unwrap();
            let labels: Vec<&str> = footer.items.iter().map(|i| i.label.as_str()).collect();
            assert_eq!(labels, ["Garden design", "About", "Contact", "RSS feed"]);
            let design = read
                .content()
                .get_by_slug("page", Some(home.id), "garden-design")
                .await
                .unwrap()
                .expect("garden design");
            assert_eq!(footer.items[0].target_id, Some(design.id.as_uuid()));
            let news = primary.items.iter().find(|i| i.label == "News").unwrap();
            assert_eq!(news.url.as_deref(), Some("/post/"));

            let title = read.options().get("site.title").await.unwrap().unwrap();
            assert_eq!(title.value, serde_json::json!("Larkspur Studio"));
        });
    });
}

#[test]
fn seed_default_profile_is_unchanged() {
    const LINE: &str =
        "Seeded demo content: 12 posts, 3 pages, 4 terms, 2 menus, 1 image (admin user: admin).\n";
    on_both_backends(|dir, pg| {
        seed_site(dir, pg);
        assert_eq!(
            stdout(seed_cmd(dir, &[])),
            format!("{LINE}{}", sign_in_hint(dir))
        );
        with_seeded_store(dir, pg, async |store| {
            let read = store.read();
            let posts = read
                .content()
                .list(&content_query("post"))
                .await
                .unwrap()
                .items;
            assert_eq!(posts.len(), 12);
            assert!(posts.iter().any(|p| p.title == "Welcome to FerriteCMS"));
            // One body, byte for byte as before M4-3b.
            let hello = posts.iter().find(|p| p.slug == "hello-world").unwrap();
            let text = |t: &str| serde_json::json!({ "type": "text", "text": t });
            let para = |t: &str| serde_json::json!({ "type": "paragraph", "content": [text(t)] });
            assert_eq!(
                hello.body,
                serde_json::json!({ "type": "doc", "content": [
                    para("Every CMS starts with a hello world post. This is ours."),
                    { "type": "heading", "attrs": { "level": 2 }, "content": [text("What you get")] },
                    { "type": "bulletList", "content": [
                        { "type": "listItem", "content": [para("A single binary and a single data folder")] },
                        { "type": "listItem", "content": [para("Plugins in a capability-scoped sandbox")] },
                        { "type": "listItem", "content": [para("Themes that are templates, not programs")] },
                    ]},
                    para("Try it on your own machine:"),
                    { "type": "codeBlock", "attrs": { "language": "sh" }, "content": [
                        text("ferrite init --data ./site --url http://localhost:8080\nferrite seed demo --data ./site\nferrite serve --data ./site")
                    ]},
                    { "type": "blockquote", "content": [
                        para("Plugins are untrusted. They get only what their manifest declares.")
                    ]},
                    { "type": "paragraph", "content": [
                        text("Read more in "),
                        { "type": "text", "text": "the about page", "marks": [{ "type": "link", "attrs": { "href": "/about/" } }] },
                        text("."),
                    ]},
                ]})
            );
            // The sample image: the same PNG (its SHA-256 is the artifact hash).
            let image = read
                .content()
                .list(&content_query("media"))
                .await
                .unwrap()
                .items;
            let row = read.media().get(image[0].id).await.unwrap().unwrap();
            assert_eq!(
                row.artifact_hash.as_str(),
                "e61905d85f1a6e2d40815f7ad69c4b8f1fdb0a2e9133e69c3539504541b247d0"
            );
            assert_eq!(row.bytes, 338_103);
            let mut pages: Vec<String> = read
                .content()
                .list(&content_query("page"))
                .await
                .unwrap()
                .items
                .into_iter()
                .map(|p| p.title)
                .collect();
            pages.sort();
            assert_eq!(pages, ["About", "Contact", "Team"]);
            assert_eq!(
                read.content().count(&content_query("media")).await.unwrap(),
                1
            );
            let title = read.options().get("site.title").await.unwrap().unwrap();
            assert_eq!(title.value, serde_json::json!("FerriteCMS Demo"));
            assert!(
                read.options()
                    .get("reading.front_page")
                    .await
                    .unwrap()
                    .is_none()
            );
            assert_eq!(read.menus().list().await.unwrap().len(), 2);
        });
        // `--profile blog` is the same profile.
        assert_eq!(
            stdout(seed_cmd(dir, &["--profile", "blog", "--force"])),
            format!("{LINE}{}", sign_in_hint(dir))
        );
    });
}

#[test]
fn seed_unknown_profile_is_a_usage_error() {
    let tmp = tempfile::tempdir().expect("tempdir");
    init_sqlite(tmp.path());
    let out = seed_cmd(tmp.path(), &["--profile", "docs"])
        .assert()
        .code(2)
        .get_output()
        .clone();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("blog") && err.contains("business"), "{err}");
    // Nothing was written.
    let n = with_site_store(tmp.path(), async |store| {
        store
            .read()
            .content()
            .count(&ferrite_domain::ContentQuery::default())
            .await
            .unwrap()
    });
    assert_eq!(n, 0);
}
