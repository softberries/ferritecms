//! ExtensionAdmin (M2a-2 P8–P10): capability and scope checks, the approval grant,
//! audit rows, and plugin delete keeping KV and settings until the purge, on both
//! backends.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::Duration;

use ferrite_core::{
    Actor, CoreError, ExtensionAdmin, ExtensionService, Scopes,
    auth::SiteKey,
    jobs::{PLUGIN_PURGE_GRACE, PLUGIN_PURGE_KIND, PluginPurge, plugin_purge_job},
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
        orphaned_at: None,
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
        .list(&ferrite_domain::AuditQuery::first(200))
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
    let admin_svc = ExtensionAdmin::new(db.shared_store(), SiteKey::generate().unwrap());
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
        admin_svc.activate_theme(&editor, "dark", &t.hash).await,
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
    forbidden(admin_svc.get_theme(&author, "dark").await, "manage_themes");

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
    let dark = admin_svc.get_theme(&reader, "dark").await.unwrap();
    assert_eq!(dark.map(|t| t.name).as_deref(), Some("dark"));
    assert_eq!(admin_svc.get_theme(&reader, "none").await.unwrap(), None);
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
        admin_svc.activate_theme(&reader, "dark", &t.hash).await,
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
    forbidden(
        admin_svc.get_theme(&content_only, "dark").await,
        "extensions:read",
    );
    assert_eq!(store.read().plugins().list().await.unwrap().len(), 1);
    assert!(store.read().plugins().get("other").await.unwrap().is_none());
}

pub async fn install_requires_a_grant(db: &TestDb) {
    let store = db.store();
    let svc = ExtensionAdmin::new(db.shared_store(), SiteKey::generate().unwrap());
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
    let svc = ExtensionAdmin::new(db.shared_store(), SiteKey::generate().unwrap());
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
    svc.activate_theme(&admin, "dark", &t.hash).await.unwrap();
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
    let rows = store
        .read()
        .audit()
        .list(&ferrite_domain::AuditQuery::first(200))
        .await
        .unwrap()
        .items;
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
    assert!(svc.activate_theme(&admin, "gone", &t.hash).await.is_err());
    assert_eq!(audit_actions(store).await.len(), before);
}

/// Review Focus 5 (P9, F13–F16): delete keeps KV and settings until the purge job; a
/// reinstall keeps them; `purge = true` removes them at once.
pub async fn delete_keeps_data_until_purge(db: &TestDb) {
    let store = db.store();
    let svc = ExtensionAdmin::new(db.shared_store(), SiteKey::generate().unwrap());
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

    // `purge = true` removes the data at once but keeps the marker; the delete's job,
    // due after the grace, is the final sweep (F16, review round 1).
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
    assert_eq!(kv.get("seo", "kept").await.unwrap().unwrap(), b"1");
    let gone = deleted_at(store, "other").await.expect("marker kept");
    let job = plugin_purge_job(&PluginPurge {
        v: 1,
        name: "other".into(),
        deleted_at: gone,
        settings: settings.clone(),
    });
    // Same job (unique key taken), due PLUGIN_PURGE_GRACE after the deletion.
    assert_eq!(store.read().jobs().enqueue(&job).await.unwrap(), None);
    assert_eq!(PLUGIN_PURGE_GRACE, Duration::from_secs(120));
    assert!(
        store
            .read()
            .jobs()
            .claim(NodeId::new(), &kinds, 10, Duration::from_secs(30))
            .await
            .unwrap()
            .is_empty(),
        "not due before the grace ends"
    );
    // A node that had not reloaded yet writes KV after the immediate purge; the final
    // sweep removes it and the marker.
    kv.set("other", "late", b"1").await.unwrap();
    assert!(
        system
            .purge_plugin_data("other", gone, &settings)
            .await
            .unwrap()
    );
    assert_eq!(kv.usage("other").await.unwrap(), (0, 0));
    assert_eq!(deleted_at(store, "other").await, None);
    assert_eq!(kv.get("seo", "kept").await.unwrap().unwrap(), b"1");

    // Deleting a missing plugin is NotFound and writes nothing.
    assert!(matches!(
        svc.delete_plugin(&admin, "other", false).await,
        Err(CoreError::NotFound)
    ));
}

