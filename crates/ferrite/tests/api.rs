#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! The `/api/v1` API end to end against the real `ferrite` binary (M2a-1 task 13), on
//! both profiles: solo (SQLite + local media) and cluster (Postgres + in-process S3;
//! skipped without `FERRITE_TEST_PG_URL`). `init` → `user create --password-stdin` →
//! `serve` on a free port, then a browser-like session (cookie + CSRF, `Origin` =
//! `site.url`, F29) and bearer tokens over plain HTTP/1.1.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto::Builder as ConnBuilder;
use s3s::auth::SimpleAuth;
use s3s::service::S3ServiceBuilder;
use serde_json::{Value, json};
use sqlx::{AssertSqlSafe, Connection, PgConnection};

const PG_URL_ENV: &str = "FERRITE_TEST_PG_URL";
const BIN: &str = env!("CARGO_BIN_EXE_ferrite");
/// `site.url`: requests go to a random port, but `Origin` must be this (F29).
const SITE_URL: &str = "http://127.0.0.1:8080";
const ADMIN_PASSWORD: &str = "correct horse battery staple";
const EDITOR_PASSWORD: &str = "editor passphrase 2026";
/// Tokens a user can hold (constraints: at most 50).
const MAX_TOKENS: usize = 50;

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
            eprintln!("SKIP: Postgres API test skipped ({PG_URL_ENV} is not set)");
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
            // Postgres + S3 is cluster mode: the site key must be configured (R1).
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

    /// `ferrite user create --password-stdin` (before `serve` takes the SQLite lock).
    fn create_admin(&self, login: &str, password: &str) {
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
        let mut cmd = p.cmd(&["serve"]);
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

    /// SIGTERM, then the exit status (panics after 15 s, the bound `serve.rs` uses).
    fn terminate(mut self) -> std::process::ExitStatus {
        let ok = Command::new("kill")
            .args(["-TERM".to_owned(), self.child.id().to_string()])
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
                "serve did not exit within 15 s"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

#[derive(Debug)]
struct Resp {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
}

impl Resp {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or_else(|e| panic!("{e}: {self:?}"))
    }

    /// Asserts a problem document with `status` and `type` `/api/v1/problems/<slug>`.
    fn problem(&self, status: u16, slug: &str) -> Value {
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

/// One HTTP/1.1 request (`Connection: close`, identity encoding).
fn request(
    addr: SocketAddr,
    method: &str,
    path: &str,
    headers: &[(&str, String)],
    body: Option<&Value>,
) -> Resp {
    let mut s = TcpStream::connect(addr).expect("connect");
    s.set_read_timeout(Some(Duration::from_secs(30)))
        .expect("timeout");
    let mut head = format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n");
    for (name, value) in headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    let body = body.map(|b| b.to_string()).unwrap_or_default();
    if !body.is_empty() || !matches!(method, "GET" | "HEAD") {
        head.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    if !body.is_empty() {
        head.push_str("Content-Type: application/json\r\n");
    }
    head.push_str("\r\n");
    s.write_all(head.as_bytes()).expect("write head");
    s.write_all(body.as_bytes()).expect("write body");
    let mut raw = Vec::new();
    // A refusal sent before the request body is read (the origin check of login) ends
    // in a reset once the response is out; what arrived is the whole response, which
    // the Content-Length check below confirms.
    if let Err(e) = s.read_to_end(&mut raw) {
        assert!(
            e.kind() == std::io::ErrorKind::ConnectionReset && !raw.is_empty(),
            "{method} {path}: read: {e}"
        );
    }
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
    if chunked {
        body = dechunk(&body);
    } else if let Some((_, len)) = headers
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case("content-length"))
    {
        assert_eq!(body.len(), len.parse::<usize>().unwrap(), "{method} {path}");
    }
    Resp {
        status,
        headers,
        body: String::from_utf8_lossy(&body).into_owned(),
    }
}

/// Every response of the run, for the secret checks at the end; `issued` is the token
/// a response legitimately shows (its creation).
#[derive(Default)]
struct Seen {
    responses: Vec<(Resp, Option<String>)>,
    tokens: Vec<String>,
    cookies: Vec<String>,
}

/// How a request authenticates.
#[derive(Clone, Copy)]
enum Auth<'a> {
    None,
    /// A browser: `Origin` = `site.url`, the cookie and (unsafe methods) the CSRF token.
    Browser(&'a Browser),
    Bearer(&'a str),
}

#[derive(Default, Clone)]
struct Browser {
    cookie: Option<String>,
    csrf: Option<String>,
}

/// The site under test plus the response log.
struct Site {
    addr: SocketAddr,
    seen: Mutex<Seen>,
}

impl Site {
    fn send(&self, auth: Auth<'_>, method: &str, path: &str, body: Option<&Value>) -> Resp {
        self.send_with(auth, method, path, body, &[])
    }

    fn send_with(
        &self,
        auth: Auth<'_>,
        method: &str,
        path: &str,
        body: Option<&Value>,
        extra: &[(&str, String)],
    ) -> Resp {
        let mut headers: Vec<(&str, String)> = extra.to_vec();
        match auth {
            Auth::None => {}
            Auth::Browser(b) => {
                headers.push(("Origin", SITE_URL.to_owned()));
                if let Some(c) = &b.cookie {
                    headers.push(("Cookie", format!("ferrite_session={c}")));
                }
                if let (false, Some(t)) = (matches!(method, "GET" | "HEAD"), &b.csrf) {
                    headers.push(("X-CSRF-Token", t.clone()));
                }
            }
            Auth::Bearer(t) => headers.push(("Authorization", format!("Bearer {t}"))),
        }
        let r = request(self.addr, method, path, &headers, body);
        let copy = Resp {
            status: r.status,
            headers: r.headers.clone(),
            body: r.body.clone(),
        };
        self.seen.lock().unwrap().responses.push((copy, None));
        r
    }

    /// `POST /auth/login` as a browser; the cookie and CSRF token on success.
    fn login(&self, login: &str, password: &str) -> (Resp, Browser) {
        let r = self.send(
            Auth::Browser(&Browser::default()),
            "POST",
            "/api/v1/auth/login",
            Some(&json!({"login": login, "password": password})),
        );
        let mut b = Browser::default();
        if r.status == 200 {
            let set = r.header("set-cookie").expect("session cookie");
            assert!(set.contains("HttpOnly"), "{set}");
            assert!(set.contains("SameSite=Lax"), "{set}");
            assert!(!set.contains("Secure"), "http site.url: no Secure ({set})");
            let value = set
                .split(';')
                .next()
                .and_then(|kv| kv.strip_prefix("ferrite_session="))
                .expect("ferrite_session")
                .to_owned();
            self.seen.lock().unwrap().cookies.push(value.clone());
            b.cookie = Some(value);
            b.csrf = r.json()["csrf_token"].as_str().map(str::to_owned);
            assert!(b.csrf.is_some(), "{r:?}");
        }
        (r, b)
    }

    /// Creates a token for `user` as `who`; returns its id and secret.
    fn create_token(&self, who: &Browser, user: &str, name: &str) -> Resp {
        let r = self.send(
            Auth::Browser(who),
            "POST",
            &format!("/api/v1/users/{user}/tokens"),
            Some(&json!({"name": name, "scopes": ["content:read"]})),
        );
        if r.status == 201 {
            let secret = r.json()["token"].as_str().unwrap().to_owned();
            let mut seen = self.seen.lock().unwrap();
            seen.tokens.push(secret.clone());
            seen.responses.last_mut().unwrap().1 = Some(secret);
        }
        r
    }

    /// No response — refusals included — carries a password, hash, TOTP or CSRF secret,
    /// a session cookie value, or a token secret other than in its creation response.
    fn assert_no_secrets(&self) {
        let seen = self.seen.lock().unwrap();
        assert!(
            seen.responses.len() > 80,
            "{} responses",
            seen.responses.len()
        );
        for (r, issued) in &seen.responses {
            for needle in [
                "password_hash",
                "totp_secret",
                "csrf_secret",
                "token_hash",
                "$argon2",
                "\"password\"",
                ADMIN_PASSWORD,
                EDITOR_PASSWORD,
            ] {
                assert!(!r.body.contains(needle), "{needle} in {r:?}");
            }
            for cookie in &seen.cookies {
                assert!(!r.body.contains(cookie.as_str()), "cookie in {r:?}");
            }
            for token in seen.tokens.iter().filter(|t| Some(*t) != issued.as_ref()) {
                assert!(!r.body.contains(token.as_str()), "token in {r:?}");
                assert!(!r.body.contains(&token[4..]), "token in {r:?}");
            }
        }
    }
}

fn api_end_to_end_on_real_binary(p: Profile) {
    p.init();
    p.create_admin("admin", ADMIN_PASSWORD);
    let server = Server::start(&p);
    let site = Site {
        addr: server.addr,
        seen: Mutex::default(),
    };
    let me = "/api/v1/auth/me";

    // Anonymous: 401; a login from a foreign origin is refused before the password.
    site.send(Auth::None, "GET", me, None)
        .problem(401, "unauthenticated");
    let r = site.send_with(
        Auth::None,
        "POST",
        "/api/v1/auth/login",
        Some(&json!({"login": "admin", "password": ADMIN_PASSWORD})),
        &[("Origin", "http://evil.example".to_owned())],
    );
    assert_eq!(r.status, 403, "{r:?}");
    assert!(r.header("set-cookie").is_none(), "{r:?}");

    // Login: the cookie and the CSRF token.
    let (r, admin) = site.login("admin", ADMIN_PASSWORD);
    assert_eq!(r.status, 200, "{r:?}");
    assert_eq!(r.header("cache-control"), Some("no-store"));
    let login = r.json();
    assert_eq!(login["status"], "session");
    assert_eq!(login["user"]["login"], "admin");
    assert_eq!(login["user"]["role"], "admin");
    let r = site.send(Auth::Browser(&admin), "GET", me, None);
    assert_eq!(r.status, 200, "{r:?}");
    assert_eq!(r.json()["login"], "admin");

    // A cookie write without the CSRF token is refused and writes nothing.
    let no_csrf = Browser {
        csrf: None,
        ..admin.clone()
    };
    let post = json!({"title": "End To End Post"});
    let r = site.send(
        Auth::Browser(&no_csrf),
        "POST",
        "/api/v1/content/post",
        Some(&post),
    );
    assert_eq!(r.status, 403, "{r:?}");

    // Create and publish (If-Match) a post; the public permalink shows it.
    let r = site.send(
        Auth::Browser(&admin),
        "POST",
        "/api/v1/content/post",
        Some(&post),
    );
    assert_eq!(r.status, 201, "{r:?}");
    let created = r.json();
    assert_eq!(created["status"], "draft");
    let id = created["id"].as_str().unwrap().to_owned();
    let version = created["version"].as_i64().unwrap();
    let publish = format!("/api/v1/content/post/{id}/publish");
    site.send(Auth::Browser(&admin), "POST", &publish, None)
        .problem(428, "precondition-required");
    let r = site.send_with(
        Auth::Browser(&admin),
        "POST",
        &publish,
        None,
        &[("If-Match", format!("\"{version}\""))],
    );
    assert_eq!(r.status, 200, "{r:?}");
    let published = r.json();
    assert_eq!(published["status"], "published");
    let permalink = published["permalink"].as_str().unwrap().to_owned();
    assert!(permalink.starts_with('/'), "{permalink}");
    let r = site.send(Auth::None, "GET", &permalink, None);
    assert_eq!(r.status, 200, "{r:?}");
    assert!(r.body.contains("End To End Post"), "{}", r.body);
    assert!(
        r.header("cache-control") != Some("private, no-store"),
        "{r:?}"
    );

    // The same page logged in: rendered for the user, never cached.
    let r = site.send(Auth::Browser(&admin), "GET", &permalink, None);
    assert_eq!(r.status, 200, "{r:?}");
    assert!(r.body.contains("End To End Post"), "{}", r.body);
    assert_eq!(r.header("cache-control"), Some("private, no-store"));

    // A second post stays a draft: the preview needs the cookie.
    let r = site.send(
        Auth::Browser(&admin),
        "POST",
        "/api/v1/content/post",
        Some(&json!({"title": "Unpublished Preview Draft"})),
    );
    assert_eq!(r.status, 201, "{r:?}");
    let draft = r.json()["id"].as_str().unwrap().to_owned();
    let preview = format!("/_preview/{draft}");
    let r = site.send(Auth::Browser(&admin), "GET", &preview, None);
    assert_eq!(r.status, 200, "{r:?}");
    assert!(r.body.contains("Unpublished Preview Draft"), "{}", r.body);
    assert_eq!(r.header("cache-control"), Some("private, no-store"));
    assert_eq!(r.header("x-robots-tag"), Some("noindex"));
    let r = site.send(Auth::None, "GET", &preview, None);
    assert_eq!(r.status, 404, "{r:?}");
    assert!(!r.body.contains("Unpublished Preview Draft"));

    // An editor account, logged in.
    let r = site.send(
        Auth::Browser(&admin),
        "POST",
        "/api/v1/users",
        Some(&json!({
            "login": "eddie", "email": "eddie@example.com", "display_name": "Eddie",
            "role": "editor", "password": EDITOR_PASSWORD
        })),
    );
    assert_eq!(r.status, 201, "{r:?}");
    let eddie = r.json()["id"].as_str().unwrap().to_owned();
    let (r, editor) = site.login("eddie", EDITOR_PASSWORD);
    assert_eq!(r.status, 200, "{r:?}");
    let tokens = format!("/api/v1/users/{eddie}/tokens");

    // The editor's token works; the admin lists and revokes it, and it stops at once.
    let r = site.create_token(&editor, &eddie, "laptop");
    assert_eq!(r.status, 201, "{r:?}");
    let first = r.json();
    let first_secret = first["token"].as_str().unwrap().to_owned();
    let first_id = first["id"].as_str().unwrap().to_owned();
    let r = site.send(Auth::Bearer(&first_secret), "GET", me, None);
    assert_eq!(r.status, 200, "{r:?}");
    assert_eq!(r.json()["scopes"], json!(["content:read"]));
    let r = site.send(Auth::Browser(&admin), "GET", &tokens, None);
    assert_eq!(r.status, 200, "{r:?}");
    let listed = r.json();
    assert_eq!(listed["items"].as_array().unwrap().len(), 1, "{listed}");
    assert_eq!(listed["items"][0]["id"], first_id.as_str());
    let r = site.send(
        Auth::Browser(&admin),
        "DELETE",
        &format!("{tokens}/{first_id}"),
        None,
    );
    assert_eq!(r.status, 204, "{r:?}");
    site.send(Auth::Bearer(&first_secret), "GET", me, None)
        .problem(401, "unauthenticated");
    let r = site.send(Auth::Browser(&admin), "GET", &tokens, None);
    assert_eq!(r.json()["items"], json!([]));

    // At most 50 tokens: the 51st is a validation problem.
    for i in 0..MAX_TOKENS {
        let r = site.create_token(&editor, &eddie, &format!("t{i}"));
        assert_eq!(r.status, 201, "token {i}: {r:?}");
    }
    let last_secret = site.seen.lock().unwrap().tokens.last().unwrap().clone();
    site.create_token(&editor, &eddie, "one too many")
        .problem(422, "validation");
    let r = site.send(Auth::Browser(&admin), "GET", &tokens, None);
    assert_eq!(r.json()["items"].as_array().unwrap().len(), MAX_TOKENS);

    // Disabling the editor rejects their cookie and their tokens on the next request.
    let r = site.send(Auth::Bearer(&last_secret), "GET", me, None);
    assert_eq!(r.status, 200, "{r:?}");
    assert_eq!(
        site.send(Auth::Browser(&editor), "GET", me, None).status,
        200
    );
    let r = site.send(
        Auth::Browser(&admin),
        "PATCH",
        &format!("/api/v1/users/{eddie}"),
        Some(&json!({"status": "disabled"})),
    );
    assert_eq!(r.status, 200, "{r:?}");
    assert_eq!(r.json()["status"], "disabled");
    site.send(Auth::Browser(&editor), "GET", me, None)
        .problem(401, "unauthenticated");
    site.send(Auth::Bearer(&last_secret), "GET", me, None)
        .problem(401, "unauthenticated");
    let (r, _) = site.login("eddie", EDITOR_PASSWORD);
    assert_ne!(r.status, 200, "a disabled account cannot log in: {r:?}");
    assert!(r.header("set-cookie").is_none(), "{r:?}");

    // Logout ends the session: the old cookie is 401.
    let r = site.send(Auth::Browser(&admin), "POST", "/api/v1/auth/logout", None);
    assert_eq!(r.status, 204, "{r:?}");
    let cleared = r.header("set-cookie").expect("cookie cleared");
    assert!(cleared.contains("Max-Age=0"), "{cleared}");
    site.send(Auth::Browser(&admin), "GET", me, None)
        .problem(401, "unauthenticated");

    site.assert_no_secrets();
    let status = server.terminate();
    assert!(status.success(), "serve exit: {status:?}");
}

#[test]
fn api_end_to_end_on_real_binary_solo() {
    api_end_to_end_on_real_binary(Profile::solo());
}

#[test]
fn api_end_to_end_on_real_binary_cluster() {
    if let Some(p) = Profile::cluster() {
        api_end_to_end_on_real_binary(p);
    }
}
