#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;

use assert_cmd::Command;
use assert_cmd::cargo::cargo_bin_cmd;
use sqlx::{AssertSqlSafe, Connection, PgConnection};

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

fn stdout(mut cmd: Command) -> String {
    let out = cmd.assert().success().get_output().stdout.clone();
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
    }
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
    assert!(out.contains("database version: 2"), "{out}");
    assert!(
        out.contains(&format!("(ferrite {})", env!("CARGO_PKG_VERSION"))),
        "{out}"
    );
    assert!(out.contains("pending migrations: none"), "{out}");
    assert!(out.contains("verdict: up to date"), "{out}");

    // Revert the newest migration: it becomes pending with its directives.
    let mut c = ferrite();
    c.args(["migrate", "down", "--to", "1", "--yes", "--data"])
        .arg(tmp.path());
    assert!(stdout(c).contains("Reverted migrations: 2"));
    let out = stdout({
        let mut c = ferrite();
        c.args(["migrate", "status", "--data"]).arg(tmp.path());
        c
    });
    assert!(out.contains("database version: 1"), "{out}");
    assert!(
        out.contains("0002 search (phase=expand, min-binary=-, lossy-down=no)"),
        "{out}"
    );
    assert!(out.contains("verdict: needs migrate up"), "{out}");

    let mut c = ferrite();
    c.args(["migrate", "up", "--data"]).arg(tmp.path());
    assert!(stdout(c).contains("Applied migrations: 2"));
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
    assert!(out.contains("database version: 2"), "{out}");
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

    let status = |tmp: &Path| {
        let mut c = ferrite();
        c.args(["migrate", "status", "--data"]).arg(tmp);
        stdout(c)
    };
    let out = status(tmp.path());
    assert!(out.contains("database version: 2"), "{out}");
    assert!(out.contains("verdict: up to date"), "{out}");

    let mut c = ferrite();
    c.args(["migrate", "down", "--to", "1", "--yes", "--data"])
        .arg(tmp.path());
    assert!(stdout(c).contains("Reverted migrations: 2"));
    assert!(status(tmp.path()).contains("verdict: needs migrate up"));
    let mut c = ferrite();
    c.args(["migrate", "up", "--data"]).arg(tmp.path());
    assert!(stdout(c).contains("Applied migrations: 2"));
    let mut c = ferrite();
    c.args(["migrate", "verify", "--data"]).arg(tmp.path());
    assert!(stdout(c).contains("checksums OK"));
}
