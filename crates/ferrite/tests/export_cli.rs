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
    let path = ex.tmp.path().join("v10.tar.zst");
    std::fs::write(&path, v10_archive(&ex)).unwrap();

    let target = ex.tmp.path().join("target");
    init(&target);
    assert_eq!(schema_version(&target), 16);
    let out = import_site(&target, &path, &[]);
    assert!(out.status.success(), "{}", text(&out));
    assert_eq!(schema_version(&target), 16);
    assert_eq!(count(&target, "artifacts"), count(&ex.source, "artifacts"));
    assert_eq!(
        count(&target, "content_items"),
        count(&ex.source, "content_items")
    );
}

/// The export of `ex` as a schema-10 binary would have written it: no `orphaned_at` in
/// the artifacts rows, no `oauth_grant_id` in the api_tokens rows, `schema_version` 10,
/// the sha256s recomputed.
fn v10_archive(ex: &Exported) -> Vec<u8> {
    let mut entries = unpack(&std::fs::read(&ex.archive).unwrap());
    assert_eq!(schema_version(&ex.source), 16);
    for (table, column) in [
        ("artifacts", "orphaned_at"),
        ("api_tokens", "oauth_grant_id"),
    ] {
        let file = format!("tables/{table}.ndjson");
        let entry = entries.iter_mut().find(|(p, _)| *p == file).unwrap();
        let mut lines = Vec::new();
        for line in entry.1.split(|b| *b == b'\n').filter(|l| !l.is_empty()) {
            let mut row: serde_json::Map<String, Value> = serde_json::from_slice(line).unwrap();
            assert!(row.remove(column).is_some());
            lines.extend(serde_json::to_vec(&row).unwrap());
            lines.push(b'\n');
        }
        entry.1 = lines;
        reseal(&mut entries, table);
    }
    let mut m = manifest(&entries);
    m["schema_version"] = Value::from(10);
    set_manifest(&mut entries, &m);
    pack(&entries)
}

