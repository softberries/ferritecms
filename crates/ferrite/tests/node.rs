#![allow(clippy::unwrap_used, clippy::expect_used)]

//! In-process nodes (`ferrite::start`) on both profiles: hot swap of theme, plugins and
//! permalinks (P18), the boot race (P22), scheduled publishing through jobs (P12) and
//! shutdown releasing jobs and leaving the registry (P10). Solo runs on SQLite (250 ms
//! poll); the Postgres cases run in cluster mode (`LISTEN` wake-ups) and print `SKIP`
//! without `FERRITE_TEST_PG_URL`.

use std::future::Future;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use ferrite::extensions::{PLUGIN_MIME, install_plugin_file, install_theme_dir};
use ferrite::serve::BootBarrier;
use ferrite::{NodeOptions, RunningNode, start};
use ferrite_cluster::{JobError, JobHandler};
use ferrite_config::Config;
use ferrite_core::{
    Actor, ContentDraft, ContentService, ExtensionService, OptionService, PermalinkRules,
    events::options_changed,
};
use ferrite_domain::{
    Artifact, ArtifactHash, ArtifactKind, Job, JobStatus, NewJob, Role, Timestamp,
};
use ferrite_media::{Backend, MediaConfig, MediaStore, PublicUrlMode};
use ferrite_store::Store;
use ferrite_store_tests::{TestDb, backend_tests_spawning, fixtures::fixture_user};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::Instant;

#[path = "../../ferrite-plugin-host/src/test_support.rs"]
mod test_support;

const SITE_URL: &str = "http://127.0.0.1:8080";
/// How fast a change must reach the node (solo: 250 ms poll; cluster: a notice).
const CONVERGE: Duration = Duration::from_secs(1);

/// A data folder with a config for `db`'s profile.
struct Site {
    dir: tempfile::TempDir,
    config: Config,
}

impl Site {
    fn new(db: &TestDb) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let mut toml =
            format!("[site]\nurl = \"{SITE_URL}/\"\n[server]\nlisten = \"127.0.0.1:0\"\n");
        if let Some((url, schema)) = db.pg_location() {
            toml.push_str(&format!(
                "[database]\nurl = \"{url}\"\nschema = \"{schema}\"\n\
                 [media]\nlocal_shared = true\n[cluster]\nmode = \"cluster\"\n"
            ));
        }
        std::fs::write(dir.path().join("ferrite.toml"), toml).unwrap();
        let config = ferrite_config::load(dir.path(), Vec::new()).unwrap();
        Self { dir, config }
    }

    fn data(&self) -> &Path {
        self.dir.path()
    }

    /// The node's media store, opened separately (same folder).
    fn media(&self) -> MediaStore {
        MediaStore::open(MediaConfig {
            backend: Backend::Local {
                root: self.data().join("media"),
            },
            prefix: String::new(),
            public: PublicUrlMode::ServeViaApp {
                path_prefix: "/_media".into(),
            },
        })
        .unwrap()
    }

    /// A theme folder `src/<name>` whose index shows `marker` and has `style.css`.
    fn theme_dir(&self, name: &str, marker: &str) -> PathBuf {
        let dir = self.data().join("src").join(name);
        std::fs::create_dir_all(dir.join("templates")).unwrap();
        std::fs::create_dir_all(dir.join("assets")).unwrap();
        std::fs::write(
            dir.join("theme.toml"),
            format!("[theme]\nname = \"{name}\"\nversion = \"1.0.0\"\n"),
        )
        .unwrap();
        std::fs::write(
            dir.join("templates/index.html"),
            format!("<!doctype html><title>{name}</title><p>{marker}</p>"),
        )
        .unwrap();
        std::fs::write(dir.join("assets/style.css"), format!("/* {name} */")).unwrap();
        dir
    }

    /// Where the local media store keeps the blob of `hash`.
    fn blob(&self, hash: &ArtifactHash) -> PathBuf {
        let h = hash.as_str();
        self.data().join("media/artifacts").join(&h[..2]).join(h)
    }

    /// Overwrites the stored blob of `hash` (bypassing the media store).
    fn tamper(&self, hash: &ArtifactHash, bytes: &[u8]) {
        let path = self.blob(hash);
        assert!(path.is_file(), "blob at {}", path.display());
        std::fs::write(path, bytes).unwrap();
    }

    async fn start(&self, db: &TestDb, opts: NodeOptions) -> RunningNode {
        start(&self.config, db.shared_store(), opts).await.unwrap()
    }
}

