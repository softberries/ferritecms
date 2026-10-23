//! ExtensionService: theme/plugin install and activation rows, events and the
//! `extensions` lock, on both backends.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::{Duration, Instant};

use ferrite_core::{CoreError, ExtensionService};
use ferrite_domain::{
    Artifact, ArtifactHash, ArtifactKind, Event, InstalledPlugin, InstalledTheme, Timestamp,
};
use ferrite_store::{LOCK_EXTENSIONS, Store, StoreError, lock};
use ferrite_store_tests::{TestDb, backend_tests};
use serde_json::json;

fn artifact(kind: ArtifactKind, seed: u8) -> Artifact {
    Artifact {
        hash: ArtifactHash::parse(&format!("{seed:02x}").repeat(32)).unwrap(),
        kind,
        bytes: 42,
        mime: "application/x-tar".into(),
        created_at: Timestamp::now(),
    }
}

fn theme(name: &str, artifact: &Artifact) -> InstalledTheme {
    InstalledTheme {
        name: name.into(),
        version: "1.0.0".into(),
        artifact_hash: artifact.hash.clone(),
        manifest: json!({"name": name}),
        active: false,
        installed_at: Timestamp::now(),
    }
}

fn plugin(name: &str, artifact: &Artifact, enabled: bool) -> InstalledPlugin {
    let now = Timestamp::now();
    InstalledPlugin {
        name: name.into(),
        version: "0.1.0".into(),
        artifact_hash: artifact.hash.clone(),
        manifest: json!({"name": name}),
        granted: json!([]),
        enabled,
        disabled_reason: None,
        installed_at: now,
        updated_at: now,
    }
}

async fn max_seq(store: &dyn Store) -> i64 {
    store.read().events().max_seq().await.unwrap()
}

async fn events_after(store: &dyn Store, seq: i64) -> Vec<(String, Vec<String>)> {
    store
        .read()
        .events()
        .since(seq, 100)
        .await
        .unwrap()
        .into_iter()
        .map(|Event { kind, keys, .. }| (kind, keys))
        .collect()
}

fn changed(kind: &str, name: &str) -> (String, Vec<String>) {
    (kind.to_owned(), vec![name.to_owned()])
}

async fn active_name(store: &dyn Store) -> Option<String> {
    store
        .read()
        .themes()
        .active()
        .await
        .unwrap()
        .map(|t| t.name)
}

pub async fn install_theme_writes_rows_and_event_atomically(db: &TestDb) {
    let store = db.store();
    let svc = ExtensionService::new(db.shared_store());
    let a = artifact(ArtifactKind::Theme, 1);

    let seq = max_seq(store).await;
    svc.install_theme(&a, &theme("blog", &a), true)
        .await
        .unwrap();
    assert_eq!(
        store.read().artifacts().get(&a.hash).await.unwrap(),
        Some(a.clone())
    );
    let row = store.read().themes().get("blog").await.unwrap().unwrap();
    assert!(row.active);
    assert_eq!(row.artifact_hash, a.hash);
    assert_eq!(
        events_after(store, seq).await,
        [changed("theme.changed", "blog")]
    );

    // An inactive install leaves the active theme alone: no event.
    let b = artifact(ArtifactKind::Theme, 2);
    let seq = max_seq(store).await;
    svc.install_theme(&b, &theme("other", &b), false)
        .await
        .unwrap();
    assert!(
        !store
            .read()
            .themes()
            .get("other")
            .await
            .unwrap()
            .unwrap()
            .active
    );
    assert_eq!(active_name(store).await.as_deref(), Some("blog"));
    assert!(events_after(store, seq).await.is_empty());

    // Replacing the active theme's files changes the active theme.
    let c = artifact(ArtifactKind::Theme, 3);
    let seq = max_seq(store).await;
    let mut v2 = theme("blog", &c);
    v2.version = "2.0.0".into();
    svc.install_theme(&c, &v2, false).await.unwrap();
    let row = store.read().themes().get("blog").await.unwrap().unwrap();
    assert!(row.active);
    assert_eq!(
        (row.version.as_str(), &row.artifact_hash),
        ("2.0.0", &c.hash)
    );
    assert_eq!(
        events_after(store, seq).await,
        [changed("theme.changed", "blog")]
    );

    // A failing row write (invalid name) after the artifact insert rolls both back.
    let d = artifact(ArtifactKind::Theme, 4);
    let seq = max_seq(store).await;
    let r = svc.install_theme(&d, &theme("bad\0name", &d), true).await;
    assert!(matches!(r, Err(CoreError::Invalid(_))), "{r:?}");
    assert_eq!(store.read().artifacts().get(&d.hash).await.unwrap(), None);
    assert_eq!(max_seq(store).await, seq);
    assert_eq!(store.read().themes().list().await.unwrap().len(), 2);
    assert_eq!(active_name(store).await.as_deref(), Some("blog"));

    // The artifact must be a theme artifact and match the row.
    let m = artifact(ArtifactKind::Media, 5);
    let r = svc.install_theme(&m, &theme("m", &m), false).await;
    assert!(matches!(r, Err(CoreError::Invalid(_))), "{r:?}");
    let r = svc.install_theme(&d, &theme("m", &c), false).await;
    assert!(matches!(r, Err(CoreError::Invalid(_))), "{r:?}");
    assert_eq!(store.read().artifacts().get(&m.hash).await.unwrap(), None);
    assert_eq!(store.read().artifacts().get(&d.hash).await.unwrap(), None);
    assert_eq!(max_seq(store).await, seq);

    // The same bytes already stored as another kind: Invalid after the insert, rolled back.
    let media = artifact(ArtifactKind::Media, 6);
    store
        .read()
        .artifacts()
        .insert_if_absent(&media)
        .await
        .unwrap();
    let as_theme = Artifact {
        kind: ArtifactKind::Theme,
        ..media.clone()
    };
    let r = svc
        .install_theme(&as_theme, &theme("dup", &as_theme), true)
        .await;
    assert!(
        matches!(&r, Err(CoreError::Invalid(m)) if m.contains("already stored as kind media")),
        "{r:?}"
    );
    assert_eq!(store.read().themes().get("dup").await.unwrap(), None);
    assert_eq!(
        store.read().artifacts().get(&media.hash).await.unwrap(),
        Some(media)
    );
    assert_eq!(active_name(store).await.as_deref(), Some("blog"));
    assert_eq!(max_seq(store).await, seq);
}

