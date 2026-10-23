//! `install_plugin_file` (ruling P21) on both backends: upload, rows, event, and a
//! plugin that does not load is refused before anything is written.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use ferrite::extensions::{PLUGIN_MIME, install_plugin_file};
use ferrite_core::events::PLUGINS_CHANGED;
use ferrite_domain::ArtifactKind;
use ferrite_http::load_plugins;
use ferrite_media::{Backend, MediaConfig, MediaStore, PublicUrlMode};
use ferrite_plugin_host::{HostConfig, InMemoryServices, PluginHost};
use ferrite_store_tests::{TestDb, backend_tests};

#[path = "../../ferrite-plugin-host/src/test_support.rs"]
mod test_support;

fn manifest(name: &str) -> String {
    let dir = test_support::repo_root()
        .join("plugins/examples")
        .join(name);
    std::fs::read_to_string(dir.join("ferrite-plugin.toml")).unwrap()
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
    let row = install_plugin_file(&store, &media, &manifest("hello"), &wasm, true)
        .await
        .unwrap();
    assert_eq!(row.name, "hello");
    assert_eq!(row.version, "0.1.0");
    assert!(row.enabled);
    assert_eq!(row.disabled_reason, None);
    assert!(
        row.granted.is_null(),
        "operator install: manifest honoured (R21)"
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
    let summary = load_plugins(store.clone(), media.clone(), host.clone())
        .await
        .unwrap();
    assert_eq!(summary.loaded, ["hello"], "{:?}", summary.skipped);

    // Installing again with `enabled = false` keeps the row's state, like a theme's
    // `activate = false`.
    let again = install_plugin_file(&store, &media, &manifest("hello"), &wasm, false)
        .await
        .unwrap();
    assert_eq!(again.artifact_hash, row.artifact_hash);
    assert!(again.enabled);

    // A new plugin installed with `enabled = false` stays disabled.
    let clock = install_plugin_file(
        &store,
        &media,
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
        // A memory limit above the host default is refused by `prepare`.
        (
            format!("{}\n[limits]\nmemory = \"1024MiB\"\n", manifest("hello")),
            hello,
            "does not load",
        ),
    ] {
        let err = install_plugin_file(&store, &media, &manifest, &wasm, true)
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

backend_tests!(
    install_plugin_file_roundtrip,
    install_plugin_file_refuses_what_does_not_load,
);
