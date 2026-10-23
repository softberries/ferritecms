//! ExtensionAdmin (M2a-2 P8–P10): capability and scope checks, the approval grant,
//! audit rows, and plugin delete keeping KV and settings until the purge, on both
//! backends.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::Duration;

use ferrite_core::{
    Actor, CoreError, ExtensionAdmin, ExtensionService, Scopes,
    jobs::{PLUGIN_PURGE_KIND, PluginPurge, plugin_purge_job},
};
use ferrite_domain::{
    Artifact, ArtifactHash, ArtifactKind, InstalledPlugin, InstalledTheme, NewUser, NodeId, Role,
    Timestamp, UserId, UserStatus,
};
use ferrite_store::Store;
use ferrite_store_tests::{TestDb, backend_tests};
use serde_json::{Value, json};

async fn actor(store: &dyn Store, role: Role) -> Actor {
    let id = UserId::new();
    let tx = store.begin().await.unwrap();
    tx.users()
        .insert(&NewUser {
            id,
            login: format!("user-{id}"),
            email: format!("{id}@example.com"),
            display_name: "Test".into(),
            password_hash: None,
            role,
            status: UserStatus::Active,
            must_reset: false,
        })
        .await
        .unwrap();
    tx.commit().await.unwrap();
    Actor::new(id, role)
}

fn scoped(actor: Actor, scopes: &[&str]) -> Actor {
    let names: Vec<String> = scopes.iter().map(|s| (*s).to_owned()).collect();
    actor.with_scopes(Scopes::from_strs(&names).unwrap())
}

fn artifact(kind: ArtifactKind, seed: u8) -> Artifact {
    Artifact {
        hash: ArtifactHash::parse(&format!("{seed:02x}").repeat(32)).unwrap(),
        kind,
        bytes: 42,
        mime: "application/octet-stream".into(),
        created_at: Timestamp::now(),
    }
}