pub async fn activate_theme_emits_theme_changed(db: &TestDb) {
    let store = db.store();
    let svc = ExtensionService::new(db.shared_store());
    let a = artifact(ArtifactKind::Theme, 1);
    let b = artifact(ArtifactKind::Theme, 2);
    svc.install_theme(&a, &theme("one", &a), true)
        .await
        .unwrap();
    svc.install_theme(&b, &theme("two", &b), false)
        .await
        .unwrap();

    let seq = max_seq(store).await;
    svc.activate_theme("two").await.unwrap();
    assert_eq!(active_name(store).await.as_deref(), Some("two"));
    assert_eq!(
        events_after(store, seq).await,
        [changed("theme.changed", "two")]
    );

    // Activating the active theme changes nothing.
    let seq = max_seq(store).await;
    svc.activate_theme("two").await.unwrap();
    assert!(events_after(store, seq).await.is_empty());

    // A missing theme is NotFound and keeps the active one.
    let r = svc.activate_theme("missing").await;
    assert!(matches!(r, Err(CoreError::NotFound)), "{r:?}");
    assert_eq!(active_name(store).await.as_deref(), Some("two"));
    assert!(events_after(store, seq).await.is_empty());

    // Installing an existing inactive theme with `activate` switches to it.
    svc.install_theme(&a, &theme("one", &a), true)
        .await
        .unwrap();
    assert_eq!(active_name(store).await.as_deref(), Some("one"));
    assert_eq!(
        events_after(store, seq).await,
        [changed("theme.changed", "one")]
    );
}