/// `GET path` (identity encoding, `Connection: close`): status and body.
async fn get(addr: SocketAddr, path: &str) -> (u16, String) {
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
    let mut body = raw[split + 4..].to_vec();
    if head
        .to_ascii_lowercase()
        .contains("transfer-encoding: chunked")
    {
        body = dechunk(&body);
    }
    (status, String::from_utf8_lossy(&body).into_owned())
}

fn dechunk(mut data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let end = data.windows(2).position(|w| w == b"\r\n").unwrap();
        let size_text = String::from_utf8_lossy(&data[..end]).into_owned();
        let size = usize::from_str_radix(size_text.split(';').next().unwrap().trim(), 16).unwrap();
        data = &data[end + 2..];
        if size == 0 {
            return out;
        }
        out.extend_from_slice(&data[..size]);
        data = &data[size + 2..];
    }
}

/// Polls `f` every 25 ms until it is true; panics with `what` after `limit`. Returns the
/// time it took.
async fn eventually<F, Fut>(limit: Duration, what: &str, mut f: F) -> Duration
where
    F: FnMut() -> Fut,
    Fut: Future<Output = bool>,
{
    let started = Instant::now();
    loop {
        if f().await {
            return started.elapsed();
        }
        assert!(started.elapsed() < limit, "{what}: not within {limit:?}");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

async fn home_contains(node: &RunningNode, needle: &str) -> bool {
    let (status, body) = get(node.addr, "/").await;
    status == 200 && body.contains(needle)
}

/// This node's `nodes.meta`.
async fn meta(db: &TestDb, node: &RunningNode) -> Value {
    db.store()
        .read()
        .nodes()
        .list()
        .await
        .unwrap()
        .into_iter()
        .find(|n| n.id == node.node_id)
        .map(|n| n.meta)
        .unwrap_or(Value::Null)
}

/// Review Focus 1 (P22): a theme activated while the node boots, after its invalidation
/// cursor is set and before it reads the theme, is served once it is up.
///
/// What it guards: `start` must set the cursor **before** it reads any node state. The
/// barrier pauses `start` right after the cursor, and the activation commits there. With
/// the correct order the node either reads the new theme directly or replays the event
/// on its first poll; with the M1c-2 order (state read first, cursor set later) the
/// event would sit below the cursor and the node would serve theme `a` forever.
/// Checked by mutation: moving the barrier after `load_theme` and the cursor to spawn
/// time makes this test fail.
async fn boot_race_change_during_startup_is_applied(db: Arc<TestDb>) {
    let site = Site::new(&db);
    let media = site.media();
    let store = db.shared_store();
    install_theme_dir(&store, &media, &site.theme_dir("a", "MARK-A"), true)
        .await
        .unwrap();
    install_theme_dir(&store, &media, &site.theme_dir("b", "MARK-B"), false)
        .await
        .unwrap();
    let (barrier, mut gate) = BootBarrier::new();
    let opts = NodeOptions {
        boot_barrier: Some(barrier),
        ..NodeOptions::default()
    };
    let race = async {
        gate.reached().await;
        ExtensionService::new(store.clone())
            .activate_theme("b")
            .await
            .unwrap();
        gate.resume();
    };
    let (node, ()) = tokio::join!(site.start(&db, opts), race);
    eventually(CONVERGE, "theme b after a boot-time activation", || {
        home_contains(&node, "MARK-B")
    })
    .await;
    node.shutdown().await.unwrap();
}

/// The plugin variant of the boot race: a plugin enabled at the barrier is running once
/// the node is up.
async fn boot_race_plugin_enable_is_applied(db: Arc<TestDb>) {
    let site = Site::new(&db);
    let media = site.media();
    let store = db.shared_store();
    let wasm = test_support::plugin_wasm("hello");
    install_plugin_file(
        &store,
        &media,
        &site.config.plugins,
        &manifest("hello"),
        &wasm,
        false,
    )
    .await
    .unwrap();
    let (barrier, mut gate) = BootBarrier::new();
    let opts = NodeOptions {
        boot_barrier: Some(barrier),
        ..NodeOptions::default()
    };
    let race = async {
        gate.reached().await;
        ExtensionService::new(store.clone())
            .set_plugin_enabled("hello", true)
            .await
            .unwrap();
        gate.resume();
    };
    let (node, ()) = tokio::join!(site.start(&db, opts), race);
    eventually(CONVERGE, "plugin enabled during boot", || {
        home_contains(&node, HELLO_META)
    })
    .await;
    node.shutdown().await.unwrap();
}

/// R14: a theme whose fetch fails is retried without another event; once the bytes are
/// readable again the node swaps to it (first retry after 5 s).
async fn failed_theme_reload_is_retried(db: Arc<TestDb>) {
    let site = Site::new(&db);
    let node = site.start(&db, NodeOptions::default()).await;
    let store = db.shared_store();
    let media = node.state.media.clone();
    let b = install_theme_dir(&store, &media, &site.theme_dir("b", "MARK-B"), false)
        .await
        .unwrap();
    let good = std::fs::read(site.blob(&b.artifact_hash)).unwrap();
    site.tamper(&b.artifact_hash, b"temporarily unreadable");
    ExtensionService::new(store.clone())
        .activate_theme("b")
        .await
        .unwrap();
    eventually(Duration::from_secs(3), "theme error reported", || {
        let db = db.clone();
        let node = &node;
        async move { meta(&db, node).await["theme"]["error"].is_string() }
    })
    .await;
    assert!(!home_contains(&node, "MARK-B").await);
    site.tamper(&b.artifact_hash, &good);
    eventually(Duration::from_secs(7), "theme b after the retry", || {
        home_contains(&node, "MARK-B")
    })
    .await;
    eventually(CONVERGE, "error cleared", || {
        let db = db.clone();
        let node = &node;
        async move { meta(&db, node).await["theme"]["error"].is_null() }
    })
    .await;
    node.shutdown().await.unwrap();
}

async fn theme_hot_swap_without_restart(db: Arc<TestDb>) {
    let site = Site::new(&db);
    let node = site.start(&db, NodeOptions::default()).await;
    let store = db.shared_store();
    let media = node.state.media.clone();
    install_theme_dir(&store, &media, &site.theme_dir("a", "MARK-A"), true)
        .await
        .unwrap();
    eventually(CONVERGE, "theme a", || home_contains(&node, "MARK-A")).await;
    let a_asset = node.state.theme.load().asset_url("style.css");
    assert_eq!(get(node.addr, &a_asset).await.0, 200);

    install_theme_dir(&store, &media, &site.theme_dir("b", "MARK-B"), true)
        .await
        .unwrap();
    eventually(CONVERGE, "theme b", || home_contains(&node, "MARK-B")).await;
    let (status, body) = get(node.addr, &a_asset).await;
    assert_eq!(status, 200, "the previous theme's assets stay served");
    assert!(body.contains("/* a */"), "{body}");

    install_theme_dir(&store, &media, &site.theme_dir("c", "MARK-C"), true)
        .await
        .unwrap();
    eventually(CONVERGE, "theme c", || home_contains(&node, "MARK-C")).await;
    assert_eq!(get(node.addr, &a_asset).await.0, 404, "two swaps ago");
    eventually(CONVERGE, "meta names theme c", || {
        let db = db.clone();
        let node = &node;
        async move { meta(&db, node).await["theme"]["name"] == "c" }
    })
    .await;
    let m = meta(&db, &node).await;
    assert_eq!(m["site_url"], SITE_URL, "normalised site.url: {m}");
    node.shutdown().await.unwrap();
}

/// Review Focus 4: an activation whose artifact bytes are garbage keeps the previous
/// theme (no fallback to `blog` at runtime) and reports the error in `nodes.meta`.
async fn bad_theme_activation_keeps_previous_theme(db: Arc<TestDb>) {
    let site = Site::new(&db);
    let node = site.start(&db, NodeOptions::default()).await;
    let store = db.shared_store();
    let media = node.state.media.clone();
    install_theme_dir(&store, &media, &site.theme_dir("a", "MARK-A"), true)
        .await
        .unwrap();
    eventually(CONVERGE, "theme a", || home_contains(&node, "MARK-A")).await;
    let bad = install_theme_dir(&store, &media, &site.theme_dir("bad", "MARK-BAD"), false)
        .await
        .unwrap();
    site.tamper(&bad.artifact_hash, b"garbage, not a tar");
    ExtensionService::new(store.clone())
        .activate_theme("bad")
        .await
        .unwrap();
    eventually(Duration::from_secs(3), "theme error in nodes.meta", || {
        let db = db.clone();
        let node = &node;
        async move {
            let m = meta(&db, node).await;
            m["theme"]["error"]
                .as_str()
                .is_some_and(|e| e.contains("hash mismatch") && e.contains("\"bad\""))
        }
    })
    .await;
    let m = meta(&db, &node).await;
    assert_eq!(m["theme"]["name"], "a", "{m}");
    let (status, body) = get(node.addr, "/").await;
    assert_eq!(status, 200);
    assert!(
        body.contains("MARK-A"),
        "previous theme still served: {body}"
    );
    assert_eq!(node.state.theme.load().name(), "a", "no fallback to blog");
    node.shutdown().await.unwrap();
}

fn manifest(name: &str) -> String {
    let dir = test_support::repo_root()
        .join("plugins/examples")
        .join(name);
    std::fs::read_to_string(dir.join("ferrite-plugin.toml")).unwrap()
}

const HELLO_META: &str = "ferrite-hello";

/// Review Focus 4: a plugin row updated to a component that does not compile keeps the
/// old version filtering; `nodes.meta.plugins.<name>.status` is `failed`.
async fn bad_plugin_update_keeps_previous_version(db: Arc<TestDb>) {
    let site = Site::new(&db);
    let node = site.start(&db, NodeOptions::default()).await;
    let store = db.shared_store();
    let media = node.state.media.clone();
    let wasm = test_support::plugin_wasm("hello");
    let row = install_plugin_file(
        &store,
        &media,
        &site.config.plugins,
        &manifest("hello"),
        &wasm,
        true,
    )
    .await
    .unwrap();
    eventually(CONVERGE, "hello's head meta", || {
        home_contains(&node, HELLO_META)
    })
    .await;

    let stored = media
        .put_artifact(Bytes::from_static(b"not a component"))
        .await
        .unwrap();
    let artifact = Artifact {
        hash: stored.hash.clone(),
        kind: ArtifactKind::Plugin,
        bytes: stored.bytes as i64,
        mime: PLUGIN_MIME.into(),
        created_at: Timestamp::now(),
    };
    let update = ferrite_domain::InstalledPlugin {
        artifact_hash: stored.hash.clone(),
        version: "0.2.0".into(),
        updated_at: Timestamp::now(),
        ..row.clone()
    };
    ExtensionService::new(store.clone())
        .install_plugin(&artifact, &update)
        .await
        .unwrap();
    eventually(
        Duration::from_secs(3),
        "plugin failure in nodes.meta",
        || {
            let db = db.clone();
            let node = &node;
            async move { meta(&db, node).await["plugins"]["hello"]["status"] == "failed" }
        },
    )
    .await;
    let m = meta(&db, &node).await;
    let entry = &m["plugins"]["hello"];
    assert_eq!(entry["hash"], stored.hash.as_str(), "{m}");
    assert!(
        entry["error"]
            .as_str()
            .is_some_and(|e| e.contains(row.artifact_hash.as_str())),
        "names the version still running: {m}"
    );
    assert!(
        home_contains(&node, HELLO_META).await,
        "old version still filters"
    );
    // The failed row is retried 5 s after the failure; the retry finds the same state,
    // so it must not flush: a page cached now is still served from the cache after it.
    assert!(home_contains(&node, HELLO_META).await);
    let renders = node.state.renders();
    assert!(home_contains(&node, HELLO_META).await);
    assert_eq!(node.state.renders(), renders, "home page is cached");
    tokio::time::sleep(reload_retry_wait()).await;
    assert!(home_contains(&node, HELLO_META).await);
    assert_eq!(
        node.state.renders(),
        renders,
        "an unchanged plugin retry kept the cache"
    );
    node.shutdown().await.unwrap();
}

/// I2 / R17: a theme and a plugin whose artifacts cannot be read at startup (a media
/// blip after the probe) are retried like a failed reload: once the bytes are readable
/// the node converges, with no event.
async fn startup_load_failure_is_retried(db: Arc<TestDb>) {
    let site = Site::new(&db);
    let store = db.shared_store();
    let media = site.media();
    let theme = install_theme_dir(&store, &media, &site.theme_dir("b", "MARK-B"), true)
        .await
        .unwrap();
    let plugin = install_plugin_file(
        &store,
        &media,
        &site.config.plugins,
        &manifest("hello"),
        &test_support::plugin_wasm("hello"),
        true,
    )
    .await
    .unwrap();
    let good_theme = std::fs::read(site.blob(&theme.artifact_hash)).unwrap();
    let good_plugin = std::fs::read(site.blob(&plugin.artifact_hash)).unwrap();
    site.tamper(&theme.artifact_hash, b"temporarily unreadable");
    site.tamper(&plugin.artifact_hash, b"temporarily unreadable");

    let node = site.start(&db, NodeOptions::default()).await;
    let seq = store.read().events().max_seq().await.unwrap();
    assert!(
        !home_contains(&node, "MARK-B").await,
        "fell back at startup"
    );
    assert!(node.state.plugins.loaded().is_empty(), "hello not loaded");
    eventually(Duration::from_secs(3), "startup failures reported", || {
        let db = db.clone();
        let node = &node;
        async move {
            let m = meta(&db, node).await;
            m["theme"]["error"].is_string() && m["plugins"]["hello"]["status"] == "failed"
        }
    })
    .await;

    site.tamper(&theme.artifact_hash, &good_theme);
    site.tamper(&plugin.artifact_hash, &good_plugin);
    eventually(
        reload_retry_wait() + CONVERGE,
        "theme b after the retry",
        || home_contains(&node, "MARK-B"),
    )
    .await;
    // Theme b has no head hook, so the plugin is checked in the host.
    eventually(CONVERGE, "hello after the retry", || async {
        node.state.plugins.loaded().contains(&"hello".to_owned())
    })
    .await;
    assert!(
        store
            .read()
            .events()
            .since(seq, 10)
            .await
            .unwrap()
            .is_empty(),
        "converged without an event"
    );
    eventually(CONVERGE, "errors cleared", || {
        let db = db.clone();
        let node = &node;
        async move {
            let m = meta(&db, node).await;
            m["theme"]["error"].is_null() && m["plugins"]["hello"]["status"] == "loaded"
        }
    })
    .await;
    node.shutdown().await.unwrap();
}

/// Long enough for the first reload retry (5 s after the failure) to have run.
fn reload_retry_wait() -> Duration {
    ferrite::reload::RETRY_MIN + Duration::from_secs(1)
}

async fn plugin_hot_reload_on_enable_and_disable(db: Arc<TestDb>) {
    let site = Site::new(&db);
    let node = site.start(&db, NodeOptions::default()).await;
    let store = db.shared_store();
    let wasm = test_support::plugin_wasm("hello");
    install_plugin_file(
        &store,
        &node.state.media,
        &site.config.plugins,
        &manifest("hello"),
        &wasm,
        true,
    )
    .await
    .unwrap();
    eventually(CONVERGE, "enabled", || home_contains(&node, HELLO_META)).await;
    let extensions = ExtensionService::new(store.clone());
    for _ in 0..2 {
        extensions.set_plugin_enabled("hello", false).await.unwrap();
        eventually(CONVERGE, "disabled", || async {
            let (status, body) = get(node.addr, "/").await;
            status == 200 && !body.contains(HELLO_META)
        })
        .await;
        extensions.set_plugin_enabled("hello", true).await.unwrap();
        eventually(CONVERGE, "enabled again", || {
            home_contains(&node, HELLO_META)
        })
        .await;
    }
    node.shutdown().await.unwrap();
}

/// A published post by a fresh admin, written through core with the default rules.
async fn published_post(store: Arc<dyn Store>, slug: &str) -> ferrite_domain::ContentItem {
    let user = fixture_user(store.as_ref()).await;
    let actor = Actor {
        user: user.id,
        role: Role::Admin,
    };
    let svc = ContentService::new(store, Arc::new(PermalinkRules::default()));
    let draft = svc
        .create(
            &actor,
            ContentDraft {
                kind: "post".into(),
                title: format!("Title {slug}"),
                slug: Some(slug.into()),
                parent_id: None,
                excerpt: None,
                body: json!({"type": "doc", "content": []}),
                fields: json!({}),
                menu_order: 0,
                terms: Vec::new(),
            },
        )
        .await
        .unwrap();
    svc.publish(&actor, draft.id, draft.version).await.unwrap()
}

async fn permalink_change_rebuilds_routes(db: Arc<TestDb>) {
    let site = Site::new(&db);
    let node = site.start(&db, NodeOptions::default()).await;
    let store = db.shared_store();
    let post = published_post(store.clone(), "moving").await;
    assert_eq!(get(node.addr, "/moving/").await.0, 200);
    let published = post.published_at.unwrap().to_rfc3339();
    let new_path = format!("/{}/{}/moving/", &published[..4], &published[5..7]);
    assert_ne!(get(node.addr, &new_path).await.0, 200);

    let user = fixture_user(store.as_ref()).await;
    let admin = Actor {
        user: user.id,
        role: Role::Admin,
    };
    OptionService::load(store.clone())
        .await
        .unwrap()
        .set(
            &admin,
            "permalink.post",
            &json!("/%year%/%month%/%slug%/"),
            true,
        )
        .await
        .unwrap();
    eventually(CONVERGE, "new permalink served", || async {
        get(node.addr, &new_path).await.0 == 200
    })
    .await;
    let old = get(node.addr, "/moving/").await.0;
    assert!(
        old == 301 || old == 404,
        "old path is redirected or gone: {old}"
    );

    // An invalid structure written past core: the node keeps its rules and says why.
    let tx = store.begin().await.unwrap();
    tx.options()
        .set("permalink.post", &json!("/category/%slug%/"), true)
        .await
        .unwrap();
    tx.events()
        .append(&options_changed("permalink.post"))
        .await
        .unwrap();
    tx.commit().await.unwrap();
    eventually(
        Duration::from_secs(3),
        "permalinks_error in nodes.meta",
        || {
            let db = db.clone();
            let node = &node;
            async move {
                meta(&db, node).await["permalinks_error"]
                    .as_str()
                    .is_some_and(|e| e.contains("invalid permalink option"))
            }
        },
    )
    .await;
    assert_eq!(
        get(node.addr, &new_path).await.0,
        200,
        "old rules still work"
    );
    node.shutdown().await.unwrap();
}

async fn scheduled_post_publishes_via_job(db: Arc<TestDb>) {
    let site = Site::new(&db);
    // Without periodics `content.publish_due` never runs: only the item's job publishes.
    let opts = NodeOptions {
        periodics: Some(Vec::new()),
        ..NodeOptions::default()
    };
    let node = site.start(&db, opts).await;
    let store = db.shared_store();
    let user = fixture_user(store.as_ref()).await;
    let actor = Actor {
        user: user.id,
        role: Role::Admin,
    };
    let svc = ContentService::new(store.clone(), Arc::new(PermalinkRules::default()));
    let draft = svc
        .create(
            &actor,
            ContentDraft {
                kind: "post".into(),
                title: "Scheduled".into(),
                slug: Some("later".into()),
                parent_id: None,
                excerpt: None,
                body: json!({"type": "doc", "content": []}),
                fields: json!({}),
                menu_order: 0,
                terms: Vec::new(),
            },
        )
        .await
        .unwrap();
    let at = Timestamp::from_micros(Timestamp::now().as_micros() + 2_000_000).unwrap();
    svc.schedule(&actor, draft.id, draft.version, at)
        .await
        .unwrap();
    let scheduled = Instant::now();
    assert_eq!(get(node.addr, "/later/").await.0, 404);
    eventually(
        Duration::from_millis(3500),
        "scheduled post visible",
        || async { get(node.addr, "/later/").await.0 == 200 },
    )
    .await;
    assert!(scheduled.elapsed() >= Duration::from_millis(1900));
    // No periodics run on this node, so the one job is the item's own.
    eventually(Duration::from_secs(2), "job done", || {
        let store = store.clone();
        async move {
            let c = counts(store.as_ref()).await;
            c[&JobStatus::Done] == 1 && c[&JobStatus::Queued] == 0 && c[&JobStatus::Running] == 0
        }
    })
    .await;
    assert_eq!(counts(store.as_ref()).await[&JobStatus::Dead], 0);
    node.shutdown().await.unwrap();
}

/// Job rows per status.
async fn counts(store: &dyn Store) -> std::collections::BTreeMap<JobStatus, u64> {
    store.read().jobs().counts().await.unwrap()
}

/// Sleeps far past the drain; shutdown must release it.
struct Sleeper;

#[async_trait]
impl JobHandler for Sleeper {
    fn kind(&self) -> &'static str {
        "test.sleep"
    }

    async fn run(&self, _job: &Job) -> Result<(), JobError> {
        tokio::time::sleep(Duration::from_secs(60)).await;
        Ok(())
    }
}

async fn shutdown_releases_running_job_and_deregisters(db: Arc<TestDb>) {
    let site = Site::new(&db);
    let node = site
        .start(
            &db,
            NodeOptions {
                extra_handlers: vec![Arc::new(Sleeper)],
                ..NodeOptions::default()
            },
        )
        .await;
    let store = db.shared_store();
    let id = store
        .read()
        .jobs()
        .enqueue(&NewJob::new("test.sleep", json!({"v": 1})))
        .await
        .unwrap()
        .unwrap();
    eventually(Duration::from_secs(3), "job running", || {
        let store = store.clone();
        async move { store.read().jobs().get(id).await.unwrap().unwrap().status == JobStatus::Running }
    })
    .await;
    let node_id = node.node_id;
    let started = Instant::now();
    node.shutdown().await.unwrap();
    let took = started.elapsed();
    assert!(took < Duration::from_secs(14), "{took:?}");
    assert!(
        took >= Duration::from_secs(8),
        "the job got its drain: {took:?}"
    );
    let job = store.read().jobs().get(id).await.unwrap().unwrap();
    assert_eq!(job.status, JobStatus::Queued, "{job:?}");
    assert_eq!(job.attempts, 0, "attempt refunded: {job:?}");
    assert_eq!(job.locked_by, None);
    assert!(
        !store
            .read()
            .nodes()
            .list()
            .await
            .unwrap()
            .iter()
            .any(|n| n.id == node_id),
        "node row removed"
    );
}

backend_tests_spawning!(
    boot_race_change_during_startup_is_applied,
    boot_race_plugin_enable_is_applied,
    failed_theme_reload_is_retried,
    startup_load_failure_is_retried,
    theme_hot_swap_without_restart,
    bad_theme_activation_keeps_previous_theme,
    bad_plugin_update_keeps_previous_version,
    plugin_hot_reload_on_enable_and_disable,
    permalink_change_rebuilds_routes,
    scheduled_post_publishes_via_job,
    shutdown_releases_running_job_and_deregisters,
);
