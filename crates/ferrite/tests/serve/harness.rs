//! The real `ferrite` binary as a child process: a data folder per profile (solo:
//! SQLite + local media; cluster: Postgres + in-process S3, skipped without
//! `FERRITE_TEST_PG_URL`), `ferrite serve` on a random port, and plain HTTP/1.1.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use ferrite_store::Store;
use ferrite_store_postgres::{PgOptions, PgStore};
use ferrite_store_sqlite::{SqliteOptions, SqliteStore};
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto::Builder as ConnBuilder;
use s3s::auth::SimpleAuth;
use s3s::service::S3ServiceBuilder;
use serde_json::Value;
use sqlx::{AssertSqlSafe, Connection, PgConnection};

const PG_URL_ENV: &str = "FERRITE_TEST_PG_URL";
const BIN: &str = env!("CARGO_BIN_EXE_ferrite");
/// `site.url`: requests go to a random port, but `Origin` must be this (F29).
pub const SITE_URL: &str = "http://127.0.0.1:8080";

pub fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("runtime")
}

/// A data folder for one profile, plus what keeps its backing services alive.
pub struct Profile {
    dir: tempfile::TempDir,
    /// Where an in-process SQLite store keeps its `ferrite.lock`: never the data folder, so
    /// no lock fd of the test process exists for a concurrently spawned child to inherit
    /// (posix_spawn/fork copy the fd table before `O_CLOEXEC` closes it) and hold briefly.
    lock_dir: tempfile::TempDir,
    env: Vec<(String, String)>,
    init_args: Vec<String>,
    pg: Option<(String, String)>,
    /// Runs the in-process S3 server (cluster profile); dropped last.
    _s3: Option<tokio::runtime::Runtime>,
}

impl Profile {
    pub fn solo() -> Self {
        Self {
            dir: tempfile::tempdir().expect("tempdir"),
            lock_dir: tempfile::tempdir().expect("tempdir"),
            env: Vec::new(),
            init_args: Vec::new(),
            pg: None,
            _s3: None,
        }
    }

