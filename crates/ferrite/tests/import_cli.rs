//! `ferrite import wordpress` (M3-1 Task 17): the CLI end to end on both backends, a
//! busy run, a dry run, and media fetched from a local server.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{ExitCode, Output};
use std::sync::Arc;

use assert_cmd::Command;
use assert_cmd::cargo::cargo_bin_cmd;
use clap::Parser;
use ferrite::{Cli, NodeOptions, OpenMode, open_store, start};
use ferrite_config::Config;
use ferrite_domain::{
    CommentQuery, ContentQuery, ImportRun, ImportRunId, ImportRunState, MEDIA_KIND, NewUser,
    RedirectQuery, Role, Timestamp, UserId, UserQuery, UserStatus,
};
use ferrite_store::Store;
use ferrite_store_tests::TestDb;
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// The site URL the imported links resolve against.
const SITE_URL: &str = "http://127.0.0.1:8080";
/// The cluster site key (64 hex characters).
const SITE_KEY: &str = "abababababababababababababababababababababababababababababababab";

fn corpus(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../ferrite-import-wp/tests/corpus")
        .join(name)
}

/// A site the CLI imports into: a data folder, the environment its commands get, and
/// (Postgres) the test database holding it.
struct Site {
    dir: tempfile::TempDir,
    env: Vec<(String, String)>,
    _db: Option<TestDb>,
}

impl Site {
    /// A SQLite site made by `ferrite init`, with an admin from `ferrite user create`.
    /// `init` writes the listen address of `SITE_URL` (port 8080); the served node takes a
    /// free port instead, so a site running on 8080 does not fail the test (R-final-1).
    async fn sqlite() -> Site {
        let site = Site {
            dir: tempfile::tempdir().unwrap(),
            env: vec![("FERRITE_SERVER__LISTEN".into(), "127.0.0.1:0".into())],
            _db: None,
        };
        let data = site.data().to_owned();
        site.ok(["init", "--url", SITE_URL, "--data"], &data).await;
        site.ok(
            [
                "user",
                "create",
                "--login",
                "importer",
                "--email",
                "a@example.test",
                "--data",
            ],
            &data,
        )
        .await;
        site
    }

    /// A Postgres site on the test database, with an admin inserted directly.
    async fn postgres() -> Option<Site> {
        let db = TestDb::postgres().await?;
        let (url, schema) = db.pg_location().unwrap();
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("ferrite.toml"),
            format!(
                "[site]\nurl = \"{SITE_URL}/\"\n[server]\nlisten = \"127.0.0.1:0\"\n\
                 [database]\nurl = \"{url}\"\nschema = \"{schema}\"\n\
                 [media]\nlocal_shared = true\n[cluster]\nmode = \"cluster\"\n"
            ),
        )
        .unwrap();
        admin(db.store()).await;
        Some(Site {
            dir,
            env: vec![("FERRITE_AUTH__SECRET_KEY".into(), SITE_KEY.into())],
            _db: Some(db),
        })
    }

    fn data(&self) -> &Path {
        self.dir.path()
    }

    fn cmd(&self) -> Command {
        let mut cmd = cargo_bin_cmd!("ferrite");
        cmd.env_clear();
        for (k, v) in &self.env {
            cmd.env(k, v);
        }
        cmd
    }

    /// Runs `ferrite <args> <data>` to the end (off the async runtime).
    async fn run<const N: usize>(&self, args: [&str; N], last: &Path) -> Output {
        let mut cmd = self.cmd();
        cmd.args(args).arg(last);
        tokio::task::spawn_blocking(move || cmd.output().unwrap())
            .await
            .unwrap()
    }

    async fn ok<const N: usize>(&self, args: [&str; N], last: &Path) -> Output {
        let out = self.run(args, last).await;
        assert!(out.status.success(), "{}", text(&out));
        out
    }

    /// `ferrite import wordpress <file> --data <data> <extra…>`.
    async fn import(&self, file: &Path, extra: &[&str]) -> Output {
        let mut cmd = self.cmd();
        cmd.args(["import", "wordpress"])
            .arg(file)
            .arg("--data")
            .arg(self.data())
            .args(extra);
        tokio::task::spawn_blocking(move || cmd.output().unwrap())
            .await
            .unwrap()
    }

    fn config(&self) -> Config {
        ferrite_config::load(self.data(), self.env.clone()).unwrap()
    }

    async fn store(&self) -> Arc<dyn Store> {
        open_store(&self.config(), OpenMode::MustExist)
            .await
            .unwrap()
    }
}

