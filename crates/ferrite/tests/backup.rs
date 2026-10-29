//! `ferrite backup` and `ferrite restore` (M4-1 Task 8, P13–P14): single-archive and
//! repository backups (a folder and an in-process S3 bucket), incremental blobs, `--keep`,
//! the site key, the integrity check, the restore target rules (`--force`, the folder lock),
//! a newer schema, `--snapshot`, missing and damaged blobs, and file modes.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::Read;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Output;

use assert_cmd::Command;
use assert_cmd::cargo::cargo_bin_cmd;
use ferrite::{NodeOptions, OpenMode, open_store, start};
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto::Builder as ConnBuilder;
use s3s::auth::SimpleAuth;
use s3s::service::S3ServiceBuilder;
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::{AssertSqlSafe, Connection, SqliteConnection};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

const BUCKET: &str = "ferrite-test";
const ACCESS_KEY: &str = "test-access";
const SECRET_KEY: &str = "test-secret";

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

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
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

fn backup(data: &Path, to: &Path, extra: &[&str]) -> Output {
    let mut c = ferrite();
    c.args(["backup", "--data"]).arg(data).arg("--to").arg(to);
    c.args(extra);
    run(c)
}

fn restore(source: &Path, data: &Path, extra: &[&str]) -> Output {
    let mut c = ferrite();
    c.arg("restore").arg(source).arg("--data").arg(data);
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

fn archive_manifest(path: &Path) -> Value {
    let entries = unpack(&std::fs::read(path).unwrap());
    assert_eq!(entries[0].0, "manifest.json");
    serde_json::from_slice(&entries[0].1).unwrap()
}

/// The snapshot names of a repository folder, oldest first.
fn snapshots(repo: &Path) -> Vec<String> {
    let Ok(dir) = std::fs::read_dir(repo.join("snapshots")) else {
        return Vec::new();
    };
    let mut names: Vec<String> = dir
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    names
}

fn repo_manifest(repo: &Path, snapshot: &str) -> Value {
    let path = repo.join("snapshots").join(snapshot).join("manifest.json");
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

fn hashes(m: &Value) -> Vec<String> {
    let mut out: Vec<String> = m["artifact_hashes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h.as_str().unwrap().to_owned())
        .collect();
    out.sort();
    out
}

fn blob_path(root: &Path, hash: &str) -> PathBuf {
    root.join("artifacts").join(&hash[..2]).join(hash)
}

/// The site's artifact hashes, sorted.
fn site_hashes(data: &Path) -> Vec<String> {
    block_on(async {
        let mut conn = sqlite(data).await;
        sqlx::query_scalar("SELECT hash FROM artifacts ORDER BY hash")
            .fetch_all(&mut conn)
            .await
            .unwrap()
    })
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
    let file = blob_path(&data.join("media"), &hash);
    assert!(file.is_file(), "{}", file.display());
    (hash, file)
}

#[cfg(unix)]
fn mode(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

/// Files left in `<data>/backups` by a backup run (`backup-*`).
fn local_snapshots(data: &Path) -> Vec<String> {
    std::fs::read_dir(data.join("backups"))
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|n| n.starts_with("backup-"))
        .collect()
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

fn listen_config(data: &Path) -> ferrite_config::Config {
    ferrite_config::load(
        data,
        vec![(
            "FERRITE_SERVER__LISTEN".to_owned(),
            "127.0.0.1:0".to_owned(),
        )],
    )
    .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn backup_to_archive_and_restore_serves() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_owned();
    let (source, target, out) = tokio::task::spawn_blocking(move || {
        let source = seeded(&root, "source");
        let archive = root.join("site.tar.zst");
        let out = backup(&source, &archive, &[]);
        assert_eq!(out.status.code(), Some(0), "{}", text(&out));
        let m = archive_manifest(&archive);
        assert_eq!(m["format"], 1);
        assert_eq!(m["kind"], "backup");
        assert_eq!(m["source_backend"], "sqlite");
        let files: Vec<&str> = m["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["path"].as_str().unwrap())
            .collect();
        assert_eq!(files, ["ferrite.toml", "secret.key", "ferrite.db"], "{m}");
        assert_eq!(
            m["artifacts"]["count"].as_u64().unwrap(),
            site_hashes(&source).len() as u64
        );
        // The local snapshot is gone.
        assert!(local_snapshots(&source).is_empty());

        let target = root.join("restored");
        let out = restore(&archive, &target, &[]);
        (source, target, out)
    })
    .await
    .unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    assert!(stdout(&out).contains("database version:"), "{}", text(&out));
    let (s, t) = (source.clone(), target.clone());
    tokio::task::spawn_blocking(move || {
        for table in ["content_items", "users", "terms", "media", "artifacts"] {
            assert_eq!(count(&s, table), count(&t, table), "{table}");
        }
        assert_eq!(
            std::fs::read(s.join("secret.key")).unwrap(),
            std::fs::read(t.join("secret.key")).unwrap()
        );
        for hash in site_hashes(&s) {
            assert!(blob_path(&t.join("media"), &hash).is_file(), "{hash}");
        }
    })
    .await
    .unwrap();

    // Served: the home page and the media file answer.
    let config = listen_config(&target);
    let store = open_store(&config, OpenMode::MustExist).await.unwrap();
    let node = start(&config, store.clone(), NodeOptions::default())
        .await
        .unwrap();
    let (status, body) = get(node.addr, "/").await;
    let body = String::from_utf8_lossy(&body).into_owned();
    assert_eq!(status, 200, "{body}");
    assert!(body.contains("Welcome to FerriteCMS"), "{body}");
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

#[test]
fn backup_repo_is_incremental() {
    let tmp = tempfile::tempdir().unwrap();
    let source = seeded(tmp.path(), "source");
    let repo = tmp.path().join("repo");
    let n = site_hashes(&source).len();
    assert!(n > 0);

    let first = backup(&source, &repo, &[]);
    assert_eq!(first.status.code(), Some(0), "{}", text(&first));
    assert!(
        stdout(&first).contains(&format!("{n} copied, 0 already in the repository")),
        "{}",
        text(&first)
    );
    let second = backup(&source, &repo, &[]);
    assert_eq!(second.status.code(), Some(0), "{}", text(&second));
    assert!(
        stdout(&second).contains(&format!("0 copied, {n} already in the repository")),
        "{}",
        text(&second)
    );
    let snaps = snapshots(&repo);
    assert_eq!(snaps.len(), 2, "{snaps:?}");
    let m1 = repo_manifest(&repo, &snaps[0]);
    let m2 = repo_manifest(&repo, &snaps[1]);
    assert_eq!(m2["kind"], "backup");
    assert_eq!(hashes(&m2), site_hashes(&source));
    assert_eq!(hashes(&m1), hashes(&m2));
    for name in ["manifest.json", "ferrite.db", "ferrite.toml", "secret.key"] {
        assert!(repo.join("snapshots").join(&snaps[1]).join(name).is_file());
    }
    for hash in site_hashes(&source) {
        assert!(blob_path(&repo, &hash).is_file(), "{hash}");
    }
    assert!(local_snapshots(&source).is_empty());
}

#[test]
fn backup_repo_keep_prunes_snapshots_and_unlisted_blobs() {
    let tmp = tempfile::tempdir().unwrap();
    let source = seeded(tmp.path(), "source");
    let repo = tmp.path().join("repo");
    ok_backup(&source, &repo, &[]);
    // A blob no manifest lists (left by an older site state).
    let stray = b"stray blob";
    let stray_hash = ferrite_domain::to_hex(&Sha256::digest(stray));
    let stray_path = blob_path(&repo, &stray_hash);
    std::fs::create_dir_all(stray_path.parent().unwrap()).unwrap();
    std::fs::write(&stray_path, stray).unwrap();
    ok_backup(&source, &repo, &[]);
    let before = snapshots(&repo);
    let out = ok_backup(&source, &repo, &["--keep", "2"]);
    let after = snapshots(&repo);
    assert_eq!(after.len(), 2, "{after:?}");
    assert_eq!(after[0], before[1]);
    assert!(!after.contains(&before[0]));
    assert!(!stray_path.exists(), "the unlisted blob is pruned");
    for hash in site_hashes(&source) {
        assert!(blob_path(&repo, &hash).is_file(), "{hash}");
    }
    assert!(
        stdout(&out).contains("Pruned 1 snapshots and 1 artifacts"),
        "{}",
        text(&out)
    );
}

fn ok_backup(data: &Path, to: &Path, extra: &[&str]) -> Output {
    let out = backup(data, to, extra);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    out
}

/// An s3s-fs server on a free port; dropping it stops the server.
struct S3Server {
    dir: tempfile::TempDir,
    handle: JoinHandle<()>,
    endpoint: String,
}

impl Drop for S3Server {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

impl S3Server {
    async fn start() -> S3Server {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(BUCKET)).unwrap();
        let fs = s3s_fs::FileSystem::new(dir.path()).unwrap();
        let mut builder = S3ServiceBuilder::new(fs);
        builder.set_auth(SimpleAuth::from_single(ACCESS_KEY, SECRET_KEY));
        let service = builder.build();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            loop {
                let Ok((socket, _)) = listener.accept().await else {
                    break;
                };
                let service = service.clone();
                tokio::spawn(async move {
                    let http = ConnBuilder::new(TokioExecutor::new());
                    let _ = http.serve_connection(TokioIo::new(socket), service).await;
                });
            }
        });
        S3Server {
            dir,
            handle,
            endpoint: format!("http://{addr}"),
        }
    }

    fn env(&self) -> Vec<(&'static str, String)> {
        vec![
            ("AWS_ACCESS_KEY_ID", ACCESS_KEY.into()),
            ("AWS_SECRET_ACCESS_KEY", SECRET_KEY.into()),
            ("AWS_REGION", "us-east-1".into()),
            ("AWS_ENDPOINT_URL", self.endpoint.clone()),
            ("AWS_ALLOW_HTTP", "true".into()),
        ]
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn backup_repo_on_s3() {
    let server = S3Server::start().await;
    let env = server.env();
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_owned();
    let bucket_dir = server.dir.path().join(BUCKET);
    tokio::task::spawn_blocking(move || {
        let source = seeded(&root, "source");
        let url = format!("s3://{BUCKET}/site-backups");
        let mut c = ferrite();
        c.args(["backup", "--data"])
            .arg(&source)
            .args(["--to", &url])
            .envs(env.clone());
        let out = ok(c);
        let n = site_hashes(&source).len();
        assert!(
            stdout(&out).contains(&format!("{n} copied")),
            "{}",
            text(&out)
        );
        let repo = bucket_dir.join("site-backups");
        let snaps = snapshots(&repo);
        assert_eq!(snaps.len(), 1, "{snaps:?}");
        assert_eq!(
            hashes(&repo_manifest(&repo, &snaps[0])),
            site_hashes(&source)
        );
        for hash in site_hashes(&source) {
            assert!(blob_path(&repo, &hash).is_file(), "{hash}");
        }
        assert!(local_snapshots(&source).is_empty());

        let target = root.join("restored");
        let mut c = ferrite();
        c.args(["restore", &url, "--data"]).arg(&target).envs(env);
        let out = ok(c);
        assert!(stdout(&out).contains(&snaps[0]), "{}", text(&out));
        for table in ["content_items", "users", "media", "artifacts"] {
            assert_eq!(count(&source, table), count(&target, table), "{table}");
        }
        for hash in site_hashes(&source) {
            assert!(blob_path(&target.join("media"), &hash).is_file(), "{hash}");
        }
    })
    .await
    .unwrap();
}

#[test]
fn backup_no_secret_key() {
    let tmp = tempfile::tempdir().unwrap();
    let source = seeded(tmp.path(), "source");
    let archive = tmp.path().join("b.tar.zst");
    ok_backup(&source, &archive, &["--no-secret-key", "--no-media"]);
    let m = archive_manifest(&archive);
    let files: Vec<&str> = m["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["path"].as_str().unwrap())
        .collect();
    assert_eq!(files, ["ferrite.toml", "ferrite.db"]);
    assert!(m["artifacts"].is_null(), "{m}");
    let entries = unpack(&std::fs::read(&archive).unwrap());
    assert!(!entries.iter().any(|(p, _)| p == "secret.key"));
    assert!(!entries.iter().any(|(p, _)| p.starts_with("artifacts/")));

    let target = tmp.path().join("restored");
    let out = restore(&archive, &target, &[]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    assert!(!target.join("secret.key").exists());
    assert!(stdout(&out).contains("secret.key"), "{}", text(&out));
}

#[test]
fn backup_refuses_postgres() {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("pg");
    std::fs::create_dir_all(&data).unwrap();
    // Refused before connecting, so a real server is optional.
    let url = std::env::var("FERRITE_TEST_PG_URL")
        .unwrap_or_else(|_| "postgres://ferrite@127.0.0.1:1/none".to_owned());
    std::fs::write(
        data.join("ferrite.toml"),
        format!(
            "[site]\nurl = \"http://127.0.0.1:8080\"\n\n[database]\nurl = \"{url}\"\nschema = \
             \"backup_refusal\"\n"
        ),
    )
    .unwrap();
    let out = backup(&data, &tmp.path().join("b.tar.zst"), &[]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    let err = stderr(&out);
    assert!(err.contains("ferrite export"), "{err}");
    assert!(err.contains("provider"), "{err}");
    assert!(!tmp.path().join("b.tar.zst").exists());
}

#[test]
fn backup_name_survives_pre_migration_prune() {
    let tmp = tempfile::tempdir().unwrap();
    let source = seeded(tmp.path(), "source");
    // A local snapshot left by an interrupted backup (named as backup writes them).
    let planted = source.join("backups/backup-20260101T000000Z.db");
    std::fs::write(&planted, b"x").unwrap();
    // Six down/up cycles: each `up` writes a `pre-*.db` and prunes to five.
    for _ in 0..6 {
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
        .arg(&source);
        ok(c);
        let mut c = ferrite();
        c.args(["migrate", "up", "--data"]).arg(&source);
        ok(c);
    }
    let pre: Vec<String> = std::fs::read_dir(source.join("backups"))
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|n| n.starts_with("pre-"))
        .collect();
    assert_eq!(pre.len(), 5, "{pre:?}");
    assert!(planted.exists(), "pruning never touches backup-*.db");
    // A backup still works and cleans up after itself, leaving the old file alone.
    ok_backup(&source, &tmp.path().join("b.tar.zst"), &[]);
    assert_eq!(local_snapshots(&source), ["backup-20260101T000000Z.db"]);
}

#[test]
fn restore_refuses_non_empty() {
    let tmp = tempfile::tempdir().unwrap();
    let source = seeded(tmp.path(), "source");
    let archive = tmp.path().join("b.tar.zst");
    ok_backup(&source, &archive, &[]);
    let target = tmp.path().join("busy");
    std::fs::create_dir_all(&target).unwrap();
    std::fs::write(target.join("notes.txt"), b"keep me").unwrap();
    let out = restore(&archive, &target, &[]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert!(stderr(&out).contains("not empty"), "{}", text(&out));
    let names: Vec<_> = std::fs::read_dir(&target).unwrap().collect();
    assert_eq!(names.len(), 1);
    assert_eq!(std::fs::read(target.join("notes.txt")).unwrap(), b"keep me");
    // Nothing left next to it either.
    let siblings: Vec<String> = std::fs::read_dir(tmp.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    assert!(
        siblings.iter().all(|n| !n.contains("restore")),
        "{siblings:?}"
    );

    // An empty folder is fine.
    let empty = tmp.path().join("empty");
    std::fs::create_dir_all(&empty).unwrap();
    let out = restore(&archive, &empty, &[]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    assert!(empty.join("ferrite.db").is_file());
}

#[test]
fn restore_force_moves_aside() {
    let tmp = tempfile::tempdir().unwrap();
    let source = seeded(tmp.path(), "source");
    let archive = tmp.path().join("b.tar.zst");
    ok_backup(&source, &archive, &[]);
    let target = tmp.path().join("site");
    init(&target);
    let old_db = std::fs::read(target.join("ferrite.db")).unwrap();
    let out = restore(&archive, &target, &["--force"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    let aside: Vec<PathBuf> = std::fs::read_dir(tmp.path())
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("site.pre-restore-")
        })
        .collect();
    assert_eq!(aside.len(), 1, "{aside:?}");
    assert_eq!(std::fs::read(aside[0].join("ferrite.db")).unwrap(), old_db);
    assert!(stdout(&out).contains("pre-restore-"), "{}", text(&out));
    assert_eq!(
        count(&source, "content_items"),
        count(&target, "content_items")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn restore_force_refuses_locked() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_owned();
    let (archive, target) = tokio::task::spawn_blocking(move || {
        let source = seeded(&root, "source");
        let archive = root.join("b.tar.zst");
        ok_backup(&source, &archive, &[]);
        let target = root.join("site");
        init(&target);
        (archive, target)
    })
    .await
    .unwrap();
    // The site is being served (the node holds ferrite.lock).
    let config = listen_config(&target);
    let store = open_store(&config, OpenMode::MustExist).await.unwrap();
    let node = start(&config, store.clone(), NodeOptions::default())
        .await
        .unwrap();
    let before = std::fs::read(target.join("ferrite.toml")).unwrap();
    let (a, t) = (archive.clone(), target.clone());
    let out = tokio::task::spawn_blocking(move || restore(&a, &t, &["--force"]))
        .await
        .unwrap();
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert!(stderr(&out).contains("in use"), "{}", text(&out));
    assert_eq!(std::fs::read(target.join("ferrite.toml")).unwrap(), before);
    let siblings: Vec<String> = std::fs::read_dir(tmp.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    assert!(
        siblings.iter().all(|n| !n.contains("restore")),
        "{siblings:?}"
    );
    node.shutdown().await.unwrap();
    store.close().await;
}

#[test]
fn restore_newer_schema_exits_3() {
    let tmp = tempfile::tempdir().unwrap();
    let source = seeded(tmp.path(), "source");
    // As a newer binary would have migrated it.
    exec(
        &source,
        "INSERT INTO _sqlx_migrations (version, description, success, checksum, execution_time) \
         VALUES (99, 'future', 1, X'00', 0)",
    );
    let archive = tmp.path().join("b.tar.zst");
    ok_backup(&source, &archive, &[]);
    assert_eq!(archive_manifest(&archive)["schema_version"], 99);
    let target = tmp.path().join("restored");
    let out = restore(&archive, &target, &[]);
    assert_eq!(out.status.code(), Some(3), "{}", text(&out));
    assert!(stderr(&out).contains("newer"), "{}", text(&out));
    assert!(!target.exists());

    // The same from a repository, onto an existing site with --force: untouched.
    let repo = tmp.path().join("repo");
    ok_backup(&source, &repo, &[]);
    let site = tmp.path().join("site");
    init(&site);
    let out = restore(&repo, &site, &["--force"]);
    assert_eq!(out.status.code(), Some(3), "{}", text(&out));
    let siblings: Vec<String> = std::fs::read_dir(tmp.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    assert!(
        siblings.iter().all(|n| !n.contains("restore")),
        "{siblings:?}"
    );
}

#[test]
fn restore_snapshot_flag_picks_older() {
    let tmp = tempfile::tempdir().unwrap();
    let source = seeded(tmp.path(), "source");
    let repo = tmp.path().join("repo");
    ok_backup(&source, &repo, &[]);
    let users = count(&source, "users");
    let mut c = ferrite();
    c.args([
        "user",
        "create",
        "--login",
        "second",
        "--email",
        "second@example.com",
        "--data",
    ])
    .arg(&source);
    ok(c);
    ok_backup(&source, &repo, &[]);
    let snaps = snapshots(&repo);
    assert_eq!(snaps.len(), 2);

    let newest = tmp.path().join("newest");
    let out = restore(&repo, &newest, &[]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    assert_eq!(count(&newest, "users"), users + 1);

    let older = tmp.path().join("older");
    let out = restore(&repo, &older, &["--snapshot", &snaps[0]]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    assert_eq!(count(&older, "users"), users);

    let out = restore(
        &repo,
        &tmp.path().join("nope"),
        &["--snapshot", "20000101T000000Z"],
    );
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert!(stderr(&out).contains("20000101T000000Z"), "{}", text(&out));
}

#[test]
fn backup_corrupt_snapshot_fails_integrity() {
    let tmp = tempfile::tempdir().unwrap();
    let source = seeded(tmp.path(), "source");
    let archive = tmp.path().join("b.tar.zst");
    let out = backup(&source, &archive, &["--test-corrupt-snapshot"]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert!(stderr(&out).contains("integrity"), "{}", text(&out));
    assert!(!archive.exists());
    assert!(local_snapshots(&source).is_empty());

    let repo = tmp.path().join("repo");
    let out = backup(&source, &repo, &["--test-corrupt-snapshot"]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert!(snapshots(&repo).is_empty());
    assert!(local_snapshots(&source).is_empty());
}

#[test]
fn backup_with_a_missing_blob_exits_2_and_a_damaged_one_fails() {
    let tmp = tempfile::tempdir().unwrap();
    let source = seeded(tmp.path(), "source");
    let (hash, file) = media_blob(&source);
    let bytes = std::fs::read(&file).unwrap();
    std::fs::remove_file(&file).unwrap();

    let archive = tmp.path().join("b.tar.zst");
    let out = backup(&source, &archive, &[]);
    assert_eq!(out.status.code(), Some(2), "{}", text(&out));
    assert!(
        stderr(&out).contains(&format!("missing artifact {hash}")),
        "{}",
        text(&out)
    );
    assert_eq!(
        archive_manifest(&archive)["missing"],
        serde_json::json!([hash])
    );
    let repo = tmp.path().join("repo");
    let out = backup(&source, &repo, &[]);
    assert_eq!(out.status.code(), Some(2), "{}", text(&out));
    let snaps = snapshots(&repo);
    let m = repo_manifest(&repo, &snaps[0]);
    assert_eq!(m["missing"], serde_json::json!([hash]));
    assert!(!hashes(&m).contains(&hash));
    // Both restore.
    let out = restore(&archive, &tmp.path().join("r1"), &[]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    let out = restore(&repo, &tmp.path().join("r2"), &[]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    assert!(
        stdout(&out).contains("1 artifacts were missing"),
        "{}",
        text(&out)
    );

    // A file whose content does not match its name fails the backup.
    let mut damaged = bytes;
    let last = damaged.len() - 1;
    damaged[last] ^= 0xff;
    std::fs::write(&file, damaged).unwrap();
    let a2 = tmp.path().join("b2.tar.zst");
    let out = backup(&source, &a2, &[]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert!(stderr(&out).contains(&hash), "{}", text(&out));
    assert!(!a2.exists());
    let repo2 = tmp.path().join("repo2");
    let out = backup(&source, &repo2, &[]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert!(snapshots(&repo2).is_empty());
    assert!(local_snapshots(&source).is_empty());
}

#[test]
fn backup_refuses_bad_targets() {
    let tmp = tempfile::tempdir().unwrap();
    let source = seeded(tmp.path(), "source");
    let archive = tmp.path().join("b.tar.zst");
    let out = backup(&source, &archive, &["--keep", "2"]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert!(stderr(&out).contains("--keep"), "{}", text(&out));
    std::fs::write(&archive, b"x").unwrap();
    let out = backup(&source, &archive, &[]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert_eq!(std::fs::read(&archive).unwrap(), b"x");
}

#[test]
fn backup_to_stdout_and_restore_from_stdin() {
    let tmp = tempfile::tempdir().unwrap();
    let source = seeded(tmp.path(), "source");
    let mut c = ferrite();
    c.args(["backup", "--to", "-", "--data"]).arg(&source);
    let out = ok(c);
    assert!(stderr(&out).contains("Backed up"), "{}", text(&out));
    assert_eq!(out.stdout[..4], [0x28, 0xb5, 0x2f, 0xfd], "a zstd frame");
    let target = tmp.path().join("restored");
    let mut c = ferrite();
    c.args(["restore", "-", "--data"])
        .arg(&target)
        .write_stdin(out.stdout);
    ok(c);
    assert_eq!(count(&source, "users"), count(&target, "users"));
    for hash in site_hashes(&source) {
        assert!(blob_path(&target.join("media"), &hash).is_file(), "{hash}");
    }
    let mut c = ferrite();
    c.args(["restore", "-", "--snapshot", "20260101T000000Z", "--data"])
        .arg(tmp.path().join("x"));
    let out = run(c);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
}

#[cfg(unix)]
#[test]
fn backup_files_mode_0600() {
    let tmp = tempfile::tempdir().unwrap();
    let source = seeded(tmp.path(), "source");
    let archive = tmp.path().join("b.tar.zst");
    ok_backup(&source, &archive, &[]);
    assert_eq!(mode(&archive), 0o600);
    let repo = tmp.path().join("repo");
    ok_backup(&source, &repo, &[]);
    let snap = repo.join("snapshots").join(&snapshots(&repo)[0]);
    for name in ["manifest.json", "ferrite.db", "ferrite.toml", "secret.key"] {
        assert_eq!(mode(&snap.join(name)), 0o600, "{name}");
    }
    for (src, dir) in [(&archive, "r1"), (&repo, "r2")] {
        let target = tmp.path().join(dir);
        let out = restore(src, &target, &[]);
        assert_eq!(out.status.code(), Some(0), "{}", text(&out));
        for name in ["ferrite.db", "ferrite.toml", "secret.key"] {
            assert_eq!(mode(&target.join(name)), 0o600, "{dir}/{name}");
        }
    }
}