/// Review round 1: purging `foo` never touches `foo-bar` (exact plugin and option
/// keys, no prefix match).
pub async fn purge_spares_name_prefix_neighbours(db: &TestDb) {
    let store = db.store();
    let svc = ExtensionAdmin::new(db.shared_store(), SiteKey::generate().unwrap());
    let admin = actor(store, Role::Admin).await;
    let kv = store.read().plugin_kv();
    for (seed, name) in [(1, "foo"), (2, "foo-bar"), (3, "foobar")] {
        let a = artifact(ArtifactKind::Plugin, seed);
        svc.install_plugin(&admin, &a, &plugin(name, &a, json!({})))
            .await
            .unwrap();
        kv.set(name, "k", b"v").await.unwrap();
        kv.set(name, "foo", b"v").await.unwrap();
        for key in ["title", "api_key"] {
            store
                .read()
                .options()
                .set(
                    &format!("plugin.{name}.settings.{key}"),
                    &json!(name),
                    false,
                )
                .await
                .unwrap();
        }
    }
    svc.delete_plugin(&admin, "foo", true).await.unwrap();
    let marker = deleted_at(store, "foo").await.unwrap();
    assert!(
        ExtensionService::new(db.shared_store())
            .purge_plugin_data("foo", marker, &["title".into(), "api_key".into()])
            .await
            .unwrap()
    );
    assert_eq!(kv.usage("foo").await.unwrap(), (0, 0));
    assert_eq!(setting(store, "foo", "title").await, None);
    for name in ["foo-bar", "foobar"] {
        assert_eq!(kv.usage(name).await.unwrap(), (2, 2), "{name}");
        assert!(store.read().plugins().get(name).await.unwrap().is_some());
        for key in ["title", "api_key"] {
            assert_eq!(setting(store, name, key).await, Some(json!(name)), "{name}");
        }
    }
}

/// Review round 1: the daily sweep re-enqueues the purge of every deletion past the
/// retention whose plugin is not installed, with the stored settings keys.
pub async fn sweep_enqueues_overdue_purges(db: &TestDb) {
    let store = db.store();
    let system = ExtensionService::new(db.shared_store());
    let opts = store.read().options();
    let kv = store.read().plugin_kv();
    let now = Timestamp::now();
    let day = 24 * 3600 * 1_000_000_i64;
    let old = Timestamp::from_micros(now.as_micros() - 31 * day).unwrap();
    let recent = Timestamp::from_micros(now.as_micros() - day).unwrap();
    // `dead`: past the retention, no row (its own job went dead).
    opts.set("plugin.dead.deleted_at", &json!(old.as_micros()), false)
        .await
        .unwrap();
    opts.set("plugin.dead.settings.b", &json!(1), false)
        .await
        .unwrap();
    opts.set("plugin.dead.settings.a", &json!(1), false)
        .await
        .unwrap();
    kv.set("dead", "k", b"v").await.unwrap();
    // `fresh`: still within the retention.
    opts.set("plugin.fresh.deleted_at", &json!(recent.as_micros()), false)
        .await
        .unwrap();
    // `back`: past the retention but installed again (a stale marker).
    opts.set("plugin.back.deleted_at", &json!(old.as_micros()), false)
        .await
        .unwrap();
    let a = artifact(ArtifactKind::Plugin, 9);
    system
        .install_plugin(&a, &plugin("back", &a, json!(null)))
        .await
        .unwrap();
    // `install_plugin` removed `back`'s marker; put a stale one back.
    opts.set("plugin.back.deleted_at", &json!(old.as_micros()), false)
        .await
        .unwrap();
    // Unrelated `plugin.` options are ignored.
    opts.set("plugin.x", &json!(1), false).await.unwrap();
    opts.set("plugin.y.deleted_at", &json!("not a number"), false)
        .await
        .unwrap();

    let sweep_at = Timestamp::from_micros(now.as_micros() - now.as_micros() % day).unwrap();
    assert_eq!(
        system.enqueue_overdue_purges(now, sweep_at).await.unwrap(),
        1
    );
    // The same sweep run enqueues nothing twice.
    assert_eq!(
        system.enqueue_overdue_purges(now, sweep_at).await.unwrap(),
        0
    );
    let kinds = [PLUGIN_PURGE_KIND.to_owned()];
    let due = store
        .read()
        .jobs()
        .claim(NodeId::new(), &kinds, 10, Duration::from_secs(30))
        .await
        .unwrap();
    assert_eq!(due.len(), 1, "{due:?}");
    let payload: PluginPurge = serde_json::from_value(due[0].payload.clone()).unwrap();
    assert_eq!(payload.name, "dead");
    assert_eq!(payload.deleted_at, old);
    assert_eq!(payload.settings, ["a", "b"]);
    assert_eq!(
        due[0].unique_key.as_deref(),
        Some(
            format!(
                "plugin.purge:dead:{}:sweep:{}",
                old.as_micros(),
                sweep_at.as_micros()
            )
            .as_str()
        )
    );
    assert!(
        system
            .purge_plugin_data("dead", payload.deleted_at, &payload.settings)
            .await
            .unwrap()
    );
    assert_eq!(kv.usage("dead").await.unwrap(), (0, 0));
    assert!(opts.get("plugin.dead.settings.a").await.unwrap().is_none());
    assert!(opts.get("plugin.dead.deleted_at").await.unwrap().is_none());
    // A later sweep finds nothing more to do for `dead`.
    let next = Timestamp::from_micros(sweep_at.as_micros() + day).unwrap();
    assert_eq!(system.enqueue_overdue_purges(now, next).await.unwrap(), 0);
}