fn text(out: &Output) -> String {
    format!(
        "status {:?}\nstdout:\n{}\nstderr:\n{}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

async fn admin(store: &dyn Store) -> UserId {
    let id = UserId::new();
    let tx = store.begin().await.unwrap();
    tx.users()
        .insert(&NewUser {
            id,
            login: "importer".into(),
            email: "a@example.test".into(),
            display_name: "Importer".into(),
            password_hash: None,
            role: Role::Admin,
            status: UserStatus::Active,
            must_reset: false,
        })
        .await
        .unwrap();
    tx.commit().await.unwrap();
    id
}

/// `GET path`: status, head and body.
async fn get(addr: SocketAddr, path: &str) -> (u16, String, String) {
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
    (
        status,
        head,
        String::from_utf8_lossy(&raw[split + 4..]).into_owned(),
    )
}

/// The run folder of the one report written under `<data>/import/reports/`.
fn report_dir(site: &Site) -> PathBuf {
    let reports = site.data().join("import/reports");
    let mut dirs =
        std::fs::read_dir(&reports).unwrap_or_else(|e| panic!("{}: {e}", reports.display()));
    let dir = dirs.next().unwrap().unwrap().path();
    assert!(dirs.next().is_none());
    dir
}

/// Site A through the CLI, then served: the post at its old URL with the privacy-mode
/// embed, the moved book redirected, the report files written.
async fn import_site_a_end_to_end(site: Site) {
    let out = site
        .import(
            &corpus("site-a.xml"),
            &["--uploads", corpus("site-a-uploads").to_str().unwrap()],
        )
        .await;
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("post"), "{stdout}");
    assert!(stdout.contains("report.json"), "{stdout}");
    assert!(
        stdout.contains("media variants are made by the next `ferrite serve`"),
        "{stdout}"
    );
    let dir = report_dir(&site);
    let json: Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("report.json")).unwrap()).unwrap();
    assert_eq!(json["counts"]["post"]["created"], 8);
    let html = std::fs::read_to_string(dir.join("report.html")).unwrap();
    assert!(html.starts_with("<!doctype html>"));

    let store = site.store().await;
    let node = start(&site.config(), store.clone(), NodeOptions::default())
        .await
        .unwrap();
    let (status, _, body) = get(node.addr, "/2024/03/every-core-block/").await;
    assert_eq!(status, 200, "{body}");
    assert!(
        body.contains("<iframe src=\"https://www.youtube-nocookie.com/embed/dQw4w9WgXcQ"),
        "{body}"
    );
    let (status, head, _) = get(node.addr, "/books/the-first-book/").await;
    assert_eq!(status, 301, "{head}");
    assert!(
        head.to_ascii_lowercase()
            .contains("location: /book/the-first-book/"),
        "{head}"
    );
    node.shutdown().await.unwrap();
    store.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn import_site_a_end_to_end_sqlite() {
    import_site_a_end_to_end(Site::sqlite().await).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn import_site_a_end_to_end_postgres() {
    let Some(site) = Site::postgres().await else {
        return;
    };
    import_site_a_end_to_end(site).await;
}

/// Another import running with a fresh heartbeat: exit 3.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn import_busy_exit_3() {
    let Some(site) = Site::postgres().await else {
        return;
    };
    let store = site.store().await;
    let user = store
        .read()
        .users()
        .get_by_login("importer")
        .await
        .unwrap()
        .unwrap();
    let now = Timestamp::now();
    store
        .read()
        .imports()
        .create_run(&ImportRun {
            id: ImportRunId::new(),
            source: "http://elsewhere.test".into(),
            token: [7; 16],
            state: ImportRunState::Running,
            started_by: user.id,
            started_at: now,
            heartbeat_at: now,
            finished_at: None,
            stats: serde_json::json!({}),
            dry_run: false,
            job_id: None,
            step: None,
            report_hash: None,
        })
        .await
        .unwrap();
    store.close().await;
    let out = site.import(&corpus("site-a.xml"), &[]).await;
    assert_eq!(out.status.code(), Some(3), "{}", text(&out));
}