    /// `None` (after a SKIP line) without `FERRITE_TEST_PG_URL`; panics in CI.
    pub fn cluster() -> Option<Self> {
        let Ok(url) = std::env::var(PG_URL_ENV) else {
            if std::env::var("CI").as_deref() == Ok("true") {
                panic!("{PG_URL_ENV} must be set in CI");
            }
            eprintln!("SKIP: Postgres serve test skipped ({PG_URL_ENV} is not set)");
            return None;
        };
        let schema = format!("t_{}", uuid::Uuid::now_v7().simple());
        let dir = tempfile::tempdir().expect("tempdir");
        let s3_root = dir.path().join("s3");
        std::fs::create_dir_all(s3_root.join("ferrite-test")).expect("bucket dir");
        let s3 = rt();
        let addr = s3.block_on(async {
            let fs = s3s_fs::FileSystem::new(&s3_root).expect("s3s-fs");
            let mut builder = S3ServiceBuilder::new(fs);
            builder.set_auth(SimpleAuth::from_single("test-access", "test-secret"));
            let service = builder.build();
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("bind");
            let addr = listener.local_addr().expect("addr");
            tokio::spawn(async move {
                while let Ok((socket, _)) = listener.accept().await {
                    let service = service.clone();
                    tokio::spawn(async move {
                        let http = ConnBuilder::new(TokioExecutor::new());
                        let _ = http.serve_connection(TokioIo::new(socket), service).await;
                    });
                }
            });
            addr
        });
        let env = [
            ("FERRITE_MEDIA__S3__ENDPOINT", format!("http://{addr}")),
            ("FERRITE_MEDIA__S3__ACCESS_KEY_ID", "test-access".to_owned()),
            (
                "FERRITE_MEDIA__S3__SECRET_ACCESS_KEY",
                "test-secret".to_owned(),
            ),
            ("FERRITE_MEDIA__S3__ALLOW_HTTP", "true".to_owned()),
            // Postgres + S3 is cluster mode: one site key for every process (R1).
            ("FERRITE_AUTH__SECRET_KEY", "5a".repeat(32)),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v))
        .collect();
        Some(Self {
            init_args: vec![
                "--db".into(),
                url.clone(),
                "--schema".into(),
                schema.clone(),
                "--media".into(),
                "s3://ferrite-test/site".into(),
            ],
            env,
            dir,
            lock_dir: tempfile::tempdir().expect("tempdir"),
            pg: Some((url, schema)),
            _s3: Some(s3),
        })
    }

    pub fn data(&self) -> &Path {
        self.dir.path()
    }

    /// Where the stored blob of artifact `hash` (64 hex) is a plain file: under the
    /// local media root (solo) or the in-process S3 server's bucket folder (cluster).
    pub fn artifact_blob(&self, hash: &str) -> std::path::PathBuf {
        let root = match self.pg {
            None => self.data().join("media"),
            Some(_) => self.dir.path().join("s3/ferrite-test/site"),
        };
        root.join("artifacts").join(&hash[..2]).join(hash)
    }

    /// `ferrite <args> --data <data>` with the profile's environment only.
    pub fn cmd(&self, args: &[&str]) -> Command {
        self.cmd_in(self.data(), args)
    }

    /// [`Self::cmd`] with another data folder.
    pub fn cmd_in(&self, data: &Path, args: &[&str]) -> Command {
        let mut c = Command::new(BIN);
        c.env_clear().envs(self.env.iter().cloned()).args(args);
        c.arg("--data").arg(data);
        c
    }

    pub fn init(&self) {
        let mut c = Command::new(BIN);
        c.env_clear()
            .envs(self.env.iter().cloned())
            .args(["init", "--url", SITE_URL])
            .args(&self.init_args)
            .arg("--data")
            .arg(self.data());
        let out = c.output().expect("run init");
        assert!(out.status.success(), "init: {out:?}");
    }

    /// The profile's store, opened directly (SQLite: only while `serve` is not running; its
    /// folder lock lives outside the data folder, see `lock_dir`).
    pub async fn store(&self) -> Box<dyn Store> {
        match &self.pg {
            None => Box::new(
                SqliteStore::open(SqliteOptions {
                    lock_dir: Some(self.lock_dir.path().to_path_buf()),
                    ..SqliteOptions::new(self.data().join("ferrite.db"))
                })
                .await
                .expect("open sqlite"),
            ),
            Some((url, schema)) => Box::new(
                PgStore::open(PgOptions {
                    max_connections: 2,
                    schema: Some(schema.clone()),
                    ..PgOptions::new(url.clone())
                })
                .await
                .expect("open pg"),
            ),
        }
    }

    /// `ferrite user create --password-stdin` (before `serve` takes the SQLite lock).
    pub fn create_admin(&self, login: &str, password: &str) {
        let mut child = self
            .cmd(&[
                "user",
                "create",
                "--login",
                login,
                "--email",
                &format!("{login}@example.com"),
                "--password-stdin",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn user create");
        writeln!(child.stdin.take().expect("stdin"), "{password}").expect("write password");
        let out = child.wait_with_output().expect("user create");
        assert!(out.status.success(), "user create: {out:?}");
        let printed = String::from_utf8_lossy(&out.stdout);
        assert!(
            !printed.contains(password),
            "stdin password echoed: {printed}"
        );
    }
}

impl Drop for Profile {
    fn drop(&mut self) {
        let Some((url, schema)) = self.pg.take() else {
            return;
        };
        let sql = format!("DROP SCHEMA IF EXISTS \"{schema}\" CASCADE");
        // Bounded, so an unreachable Postgres cannot hang the test at cleanup.
        let done = std::thread::spawn(move || {
            rt().block_on(async move {
                tokio::time::timeout(Duration::from_secs(10), async {
                    let mut conn = PgConnection::connect(&url).await?;
                    sqlx::raw_sql(AssertSqlSafe(sql)).execute(&mut conn).await?;
                    conn.close().await
                })
                .await
                .map_err(|_| "timed out".to_owned())?
                .map_err(|e| e.to_string())
            })
        })
        .join();
        if let Ok(Err(e)) = done {
            eprintln!("warning: could not drop test schema {schema}: {e}");
        }
    }
}

/// `ferrite serve`, killed on drop if still running. Its stderr (the log) is collected
/// and printed only when the test fails.
pub struct Server {
    pub child: Child,
    pub addr: SocketAddr,
    log: Arc<Mutex<Vec<u8>>>,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if std::thread::panicking() {
            let log = self.log.lock().map(|l| l.clone()).unwrap_or_default();
            eprintln!(
                "---- ferrite serve stderr ----\n{}",
                String::from_utf8_lossy(&log)
            );
        }
    }
}

