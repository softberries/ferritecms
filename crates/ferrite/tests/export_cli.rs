//! `ferrite export`, `ferrite import-site` and `ferrite db copy` (M4-1 Task 5, P5–P8): the
//! archive round trip on SQLite and into Postgres, stdin and stdout, the refusals (a
//! database that is not fresh, a newer schema or format, a tampered table, a hostile path,
//! a truncated stream), the migrate dance into a newer fresh target, the TOTP hint, `db
//! copy` and the archive's file mode.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::Read;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Output;

use assert_cmd::Command;
use assert_cmd::cargo::cargo_bin_cmd;
use ferrite::{NodeOptions, OpenMode, open_store, start};
use ferrite_store_tests::TestDb;
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::{AssertSqlSafe, Connection, PgConnection, SqliteConnection};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn ferrite() -> Command {
    let mut cmd = cargo_bin_cmd!("ferrite");
    cmd.env_clear();
    cmd
}

fn text(out: &Output) -> String {
    format!(
        "status {:?}\nstdout:\n{}\nstderr:\n{}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

fn run(mut cmd: Command) -> Output {
    cmd.output().unwrap()
}

fn ok(cmd: Command) -> Output {
    let out = run(cmd);
    assert!(out.status.success(), "{}", text(&out));
    out
}

fn block_on<F: std::future::Future>(f: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(f)
}

fn init(data: &Path) {
    let mut c = ferrite();
    c.args(["init", "--url", "http://127.0.0.1:8080", "--data"])
        .arg(data);
    ok(c);
}

/// `init` + `seed demo` in `<tmp>/<name>`.
fn seeded(tmp: &Path, name: &str) -> PathBuf {
    let data = tmp.join(name);
    init(&data);
    let mut c = ferrite();
    c.args(["seed", "demo", "--data"]).arg(&data);
    ok(c);
    data
}

fn export(data: &Path, out: &Path, with_media: bool) -> Output {
    let mut c = ferrite();
    c.args(["export", "--data"]).arg(data).arg("--out").arg(out);
    if with_media {
        c.arg("--with-media");
    }
    run(c)
}

fn import_site(data: &Path, archive: &Path, extra: &[&str]) -> Output {
    let mut c = ferrite();
    c.arg("import-site").arg(archive).arg("--data").arg(data);
    c.args(extra);
    run(c)
}

async fn sqlite(data: &Path) -> SqliteConnection {
    SqliteConnection::connect(&format!("sqlite://{}", data.join("ferrite.db").display()))
        .await
        .unwrap()
}

fn count(data: &Path, table: &str) -> i64 {
    block_on(async {
        let mut conn = sqlite(data).await;
        sqlx::query_scalar(AssertSqlSafe(format!("SELECT count(*) FROM {table}")))
            .fetch_one(&mut conn)
            .await
            .unwrap()
    })
}

fn exec(data: &Path, sql: &str) {
    let sql = sql.to_owned();
    block_on(async {
        let mut conn = sqlite(data).await;
        sqlx::query(AssertSqlSafe(sql))
            .execute(&mut conn)
            .await
            .unwrap();
    });
}

fn schema_version(data: &Path) -> i64 {
    block_on(async {
        let mut conn = sqlite(data).await;
        sqlx::query_scalar("SELECT max(version) FROM _sqlx_migrations")
            .fetch_one(&mut conn)
            .await
            .unwrap()
    })
}

/// Every entry of an archive, in order.
fn unpack(bytes: &[u8]) -> Vec<(String, Vec<u8>)> {
    let decoder = zstd::stream::read::Decoder::new(bytes).unwrap();
    let mut archive = tar::Archive::new(decoder);
    let mut out = Vec::new();
    for entry in archive.entries().unwrap() {
        let mut entry = entry.unwrap();
        let path = String::from_utf8(entry.path_bytes().into_owned()).unwrap();
        let mut data = Vec::new();
        entry.read_to_end(&mut data).unwrap();
        out.push((path, data));
    }
    out
}

/// Packs entries as a `tar.zst`, writing each name into the header as given (no checks,
/// so hostile paths can be built).
fn pack(entries: &[(String, Vec<u8>)]) -> Vec<u8> {
    let encoder = zstd::stream::write::Encoder::new(Vec::new(), 3).unwrap();
    let mut builder = tar::Builder::new(encoder);
    for (path, data) in entries {
        let mut header = tar::Header::new_gnu();
        let name = &mut header.as_gnu_mut().unwrap().name;
        name[..path.len()].copy_from_slice(path.as_bytes());
        header.set_size(data.len() as u64);
        header.set_mode(0o600);
        header.set_entry_type(tar::EntryType::Regular);
        header.set_cksum();
        builder.append(&header, data.as_slice()).unwrap();
    }
    builder.into_inner().unwrap().finish().unwrap()
}

fn hex_sha(data: &[u8]) -> String {
    Sha256::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn manifest(entries: &[(String, Vec<u8>)]) -> Value {
    assert_eq!(entries[0].0, "manifest.json");
    serde_json::from_slice(&entries[0].1).unwrap()
}

/// Replaces the manifest (entry 0) with `m`.
fn set_manifest(entries: &mut [(String, Vec<u8>)], m: &Value) {
    entries[0].1 = serde_json::to_vec(m).unwrap();
}

/// Sets the manifest's sha256 and row count of table file `name` to match its content.
fn reseal(entries: &mut [(String, Vec<u8>)], name: &str) {
    let path = format!("tables/{name}.ndjson");
    let data = entries.iter().find(|(p, _)| *p == path).unwrap().1.clone();
    let rows = data
        .split(|b| *b == b'\n')
        .filter(|l| !l.is_empty())
        .count();
    let mut m = manifest(entries);
    for t in m["tables"].as_array_mut().unwrap() {
        if t["name"] == name {
            t["sha256"] = Value::from(hex_sha(&data));
            t["rows"] = Value::from(rows);
        }
    }
    set_manifest(entries, &m);
}

/// `GET path`: status and body.
async fn get(addr: SocketAddr, path: &str) -> (u16, Vec<u8>) {
    let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
    s.write_all(
        format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n").as_bytes(),
    )
    .await
    .unwrap();
    let mut raw = Vec::new();
    s.read_to_end(&mut raw).await.unwrap();
    let split = raw.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    let head = String::from_utf8_lossy(&raw[..split]).into_owned();
    let status = head.split(' ').nth(1).and_then(|c| c.parse().ok()).unwrap();
    (status, raw[split + 4..].to_vec())
}

/// A shared fixture: a seeded site and its export with media.
struct Exported {
    tmp: tempfile::TempDir,
    source: PathBuf,
    archive: PathBuf,
}

fn exported() -> Exported {
    let tmp = tempfile::tempdir().unwrap();
    let source = seeded(tmp.path(), "source");
    let archive = tmp.path().join("site.tar.zst");
    let out = export(&source, &archive, true);
    assert!(out.status.success(), "{}", text(&out));
    Exported {
        tmp,
        source,
        archive,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn export_import_site_round_trip_sqlite() {
    let ex = tokio::task::spawn_blocking(exported).await.unwrap();
    let entries = unpack(&std::fs::read(&ex.archive).unwrap());
    let m = manifest(&entries);
    assert_eq!(m["format"], 1);
    assert_eq!(m["kind"], "export");
    assert_eq!(m["source_backend"], "sqlite");
    assert!(m["artifacts"]["count"].as_u64().unwrap() > 0, "{m}");
    assert!(
        entries
            .iter()
            .any(|(p, _)| p == "tables/content_items.ndjson")
    );
    assert!(entries.iter().any(|(p, _)| p.starts_with("artifacts/")));
    assert!(!entries.iter().any(|(p, _)| p == "tables/sessions.ndjson"));

    let target = ex.tmp.path().join("target");
    let (t2, a2) = (target.clone(), ex.archive.clone());
    let out = tokio::task::spawn_blocking(move || {
        init(&t2);
        import_site(&t2, &a2, &["--with-media"])
    })
    .await
    .unwrap();
    assert!(out.status.success(), "{}", text(&out));
    let source = ex.source.clone();
    let t3 = target.clone();
    tokio::task::spawn_blocking(move || {
        for table in [
            "content_items",
            "users",
            "terms",
            "media",
            "artifacts",
            "themes",
        ] {
            assert_eq!(count(&source, table), count(&t3, table), "{table}");
        }
        // The site id travels; search is rebuilt.
        let site_id = |d: &Path| {
            block_on(async {
                let mut conn = sqlite(d).await;
                sqlx::query_scalar::<_, String>(
                    "SELECT value FROM schema_meta WHERE key = 'site_id'",
                )
                .fetch_one(&mut conn)
                .await
                .unwrap()
            })
        };
        assert_eq!(site_id(&source), site_id(&t3));
        assert_eq!(count(&source, "search_docs"), count(&t3, "search_docs"));
        assert!(count(&t3, "search_docs") > 0);
    })
    .await
    .unwrap();

    // Served: the home page lists a post, the post and a media file answer.
    let config = ferrite_config::load(
        &target,
        vec![(
            "FERRITE_SERVER__LISTEN".to_owned(),
            "127.0.0.1:0".to_owned(),
        )],
    )
    .unwrap();
    let store = open_store(&config, OpenMode::MustExist).await.unwrap();
    let node = start(&config, store.clone(), NodeOptions::default())
        .await
        .unwrap();
    let (status, body) = get(node.addr, "/").await;
    let body = String::from_utf8_lossy(&body).into_owned();
    assert_eq!(status, 200, "{body}");
    assert!(body.contains("Welcome to FerriteCMS"), "{body}");
    let href = body
        .split("href=\"")
        .skip(1)
        .map(|s| s.split('"').next().unwrap())
        .find(|h| h.contains("welcome-to-ferritecms"))
        .unwrap()
        .replace("&#x2f;", "/")
        .replace("http://127.0.0.1:8080", "");
    let (status, _) = get(node.addr, &href).await;
    assert_eq!(status, 200, "{href}");
    let (hash, name, bytes): (String, String, i64) = {
        let mut conn = sqlite(&target).await;
        sqlx::query_as("SELECT artifact_hash, original_name, bytes FROM media")
            .fetch_one(&mut conn)
            .await
            .unwrap()
    };
    let (status, body) = get(node.addr, &format!("/_media/{hash}/{name}")).await;
    assert_eq!(status, 200);
    assert_eq!(body.len() as i64, bytes);
    node.shutdown().await.unwrap();
    store.close().await;
}

/// Table files of an archive, by path.
fn tables(bytes: &[u8]) -> Vec<(String, Vec<u8>)> {
    unpack(bytes)
        .into_iter()
        .filter(|(p, _)| p.starts_with("tables/"))
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn export_import_site_sqlite_to_postgres() {
    let Some(db) = TestDb::postgres_unmigrated().await else {
        return;
    };
    let (url, schema) = db.pg_location().unwrap();
    let ex = tokio::task::spawn_blocking(exported).await.unwrap();
    let target = ex.tmp.path().join("pg");
    std::fs::create_dir_all(&target).unwrap();
    std::fs::write(
        target.join("ferrite.toml"),
        format!(
            "[site]\nurl = \"http://127.0.0.1:8080/\"\n[database]\nurl = \"{url}\"\n\
             schema = \"{schema}\"\n"
        ),
    )
    .unwrap();
    let (t2, a2) = (target.clone(), ex.archive.clone());
    let again = ex.tmp.path().join("again.tar.zst");
    let a3 = again.clone();
    tokio::task::spawn_blocking(move || {
        let out = import_site(&t2, &a2, &["--with-media"]);
        assert!(out.status.success(), "{}", text(&out));
        let out = export(&t2, &a3, false);
        assert!(out.status.success(), "{}", text(&out));
    })
    .await
    .unwrap();
    // Exported again from Postgres: the same table files, byte for byte.
    let first = tables(&std::fs::read(&ex.archive).unwrap());
    let second = tables(&std::fs::read(&again).unwrap());
    assert_eq!(first.len(), second.len());
    for ((p1, d1), (p2, d2)) in first.iter().zip(&second) {
        assert_eq!(p1, p2);
        assert!(
            d1 == d2,
            "{p1} differs:\n{}\n---\n{}",
            String::from_utf8_lossy(d1),
            String::from_utf8_lossy(d2)
        );
    }
    let m = manifest(&unpack(&std::fs::read(&again).unwrap()));
    assert_eq!(m["source_backend"], "postgres");
    let mut conn = PgConnection::connect(&url).await.unwrap();
    let docs: i64 = sqlx::query_scalar(AssertSqlSafe(format!(
        "SELECT count(*) FROM \"{schema}\".search_docs"
    )))
    .fetch_one(&mut conn)
    .await
    .unwrap();
    assert!(docs > 0);
    conn.close().await.unwrap();
}

#[test]
fn export_to_stdout_and_import_from_stdin() {
    let tmp = tempfile::tempdir().unwrap();
    let source = seeded(tmp.path(), "source");
    let mut c = ferrite();
    c.args(["export", "--out", "-", "--data"]).arg(&source);
    let out = ok(c);
    assert!(out.stdout.len() > 100);
    assert_eq!(manifest(&unpack(&out.stdout))["kind"], "export");
    let target = tmp.path().join("target");
    init(&target);
    let mut c = ferrite();
    c.args(["import-site", "-", "--data"])
        .arg(&target)
        .write_stdin(out.stdout);
    ok(c);
    assert_eq!(
        count(&source, "content_items"),
        count(&target, "content_items")
    );
}

#[test]
fn import_site_refuses_non_fresh() {
    let ex = exported();
    let out = import_site(&ex.source, &ex.archive, &[]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("not empty") && err.contains("users"), "{err}");
}

#[test]
fn import_site_refuses_newer_schema() {
    let ex = exported();
    let entries = unpack(&std::fs::read(&ex.archive).unwrap());
    for (field, value) in [("schema_version", 9999), ("format", 2)] {
        let mut changed = entries.clone();
        let mut m = manifest(&changed);
        m[field] = Value::from(value);
        set_manifest(&mut changed, &m);
        let path = ex.tmp.path().join(format!("{field}.tar.zst"));
        std::fs::write(&path, pack(&changed)).unwrap();
        let target = ex.tmp.path().join(format!("t-{field}"));
        init(&target);
        let out = import_site(&target, &path, &[]);
        assert_eq!(out.status.code(), Some(3), "{field}: {}", text(&out));
        assert_eq!(count(&target, "content_items"), 0);
    }
}

#[test]
fn import_site_refuses_tampered_table() {
    let ex = exported();
    let mut entries = unpack(&std::fs::read(&ex.archive).unwrap());
    let users = entries
        .iter_mut()
        .find(|(p, _)| p == "tables/users.ndjson")
        .unwrap();
    let text_ = String::from_utf8(users.1.clone()).unwrap();
    users.1 = text_
        .replace("\"role\":\"admin\"", "\"role\":\"editor\"")
        .into_bytes();
    assert_ne!(users.1, text_.as_bytes());
    let path = ex.tmp.path().join("tampered.tar.zst");
    std::fs::write(&path, pack(&entries)).unwrap();
    let target = ex.tmp.path().join("target");
    init(&target);
    let out = import_site(&target, &path, &[]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("sha256"),
        "{}",
        text(&out)
    );
    for table in ["users", "content_types", "content_items"] {
        assert_eq!(
            count(&target, table),
            if table == "content_types" { 3 } else { 0 },
            "{table}"
        );
    }
}

#[test]
fn import_site_refuses_path_traversal() {
    let ex = exported();
    let entries = unpack(&std::fs::read(&ex.archive).unwrap());
    for bad in ["../evil", "/tmp/evil", "tables/../../evil"] {
        let mut hostile = vec![entries[0].clone()];
        hostile.push((bad.to_owned(), b"x".to_vec()));
        hostile.extend(entries[1..].iter().cloned());
        let path = ex.tmp.path().join("hostile.tar.zst");
        std::fs::write(&path, pack(&hostile)).unwrap();
        let target = ex.tmp.path().join(format!("t{}", bad.len()));
        init(&target);
        let out = import_site(&target, &path, &[]);
        assert_eq!(out.status.code(), Some(1), "{bad}: {}", text(&out));
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("path"),
            "{bad}: {}",
            text(&out)
        );
        assert_eq!(count(&target, "users"), 0);
    }
    assert!(!ex.tmp.path().join("evil").exists());
}

#[test]
fn import_site_truncated_leaves_target_fresh() {
    let ex = exported();
    let bytes = std::fs::read(&ex.archive).unwrap();
    // Cut inside the table files (the media follow them) and near the end.
    for cut in [bytes.len() / 4, bytes.len() / 2, bytes.len() - 10] {
        let path = ex.tmp.path().join("cut.tar.zst");
        std::fs::write(&path, &bytes[..cut]).unwrap();
        let target = ex.tmp.path().join(format!("t{cut}"));
        init(&target);
        let out = import_site(&target, &path, &["--with-media"]);
        assert_eq!(out.status.code(), Some(1), "{cut}: {}", text(&out));
        for table in ["users", "content_items", "terms", "artifacts", "media"] {
            assert_eq!(count(&target, table), 0, "{cut}: {table}");
        }
        // Still fresh: a whole archive goes in afterwards.
        let out = import_site(&target, &ex.archive, &[]);
        assert!(out.status.success(), "{}", text(&out));
    }
}

#[test]
fn import_site_into_newer_fresh_target_migrates_down_then_up() {
    let ex = exported();
    let mut entries = unpack(&std::fs::read(&ex.archive).unwrap());
    let latest = schema_version(&ex.source);
    assert_eq!(latest, 11);
    // The archive as a schema-10 binary would have written it.
    let artifacts = entries
        .iter_mut()
        .find(|(p, _)| p == "tables/artifacts.ndjson")
        .unwrap();
    let mut lines = Vec::new();
    for line in artifacts.1.split(|b| *b == b'\n').filter(|l| !l.is_empty()) {
        let mut row: serde_json::Map<String, Value> = serde_json::from_slice(line).unwrap();
        assert!(row.remove("orphaned_at").is_some());
        lines.extend(serde_json::to_vec(&row).unwrap());
        lines.push(b'\n');
    }
    artifacts.1 = lines;
    reseal(&mut entries, "artifacts");
    let mut m = manifest(&entries);
    m["schema_version"] = Value::from(10);
    set_manifest(&mut entries, &m);
    let path = ex.tmp.path().join("v10.tar.zst");
    std::fs::write(&path, pack(&entries)).unwrap();

    let target = ex.tmp.path().join("target");
    init(&target);
    assert_eq!(schema_version(&target), 11);
    let out = import_site(&target, &path, &[]);
    assert!(out.status.success(), "{}", text(&out));
    assert_eq!(schema_version(&target), 11);
    assert_eq!(count(&target, "artifacts"), count(&ex.source, "artifacts"));
    assert_eq!(
        count(&target, "content_items"),
        count(&ex.source, "content_items")
    );
}

#[test]
fn import_site_prints_totp_hint() {
    let tmp = tempfile::tempdir().unwrap();
    let source = seeded(tmp.path(), "source");
    exec(&source, "UPDATE users SET totp_secret = 'sealed'");
    let archive = tmp.path().join("a.tar.zst");
    let out = export(&source, &archive, false);
    assert!(out.status.success(), "{}", text(&out));
    let target = tmp.path().join("target");
    init(&target);
    let out = import_site(&target, &archive, &[]);
    assert!(out.status.success(), "{}", text(&out));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("two-factor"), "{stdout}");
    assert!(stdout.contains("secret.key"), "{stdout}");
    assert!(stdout.contains("ferrite user disable-totp"), "{stdout}");
    // Without TOTP users there is no hint.
    let plain = seeded(tmp.path(), "plain");
    let archive = tmp.path().join("b.tar.zst");
    assert!(export(&plain, &archive, false).status.success());
    let target = tmp.path().join("target2");
    init(&target);
    let out = import_site(&target, &archive, &[]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(!String::from_utf8_lossy(&out.stdout).contains("two-factor"));
}

#[test]
fn db_copy_sqlite_to_new_sqlite_file() {
    let tmp = tempfile::tempdir().unwrap();
    let source = seeded(tmp.path(), "source");
    let dest = tmp.path().join("copy");
    std::fs::create_dir_all(&dest).unwrap();
    let mut c = ferrite();
    c.args(["db", "copy", "--from"])
        .arg(format!("sqlite:{}", source.join("ferrite.db").display()))
        .arg("--to")
        .arg(format!("sqlite:{}", dest.join("ferrite.db").display()));
    let out = ok(c);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("[database]"), "{stdout}");
    for table in ["content_items", "users", "artifacts", "media"] {
        assert_eq!(count(&source, table), count(&dest, table), "{table}");
    }
    assert_eq!(schema_version(&dest), 11);
    // A second copy into the now filled database is refused.
    let mut c = ferrite();
    c.args(["db", "copy", "--from"])
        .arg(format!("sqlite:{}", source.join("ferrite.db").display()))
        .arg("--to")
        .arg(format!("sqlite:{}", dest.join("ferrite.db").display()));
    let out = run(c);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn db_copy_sqlite_to_postgres() {
    let Some(db) = TestDb::postgres_unmigrated().await else {
        return;
    };
    let (url, schema) = db.pg_location().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().to_owned();
    let (u2, s2) = (url.clone(), schema.clone());
    let source = tokio::task::spawn_blocking(move || {
        let source = seeded(&path, "source");
        let mut c = ferrite();
        c.args(["db", "copy", "--from"])
            .arg(format!("sqlite:{}", source.join("ferrite.db").display()))
            .args(["--to", &u2, "--to-schema", &s2]);
        let out = ok(c);
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        assert!(stdout.contains("[database]"), "{stdout}");
        assert!(stdout.contains(&format!("schema = \"{s2}\"")), "{stdout}");
        source
    })
    .await
    .unwrap();
    let mut conn = PgConnection::connect(&url).await.unwrap();
    for table in ["content_items", "users", "terms", "artifacts", "media"] {
        let n: i64 = sqlx::query_scalar(AssertSqlSafe(format!(
            "SELECT count(*) FROM \"{schema}\".{table}"
        )))
        .fetch_one(&mut conn)
        .await
        .unwrap();
        let s = source.clone();
        let expected = tokio::task::spawn_blocking(move || count(&s, table))
            .await
            .unwrap();
        assert_eq!(n, expected, "{table}");
    }
    conn.close().await.unwrap();
}

#[cfg(unix)]
#[test]
fn export_file_mode_is_0600() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    let source = tmp.path().join("source");
    init(&source);
    let archive = tmp.path().join("a.tar.zst");
    let out = export(&source, &archive, false);
    assert!(out.status.success(), "{}", text(&out));
    let mode = std::fs::metadata(&archive).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
    // An existing file is not overwritten.
    let out = export(&source, &archive, false);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    let mut f = std::fs::File::open(&archive).unwrap();
    let mut head = [0u8; 4];
    f.read_exact(&mut head).unwrap();
    assert_eq!(head, [0x28, 0xb5, 0x2f, 0xfd], "zstd magic");
}