/// A dry run says so and writes no content.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dry_run_writes_nothing() {
    let site = Site::sqlite().await;
    let before = snapshot(&site).await;
    let out = site
        .import(
            &corpus("site-a.xml"),
            &[
                "--dry-run",
                "--uploads",
                corpus("site-a-uploads").to_str().unwrap(),
            ],
        )
        .await;
    assert!(
        out.status.code().is_some_and(|c| c == 0 || c == 2),
        "{}",
        text(&out)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("dry run:"), "{stdout}");
    assert!(!stdout.contains("variants"), "{stdout}");
    assert_eq!(snapshot(&site).await, before);
    // The report is kept; no work file (IR folder) is.
    let left: Vec<String> = std::fs::read_dir(site.data().join("import"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(left, ["reports"]);
}

/// What a dry run must leave as it was: content (media rows included), users, terms,
/// comments, redirects, menus, options and the media files.
async fn snapshot(site: &Site) -> String {
    let store = site.store().await;
    let r = store.read();
    let content = r.content().count(&ContentQuery::default()).await.unwrap();
    let media = r
        .content()
        .count(&ContentQuery {
            kind: Some(MEDIA_KIND.into()),
            ..ContentQuery::default()
        })
        .await
        .unwrap();
    let users: Vec<String> = r
        .users()
        .list(&UserQuery {
            limit: 100,
            ..UserQuery::default()
        })
        .await
        .unwrap()
        .items
        .into_iter()
        .map(|u| u.login)
        .collect();
    let mut terms = Vec::new();
    for tax in r.taxonomies().list().await.unwrap() {
        for t in r.terms().list(&tax.name, None).await.unwrap() {
            terms.push(format!("{}/{}", t.taxonomy, t.slug));
        }
    }
    let comments = r
        .comments()
        .list(&CommentQuery {
            limit: 100,
            ..CommentQuery::default()
        })
        .await
        .unwrap()
        .items
        .len();
    let redirects = r
        .redirects()
        .list(&RedirectQuery {
            limit: 100,
            ..RedirectQuery::default()
        })
        .await
        .unwrap()
        .items
        .len();
    let menus = r.menus().list().await.unwrap();
    let options: Vec<(String, Value)> = r
        .options()
        .list_prefix("", "", 200)
        .await
        .unwrap()
        .into_iter()
        .map(|o| (o.key, o.value))
        .collect();
    store.close().await;
    let files = walkdir_count(&site.data().join("media"));
    format!(
        "content {content} media {media} files {files}\nusers {users:?}\nterms {terms:?}\n\
         comments {comments} redirects {redirects}\nmenus {menus:?}\noptions {options:?}"
    )
}

/// The files under `dir` (none when it does not exist).
fn walkdir_count(dir: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .map(|e| {
            let path = e.unwrap().path();
            if path.is_dir() {
                walkdir_count(&path)
            } else {
                1
            }
        })
        .sum()
}

/// Serves `site-b-uploads` under `/wp-content/uploads/` on 127.0.0.1.
async fn upload_server() -> SocketAddr {
    use axum::extract::Path as UrlPath;
    use axum::http::StatusCode;
    async fn file(UrlPath(rest): UrlPath<String>) -> Result<Vec<u8>, StatusCode> {
        if rest.split('/').any(|s| s == "..") {
            return Err(StatusCode::NOT_FOUND);
        }
        std::fs::read(corpus("site-b-uploads").join(rest)).map_err(|_| StatusCode::NOT_FOUND)
    }
    let app = axum::Router::new().route("/wp-content/uploads/{*rest}", axum::routing::get(file));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    addr
}

/// `--fetch-media` without `--uploads`: the attachments come from the old site (here a
/// local server, which only the test seam allows). Run in-process, where the seam is
/// compiled.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fetch_media_from_local_server() {
    let site = Site::sqlite().await;
    let addr = upload_server().await;
    let xml = std::fs::read_to_string(corpus("site-b.xml"))
        .unwrap()
        .replace(
            "http://localhost:8099/wp-content/uploads",
            &format!("http://{addr}/wp-content/uploads"),
        );
    let wxr = site.data().join("site-b-local.xml");
    std::fs::write(&wxr, xml).unwrap();
    let cli = Cli::try_parse_from([
        "ferrite",
        "import",
        "wordpress",
        wxr.to_str().unwrap(),
        "--data",
        site.data().to_str().unwrap(),
        "--fetch-media",
        "--allow-private-fetch",
    ])
    .unwrap();
    let code = ferrite::run(cli).await.unwrap();
    assert_eq!(code, ExitCode::SUCCESS);
    let store = site.store().await;
    let media = store
        .read()
        .content()
        .count(&ContentQuery {
            kind: Some(MEDIA_KIND.into()),
            ..ContentQuery::default()
        })
        .await
        .unwrap();
    store.close().await;
    assert_eq!(media, 4);
}

