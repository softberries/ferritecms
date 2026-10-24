//! `ferrite seed demo` + `ferrite serve` end to end: the seeded public site, the API
//! next to it, two processes sharing invalidation (cluster), startup refusals and
//! shutdown by signal.

use std::io::Write;
use std::net::TcpStream;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ferrite_core::{Actor, ContentDraft, ContentService, PermalinkRules};
use ferrite_domain::{ContentId, ContentStatus, NewContent, Role};
use ferrite_store::Store;
use serde_json::json;
use sqlx::{Connection, SqliteConnection};

use crate::harness::{Profile, Server, rt};

/// The newest seeded post (first on the home page) and another one.
const NEWEST_TITLE: &str = "Welcome to FerriteCMS";
const NEWEST_SLUG: &str = "welcome-to-ferritecms";
const OTHER_TITLE: &str = "Writing your first plugin";
const DRAFT_SLUG: &str = "secret-draft";

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

    // The API is served by the same process, never cached.
    let meta = server.get("/api/v1/meta");
    assert_eq!(meta.status, 200, "{meta:?}");
    assert_eq!(meta.header("cache-control"), Some("no-store"), "{meta:?}");
    let meta: serde_json::Value = serde_json::from_slice(&meta.body).expect("meta JSON");
    assert_eq!(meta["api"], "v1");
    assert_eq!(meta["version"], env!("CARGO_PKG_VERSION"));
    let missing = server.get("/api/v1/nope");
    assert_eq!(missing.status, 404);
    assert_eq!(
        missing.header("content-type"),
        Some("application/problem+json")
    );

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
        let actor = Actor::new(admin.id, Role::Admin);
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

/// `[mail]` (P17): a node with an SMTP URL (from the environment, credentials inside)
/// starts and shuts down cleanly, and the URL never reaches the log.
#[test]
fn serve_with_mail_starts_and_stops() {
    let p = Profile::solo();
    p.init();
    let mut cmd = p.cmd(&["serve"]);
    cmd.env(
        "FERRITE_MAIL__SMTP_URL",
        "smtp://user:topsecretpw@127.0.0.1:1",
    )
    .env("FERRITE_MAIL__FROM", "Site <noreply@example.com>")
    .env("FERRITE_LOG", "debug");
    let server = Server::spawn(cmd);
    assert_eq!(server.get("/healthz").status, 200);
    let log = server.log();
    let status = server.stop_with("TERM");
    assert!(status.success(), "{status:?}");
    assert!(!log.contains("topsecretpw"), "{log}");
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
