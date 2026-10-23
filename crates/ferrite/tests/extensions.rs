//! `install_plugin_file` (ruling P21) on both backends: upload, rows, event, and a
//! plugin that does not load is refused before anything is written.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use ferrite::extensions::{PLUGIN_MIME, install_plugin_file};
use ferrite_config::PluginsConfig;
use ferrite_core::events::PLUGINS_CHANGED;
use ferrite_domain::ArtifactKind;
use ferrite_http::{LoadSummary, PluginReloader};
use ferrite_media::{Backend, MediaConfig, MediaStore, PublicUrlMode};
use ferrite_plugin_host::{HostConfig, InMemoryServices, PluginHost};
use ferrite_store_tests::{TestDb, backend_tests};
use serde_json::json;

#[path = "../../ferrite-plugin-host/src/test_support.rs"]
mod test_support;

fn manifest(name: &str) -> String {
    let dir = test_support::repo_root()
        .join("plugins/examples")
        .join(name);
    std::fs::read_to_string(dir.join("ferrite-plugin.toml")).unwrap()
}

/// The `[plugins]` section of a site whose `ferrite.toml` adds `extra`.
fn limits(extra: &str) -> PluginsConfig {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("ferrite.toml"),
        format!("[site]\nurl = \"https://example.com\"\n{extra}"),
    )
    .unwrap();
    ferrite_config::load(dir.path(), Vec::new())
        .unwrap()
        .plugins
}

fn media(dir: &tempfile::TempDir) -> Arc<MediaStore> {
    Arc::new(
        MediaStore::open(MediaConfig {
            backend: Backend::Local {
                root: dir.path().join("media"),
            },
            prefix: String::new(),
            public: PublicUrlMode::ServeViaApp {
                path_prefix: "/_media".into(),
            },
        })
        .unwrap(),
    )
}

async fn install_plugin_file_roundtrip(db: &TestDb) {
    let dir = tempfile::tempdir().unwrap();
    let media = media(&dir);
    let store = db.shared_store();
    let wasm = test_support::plugin_wasm("hello");
    let row = install_plugin_file(&store, &media, &limits(""), &manifest("hello"), &wasm, true)
        .await
        .unwrap();
    assert_eq!(row.name, "hello");
    assert_eq!(row.version, "0.1.0");
    assert!(row.enabled);
    assert_eq!(row.disabled_reason, None);
    assert_eq!(
        row.granted,
        json!({ "v": 1, "capabilities": { "render": { "head": true } },
                "limits": { "memory": null, "fuel": null } }),
        "operator install: the manifest is approved as written (R21, P26)"
    );
    assert_eq!(row.manifest["plugin"]["name"], "hello");
    let artifact = store
        .read()
        .artifacts()
        .get(&row.artifact_hash)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(artifact.kind, ArtifactKind::Plugin);
    assert_eq!(artifact.mime, PLUGIN_MIME);
    assert_eq!(artifact.bytes, wasm.len() as i64);
    let events = store.read().events().since(0, 100).await.unwrap();
    assert!(
        events
            .iter()
            .any(|e| e.kind == PLUGINS_CHANGED && e.keys == ["hello"]),
        "{events:?}"
    );

    let host = Arc::new(
        PluginHost::new(HostConfig::default(), Arc::new(InMemoryServices::new())).unwrap(),
    );
    let report = PluginReloader::new(store.clone(), media.clone(), host.clone())
        .reload()
        .await
        .unwrap();
    let summary = LoadSummary::from(&report);
    assert_eq!(summary.loaded, ["hello"], "{:?}", summary.skipped);

    // Installing again with `enabled = false` keeps the row's state, like a theme's
    // `activate = false`.
    let again = install_plugin_file(
        &store,
        &media,
        &limits(""),
        &manifest("hello"),
        &wasm,
        false,
    )
    .await
    .unwrap();
    assert_eq!(again.artifact_hash, row.artifact_hash);
    assert!(again.enabled);

    // A new plugin installed with `enabled = false` stays disabled.
    let clock = install_plugin_file(
        &store,
        &media,
        &limits(""),
        &manifest("clock"),
        &test_support::plugin_wasm("clock"),
        false,
    )
    .await
    .unwrap();
    assert!(!clock.enabled);
}

async fn install_plugin_file_refuses_what_does_not_load(db: &TestDb) {
    let dir = tempfile::tempdir().unwrap();
    let media = media(&dir);
    let store = db.shared_store();
    let hello = test_support::plugin_wasm("hello");
    for (manifest, wasm, expect) in [
        (manifest("hello"), b"not wasm".to_vec(), "does not load"),
        (
            "[plugin]\nname = \"x\"\n".to_owned(),
            hello.clone(),
            "manifest",
        ),
        // A memory limit above the 256 MiB ceiling is refused, approved or not (P5).
        (
            format!("{}\n[limits]\nmemory = \"1024MiB\"\n", manifest("hello")),
            hello,
            "does not load",
        ),
    ] {
        let err = install_plugin_file(&store, &media, &limits(""), &manifest, &wasm, true)
            .await
            .unwrap_err();
        assert!(format!("{err:#}").contains(expect), "{err:#}");
    }
    assert!(store.read().plugins().list().await.unwrap().is_empty());
    assert!(
        !dir.path().join("media/artifacts").exists(),
        "nothing uploaded"
    );
}

/// The check uses the site's limits (pre-flight F8): an operator install approves the
/// manifest's `[limits]`, so a request above the site's `plugins.default_memory` loads,
/// with the approval recorded in `granted.limits`, and a node with that default loads
/// it; a request above the ceiling is refused with nothing written.
async fn install_plugin_file_uses_the_site_limits(db: &TestDb) {
    let dir = tempfile::tempdir().unwrap();
    let media = media(&dir);
    let store = db.shared_store();
    let wasm = test_support::plugin_wasm("hello");
    let small = limits("[plugins]\ndefault_memory = \"16MiB\"\n");
    let over_ceiling = format!("{}\n[limits]\nmemory = \"257MiB\"\n", manifest("hello"));
    let err = install_plugin_file(&store, &media, &small, &over_ceiling, &wasm, true)
        .await
        .unwrap_err();
    assert!(format!("{err:#}").contains("above the ceiling"), "{err:#}");
    assert!(store.read().plugins().list().await.unwrap().is_empty());

    let manifest = format!("{}\n[limits]\nmemory = \"32MiB\"\n", manifest("hello"));
    let row = install_plugin_file(&store, &media, &small, &manifest, &wasm, true)
        .await
        .unwrap();
    assert_eq!(row.granted["limits"]["memory"], 32 << 20);
    let host = HostConfig {
        memory_bytes: 16 << 20,
        ..HostConfig::default()
    };
    let host = Arc::new(PluginHost::new(host, Arc::new(InMemoryServices::new())).unwrap());
    let report = PluginReloader::new(store.clone(), media.clone(), host)
        .reload()
        .await
        .unwrap();
    let summary = LoadSummary::from(&report);
    assert_eq!(summary.loaded, ["hello"], "{:?}", summary.skipped);
}

backend_tests!(
    install_plugin_file_uses_the_site_limits,
    install_plugin_file_roundtrip,
    install_plugin_file_refuses_what_does_not_load,
);
