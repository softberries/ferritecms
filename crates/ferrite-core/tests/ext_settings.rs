//! Extension settings (M2a-2 P13): the one schema and its validator, theme settings as
//! one option object, plugin settings one option per key with `secret` values sealed by
//! the site key per plugin and key (Review Focus 3), on both backends.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use ferrite_core::{
    Actor, CoreError, ExtensionAdmin, Scopes,
    auth::{SiteKey, open},
    events::option_key,
    ext_settings::{SettingDef, SettingType, SettingsSchema, secret_purpose, validate_value},
};
use ferrite_domain::{
    Artifact, ArtifactHash, ArtifactKind, Event, InstalledPlugin, NewUser, Role, Timestamp, UserId,
    UserStatus,
};
use ferrite_store::Store;
use ferrite_store_tests::{TestDb, backend_tests};
use serde_json::{Map, Value, json};

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

fn def(key: &str, kind: SettingType) -> SettingDef {
    SettingDef {
        key: key.into(),
        kind,
        label: key.into(),
        default: None,
        options: Vec::new(),
    }
}

fn with_default(mut d: SettingDef, default: Value) -> SettingDef {
    d.default = Some(default);
    d
}

fn select(key: &str, options: &[&str]) -> SettingDef {
    SettingDef {
        options: options.iter().map(|o| (*o).to_owned()).collect(),
        ..def(key, SettingType::Select)
    }
}

fn patch(v: Value) -> Map<String, Value> {
    v.as_object().unwrap().clone()
}

fn invalid_key<T: std::fmt::Debug>(r: Result<T, CoreError>, want: &str) {
    match r {
        Err(CoreError::InvalidKey { key, .. }) if key == want => {}
        other => panic!("expected InvalidKey({want:?}), got {other:?}"),
    }
}

fn invalid<T: std::fmt::Debug>(r: Result<T, CoreError>) {
    match r {
        Err(CoreError::Invalid(_)) => {}
        other => panic!("expected Invalid, got {other:?}"),
    }
}

fn forbidden<T: std::fmt::Debug>(r: Result<T, CoreError>, what: &str) {
    match r {
        Err(CoreError::Forbidden(name)) if name == what => {}
        other => panic!("expected Forbidden({what:?}), got {other:?}"),
    }
}

/// One case per rule of `validate_value`: each value valid or not for its definition;
/// a refusal names the setting's key.
#[test]
fn setting_values_validated_per_kind() {
    let long = |n: usize| Value::String("é".repeat(n));
    let uuid = "0190a3a4-7c1e-7b8a-9c1d-2e3f4a5b6c7d";
    let cases: Vec<(SettingDef, Value, bool)> = vec![
        // text: a string of at most 1 000 characters (not bytes), no NUL.
        (def("t", SettingType::Text), json!("hello"), true),
        (def("t", SettingType::Text), long(1_000), true),
        (def("t", SettingType::Text), long(1_001), false),
        (def("t", SettingType::Text), json!(""), true),
        (def("t", SettingType::Text), json!(5), false),
        (def("t", SettingType::Text), json!("a\u{0}b"), false),
        // textarea: at most 65 536 characters.
        (def("ta", SettingType::Textarea), long(65_536), true),
        (def("ta", SettingType::Textarea), long(65_537), false),
        (def("ta", SettingType::Textarea), json!("line\nline"), true),
        (def("ta", SettingType::Textarea), json!("\u{0}"), false),
        // secret: 1..=4 096 characters.
        (def("s", SettingType::Secret), json!("k"), true),
        (def("s", SettingType::Secret), long(4_096), true),
        (def("s", SettingType::Secret), long(4_097), false),
        (def("s", SettingType::Secret), json!(""), false),
        (def("s", SettingType::Secret), json!("a\u{0}"), false),
        (def("s", SettingType::Secret), json!(42), false),
        // number: a JSON number.
        (def("n", SettingType::Number), json!(3), true),
        (def("n", SettingType::Number), json!(-1.5), true),
        (def("n", SettingType::Number), json!("3"), false),
        (def("n", SettingType::Number), json!(null), false),
        // bool.
        (def("b", SettingType::Bool), json!(true), true),
        (def("b", SettingType::Bool), json!("true"), false),
        (def("b", SettingType::Bool), json!(1), false),
        // select: one of the options.
        (select("sel", &["a", "b"]), json!("b"), true),
        (select("sel", &["a", "b"]), json!("c"), false),
        (select("sel", &["a", "b"]), json!(["a"]), false),
        // color: `#rgb` or `#rrggbb`.
        (def("c", SettingType::Color), json!("#abc"), true),
        (def("c", SettingType::Color), json!("#A1b2C3"), true),
        (def("c", SettingType::Color), json!("#abcd"), false),
        (def("c", SettingType::Color), json!("#ggg"), false),
        (def("c", SettingType::Color), json!("red"), false),
        (def("c", SettingType::Color), json!("#abc;}"), false),
        // media: a UUID string.
        (def("m", SettingType::Media), json!(uuid), true),
        (def("m", SettingType::Media), json!("not-a-uuid"), false),
        (def("m", SettingType::Media), json!(7), false),
    ];
    for (d, value, ok) in cases {
        let r = validate_value(&d, &value);
        if ok {
            assert!(r.is_ok(), "{:?} {value}: {r:?}", d.kind);
        } else {
            invalid_key(r, &d.key);
        }
    }
}

