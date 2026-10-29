//! Compile-time check: every `ExtensionService` and `ExtensionAdmin` call is a `Send`
//! future, so callers can `tokio::spawn` it or hold it inside other `Send` futures (job
//! handlers, test seams). The futures are built and dropped, never run.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::future::Future;

use ferrite_core::{Actor, ExtensionAdmin, ExtensionService, auth::SiteKey};
use ferrite_domain::{
    Artifact, ArtifactHash, ArtifactKind, InstalledPlugin, InstalledTheme, Role, Timestamp, UserId,
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
        orphaned_at: None,
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
    assert_send(svc.activate_theme("t", &artifact.hash));
    assert_send(svc.install_plugin(&artifact, &plugin));
    assert_send(svc.set_plugin_enabled("p", true));
    assert_send(svc.delete_plugin("p"));
    assert_send(svc.purge_plugin_data("p", Timestamp::now(), &[]));
    let admin = ExtensionAdmin::new(db.shared_store(), SiteKey::generate().unwrap());
    let actor = Actor::new(UserId::new(), Role::Admin);
    assert_send(admin.list_plugins(&actor));
    assert_send(admin.get_plugin(&actor, "p"));
    assert_send(admin.list_themes(&actor));
    assert_send(admin.install_plugin(&actor, &artifact, &plugin));
    assert_send(admin.set_plugin_enabled(&actor, "p", true));
    assert_send(admin.delete_plugin(&actor, "p", true));
    assert_send(admin.install_theme(&actor, &artifact, &theme, true));
    assert_send(admin.activate_theme(&actor, "t", &artifact.hash));
    let schema = ferrite_core::ext_settings::SettingsSchema::default();
    assert_send(admin.theme_settings(&actor, "t", &schema));
    assert_send(admin.set_theme_settings(&actor, "t", &schema, serde_json::Map::new()));
    assert_send(admin.plugin_settings(&actor, "p"));
    assert_send(admin.set_plugin_settings(&actor, "p", serde_json::Map::new()));
    db.teardown().await;
}
