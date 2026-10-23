//! Compile-time check: every `ExtensionService` write is a `Send` future, so callers can
//! `tokio::spawn` it or hold it inside other `Send` futures (job handlers, test seams).
//! The futures are built and dropped, never run.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::future::Future;

use ferrite_core::ExtensionService;
use ferrite_domain::{
    Artifact, ArtifactHash, ArtifactKind, InstalledPlugin, InstalledTheme, Timestamp,
};
use ferrite_store_tests::TestDb;
use serde_json::json;

fn assert_send<F: Future + Send>(_: F) {}

#[tokio::test]
async fn extension_service_futures_are_send() {
    let db = TestDb::sqlite().await;
    let svc = ExtensionService::new(db.shared_store());
    let hash = ArtifactHash::parse(&"a".repeat(64)).unwrap();
    let artifact = Artifact {
        hash: hash.clone(),
        kind: ArtifactKind::Plugin,
        bytes: 1,
        mime: "application/wasm".into(),
        created_at: Timestamp::now(),
    };
    let theme = InstalledTheme {
        name: "t".into(),
        version: "1.0.0".into(),
        artifact_hash: hash.clone(),
        manifest: json!({}),
        active: false,
        installed_at: Timestamp::now(),
    };
    let plugin = InstalledPlugin {
        name: "p".into(),
        version: "1.0.0".into(),
        artifact_hash: hash,
        manifest: json!({}),
        granted: json!(null),
        enabled: false,
        disabled_reason: None,
        installed_at: Timestamp::now(),
        updated_at: Timestamp::now(),
    };
    assert_send(svc.install_theme(&artifact, &theme, true));
    assert_send(svc.activate_theme("t"));
    assert_send(svc.install_plugin(&artifact, &plugin));
    assert_send(svc.set_plugin_enabled("p", true));
    db.teardown().await;
}