/// The stored schema form: an array of definitions, `type` lowercase.
#[test]
fn schema_round_trips_as_an_array() {
    let schema = SettingsSchema(vec![
        with_default(def("accent", SettingType::Color), json!("#c2410c")),
        select("layout", &["wide", "narrow"]),
    ]);
    let v = serde_json::to_value(&schema).unwrap();
    assert_eq!(
        v,
        json!([
            {"key": "accent", "type": "color", "label": "accent", "default": "#c2410c", "options": []},
            {"key": "layout", "type": "select", "label": "layout", "default": null, "options": ["wide", "narrow"]},
        ])
    );
    let back: SettingsSchema = serde_json::from_value(v).unwrap();
    assert_eq!(back, schema);
}

fn theme_schema() -> SettingsSchema {
    SettingsSchema(vec![
        with_default(def("accent_color", SettingType::Color), json!("#c2410c")),
        with_default(def("show_author", SettingType::Bool), json!(true)),
        select("layout", &["wide", "narrow"]),
        def("logo", SettingType::Media),
        def("tagline", SettingType::Text),
    ])
}

async fn option(store: &dyn Store, key: &str) -> Option<Value> {
    store
        .read()
        .options()
        .get(key)
        .await
        .unwrap()
        .map(|e| e.value)
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

/// The newest audit row: (action, details).
async fn last_audit(store: &dyn Store) -> Option<(String, Value)> {
    let rows = store.read().audit().list(1, None).await.unwrap().items;
    rows.into_iter().next().map(|a| (a.action, a.details))
}

async fn audit_count(store: &dyn Store) -> usize {
    store
        .read()
        .audit()
        .list(200, None)
        .await
        .unwrap()
        .items
        .len()
}

async fn theme_settings_roundtrip_and_reset(db: &TestDb) {
    let store = db.store();
    let svc = ExtensionAdmin::new(db.shared_store(), SiteKey::generate().unwrap());
    let admin = actor(store, Role::Admin).await;
    let schema = theme_schema();

    // Nothing stored: every key at its default, else the type's empty value (as
    // templates see them).
    let values = svc.theme_settings(&admin, "blog", &schema).await.unwrap();
    assert_eq!(
        Value::Object(values),
        json!({"accent_color": "#c2410c", "show_author": true, "layout": "wide",
               "logo": null, "tagline": ""})
    );

    // A patch writes one object option, one `options.changed`, one audit row with the
    // keys.
    let seq = max_seq(store).await;
    let values = svc
        .set_theme_settings(
            &admin,
            "blog",
            &schema,
            patch(json!({"accent_color": "#123456", "show_author": false})),
        )
        .await
        .unwrap();
    assert_eq!(values["accent_color"], "#123456");
    assert_eq!(values["show_author"], false);
    assert_eq!(values["layout"], "wide");
    assert_eq!(
        option(store, "theme.blog.settings").await,
        Some(json!({"accent_color": "#123456", "show_author": false}))
    );
    assert_eq!(
        events_after(store, seq).await,
        [(
            "options.changed".to_owned(),
            vec![option_key("theme.blog.settings")]
        )]
    );
    assert_eq!(
        last_audit(store).await,
        Some((
            "theme.settings".to_owned(),
            json!({"name": "blog", "keys": ["accent_color", "show_author"]})
        ))
    );
    let read = svc.theme_settings(&admin, "blog", &schema).await.unwrap();
    assert_eq!(read, values);

    // `null` resets a key to its default; other keys stay.
    let values = svc
        .set_theme_settings(
            &admin,
            "blog",
            &schema,
            patch(json!({"accent_color": null, "layout": "narrow"})),
        )
        .await
        .unwrap();
    assert_eq!(values["accent_color"], "#c2410c");
    assert_eq!(values["layout"], "narrow");
    assert_eq!(values["show_author"], false);
    assert_eq!(
        option(store, "theme.blog.settings").await,
        Some(json!({"layout": "narrow", "show_author": false}))
    );

    // Refusals write nothing: an unknown key, a bad value (naming the key), a secret
    // in a theme schema.
    let before = (
        option(store, "theme.blog.settings").await,
        max_seq(store).await,
        audit_count(store).await,
    );
    invalid(
        svc.set_theme_settings(&admin, "blog", &schema, patch(json!({"nope": 1})))
            .await,
    );
    invalid_key(
        svc.set_theme_settings(
            &admin,
            "blog",
            &schema,
            patch(json!({"show_author": true, "accent_color": "red"})),
        )
        .await,
        "accent_color",
    );
    let mut with_secret = theme_schema();
    with_secret.0.push(def("token", SettingType::Secret));
    invalid(
        svc.set_theme_settings(&admin, "blog", &with_secret, patch(json!({"tagline": "x"})))
            .await,
    );
    let after = (
        option(store, "theme.blog.settings").await,
        max_seq(store).await,
        audit_count(store).await,
    );
    assert_eq!(before, after);

    // A stored value the schema no longer accepts reads as the default.
    store
        .read()
        .options()
        .set(
            "theme.blog.settings",
            &json!({"accent_color": "javascript:", "show_author": "yes"}),
            false,
        )
        .await
        .unwrap();
    let values = svc.theme_settings(&admin, "blog", &schema).await.unwrap();
    assert_eq!(values["accent_color"], "#c2410c");
    assert_eq!(values["show_author"], true);

    // Capabilities: editors neither read nor write; a read token reads only.
    let editor = actor(store, Role::Editor).await;
    forbidden(
        svc.theme_settings(&editor, "blog", &schema).await,
        "manage_themes",
    );
    forbidden(
        svc.set_theme_settings(&editor, "blog", &schema, patch(json!({"tagline": "x"})))
            .await,
        "manage_themes",
    );
    let reader = scoped(admin, &["extensions:read"]);
    svc.theme_settings(&reader, "blog", &schema).await.unwrap();
    forbidden(
        svc.set_theme_settings(&reader, "blog", &schema, patch(json!({"tagline": "x"})))
            .await,
        "extensions:write",
    );
}

fn artifact(seed: u8) -> Artifact {
    Artifact {
        hash: ArtifactHash::parse(&format!("{seed:02x}").repeat(32)).unwrap(),
        kind: ArtifactKind::Plugin,
        bytes: 42,
        mime: "application/wasm".into(),
        created_at: Timestamp::now(),
    }
}

/// A plugin row declaring `title` (text, default), `limit` (number) and `api_key`
/// (secret), as the row stores the manifest's `[settings]` table.
fn plugin_row(name: &str, artifact: &Artifact) -> InstalledPlugin {
    let now = Timestamp::now();
    InstalledPlugin {
        name: name.into(),
        version: "0.1.0".into(),
        artifact_hash: artifact.hash.clone(),
        manifest: json!({
            "plugin": {"name": name, "version": "0.1.0"},
            "settings": {
                "title": {"type": "text", "label": "Title", "default": "Hi"},
                "limit": {"type": "number", "label": "Limit"},
                "api_key": {"type": "secret", "label": "API key"},
            },
        }),
        granted: json!({}),
        enabled: false,
        disabled_reason: None,
        installed_at: now,
        updated_at: now,
    }
}

/// Review Focus 3 (core side): a secret is write-only (the view says only whether it is
/// set), stored sealed under its plugin and key (no other purpose opens it), never in
/// the audit, and plugins' settings are separate namespaces. The host side (`options.get`
/// and `settings.get` through real guests) is `ferrite-http`'s
/// `secrets_open_only_for_the_owner`.
async fn secrets_are_write_only_and_owner_scoped(db: &TestDb) {
    let store = db.store();
    let key = SiteKey::generate().unwrap();
    let svc = ExtensionAdmin::new(db.shared_store(), key.clone());
    let admin = actor(store, Role::Admin).await;
    let a = artifact(1);
    svc.install_plugin(&admin, &a, &plugin_row("seo", &a))
        .await
        .unwrap();
    svc.install_plugin(&admin, &a, &plugin_row("other", &a))
        .await
        .unwrap();

    // Fresh: defaults shown, the secret not set.
    let view = svc.plugin_settings(&admin, "seo").await.unwrap();
    assert_eq!(
        Value::Object(view.values.clone()),
        json!({"title": "Hi", "limit": null})
    );
    assert_eq!(
        serde_json::to_value(&view.secrets).unwrap(),
        json!({"api_key": false})
    );

    // PATCH a secret: the view shows `true` and no value; the option is sealed.
    let seq = max_seq(store).await;
    let plaintext = "sk-live-0123456789";
    let view = svc
        .set_plugin_settings(
            &admin,
            "seo",
            patch(json!({"api_key": plaintext, "limit": 5})),
        )
        .await
        .unwrap();
    assert!(view.secrets["api_key"]);
    assert!(!view.values.contains_key("api_key"), "{:?}", view.values);
    assert_eq!(view.values["limit"], 5);
    let shown = format!("{view:?}");
    assert!(!shown.contains(plaintext), "{shown}");
    let stored = option(store, "plugin.seo.settings.api_key").await.unwrap();
    let text = serde_json::to_string(&stored).unwrap();
    assert!(text.starts_with("{\"sealed\":\"v1."), "{text}");
    assert!(!text.contains(plaintext), "{text}");
    assert_eq!(
        option(store, "plugin.seo.settings.limit").await,
        Some(json!(5))
    );
    // One `options.changed` per key; the audit lists keys, never values.
    let mut events = events_after(store, seq).await;
    events.sort();
    assert_eq!(
        events,
        [
            (
                "options.changed".to_owned(),
                vec![option_key("plugin.seo.settings.api_key")]
            ),
            (
                "options.changed".to_owned(),
                vec![option_key("plugin.seo.settings.limit")]
            ),
        ]
    );
    let (action, details) = last_audit(store).await.unwrap();
    assert_eq!(action, "plugin.settings");
    assert_eq!(
        details,
        json!({"name": "seo", "keys": ["api_key", "limit"]})
    );
    assert!(!details.to_string().contains(plaintext));

    // The owner's purpose opens it; another plugin's or key's purpose does not.
    let sealed = stored["sealed"].as_str().unwrap();
    assert_eq!(
        open(&key, &secret_purpose("seo", "api_key"), sealed).unwrap(),
        plaintext.as_bytes()
    );
    for purpose in [
        secret_purpose("other", "api_key"),
        secret_purpose("seo", "title"),
        "login-challenge".to_owned(),
    ] {
        assert!(open(&key, &purpose, sealed).is_err(), "{purpose}");
    }
    // Another site key does not open it either.
    let other_key = SiteKey::generate().unwrap();
    assert!(open(&other_key, &secret_purpose("seo", "api_key"), sealed).is_err());

    // The other plugin's namespace is untouched.
    let view = svc.plugin_settings(&admin, "other").await.unwrap();
    assert!(!view.secrets["api_key"]);
    assert_eq!(view.values["limit"], Value::Null);
    assert_eq!(option(store, "plugin.other.settings.api_key").await, None);

    // A sealed value under a key that is not (or no longer) a secret is not shown.
    store
        .read()
        .options()
        .set("plugin.other.settings.title", &stored, false)
        .await
        .unwrap();
    let view = svc.plugin_settings(&admin, "other").await.unwrap();
    assert_eq!(view.values["title"], "Hi");
    store
        .read()
        .options()
        .delete("plugin.other.settings.title")
        .await
        .unwrap();

    // `null` clears a secret.
    let view = svc
        .set_plugin_settings(&admin, "seo", patch(json!({"api_key": null})))
        .await
        .unwrap();
    assert!(!view.secrets["api_key"]);
    assert_eq!(option(store, "plugin.seo.settings.api_key").await, None);

    // Refusals write nothing: unknown key, bad value (the key is named, never the
    // value), unknown plugin.
    let before = (max_seq(store).await, audit_count(store).await);
    invalid(
        svc.set_plugin_settings(&admin, "seo", patch(json!({"nope": "x"})))
            .await,
    );
    match svc
        .set_plugin_settings(&admin, "seo", patch(json!({"api_key": "", "limit": 1})))
        .await
    {
        Err(CoreError::InvalidKey { key, message }) => {
            assert_eq!(key, "api_key");
            assert!(!message.is_empty());
        }
        other => panic!("expected InvalidKey, got {other:?}"),
    }
    assert!(matches!(
        svc.set_plugin_settings(&admin, "none", patch(json!({"limit": 1})))
            .await,
        Err(CoreError::NotFound)
    ));
    assert!(matches!(
        svc.plugin_settings(&admin, "none").await,
        Err(CoreError::NotFound)
    ));
    assert_eq!(before, (max_seq(store).await, audit_count(store).await));
    assert_eq!(
        option(store, "plugin.seo.settings.limit").await,
        Some(json!(5))
    );

    // Capabilities.
    let editor = actor(store, Role::Editor).await;
    forbidden(svc.plugin_settings(&editor, "seo").await, "manage_plugins");
    forbidden(
        svc.set_plugin_settings(&editor, "seo", patch(json!({"limit": 1})))
            .await,
        "manage_plugins",
    );
    let reader = scoped(admin, &["extensions:read"]);
    svc.plugin_settings(&reader, "seo").await.unwrap();
    forbidden(
        svc.set_plugin_settings(&reader, "seo", patch(json!({"limit": 1})))
            .await,
        "extensions:write",
    );
}

backend_tests!(
    theme_settings_roundtrip_and_reset,
    secrets_are_write_only_and_owner_scoped,
);