impl Server {
    /// Spawns `serve` on a random port and waits for its "Listening on" line.
    pub fn start(p: &Profile) -> Self {
        Self::start_in(p, p.data())
    }

    /// [`Self::start`] with another data folder (same environment).
    pub fn start_in(p: &Profile, data: &Path) -> Self {
        Self::spawn(p.cmd_in(data, &["serve"]))
    }

    /// Spawns `cmd` (a `serve` command) on a random port and waits for its "Listening
    /// on" line.
    pub fn spawn(mut cmd: Command) -> Self {
        cmd.env("FERRITE_SERVER__LISTEN", "127.0.0.1:0")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn().expect("spawn serve");
        let stdout = child.stdout.take().expect("stdout");
        let mut stderr = child.stderr.take().expect("stderr");
        let log = Arc::new(Mutex::new(Vec::new()));
        let sink = log.clone();
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            while let Ok(n) = stderr.read(&mut buf) {
                if n == 0 {
                    return;
                }
                if let Ok(mut l) = sink.lock() {
                    l.extend_from_slice(&buf[..n]);
                }
            }
        });
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { return };
                if let Some(url) = line.strip_prefix("Listening on http://") {
                    let _ = tx.send(url.trim_end_matches('/').to_owned());
                }
            }
        });
        let addr = rx
            .recv_timeout(Duration::from_secs(60))
            .map(|a| a.parse().expect("listen address"));
        let server = Self {
            child,
            addr: addr.unwrap_or_else(|_| SocketAddr::from(([0, 0, 0, 0], 0))),
            log,
        };
        assert!(addr.is_ok(), "serve did not report its address");
        server
    }

    pub fn get(&self, path: &str) -> Resp {
        request(self.addr, "GET", path, &[], None)
    }

    pub fn signal(&self, name: &str) {
        let pid = self.child.id().to_string();
        let ok = Command::new("kill")
            .args([format!("-{name}"), pid])
            .status()
            .expect("kill");
        assert!(ok.success());
    }

    /// The exit status once the process ends (panics after `limit`).
    pub fn wait_exit(&mut self, limit: Duration) -> ExitStatus {
        let start = Instant::now();
        loop {
            if let Some(status) = self.child.try_wait().expect("try_wait") {
                return status;
            }
            assert!(
                start.elapsed() < limit,
                "serve did not exit within {limit:?}"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// `signal`, then the exit status (panics after 15 s).
    pub fn stop_with(mut self, signal: &str) -> ExitStatus {
        self.signal(signal);
        self.wait_exit(Duration::from_secs(15))
    }

    /// The collected log so far.
    pub fn log(&self) -> String {
        String::from_utf8_lossy(&self.log.lock().map(|l| l.clone()).unwrap_or_default())
            .into_owned()
    }

    /// Waits until the log contains `needle` (the reader thread may lag the process).
    pub fn wait_log(&self, needle: &str, limit: Duration) {
        let start = Instant::now();
        while !self.log().contains(needle) {
            assert!(
                start.elapsed() < limit,
                "no {needle:?} in log: {}",
                self.log()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

#[derive(Debug, Clone)]
pub struct Resp {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Resp {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or_else(|e| panic!("{e}: {self:?}"))
    }

    /// Asserts a problem document with `status` and `type` `/api/v1/problems/<slug>`.
    pub fn problem(&self, status: u16, slug: &str) -> Value {
        assert_eq!(self.status, status, "{self:?}");
        assert_eq!(
            self.header("content-type"),
            Some("application/problem+json"),
            "{self:?}"
        );
        let p = self.json();
        assert_eq!(p["type"], format!("/api/v1/problems/{slug}"), "{p}");
        p
    }
}

/// One HTTP/1.1 request (`Connection: close`, identity encoding, chunked or
/// `Content-Length` bodies; a JSON `body` gets its `Content-Type`).
pub fn request(
    addr: SocketAddr,
    method: &str,
    path: &str,
    headers: &[(&str, String)],
    body: Option<&Value>,
) -> Resp {
    let body = body.map(|b| b.to_string()).unwrap_or_default();
    let mut headers = headers.to_vec();
    if !body.is_empty() {
        headers.push(("Content-Type", "application/json".to_owned()));
    }
    request_bytes(addr, method, path, &headers, body.as_bytes())
}

/// [`request`] with a raw `body` (its `Content-Type`, if any, in `headers`).
pub fn request_bytes(
    addr: SocketAddr,
    method: &str,
    path: &str,
    headers: &[(&str, String)],
    body: &[u8],
) -> Resp {
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_secs(5)).expect("connect");
    s.set_read_timeout(Some(Duration::from_secs(30)))
        .expect("read timeout");
    s.set_write_timeout(Some(Duration::from_secs(5)))
        .expect("write timeout");
    let mut head = format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n");
    for (name, value) in headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    if !body.is_empty() || !matches!(method, "GET" | "HEAD") {
        head.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    head.push_str("\r\n");
    s.write_all(head.as_bytes()).expect("write head");
    s.write_all(body).expect("write body");
    let mut raw = Vec::new();
    // A refusal sent before the request body is read (the origin check of login) can end
    // in a reset once the response is out. That is accepted only for a response with a
    // Content-Length whose whole body arrived (checked below).
    let reset = match s.read_to_end(&mut raw) {
        Ok(_) => false,
        Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => true,
        Err(e) => panic!("{method} {path}: read: {e}"),
    };
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("header end");
    let text = String::from_utf8_lossy(&raw[..split]).into_owned();
    let mut lines = text.split("\r\n");
    let status = lines
        .next()
        .and_then(|l| l.split(' ').nth(1))
        .and_then(|c| c.parse().ok())
        .expect("status line");
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(n, v)| (n.trim().to_owned(), v.trim().to_owned()))
        .collect();
    let mut body = raw[split + 4..].to_vec();
    let chunked = headers
        .iter()
        .any(|(n, v)| n.eq_ignore_ascii_case("transfer-encoding") && v.contains("chunked"));
    let length = headers
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case("content-length"))
        .map(|(_, v)| v.parse::<usize>().expect("content-length"));
    assert!(
        !reset || (!chunked && length.is_some()),
        "{method} {path}: connection reset without a Content-Length response"
    );
    if chunked {
        body = dechunk(&body);
    } else if let Some(len) = length {
        assert_eq!(body.len(), len, "{method} {path}: truncated body");
    }
    Resp {
        status,
        headers,
        body,
    }
}

fn dechunk(mut data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let line_end = data
            .windows(2)
            .position(|w| w == b"\r\n")
            .expect("chunk size line");
        let size_text = String::from_utf8_lossy(&data[..line_end]).into_owned();
        let size = usize::from_str_radix(size_text.split(';').next().unwrap_or("").trim(), 16)
            .expect("chunk size");
        data = &data[line_end + 2..];
        if size == 0 {
            return out;
        }
        out.extend_from_slice(&data[..size]);
        data = &data[size + 2..];
    }
}