/// Without the seam, the same local server is refused (an internal address): the
/// attachments are missing and the run exits 2.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fetch_media_refuses_internal_addresses() {
    let site = Site::sqlite().await;
    let addr = upload_server().await;
    let xml = std::fs::read_to_string(corpus("site-b.xml"))
        .unwrap()
        .replace(
            "http://localhost:8099/wp-content/uploads",
            &format!("http://{addr}/wp-content/uploads"),
        );
    let wxr = site.data().join("site-b-local.xml");
    std::fs::write(&wxr, xml).unwrap();
    let out = site.import(&wxr, &["--fetch-media"]).await;
    assert_eq!(out.status.code(), Some(2), "{}", text(&out));
    let json: Value = serde_json::from_str(
        &std::fs::read_to_string(report_dir(&site).join("report.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(json["missing_media"].as_array().unwrap().len(), 4);
}

/// Serves `site-b-uploads`, but cuts `bridge.jpg` off halfway through its body (the
/// connection drops after half the promised bytes).
async fn flaky_upload_server() -> SocketAddr {
    use axum::body::Body;
    use axum::extract::Path as UrlPath;
    use axum::http::{StatusCode, header};
    use axum::response::Response;
    async fn file(UrlPath(rest): UrlPath<String>) -> Result<Response, StatusCode> {
        if rest.split('/').any(|s| s == "..") {
            return Err(StatusCode::NOT_FOUND);
        }
        let bytes = std::fs::read(corpus("site-b-uploads").join(&rest))
            .map_err(|_| StatusCode::NOT_FOUND)?;
        if !rest.ends_with("/bridge.jpg") {
            return Ok(Response::new(Body::from(bytes)));
        }
        // The first half goes out with the headers; the drop comes a moment later, so
        // the client has started reading the body.
        let half = bytes::Bytes::copy_from_slice(&bytes[..bytes.len() / 2]);
        let body = futures::stream::unfold(0u8, move |step| {
            let half = half.clone();
            async move {
                match step {
                    0 => Some((Ok::<_, std::io::Error>(half), 1)),
                    1 => {
                        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                        Some((Err(std::io::Error::other("connection reset")), 2))
                    }
                    _ => None,
                }
            }
        });
        Response::builder()
            .header(header::CONTENT_LENGTH, bytes.len())
            .body(Body::from_stream(body))
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
    }
    let app = axum::Router::new().route("/wp-content/uploads/{*rest}", axum::routing::get(file));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    addr
}

/// A download that fails part-way is a missing attachment, not a failed run: the rest
/// is imported and the run exits 2.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn download_dropped_mid_body_is_missing() {
    let site = Site::sqlite().await;
    let addr = flaky_upload_server().await;
    let xml = std::fs::read_to_string(corpus("site-b.xml"))
        .unwrap()
        .replace(
            "http://localhost:8099/wp-content/uploads",
            &format!("http://{addr}/wp-content/uploads"),
        );
    let wxr = site.data().join("site-b-flaky.xml");
    std::fs::write(&wxr, xml).unwrap();
    let cli = Cli::try_parse_from([
        "ferrite",
        "import",
        "wordpress",
        wxr.to_str().unwrap(),
        "--data",
        site.data().to_str().unwrap(),
        "--fetch-media",
        "--allow-private-fetch",
    ])
    .unwrap();
    let code = ferrite::run(cli).await.unwrap();
    assert_eq!(code, ExitCode::from(2));
    let json: Value = serde_json::from_str(
        &std::fs::read_to_string(report_dir(&site).join("report.json")).unwrap(),
    )
    .unwrap();
    let missing = json["missing_media"].as_array().unwrap();
    assert_eq!(missing.len(), 1, "{missing:?}");
    assert!(missing[0]["url"].as_str().unwrap().ends_with("/bridge.jpg"));
    let notes = json["notes"].to_string();
    assert!(notes.contains("bridge.jpg"), "{notes}");
    assert_eq!(json["counts"]["media"]["created"], 3);
}

/// Attachments refused for their size are counted as failed, and the run exits 2.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn refused_files_exit_2() {
    let mut site = Site::sqlite().await;
    site.env
        .push(("FERRITE_MEDIA__MAX_UPLOAD".into(), "100".into()));
    let out = site
        .import(
            &corpus("site-a.xml"),
            &["--uploads", corpus("site-a-uploads").to_str().unwrap()],
        )
        .await;
    assert_eq!(out.status.code(), Some(2), "{}", text(&out));
    let json: Value = serde_json::from_str(
        &std::fs::read_to_string(report_dir(&site).join("report.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(json["counts"]["media"]["failed"], 4);
    assert!(json["missing_media"].as_array().unwrap().is_empty());
}

/// `--as` takes an active admin only; with no active admin there is no default.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn as_takes_active_admins_only() {
    let site = Site::sqlite().await;
    let store = site.store().await;
    let mut user = store
        .read()
        .users()
        .get_by_login("importer")
        .await
        .unwrap()
        .unwrap();
    user.status = UserStatus::Disabled;
    store.read().users().update(&user).await.unwrap();
    store.close().await;
    drop(store);
    let out = site
        .import(&corpus("site-a.xml"), &["--as", "importer"])
        .await;
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("not an active admin"),
        "{}",
        text(&out)
    );
    let out = site.import(&corpus("site-a.xml"), &[]).await;
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("no active admin"),
        "{}",
        text(&out)
    );
}

