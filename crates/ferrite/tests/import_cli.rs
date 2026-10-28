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
    ContentQuery, ImportRun, ImportRunId, ImportRunState, MEDIA_KIND, NewUser, Role, Timestamp,
    UserId, UserStatus,
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
    async fn sqlite() -> Site {
        let site = Site {
            dir: tempfile::tempdir().unwrap(),
            env: Vec::new(),
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
    let store = site.store().await;
    let rows = store
        .read()
        .content()
        .count(&ContentQuery::default())
        .await
        .unwrap();
    store.close().await;
    assert_eq!(rows, 0);
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
