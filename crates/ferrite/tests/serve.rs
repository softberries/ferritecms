#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! `ferrite seed demo` + `ferrite serve` end to end, as a child process, on both
//! profiles: solo (SQLite + local media) and cluster (Postgres + in-process S3; skipped
//! without `FERRITE_TEST_PG_URL`).

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use ferrite_domain::{ContentId, ContentStatus, NewContent};
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
        let mut c = Command::new(BIN);
        c.env_clear().envs(self.env.iter().cloned()).args(args);
        c.arg("--data").arg(self.data());
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
                    url: url.clone(),
                    max_connections: 2,
                    schema: Some(schema.clone()),
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

/// `ferrite serve`, killed on drop if still running.
struct Server {
    child: Child,
    addr: SocketAddr,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Server {
    /// Spawns `serve` on a random port and waits for its "Listening on" line.
    fn start(p: &Profile) -> Self {
        let mut cmd = p.cmd(&["serve"]);
        cmd.env("FERRITE_SERVER__LISTEN", "127.0.0.1:0")
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        let mut child = cmd.spawn().expect("spawn serve");
        let stdout = child.stdout.take().expect("stdout");
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { return };
                if let Some(url) = line.strip_prefix("Listening on http://") {
                    let _ = tx.send(url.trim_end_matches('/').to_owned());
                }
            }
        });
        let addr = match rx.recv_timeout(Duration::from_secs(60)) {
            Ok(a) => a.parse().expect("listen address"),
            Err(_) => {
                let _ = child.kill();
                panic!("serve did not report its address: {:?}", child.wait());
            }
        };
        Self { child, addr }
    }

    fn get(&self, path: &str) -> Resp {
        get(self.addr, path)
    }

    /// SIGTERM, then the exit status (panics after 15 s).
    fn terminate(mut self) -> std::process::ExitStatus {
        let pid = self.child.id().to_string();
        let ok = Command::new("kill")
            .args(["-TERM", &pid])
            .status()
            .expect("kill");
        assert!(ok.success());
        let start = Instant::now();
        loop {
            if let Some(status) = self.child.try_wait().expect("try_wait") {
                return status;
            }
            assert!(
                start.elapsed() < Duration::from_secs(15),
                "serve did not exit within 15 s of SIGTERM"
            );
            std::thread::sleep(Duration::from_millis(50));
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

    // The sample image: a featured image on the home page cards.
    let url = media_url(&html).expect("a /_media URL on the home page");
    let img = server.get(&url);
    assert_eq!(img.status, 200, "{url}: {img:?}");
    assert_eq!(img.header("content-type"), Some("image/png"), "{img:?}");
    assert!(img.body.starts_with(b"\x89PNG"), "png bytes");

    let draft = server.get(&format!("/{DRAFT_SLUG}/"));
    assert_eq!(draft.status, 404, "drafts are not public");

    let status = server.terminate();
    assert!(status.success(), "serve exit status: {status:?}");

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