/// A run that fails after it started (here the WXR file breaks off part-way) still
/// writes its report, which names the failed step, and exits 1 (P29).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_import_writes_a_partial_report_and_exits_1() {
    let site = Site::sqlite().await;
    let whole = std::fs::read_to_string(corpus("site-a.xml")).unwrap();
    let cut = whole.find("<item>").unwrap() + (whole.len() - whole.find("<item>").unwrap()) / 2;
    let broken = site.data().join("broken.xml");
    std::fs::write(&broken, format!("{}<<<", &whole[..cut])).unwrap();
    let out = site.import(&broken, &[]).await;
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("the import failed"),
        "{}",
        text(&out)
    );
    let dir = report_dir(&site);
    assert!(
        String::from_utf8_lossy(&out.stdout).contains(dir.join("report.json").to_str().unwrap()),
        "{}",
        text(&out)
    );
    let json: Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("report.json")).unwrap()).unwrap();
    assert_eq!(json["failed"]["step"], "parse", "{json}");
    assert!(json["notes"][0].as_str().unwrap().starts_with("failed:"));
    let html = std::fs::read_to_string(dir.join("report.html")).unwrap();
    assert!(html.contains("parse"));
}

/// A copy of `site-a-uploads` whose `cover.png` carries an XMP `iTXt` chunk holding
/// [`XMP_MARKER`] (the walker does not check CRCs; the chunk's is zero).
fn uploads_with_xmp() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let from = corpus("site-a-uploads/2026/10");
    let to = dir.path().join("2026/10");
    std::fs::create_dir_all(&to).unwrap();
    for entry in std::fs::read_dir(&from).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(entry.path(), to.join(entry.file_name())).unwrap();
    }
    let cover = to.join("cover.png");
    let png = std::fs::read(&cover).unwrap();
    let mut data = b"XML:com.adobe.xmp\0\0\0\0\0".to_vec();
    data.extend_from_slice(XMP_MARKER.as_bytes());
    let mut chunk = (data.len() as u32).to_be_bytes().to_vec();
    chunk.extend_from_slice(b"iTXt");
    chunk.extend_from_slice(&data);
    chunk.extend_from_slice(&[0; 4]);
    // After the 8-byte signature and the 25-byte IHDR chunk.
    let mut out = png[..33].to_vec();
    out.extend(chunk);
    out.extend_from_slice(&png[33..]);
    std::fs::write(&cover, out).unwrap();
    dir
}