pub async fn plugin_enable_disable_emit_plugins_changed(db: &TestDb) {
    let store = db.store();
    let svc = ExtensionService::new(db.shared_store());
    let a = artifact(ArtifactKind::Plugin, 7);

    let seq = max_seq(store).await;
    svc.install_plugin(&a, &plugin("seo", &a, false))
        .await
        .unwrap();
    assert_eq!(
        store.read().artifacts().get(&a.hash).await.unwrap(),
        Some(a.clone())
    );
    let row = store.read().plugins().get("seo").await.unwrap().unwrap();
    assert!(!row.enabled);
    assert_eq!(
        events_after(store, seq).await,
        [changed("plugins.changed", "seo")]
    );

    // Enabling clears an auto-disable reason.
    store
        .read()
        .plugins()
        .set_enabled("seo", false, Some("auto: 20 failures"))
        .await
        .unwrap();
    let seq = max_seq(store).await;
    svc.set_plugin_enabled("seo", true).await.unwrap();
    let row = store.read().plugins().get("seo").await.unwrap().unwrap();
    assert!(row.enabled);
    assert_eq!(row.disabled_reason, None);
    svc.set_plugin_enabled("seo", false).await.unwrap();
    let row = store.read().plugins().get("seo").await.unwrap().unwrap();
    assert!(!row.enabled);
    assert_eq!(row.disabled_reason, None);
    assert_eq!(
        events_after(store, seq).await,
        [
            changed("plugins.changed", "seo"),
            changed("plugins.changed", "seo")
        ]
    );

    // Reinstalling (a new version) keeps the enabled state and reason of the row; a
    // new plugin is installed disabled whatever the row says.
    svc.set_plugin_enabled("seo", true).await.unwrap();
    let b = artifact(ArtifactKind::Plugin, 9);
    let mut v2 = plugin("seo", &b, false);
    v2.version = "0.2.0".into();
    v2.disabled_reason = Some("ignored".into());
    svc.install_plugin(&b, &v2).await.unwrap();
    let row = store.read().plugins().get("seo").await.unwrap().unwrap();
    assert_eq!(
        (row.version.as_str(), &row.artifact_hash),
        ("0.2.0", &b.hash)
    );
    assert!(row.enabled);
    assert_eq!(row.disabled_reason, None);
    store
        .read()
        .plugins()
        .set_enabled("seo", false, Some("auto: 20 failures"))
        .await
        .unwrap();
    svc.install_plugin(&b, &plugin("seo", &b, true))
        .await
        .unwrap();
    let row = store.read().plugins().get("seo").await.unwrap().unwrap();
    assert!(!row.enabled);
    assert_eq!(row.disabled_reason.as_deref(), Some("auto: 20 failures"));
    let c = artifact(ArtifactKind::Plugin, 10);
    svc.install_plugin(&c, &plugin("fresh", &c, true))
        .await
        .unwrap();
    let fresh = store.read().plugins().get("fresh").await.unwrap().unwrap();
    assert!(!fresh.enabled);

    // Missing plugin: NotFound, no event. Wrong artifact kind: Invalid, nothing written.
    let seq = max_seq(store).await;
    let r = svc.set_plugin_enabled("missing", true).await;
    assert!(matches!(r, Err(CoreError::NotFound)), "{r:?}");
    let t = artifact(ArtifactKind::Theme, 8);
    let r = svc.install_plugin(&t, &plugin("x", &t, true)).await;
    assert!(matches!(r, Err(CoreError::Invalid(_))), "{r:?}");
    assert_eq!(store.read().artifacts().get(&t.hash).await.unwrap(), None);
    assert_eq!(max_seq(store).await, seq);
}

pub async fn extensions_writes_take_the_lock(db: &TestDb) {
    let store = db.store();
    let svc = ExtensionService::new(db.shared_store()).with_lock_wait(Duration::from_millis(300));
    let a = artifact(ArtifactKind::Theme, 1);
    let p = artifact(ArtifactKind::Plugin, 2);
    let seq = max_seq(store).await;

    let guard = lock(store, LOCK_EXTENSIONS, Duration::ZERO).await.unwrap();
    let busy = |r: Result<(), CoreError>| {
        assert!(
            matches!(r, Err(CoreError::Store(StoreError::Busy))),
            "{r:?}"
        );
    };
    let started = Instant::now();
    busy(svc.install_theme(&a, &theme("t", &a), true).await);
    assert!(started.elapsed() >= Duration::from_millis(300));
    busy(svc.activate_theme("t").await);
    busy(svc.install_plugin(&p, &plugin("p", &p, true)).await);
    busy(svc.set_plugin_enabled("p", false).await);
    assert_eq!(store.read().artifacts().get(&a.hash).await.unwrap(), None);
    assert_eq!(max_seq(store).await, seq);

    // Once the holder lets go, the writes go through (and release the lock again).
    guard.release().await.unwrap();
    svc.install_theme(&a, &theme("t", &a), true).await.unwrap();
    svc.install_plugin(&p, &plugin("p", &p, true))
        .await
        .unwrap();
    let again = lock(store, LOCK_EXTENSIONS, Duration::ZERO).await.unwrap();
    again.release().await.unwrap();
}

backend_tests!(
    install_theme_writes_rows_and_event_atomically,
    activate_theme_emits_theme_changed,
    plugin_enable_disable_emit_plugins_changed,
    extensions_writes_take_the_lock,
);
