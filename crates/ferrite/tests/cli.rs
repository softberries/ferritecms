#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;

use assert_cmd::Command;
use assert_cmd::cargo::cargo_bin_cmd;
use sqlx::{AssertSqlSafe, Connection, PgConnection, SqliteConnection};

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
    assert!(out.contains("database version: 4"), "{out}");
    assert!(
        out.contains(&format!("(ferrite {})", env!("CARGO_PKG_VERSION"))),
        "{out}"
    );
    assert!(out.contains("pending migrations: none"), "{out}");
    assert!(out.contains("verdict: up to date"), "{out}");

    // 0004's down is lossy: refused without --allow-lossy.
    ferrite()
        .args(["migrate", "down", "--to", "3", "--yes", "--data"])
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
    assert!(stdout(c).contains("Reverted migrations: 4, 3"));
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
    assert!(out.contains("verdict: needs migrate up"), "{out}");

    let mut c = ferrite();
    c.args(["migrate", "up", "--data"]).arg(tmp.path());
    assert!(stdout(c).contains("Applied migrations: 3, 4"));
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
    assert!(out.contains("database version: 4"), "{out}");
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
    assert!(out.contains("database version: 4"), "{out}");
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
    assert!(stdout(c).contains("Reverted migrations: 4, 3"));
    let mut c = ferrite();
    c.args(["migrate", "status", "--data"]).arg(tmp.path());
    assert!(stdout_code(c, 2).contains("verdict: needs migrate up"));
    let mut c = ferrite();
    c.args(["migrate", "up", "--data"]).arg(tmp.path());
    assert!(stdout(c).contains("Applied migrations: 3, 4"));
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
        "ok: mode=solo database=sqlite media=local workers=3"
    );
    // Never touches the database or media folder.
    assert!(!tmp.path().join("ferrite.db").exists());
    assert!(!tmp.path().join("media").exists());
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
            .env("FERRITE_CLUSTER__WORKERS", "2");
        c
    });
    // auto + Postgres + S3 resolves to cluster.
    assert_eq!(
        out.trim(),
        "ok: mode=cluster database=postgres media=s3 workers=2"
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