async fn erased_at(store: &dyn Store, name: &str) -> Option<Value> {
    store
        .read()
        .options()
        .get(&format!("plugin.{name}.erased_at"))
        .await
        .unwrap()
        .map(|e| e.value)
}

/// M2b-3 Task 10: a `purge=true` delete is hidden from the retained list only once its
/// immediate erase has committed. If that erase did not happen, the data is listed and
/// Erase now erases it; after a successful one, the delete's final sweep removes both
/// markers.
pub async fn erased_marker_follows_the_erase(db: &TestDb) {
    let store = db.store();
    let svc = ExtensionAdmin::new(db.shared_store(), SiteKey::generate().unwrap());
    let system = ExtensionService::new(db.shared_store());
    let admin = actor(store, Role::Admin).await;
    let p = artifact(ArtifactKind::Plugin, 1);
    let row = plugin("seo", &p, json!({}));
    let settings = vec!["api_key".to_owned(), "title".to_owned()];
    let kv = store.read().plugin_kv();

    // The immediate erase did not run (it failed): still retained, and erasable.
    svc.install_plugin(&admin, &p, &row).await.unwrap();
    kv.set("seo", "counter", b"7").await.unwrap();
    system
        .delete_plugin_without_erase("seo", Timestamp::now())
        .await
        .unwrap();
    assert_eq!(erased_at(store, "seo").await, None);
    let retained = svc.retained_plugins(&admin).await.unwrap();
    assert_eq!(retained.len(), 1, "{retained:?}");
    assert_eq!(retained[0].name, "seo");
    svc.purge_plugin(&admin, "seo").await.unwrap();
    assert!(kv.get("seo", "counter").await.unwrap().is_none());
    assert!(svc.retained_plugins(&admin).await.unwrap().is_empty());

    // A purge=true delete whose erase committed: not listed, nothing to purge.
    svc.install_plugin(&admin, &p, &row).await.unwrap();
    kv.set("seo", "counter", b"7").await.unwrap();
    svc.delete_plugin(&admin, "seo", true).await.unwrap();
    let deleted = deleted_at(store, "seo")
        .await
        .expect("marker kept for the sweep");
    assert_eq!(
        erased_at(store, "seo").await,
        Some(json!(deleted.as_micros()))
    );
    assert!(svc.retained_plugins(&admin).await.unwrap().is_empty());
    assert!(matches!(
        svc.purge_plugin(&admin, "seo").await,
        Err(CoreError::NotFound)
    ));

    // The final sweep (the delete's purge job) removes both markers.
    assert!(
        system
            .purge_plugin_data("seo", deleted, &settings)
            .await
            .unwrap()
    );
    assert_eq!(deleted_at(store, "seo").await, None);
    assert_eq!(erased_at(store, "seo").await, None);
}

backend_tests!(
    extension_writes_need_capabilities,
    install_requires_a_grant,
    writes_are_audited,
    delete_keeps_data_until_purge,
    purge_spares_name_prefix_neighbours,
    sweep_enqueues_overdue_purges,
    erased_marker_follows_the_erase,
);
