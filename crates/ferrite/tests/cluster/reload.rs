//! Extension changes reach every node: theme activation and plugin disable through one
//! node (P18, P21), and the shared failure counters disabling a plugin cluster-wide
//! (P20).

use std::time::Duration;

use ferrite::NodeOptions;
use ferrite::extensions::{install_plugin_file, install_theme_dir};
use ferrite_core::ExtensionService;
use ferrite_core::events::PLUGIN_DISABLED;
use ferrite_domain::Timestamp;
use tokio::time::Instant;

use crate::harness::{
    CONVERGE, Cluster, SETUP, Spec, admin, content, manifest, published_post, wait_until,
};
use crate::test_support;

const HELLO_META: &str = "ferrite-hello";

/// Does every node answer `/` with 200 and a body for which `f` holds?
async fn all_home(c: &Cluster, f: impl Fn(&str) -> bool) -> bool {
    for i in 0..c.len() {
        let (status, body) = c.get(i, "/").await;
        if status != 200 || !f(&body) {
            return false;
        }
    }
    true
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn theme_and_plugin_reload_reach_every_node() {
    let Some(c) = Cluster::new(3, |_| NodeOptions::default()).await else {
        return;
    };
    let store = c.store(0);
    let media = c.node(0).state.media.clone();
    let extensions = ExtensionService::new(store.clone());

    // Plugins (on the embedded theme, whose head slot shows hello's meta tag).
    install_plugin_file(
        &store,
        &media,
        &c.config(0).plugins,
        &manifest("hello"),
        &test_support::plugin_wasm("hello"),
        true,
    )
    .await
    .unwrap();
    wait_until(SETUP, "hello enabled everywhere", || {
        all_home(&c, |b| b.contains(HELLO_META))
    })
    .await;
    let saved = Instant::now();
    extensions.set_plugin_enabled("hello", false).await.unwrap();
    wait_until(SETUP, "hello gone everywhere", || {
        all_home(&c, |b| !b.contains(HELLO_META))
    })
    .await;
    let plugin_took = saved.elapsed();

    // Themes: a installed active, b installed inactive, then b activated on node 0.
    install_theme_dir(&store, &media, &c.theme_dir("a", "MARK-A"), true)
        .await
        .unwrap();
    let b = install_theme_dir(&store, &media, &c.theme_dir("b", "MARK-B"), false)
        .await
        .unwrap();
    wait_until(SETUP, "theme a everywhere", || {
        all_home(&c, |body| body.contains("MARK-A"))
    })
    .await;
    let saved = Instant::now();
    extensions.activate_theme("b").await.unwrap();
    wait_until(SETUP, "theme b everywhere", || {
        all_home(&c, |body| body.contains("MARK-B"))
    })
    .await;
    let theme_took = saved.elapsed();
    eprintln!("reload: plugin disable on 3 nodes {plugin_took:?}, theme activation {theme_took:?}");
    assert!(
        plugin_took < CONVERGE,
        "plugin disable took {plugin_took:?}"
    );
    assert!(
        theme_took < CONVERGE,
        "theme activation took {theme_took:?}"
    );

    // Every node reports the same theme build (the artifact `b`).
    wait_until(SETUP, "nodes.meta names theme b on every node", || async {
        let mut ok = true;
        for i in 0..c.len() {
            ok &= c.meta(i).await["theme"]["name"] == "b";
        }
        ok
    })
    .await;
    let mut hashes = Vec::new();
    for i in 0..c.len() {
        let m = c.meta(i).await;
        assert_eq!(
            m["theme"]["origin"]["artifact"],
            b.artifact_hash.as_str(),
            "{m}"
        );
        assert!(m["theme"]["error"].is_null(), "{m}");
        assert_eq!(m["plugins"]["hello"]["status"], "disabled", "{m}");
        hashes.push(m["theme"]["hash"].clone());
    }
    assert!(hashes[0].is_string(), "{hashes:?}");
    assert!(hashes.iter().all(|h| *h == hashes[0]), "{hashes:?}");
    c.finish().await;
}

const THRESHOLD: u32 = 20;
const PER_NODE: usize = 12;

/// Neither node alone reaches the threshold (12 < 20); together they do (24), so the
/// plugin is disabled only through the shared counters, and on both nodes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn auto_disable_is_cluster_wide() {
    let spec = Spec::new(2).env(
        "FERRITE_PLUGINS__DISABLE_AFTER_FAILURES",
        THRESHOLD.to_string(),
    );
    let Some(c) = Cluster::with(spec, |_| NodeOptions::default()).await else {
        return;
    };
    let store = c.store(0);
    // One fresh page per request: a page whose plugin call failed is cached (short TTL),
    // so a repeated path would not call the plugin again.
    let svc = content(store.clone());
    let actor = admin(store.as_ref()).await;
    let mut paths = Vec::new();
    for k in 0..PER_NODE {
        let slug = format!("flap-{k}");
        published_post(&svc, &actor, &slug).await;
        paths.push(format!("/{slug}/"));
    }
    install_plugin_file(
        &store,
        &c.node(0).state.media,
        &c.config(0).plugins,
        &manifest("flapper"),
        &test_support::plugin_wasm("flapper"),
        true,
    )
    .await
    .unwrap();
    wait_until(SETUP, "flapper loaded on both nodes", || async {
        (0..c.len()).all(|i| c.node(i).state.plugins.is_enabled("flapper"))
    })
    .await;
    for i in 0..c.len() {
        for path in &paths {
            assert_eq!(
                c.get(i, path).await.0,
                200,
                "a failing plugin never breaks a page"
            );
        }
    }
    let last_request = Instant::now();
    wait_until(
        Duration::from_secs(6),
        "flapper disabled on both nodes",
        || async { (0..c.len()).all(|i| !c.node(i).state.plugins.is_enabled("flapper")) },
    )
    .await;
    let took = last_request.elapsed();

    let row =
        c.db.store()
            .read()
            .plugins()
            .get("flapper")
            .await
            .unwrap()
            .unwrap();
    assert!(!row.enabled, "{row:?}");
    let reason = row.disabled_reason.clone().unwrap();
    let epoch = Timestamp::from_micros(0).unwrap();
    let failures =
        c.db.store()
            .read()
            .plugin_failures()
            .sum_since("flapper", epoch)
            .await
            .unwrap();
    eprintln!(
        "auto-disable: {failures} shared failures, disabled {took:?} after the last request ({reason})"
    );
    assert_eq!(
        failures,
        (2 * PER_NODE) as u64,
        "one failure per render, both nodes counted"
    );
    let events: Vec<_> =
        c.db.store()
            .read()
            .events()
            .since(0, 1000)
            .await
            .unwrap()
            .into_iter()
            .filter(|e| e.kind == PLUGIN_DISABLED)
            .collect();
    assert_eq!(events.len(), 1, "one plugin.disabled event: {events:?}");
    assert_eq!(events[0].keys, ["flapper".to_owned(), reason.clone()]);
    wait_until(
        SETUP,
        "nodes.meta reports flapper disabled on both nodes",
        || async {
            let mut ok = true;
            for i in 0..c.len() {
                let m = c.meta(i).await;
                let entry = &m["plugins"]["flapper"];
                ok &= entry["status"] == "disabled" && entry["error"] == reason.as_str();
            }
            ok
        },
    )
    .await;
    c.finish().await;
}
