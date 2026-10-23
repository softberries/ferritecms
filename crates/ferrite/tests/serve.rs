#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! `ferrite seed demo` + `ferrite serve` end to end, as a child process, on both
//! profiles: solo (SQLite + local media) and cluster (Postgres + in-process S3; skipped
//! without `FERRITE_TEST_PG_URL`).

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use ferrite_core::{Actor, ContentDraft, ContentService, PermalinkRules};
use ferrite_domain::{ContentId, ContentStatus, NewContent, Role};
use ferrite_store::Store;
use ferrite_store_postgres::{PgOptions, PgStore};
use ferrite_store_sqlite::{SqliteOptions, SqliteStore};
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto::Builder as ConnBuilder;
use s3s::auth::SimpleAuth;
use s3s::service::S3ServiceBuilder;
use serde_json::json;
use sqlx::{AssertSqlSafe, Connection, PgConnection, SqliteConnection};

const PG_URL_ENV: &str = "FERRITE_TEST_PG_URL";
const BIN: &str = env!("CARGO_BIN_EXE_ferrite");
const SITE_URL: &str = "http://127.0.0.1:8080";

/// The newest seeded post (first on the home page) and another one.
const NEWEST_TITLE: &str = "Welcome to FerriteCMS";
const NEWEST_SLUG: &str = "welcome-to-ferritecms";
const OTHER_TITLE: &str = "Writing your first plugin";
const DRAFT_SLUG: &str = "secret-draft";

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("runtime")
}

/// A data folder for one profile, plus what keeps its backing services alive.
struct Profile {
    dir: tempfile::TempDir,
    env: Vec<(String, String)>,
    init_args: Vec<String>,
    pg: Option<(String, String)>,
    /// Runs the in-process S3 server (cluster profile); dropped last.
    _s3: Option<tokio::runtime::Runtime>,
}

impl Profile {
    fn solo() -> Self {
        Self {
            dir: tempfile::tempdir().expect("tempdir"),
            env: Vec::new(),
            init_args: Vec::new(),
            pg: None,
            _s3: None,
        }
    }