#[test]
fn import_site_failure_after_migrate_down_restores_the_schema() {
    let ex = exported();
    let good = v10_archive(&ex);
    let cut = ex.tmp.path().join("v10-cut.tar.zst");
    std::fs::write(&cut, &good[..good.len() / 2]).unwrap();
    let whole = ex.tmp.path().join("v10.tar.zst");
    std::fs::write(&whole, &good).unwrap();
    let target = ex.tmp.path().join("target");
    init(&target);
    let out = import_site(&target, &cut, &[]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert_eq!(schema_version(&target), 13, "the migrate-down is undone");
    assert_eq!(count(&target, "users"), 0);
    let out = import_site(&target, &whole, &[]);
    assert!(out.status.success(), "{}", text(&out));
    assert_eq!(schema_version(&target), 16);
    assert_eq!(count(&target, "users"), count(&ex.source, "users"));
}

#[test]
fn import_site_refuses_an_overlong_line() {
    let ex = exported();
    let entries = unpack(&std::fs::read(&ex.archive).unwrap());
    // `tables/content_types.ndjson` (the first table) as 65 MiB without a newline,
    // streamed: it compresses to a few kilobytes.
    let big = 65 * 1024 * 1024u64;
    let encoder = zstd::stream::write::Encoder::new(Vec::new(), 3).unwrap();
    let mut builder = tar::Builder::new(encoder);
    for (path, data) in &entries {
        let mut header = tar::Header::new_gnu();
        header.set_mode(0o600);
        header.set_entry_type(tar::EntryType::Regular);
        if path == "tables/content_types.ndjson" {
            header.set_size(big);
            builder
                .append_data(&mut header, path, std::io::repeat(b'x').take(big))
                .unwrap();
        } else {
            header.set_size(data.len() as u64);
            builder
                .append_data(&mut header, path, data.as_slice())
                .unwrap();
        }
    }
    let bytes = builder.into_inner().unwrap().finish().unwrap();
    assert!(bytes.len() < 16 * 1024 * 1024);
    let path = ex.tmp.path().join("big.tar.zst");
    std::fs::write(&path, bytes).unwrap();
    let target = ex.tmp.path().join("target");
    init(&target);
    let out = import_site(&target, &path, &[]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("tables/content_types.ndjson: line 1 is longer than 64 MiB"),
        "{err}"
    );
    assert_eq!(count(&target, "content_types"), 3);
}

#[test]
fn import_site_refuses_more_artifact_bytes_than_declared() {
    let ex = exported();
    let mut entries = unpack(&std::fs::read(&ex.archive).unwrap());
    let mut m = manifest(&entries);
    m["artifacts"]["bytes"] = Value::from(1);
    set_manifest(&mut entries, &m);
    let path = ex.tmp.path().join("over.tar.zst");
    std::fs::write(&path, pack(&entries)).unwrap();
    let target = ex.tmp.path().join("target");
    init(&target);
    let out = import_site(&target, &path, &["--with-media"]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("artifact bytes than the manifest declares"),
        "{}",
        text(&out)
    );
    assert_eq!(count(&target, "users"), 0);
}

/// The stored file of the site's one media original.
fn media_blob(data: &Path) -> (String, PathBuf) {
    let hash: String = block_on(async {
        let mut conn = sqlite(data).await;
        sqlx::query_scalar("SELECT artifact_hash FROM media")
            .fetch_one(&mut conn)
            .await
            .unwrap()
    });
    let file = data.join("media/artifacts").join(&hash[..2]).join(&hash);
    assert!(file.is_file(), "{}", file.display());
    (hash, file)
}

#[test]
fn export_with_a_missing_blob_exits_2_and_lists_it() {
    let tmp = tempfile::tempdir().unwrap();
    let source = seeded(tmp.path(), "source");
    let (hash, file) = media_blob(&source);
    std::fs::remove_file(&file).unwrap();
    let archive = tmp.path().join("a.tar.zst");
    let out = export(&source, &archive, true);
    assert_eq!(out.status.code(), Some(2), "{}", text(&out));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains(&format!("missing artifact {hash}")), "{err}");
    assert!(err.contains("1 artifacts are missing"), "{err}");
    let m = manifest(&unpack(&std::fs::read(&archive).unwrap()));
    assert_eq!(m["missing"], serde_json::json!([hash]));
    // The archive still imports.
    let target = tmp.path().join("target");
    init(&target);
    let out = import_site(&target, &archive, &["--with-media"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(String::from_utf8_lossy(&out.stdout).contains("1 artifacts were missing"));
}

#[test]
fn export_with_a_damaged_blob_fails() {
    let tmp = tempfile::tempdir().unwrap();
    let source = seeded(tmp.path(), "source");
    let (hash, file) = media_blob(&source);
    let mut bytes = std::fs::read(&file).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 0xff;
    std::fs::write(&file, bytes).unwrap();
    let archive = tmp.path().join("a.tar.zst");
    let out = export(&source, &archive, true);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains(&format!("artifact {hash}")),
        "{}",
        text(&out)
    );
    assert!(!archive.exists());
}

/// Records a migration this binary does not know, as a newer binary would.
fn make_newer(data: &Path) {
    exec(
        data,
        "INSERT INTO _sqlx_migrations (version, description, success, checksum, execution_time) \
         VALUES (99, 'future', 1, X'00', 0)",
    );
}

fn make_older(data: &Path) {
    let mut c = ferrite();
    c.args([
        "migrate",
        "down",
        "--to",
        "10",
        "--allow-lossy",
        "--yes",
        "--data",
    ])
    .arg(data);
    ok(c);
}

fn db_copy(from: &Path, to: &Path) -> Output {
    let mut c = ferrite();
    c.args(["db", "copy", "--from"])
        .arg(format!("sqlite:{}", from.join("ferrite.db").display()))
        .arg("--to")
        .arg(format!("sqlite:{}", to.join("ferrite.db").display()));
    run(c)
}

#[test]
fn export_and_db_copy_refuse_an_older_or_newer_source() {
    let tmp = tempfile::tempdir().unwrap();
    let older = tmp.path().join("older");
    init(&older);
    make_older(&older);
    let newer = tmp.path().join("newer");
    init(&newer);
    make_newer(&newer);
    let dest = tmp.path().join("dest");
    std::fs::create_dir_all(&dest).unwrap();
    for (source, code, needle) in [
        (&older, 1, "migrate up"),
        (&newer, 3, "newer than this binary"),
    ] {
        let archive = tmp.path().join("a.tar.zst");
        let out = export(source, &archive, false);
        assert_eq!(out.status.code(), Some(code), "{}", text(&out));
        assert!(
            String::from_utf8_lossy(&out.stderr).contains(needle),
            "{}",
            text(&out)
        );
        assert!(!archive.exists());
        let out = db_copy(source, &dest);
        assert_eq!(out.status.code(), Some(code), "{}", text(&out));
        assert!(
            String::from_utf8_lossy(&out.stderr).contains(needle),
            "{}",
            text(&out)
        );
        assert!(!dest.join("ferrite.db").exists());
    }
}

#[test]
fn db_copy_refuses_the_same_source_and_target() {
    let tmp = tempfile::tempdir().unwrap();
    let source = seeded(tmp.path(), "source");
    let out = db_copy(&source, &source);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert_eq!(schema_version(&source), 16);
    assert!(count(&source, "content_items") > 0);
}

#[test]
fn refused_db_copy_removes_the_sqlite_file_it_created() {
    let tmp = tempfile::tempdir().unwrap();
    let source = seeded(tmp.path(), "source");
    // A term that is its own parent can never be loaded (the loader refuses cycles).
    exec(
        &source,
        "UPDATE terms SET parent_id = id WHERE id = (SELECT min(id) FROM terms)",
    );
    let dest = tmp.path().join("dest");
    std::fs::create_dir_all(&dest).unwrap();
    let out = db_copy(&source, &dest);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("terms"),
        "{}",
        text(&out)
    );
    assert!(!dest.join("ferrite.db").exists());
    assert!(!dest.join("ferrite.db-wal").exists());
    assert!(!dest.join("ferrite.lock").exists(), "B-M14");
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

fn meta(data: &Path, key: &str) -> Option<String> {
    let key = key.to_owned();
    block_on(async {
        let mut conn = sqlite(data).await;
        sqlx::query_scalar("SELECT value FROM schema_meta WHERE key = ?")
            .bind(key)
            .fetch_optional(&mut conn)
            .await
            .unwrap()
    })
}

fn marked(data: &Path) -> i64 {
    block_on(async {
        let mut conn = sqlite(data).await;
        sqlx::query_scalar("SELECT count(*) FROM artifacts WHERE orphaned_at IS NOT NULL")
            .fetch_one(&mut conn)
            .await
            .unwrap()
    })
}

/// R16: the install id never travels (the archive's `schema_meta` holds only the site id);
/// an import-site or db copy target keeps its own, and no artifact keeps a GC mark of the
/// source's.
#[test]
fn import_site_and_db_copy_keep_their_install_id_and_drop_gc_marks() {
    let tmp = tempfile::tempdir().unwrap();
    let source = seeded(tmp.path(), "source");
    exec(&source, "UPDATE artifacts SET orphaned_at = 1000000");
    assert!(marked(&source) > 0);
    let source_install = meta(&source, "install_id").expect("init wrote one");
    let archive = tmp.path().join("site.tar.zst");
    let out = export(&source, &archive, true);
    assert!(out.status.success(), "{}", text(&out));
    let entries = unpack(&std::fs::read(&archive).unwrap());
    let (_, schema_meta) = entries
        .iter()
        .find(|(p, _)| p == "tables/schema_meta.ndjson")
        .expect("schema_meta is exported");
    let schema_meta = String::from_utf8_lossy(schema_meta);
    assert_eq!(schema_meta.lines().count(), 1, "{schema_meta}");
    assert!(schema_meta.contains("site_id"), "{schema_meta}");
    assert!(!schema_meta.contains("install_id"), "{schema_meta}");

    let target = tmp.path().join("target");
    init(&target);
    let target_install = meta(&target, "install_id").expect("init wrote one");
    let out = import_site(&target, &archive, &["--with-media"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(
        !String::from_utf8_lossy(&out.stderr).contains("is outside"),
        "{}",
        text(&out)
    );
    assert_eq!(meta(&target, "site_id"), meta(&source, "site_id"));
    assert_eq!(meta(&target, "install_id"), Some(target_install.clone()));
    assert_ne!(target_install, source_install);
    assert!(count(&target, "artifacts") > 0);
    assert_eq!(marked(&target), 0);

    let copy = tmp.path().join("copy");
    std::fs::create_dir_all(&copy).unwrap();
    let out = db_copy(&source, &copy);
    assert!(out.status.success(), "{}", text(&out));
    assert_eq!(meta(&copy, "site_id"), meta(&source, "site_id"));
    let copy_install = meta(&copy, "install_id").expect("migrations wrote one");
    assert_ne!(copy_install, source_install);
    assert_eq!(marked(&copy), 0);
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
    // B-I2: relative, the form backup and restore accept; the R16 claim note.
    assert!(stdout.contains("url = \"sqlite://ferrite.db\""), "{stdout}");
    assert!(stdout.contains("--claim"), "{stdout}");
    for table in ["content_items", "users", "artifacts", "media"] {
        assert_eq!(count(&source, table), count(&dest, table), "{table}");
    }
    assert_eq!(schema_version(&dest), 16);
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

/// An error reported after the commit landed (as a lost connection can): the schema step
/// is not undone, so the committed rows stay.
#[test]
fn import_site_error_after_commit_keeps_the_rows() {
    let ex = exported();
    let path = ex.tmp.path().join("v10.tar.zst");
    std::fs::write(&path, v10_archive(&ex)).unwrap();
    let target = ex.tmp.path().join("target");
    init(&target);
    let out = import_site(&target, &path, &["--test-fail-after-commit"]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("left at schema 10"), "{err}");
    assert!(err.contains("may have been committed"), "{err}");
    assert_eq!(schema_version(&target), 10);
    assert_eq!(count(&target, "users"), count(&ex.source, "users"));
}

/// The same into an unmigrated Postgres schema, where the undo would be a migration down
/// to 0: the freshness re-check finds the rows and keeps them.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn import_site_error_after_commit_keeps_the_rows_postgres() {
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
    let out =
        tokio::task::spawn_blocking(move || import_site(&t2, &a2, &["--test-fail-after-commit"]))
            .await
            .unwrap();
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("has rows"), "{err}");
    let mut conn = PgConnection::connect(&url).await.unwrap();
    let users: i64 = sqlx::query_scalar(AssertSqlSafe(format!(
        "SELECT count(*) FROM \"{schema}\".users"
    )))
    .fetch_one(&mut conn)
    .await
    .unwrap();
    assert!(users > 0);
    conn.close().await.unwrap();
}