/// A plugin row declaring settings `title` and `api_key`, approved with `granted`.
fn plugin(name: &str, artifact: &Artifact, granted: Value) -> InstalledPlugin {
    let now = Timestamp::now();
    InstalledPlugin {
        name: name.into(),
        version: "0.1.0".into(),
        artifact_hash: artifact.hash.clone(),
        manifest: json!({
            "plugin": {"name": name, "version": "0.1.0"},
            "capabilities": {"kv": true},
            "settings": {
                "title": {"type": "text", "label": "Title"},
                "api_key": {"type": "secret", "label": "API key"},
            },
        }),
        granted,
        enabled: false,
        disabled_reason: None,
        installed_at: now,
        updated_at: now,
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

fn forbidden<T: std::fmt::Debug>(r: Result<T, CoreError>, what: &str) {
    match r {
        Err(CoreError::Forbidden(name)) if name == what => {}
        other => panic!("expected Forbidden({what:?}), got {other:?}"),
    }
}

async fn audit_actions(store: &dyn Store) -> Vec<String> {
    let mut rows: Vec<String> = store
        .read()
        .audit()
        .list(200, None)
        .await
        .unwrap()
        .items
        .into_iter()
        .map(|a| a.action)
        .collect();
    rows.reverse();
    rows
}

async fn deleted_at(store: &dyn Store, name: &str) -> Option<Timestamp> {
    let entry = store
        .read()
        .options()
        .get(&format!("plugin.{name}.deleted_at"))
        .await
        .unwrap()?;
    Some(Timestamp::from_micros(entry.value.as_i64().unwrap()).unwrap())
}

async fn setting(store: &dyn Store, name: &str, key: &str) -> Option<Value> {
    store
        .read()
        .options()
        .get(&format!("plugin.{name}.settings.{key}"))
        .await
        .unwrap()
        .map(|e| e.value)
}

pub async fn extension_writes_need_capabilities(db: &TestDb) {
    let store = db.store();
    let admin_svc = ExtensionAdmin::new(db.shared_store());
    let admin = actor(store, Role::Admin).await;
    let editor = actor(store, Role::Editor).await;
    let author = actor(store, Role::Author).await;
    let p = artifact(ArtifactKind::Plugin, 1);
    let t = artifact(ArtifactKind::Theme, 2);

    forbidden(admin_svc.authorize_plugins(&editor), "manage_plugins");
    forbidden(admin_svc.authorize_themes(&editor), "manage_themes");
    forbidden(
        admin_svc
            .install_plugin(&editor, &p, &plugin("seo", &p, json!({})))
            .await,
        "manage_plugins",
    );
    forbidden(
        admin_svc.set_plugin_enabled(&editor, "seo", true).await,
        "manage_plugins",
    );
    forbidden(
        admin_svc.delete_plugin(&editor, "seo", false).await,
        "manage_plugins",
    );
    forbidden(
        admin_svc
            .install_theme(&editor, &t, &theme("dark", &t), true)
            .await,
        "manage_themes",
    );
    forbidden(
        admin_svc.activate_theme(&editor, "dark").await,
        "manage_themes",
    );
    forbidden(admin_svc.list_plugins(&editor).await, "manage_plugins");
    forbidden(admin_svc.list_themes(&editor).await, "manage_themes");
    // Nothing was written by the refused calls.
    assert!(store.read().plugins().list().await.unwrap().is_empty());
    assert!(store.read().themes().list().await.unwrap().is_empty());
    assert!(audit_actions(store).await.is_empty());

    // Editors and authors see no plugin list (P10).
    forbidden(admin_svc.list_plugins(&author).await, "manage_plugins");
    forbidden(admin_svc.get_plugin(&author, "seo").await, "manage_plugins");
    forbidden(admin_svc.list_themes(&author).await, "manage_themes");

    // A full admin installs; an `extensions:read` admin token reads but cannot write.
    admin_svc
        .install_plugin(&admin, &p, &plugin("seo", &p, json!({})))
        .await
        .unwrap();
    admin_svc
        .install_theme(&admin, &t, &theme("dark", &t), false)
        .await
        .unwrap();
    let reader = scoped(admin, &["extensions:read"]);
    admin_svc.authorize_plugins(&admin).unwrap();
    admin_svc.authorize_themes(&admin).unwrap();
    forbidden(admin_svc.authorize_plugins(&reader), "extensions:write");
    forbidden(admin_svc.authorize_themes(&reader), "extensions:write");
    let names: Vec<String> = admin_svc
        .list_plugins(&reader)
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.name)
        .collect();
    assert_eq!(names, ["seo"]);
    assert_eq!(
        admin_svc.get_plugin(&reader, "seo").await.unwrap().name,
        "seo"
    );
    assert!(matches!(
        admin_svc.get_plugin(&reader, "none").await,
        Err(CoreError::NotFound)
    ));
    assert_eq!(admin_svc.list_themes(&reader).await.unwrap().len(), 1);
    let q = artifact(ArtifactKind::Plugin, 3);
    forbidden(
        admin_svc
            .install_plugin(&reader, &q, &plugin("other", &q, json!({})))
            .await,
        "extensions:write",
    );
    forbidden(
        admin_svc.set_plugin_enabled(&reader, "seo", true).await,
        "extensions:write",
    );
    forbidden(
        admin_svc.delete_plugin(&reader, "seo", true).await,
        "extensions:write",
    );
    forbidden(
        admin_svc.activate_theme(&reader, "dark").await,
        "extensions:write",
    );
    // A token without any extensions scope reads nothing.
    let content_only = scoped(admin, &["content:read"]);
    forbidden(
        admin_svc.list_plugins(&content_only).await,
        "extensions:read",
    );
    forbidden(
        admin_svc.list_themes(&content_only).await,
        "extensions:read",
    );
    assert_eq!(store.read().plugins().list().await.unwrap().len(), 1);
    assert!(store.read().plugins().get("other").await.unwrap().is_none());
}

pub async fn install_requires_a_grant(db: &TestDb) {
    let store = db.store();
    let svc = ExtensionAdmin::new(db.shared_store());
    let admin = actor(store, Role::Admin).await;
    let p = artifact(ArtifactKind::Plugin, 1);
    let seq = store.read().events().max_seq().await.unwrap();
    for granted in [json!(null), json!([]), json!("all")] {
        let r = svc
            .install_plugin(&admin, &p, &plugin("seo", &p, granted.clone()))
            .await;
        match r {
            Err(CoreError::InvalidField {
                field: "approve", ..
            }) => {}
            other => panic!("granted {granted}: {other:?}"),
        }
    }
    assert!(store.read().plugins().get("seo").await.unwrap().is_none());
    assert!(
        store
            .read()
            .artifacts()
            .get(&p.hash)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(store.read().events().max_seq().await.unwrap(), seq);
    assert!(audit_actions(store).await.is_empty());

    // The operator path (no actor) still installs a null grant, without audit.
    ExtensionService::new(db.shared_store())
        .install_plugin(&p, &plugin("seo", &p, json!(null)))
        .await
        .unwrap();
    assert!(store.read().plugins().get("seo").await.unwrap().is_some());
    assert!(audit_actions(store).await.is_empty());
}

pub async fn writes_are_audited(db: &TestDb) {
    let store = db.store();
    let svc = ExtensionAdmin::new(db.shared_store());
    let admin = actor(store, Role::Admin).await;
    let p = artifact(ArtifactKind::Plugin, 1);
    let t = artifact(ArtifactKind::Theme, 2);
    let granted = json!({"v": 1, "capabilities": {"kv": true}});

    svc.install_plugin(&admin, &p, &plugin("seo", &p, granted.clone()))
        .await
        .unwrap();
    svc.set_plugin_enabled(&admin, "seo", true).await.unwrap();
    svc.set_plugin_enabled(&admin, "seo", false).await.unwrap();
    svc.install_theme(&admin, &t, &theme("dark", &t), false)
        .await
        .unwrap();
    svc.activate_theme(&admin, "dark").await.unwrap();
    // A secret setting value stored before the delete never reaches the audit row.
    store
        .read()
        .options()
        .set("plugin.seo.settings.api_key", &json!("s3cret-value"), false)
        .await
        .unwrap();
    svc.delete_plugin(&admin, "seo", false).await.unwrap();

    assert_eq!(
        audit_actions(store).await,
        [
            "plugin.install",
            "plugin.enable",
            "plugin.disable",
            "theme.install",
            "theme.activate",
            "plugin.delete",
        ]
    );
    let rows = store.read().audit().list(200, None).await.unwrap().items;
    for row in &rows {
        assert_eq!(row.actor_id, Some(admin.user), "{row:?}");
        assert!(row.details.is_object(), "{row:?}");
        assert!(!row.details.to_string().contains("s3cret-value"), "{row:?}");
        let (kind, name) = if row.action.starts_with("plugin.") {
            ("plugin", "seo")
        } else {
            ("theme", "dark")
        };
        assert_eq!(row.target_kind, kind, "{row:?}");
        assert_eq!(row.target_id.as_deref(), Some(name), "{row:?}");
    }
    let by_action = |action: &str| {
        rows.iter()
            .find(|r| r.action == action)
            .map(|r| r.details.clone())
            .unwrap()
    };
    assert_eq!(
        by_action("plugin.install"),
        json!({
            "name": "seo",
            "version": "0.1.0",
            "artifact": p.hash.as_str(),
            "granted": granted,
        })
    );
    assert_eq!(
        by_action("plugin.delete"),
        json!({"name": "seo", "purge": false})
    );

    // A refused write is not audited.
    let before = rows.len();
    assert!(svc.set_plugin_enabled(&admin, "gone", true).await.is_err());
    assert!(svc.delete_plugin(&admin, "gone", false).await.is_err());
    assert!(svc.activate_theme(&admin, "gone").await.is_err());
    assert_eq!(audit_actions(store).await.len(), before);
}

/// Review Focus 5 (P9, F13–F16): delete keeps KV and settings until the purge job; a
/// reinstall keeps them; `purge = true` removes them at once.
pub async fn delete_keeps_data_until_purge(db: &TestDb) {
    let store = db.store();
    let svc = ExtensionAdmin::new(db.shared_store());
    let system = ExtensionService::new(db.shared_store());
    let admin = actor(store, Role::Admin).await;
    let p = artifact(ArtifactKind::Plugin, 1);
    let row = plugin("seo", &p, json!({}));
    let settings = vec!["api_key".to_owned(), "title".to_owned()];

    svc.install_plugin(&admin, &p, &row).await.unwrap();
    let kv = store.read().plugin_kv();
    kv.set("seo", "counter", b"7").await.unwrap();
    store
        .read()
        .options()
        .set("plugin.seo.settings.title", &json!("Hello"), false)
        .await
        .unwrap();

    let before = Timestamp::now();
    svc.delete_plugin(&admin, "seo", false).await.unwrap();
    assert!(store.read().plugins().get("seo").await.unwrap().is_none());
    assert_eq!(kv.get("seo", "counter").await.unwrap().unwrap(), b"7");
    assert_eq!(setting(store, "seo", "title").await, Some(json!("Hello")));
    let first = deleted_at(store, "seo").await.expect("deletion marker");
    assert!(first >= before && first <= Timestamp::now());

    // The purge job exists (its unique key is taken), due 30 days after the delete.
    let job = plugin_purge_job(&PluginPurge {
        v: 1,
        name: "seo".into(),
        deleted_at: first,
        settings: settings.clone(),
    });
    assert_eq!(job.kind, PLUGIN_PURGE_KIND);
    let thirty_days = Duration::from_secs(30 * 24 * 3600);
    let due = job.run_at.unwrap();
    assert_eq!(
        due.as_micros() - first.as_micros(),
        i64::try_from(thirty_days.as_micros()).unwrap()
    );
    assert_eq!(store.read().jobs().enqueue(&job).await.unwrap(), None);
    // Not due yet: a claim finds nothing.
    let kinds = [PLUGIN_PURGE_KIND.to_owned()];
    assert!(
        store
            .read()
            .jobs()
            .claim(NodeId::new(), &kinds, 10, Duration::from_secs(30))
            .await
            .unwrap()
            .is_empty()
    );

    // A reinstall within the retention keeps the data; the old job's purge does nothing.
    svc.install_plugin(&admin, &p, &row).await.unwrap();
    assert!(
        !system
            .purge_plugin_data("seo", first, &settings)
            .await
            .unwrap()
    );
    assert_eq!(kv.get("seo", "counter").await.unwrap().unwrap(), b"7");
    assert_eq!(setting(store, "seo", "title").await, Some(json!("Hello")));

    // Delete again (the system path enqueues the purge too, F13): only the latest
    // deletion's job purges (F14).
    system.delete_plugin("seo").await.unwrap();
    let second = deleted_at(store, "seo").await.expect("deletion marker");
    assert_ne!(second, first);
    assert!(
        !system
            .purge_plugin_data("seo", first, &settings)
            .await
            .unwrap()
    );
    assert_eq!(kv.get("seo", "counter").await.unwrap().unwrap(), b"7");
    assert!(
        system
            .purge_plugin_data("seo", second, &settings)
            .await
            .unwrap()
    );
    assert!(kv.get("seo", "counter").await.unwrap().is_none());
    assert_eq!(kv.usage("seo").await.unwrap(), (0, 0));
    assert_eq!(setting(store, "seo", "title").await, None);
    assert_eq!(deleted_at(store, "seo").await, None);
    // Running it again is a no-op.
    assert!(
        !system
            .purge_plugin_data("seo", second, &settings)
            .await
            .unwrap()
    );

    // `purge = true` removes the data at once (and still leaves a due job, F16).
    let q = artifact(ArtifactKind::Plugin, 2);
    svc.install_plugin(&admin, &q, &plugin("other", &q, json!({})))
        .await
        .unwrap();
    kv.set("other", "a", b"1").await.unwrap();
    kv.set("seo", "kept", b"1").await.unwrap();
    store
        .read()
        .options()
        .set("plugin.other.settings.api_key", &json!("k"), false)
        .await
        .unwrap();
    svc.delete_plugin(&admin, "other", true).await.unwrap();
    assert_eq!(kv.usage("other").await.unwrap(), (0, 0));
    assert_eq!(setting(store, "other", "api_key").await, None);
    assert_eq!(deleted_at(store, "other").await, None);
    assert_eq!(kv.get("seo", "kept").await.unwrap().unwrap(), b"1");
    let due = store
        .read()
        .jobs()
        .claim(NodeId::new(), &kinds, 10, Duration::from_secs(30))
        .await
        .unwrap();
    assert_eq!(due.len(), 1, "{due:?}");
    let payload: PluginPurge = serde_json::from_value(due[0].payload.clone()).unwrap();
    assert_eq!(payload.name, "other");
    assert_eq!(payload.settings, settings);
    assert!(
        !system
            .purge_plugin_data("other", payload.deleted_at, &payload.settings)
            .await
            .unwrap()
    );

    // Deleting a missing plugin is NotFound and writes nothing.
    assert!(matches!(
        svc.delete_plugin(&admin, "other", false).await,
        Err(CoreError::NotFound)
    ));
}

backend_tests!(
    extension_writes_need_capabilities,
    install_requires_a_grant,
    writes_are_audited,
    delete_keeps_data_until_purge,
);