const XMP_MARKER: &str = "<x:xmpmeta>GPS 52.5N 13.4E</x:xmpmeta>";

/// Whether any file under `dir` contains `needle`.
fn any_file_contains(dir: &Path, needle: &str) -> bool {
    std::fs::read_dir(dir).unwrap().any(|e| {
        let path = e.unwrap().path();
        if path.is_dir() {
            any_file_contains(&path, needle)
        } else {
            let bytes = std::fs::read(&path).unwrap();
            bytes.windows(needle.len()).any(|w| w == needle.as_bytes())
        }
    })
}

/// `media.strip_metadata = true` (here through the environment) strips imported files
/// (M4-1 P17); with it off the XMP is stored.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn import_strips_metadata_when_the_option_is_on() {
    let uploads = uploads_with_xmp();
    for (strip, label) in [(false, "off"), (true, "on")] {
        let mut site = Site::sqlite().await;
        if strip {
            site.env
                .push(("FERRITE_MEDIA__STRIP_METADATA".into(), "true".into()));
        }
        let out = site
            .import(
                &corpus("site-a.xml"),
                &["--uploads", uploads.path().to_str().unwrap()],
            )
            .await;
        assert!(out.status.success(), "{label}: {}", text(&out));
        assert_eq!(
            any_file_contains(&site.data().join("media"), XMP_MARKER),
            !strip,
            "{label}: the XMP is stored only with stripping off"
        );
    }
}

/// `--uploads` takes a zip of the uploads as well as a folder; another file is refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn uploads_zip_is_accepted() {
    use std::io::Write;
    let site = Site::sqlite().await;
    let archive = site.data().join("uploads.zip");
    let mut zip = zip::ZipWriter::new(std::fs::File::create(&archive).unwrap());
    let root = corpus("site-a-uploads/2026/10");
    let mut names: Vec<_> = std::fs::read_dir(&root)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    for name in names {
        zip.start_file(
            format!("wp-content/uploads/2026/10/{name}"),
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
        zip.write_all(&std::fs::read(root.join(&name)).unwrap())
            .unwrap();
    }
    zip.finish().unwrap();
    let not_zip = site.data().join("uploads.txt");
    std::fs::write(&not_zip, "no").unwrap();
    let out = site
        .import(
            &corpus("site-a.xml"),
            &["--uploads", not_zip.to_str().unwrap()],
        )
        .await;
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("neither a folder nor a .zip file"),
        "{}",
        text(&out)
    );
    let out = site
        .import(
            &corpus("site-a.xml"),
            &["--uploads", archive.to_str().unwrap()],
        )
        .await;
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    let json: Value = serde_json::from_str(
        &std::fs::read_to_string(report_dir(&site).join("report.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(json["counts"]["media"]["created"], 4);
    assert!(json["missing_media"].as_array().unwrap().is_empty());
}