    /// `None` (after a SKIP line) without `FERRITE_TEST_PG_URL`; panics in CI.
    fn cluster() -> Option<Self> {
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
            pg: Some((url, schema)),
            _s3: Some(s3),
        })
    }

    fn data(&self) -> &Path {
        self.dir.path()
    }

    fn cmd(&self, args: &[&str]) -> Command {
        self.cmd_in(self.data(), args)
    }

    fn cmd_in(&self, data: &Path, args: &[&str]) -> Command {
        let mut c = Command::new(BIN);
        c.env_clear().envs(self.env.iter().cloned()).args(args);
        c.arg("--data").arg(data);
        c
    }

    fn init(&self) {
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

    async fn store(&self) -> Box<dyn Store> {
        match &self.pg {
            None => Box::new(
                SqliteStore::open(SqliteOptions::new(self.data().join("ferrite.db")))
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
}

impl Drop for Profile {
    fn drop(&mut self) {
        let Some((url, schema)) = self.pg.take() else {
            return;
        };
        let sql = format!("DROP SCHEMA IF EXISTS \"{schema}\" CASCADE");
        let done = std::thread::spawn(move || {
            rt().block_on(async move {
                let mut conn = PgConnection::connect(&url).await?;
                sqlx::raw_sql(AssertSqlSafe(sql)).execute(&mut conn).await?;
                conn.close().await
            })
        })
        .join();
        if let Ok(Err(e)) = done {
            eprintln!("warning: could not drop test schema {schema}: {e}");
        }
    }
}

/// A draft post written straight to the store (before `serve` takes the lock).
async fn insert_draft(store: &dyn Store) {
    let admin = store
        .read()
        .users()
        .get_by_login("admin")
        .await
        .expect("users")
        .expect("seeded admin");
    let tx = store.begin().await.expect("begin");
    tx.content()
        .insert(&NewContent {
            id: ContentId::new(),
            kind: "post".into(),
            status: ContentStatus::Draft,
            slug: DRAFT_SLUG.into(),
            parent_id: None,
            author_id: admin.id,
            title: "Secret Draft Title".into(),
            excerpt: String::new(),
            body: json!({"type": "doc", "content": []}),
            body_html: None,
            fields: json!({}),
            menu_order: 0,
            published_at: None,
            scheduled_at: None,
        })
        .await
        .expect("insert draft");
    tx.commit().await.expect("commit");
}

/// `ferrite serve`, killed on drop if still running. Its stderr (the log) is collected
/// and printed only when the test fails.
struct Server {
    child: Child,
    addr: SocketAddr,
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
    fn start(p: &Profile) -> Self {
        Self::start_in(p, p.data())
    }

    /// [`Self::start`] with another data folder (same environment).
    fn start_in(p: &Profile, data: &Path) -> Self {
        Self::spawn(p.cmd_in(data, &["serve"]))
    }

    /// Spawns `cmd` (a `serve` command) on a random port and waits for its "Listening
    /// on" line.
    fn spawn(mut cmd: Command) -> Self {
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

    fn get(&self, path: &str) -> Resp {
        get(self.addr, path)
    }

    fn signal(&self, name: &str) {
        let pid = self.child.id().to_string();
        let ok = Command::new("kill")
            .args([format!("-{name}"), pid])
            .status()
            .expect("kill");
        assert!(ok.success());
    }

    /// The exit status once the process ends (panics after `limit`).
    fn wait_exit(&mut self, limit: Duration) -> std::process::ExitStatus {
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
    fn stop_with(mut self, signal: &str) -> std::process::ExitStatus {
        self.signal(signal);
        self.wait_exit(Duration::from_secs(15))
    }

    /// The collected log so far.
    fn log(&self) -> String {
        String::from_utf8_lossy(&self.log.lock().map(|l| l.clone()).unwrap_or_default())
            .into_owned()
    }

    /// Waits until the log contains `needle` (the reader thread may lag the process).
    fn wait_log(&self, needle: &str, limit: Duration) {
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

#[derive(Debug)]
struct Resp {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Resp {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

/// A minimal HTTP/1.1 GET (`Connection: close`, identity encoding, chunked bodies).
fn get(addr: SocketAddr, path: &str) -> Resp {
    let mut s = TcpStream::connect(addr).expect("connect");
    s.set_read_timeout(Some(Duration::from_secs(30)))
        .expect("timeout");
    write!(
        s,
        "GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"
    )
    .expect("write");
    let mut raw = Vec::new();
    s.read_to_end(&mut raw).expect("read");
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("header end");
    let head = String::from_utf8_lossy(&raw[..split]).into_owned();
    let mut lines = head.split("\r\n");
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
    if chunked {
        body = dechunk(&body);
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

/// The first `/_media/…` URL in `html` (an `src` or `srcset` entry; MiniJinja escapes
/// `/` as `&#x2f;`).
fn media_url(html: &str) -> Option<String> {
    let html = html.replace("&#x2f;", "/");
    let start = html.find("/_media/")?;
    let rest = &html[start..];
    let end = rest.find(['"', ' ', '\'', ',']).unwrap_or(rest.len());
    Some(rest[..end].to_owned())
}

fn seed(p: &Profile, extra: &[&str]) -> std::process::Output {
    let mut args = vec!["seed", "demo"];
    args.extend_from_slice(extra);
    p.cmd(&args).output().expect("run seed")
}

fn serve_seeded_site(p: Profile) {
    p.init();
    let out = seed(&p, &[]);
    assert!(out.status.success(), "seed: {out:?}");
    // A second seed without --force refuses and changes nothing.
    let again = seed(&p, &[]);
    assert!(
        !again.status.success(),
        "second seed must refuse: {again:?}"
    );
    let err = String::from_utf8_lossy(&again.stderr);
    assert!(err.contains("--force"), "{err}");

    rt().block_on(async {
        let store = p.store().await;
        insert_draft(store.as_ref()).await;
    });

    let server = Server::start(&p);
    let home = server.get("/");
    assert_eq!(home.status, 200, "{home:?}");
    let html = home.text();
    assert!(html.contains("FerriteCMS Demo"), "{html}");
    assert!(html.contains(NEWEST_TITLE), "{html}");
    assert!(html.contains(OTHER_TITLE), "{html}");
    assert!(!html.contains("Secret Draft Title"), "{html}");

    for path in [
        format!("/{NEWEST_SLUG}/"),
        "/about/".to_owned(),
        "/about/team/".to_owned(),
        "/contact/".to_owned(),
        "/category/news/".to_owned(),
        "/tag/rust/".to_owned(),
        "/feed/".to_owned(),
    ] {
        let r = server.get(&path);
        assert_eq!(r.status, 200, "{path}: {r:?}");
    }
    let post = server.get(&format!("/{NEWEST_SLUG}/")).text();
    assert!(post.contains(NEWEST_TITLE), "{post}");
    assert!(post.contains("<pre><code"), "code block rendered: {post}");
    let team = server.get("/about/team/").text();
    assert!(team.contains("Team"), "{team}");

    // srcset lists each width once (the 1280 px original vs a 1280 px crop).
    let unescaped = html.replace("&#x2f;", "/");
    let mut srcsets = 0;
    for part in unescaped.split("srcset=\"").skip(1) {
        let set = part.split('"').next().unwrap_or("");
        let mut widths: Vec<&str> = set
            .split(',')
            .filter_map(|c| c.split_whitespace().nth(1))
            .collect();
        let n = widths.len();
        widths.sort_unstable();
        widths.dedup();
        assert_eq!(widths.len(), n, "duplicate widths in srcset {set:?}");
        srcsets += 1;
    }
    assert!(srcsets > 0, "the cards have a srcset");
    // The sample image: a featured image on the home page cards.
    let url = media_url(&html).expect("a /_media URL on the home page");
    let img = server.get(&url);
    assert_eq!(img.status, 200, "{url}: {img:?}");
    assert_eq!(img.header("content-type"), Some("image/png"), "{img:?}");
    assert!(img.body.starts_with(b"\x89PNG"), "png bytes");

    let draft = server.get(&format!("/{DRAFT_SLUG}/"));
    assert_eq!(draft.status, 404, "drafts are not public");

    let status = server.stop_with("TERM");
    assert!(status.success(), "serve exit status: {status:?}");
    // A clean shutdown leaves the node registry.
    let nodes =
        rt().block_on(async { p.store().await.read().nodes().list().await.expect("nodes") });
    assert!(nodes.is_empty(), "node rows after shutdown: {nodes:?}");

    // --force adds another batch with fresh slugs.
    let forced = seed(&p, &["--force"]);
    assert!(forced.status.success(), "seed --force: {forced:?}");
    let posts = rt().block_on(async {
        let store = p.store().await;
        store
            .read()
            .content()
            .count(&ferrite_domain::ContentQuery {
                kind: Some("post".into()),
                ..Default::default()
            })
            .await
            .expect("count")
    });
    assert_eq!(posts, 12 * 2 + 1, "two batches plus the draft");
}

#[test]
fn serve_solo() {
    serve_seeded_site(Profile::solo());
}

#[test]
fn serve_cluster() {
    let Some(p) = Profile::cluster() else { return };
    serve_seeded_site(p);
}

/// Two `ferrite serve` processes (own data folders, one Postgres schema, one S3 bucket):
/// an edit written through core by a third process reaches both within 1 s (the exit
/// criterion across real processes; the in-process cluster tests cover the rest).
#[test]
fn two_processes_share_invalidation() {
    let Some(p) = Profile::cluster() else { return };
    p.init();
    let out = seed(&p, &[]);
    assert!(out.status.success(), "seed: {out:?}");
    let second = tempfile::tempdir().expect("tempdir");
    std::fs::copy(
        p.data().join("ferrite.toml"),
        second.path().join("ferrite.toml"),
    )
    .expect("copy config");
    let servers = [Server::start(&p), Server::start_in(&p, second.path())];
    let path = format!("/{NEWEST_SLUG}/");
    for s in &servers {
        wait_for(Duration::from_secs(10), || {
            s.get(&path).text().contains(NEWEST_TITLE)
        });
    }

    let title = "Edited across processes";
    let saved = rt().block_on(async {
        let store: Arc<dyn Store> = p.store().await.into();
        let admin = store
            .read()
            .users()
            .get_by_login("admin")
            .await
            .expect("users")
            .expect("seeded admin");
        let actor = Actor {
            user: admin.id,
            role: Role::Admin,
        };
        let item = store
            .read()
            .content()
            .get_by_slug("post", None, NEWEST_SLUG)
            .await
            .expect("content")
            .expect("seeded post");
        let terms = store
            .read()
            .terms()
            .for_item(item.id)
            .await
            .expect("terms")
            .into_iter()
            .map(|t| t.id)
            .collect();
        let draft = ContentDraft {
            kind: item.kind.clone(),
            title: title.into(),
            slug: Some(item.slug.clone()),
            parent_id: item.parent_id,
            excerpt: Some(item.excerpt.clone()),
            body: item.body.clone(),
            fields: item.fields.clone(),
            menu_order: item.menu_order,
            terms,
        };
        let svc = ContentService::new(store.clone(), Arc::new(PermalinkRules::default()));
        // Timed from the start of the save.
        let saved = Instant::now();
        svc.update(&actor, item.id, item.version, draft)
            .await
            .expect("update");
        store.close().await;
        saved
    });
    let mut took = Vec::new();
    for s in &servers {
        wait_for(Duration::from_secs(10), || {
            s.get(&path).text().contains(title)
        });
        took.push(saved.elapsed());
    }
    eprintln!("two processes: edit served by both after {took:?}");
    for (i, t) in took.iter().enumerate() {
        assert!(*t < Duration::from_secs(1), "process {i}: {t:?}");
    }
    for s in servers {
        assert!(s.stop_with("TERM").success());
    }
}

/// Polls `f` every 10 ms; panics after `limit`.
fn wait_for(limit: Duration, mut f: impl FnMut() -> bool) {
    let start = Instant::now();
    while !f() {
        assert!(
            start.elapsed() < limit,
            "condition not met within {limit:?}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn serve_refuses_incompatible_schema() {
    let p = Profile::solo();
    p.init();
    // A newer schema that requires a newer binary (as in `tests/cli.rs`).
    let db = p.data().join("ferrite.db");
    rt().block_on(async {
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
    let out = p
        .cmd(&["serve"])
        .env("FERRITE_SERVER__LISTEN", "127.0.0.1:0")
        .output()
        .expect("run serve");
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("requires ferrite >= 99.0.0"), "{err}");
}

/// Startup checks the permalink options against the router (rulings R3, R13).
#[test]
fn serve_refuses_permalink_shadowing_a_route() {
    for (structure, why) in [
        ("/category/%slug%/", "taxonomy"),
        ("/search/%slug%/", "route word"),
        ("/2024/%slug%/", "year archive"),
        ("/media/%slug%/", "content type"),
    ] {
        let p = Profile::solo();
        p.init();
        rt().block_on(async {
            let store = p.store().await;
            store
                .read()
                .options()
                .set("permalink.post", &json!(structure), true)
                .await
                .expect("set option");
            store.close().await;
        });
        let out = p
            .cmd(&["serve"])
            .env("FERRITE_SERVER__LISTEN", "127.0.0.1:0")
            .output()
            .expect("run serve");
        assert_eq!(out.status.code(), Some(1), "{structure}: {out:?}");
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(
            err.contains("invalid permalink option") && err.contains(why),
            "{structure}: {err}"
        );
    }
}

/// I1 / R17: after a `kill -9` on SQLite the restarted process does not wait for the
/// crashed one's row to go stale (30 s), and a `site.url` changed in between is not
/// refused: the data-folder lock already proves the old process is gone.
#[test]
fn solo_restart_after_kill_is_immediate() {
    let p = Profile::solo();
    p.init();
    let mut first = Server::start(&p);
    assert_eq!(first.get("/healthz").status, 200);
    first.signal("KILL");
    first.wait_exit(Duration::from_secs(15));
    drop(first);

    let mut cmd = p.cmd(&["serve"]);
    cmd.env("FERRITE_SITE__URL", "http://127.0.0.1:9");
    let start = Instant::now();
    let second = Server::spawn(cmd);
    let took = start.elapsed();
    assert!(
        took < Duration::from_secs(15),
        "restart took {took:?}: {}",
        second.log()
    );
    assert!(
        !second.log().contains("another node is live"),
        "{}",
        second.log()
    );
    assert_eq!(second.get("/healthz").status, 200);
    let status = second.stop_with("TERM");
    assert!(status.success(), "{status:?}");
}

/// SIGINT shuts down cleanly (exit 0) like SIGTERM.
#[test]
fn sigint_shuts_down_cleanly() {
    let p = Profile::solo();
    p.init();
    let server = Server::start(&p);
    assert_eq!(server.get("/healthz").status, 200);
    let status = server.stop_with("INT");
    assert!(status.success(), "{status:?}");
}

/// A second signal during the drain exits at once with 128 + the signal number.
#[test]
fn second_signal_during_drain_exits_immediately() {
    let p = Profile::solo();
    p.init();
    let mut server = Server::start(&p);
    // An unfinished request keeps the drain waiting (up to 10 s).
    let mut slow = TcpStream::connect(server.addr).expect("connect");
    write!(slow, "GET / HTTP/1.1\r\nHost: 127.0.0.1\r\n").expect("partial request");
    std::thread::sleep(Duration::from_millis(200));
    server.signal("TERM");
    server.wait_log("draining connections", Duration::from_secs(10));
    assert!(
        server.child.try_wait().expect("try_wait").is_none(),
        "still draining"
    );
    server.signal("INT");
    let status = server.wait_exit(Duration::from_secs(3));
    assert_eq!(status.code(), Some(130), "{status:?}");
    server.wait_log("second signal during shutdown", Duration::from_secs(3));
    drop(slow);
}

/// A connection that never finishes its request cannot hold shutdown past the bound:
/// 10 s drain, then the process exits 0 well within 15 s of the signal.
#[test]
fn stuck_connection_does_not_block_shutdown() {
    let p = Profile::solo();
    p.init();
    let mut server = Server::start(&p);
    let mut stuck = TcpStream::connect(server.addr).expect("connect");
    write!(stuck, "GET / HTTP/1.1\r\nHost: 127.0.0.1\r\n").expect("partial request");
    std::thread::sleep(Duration::from_millis(200));
    let start = Instant::now();
    server.signal("TERM");
    let status = server.wait_exit(Duration::from_secs(15));
    let took = start.elapsed();
    assert!(status.success(), "{status:?}");
    assert!(took >= Duration::from_secs(9), "drained first: {took:?}");
    server.wait_log("connections still open after", Duration::from_secs(3));
    drop(stuck);
}
