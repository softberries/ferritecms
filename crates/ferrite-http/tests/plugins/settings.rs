//! Plugin settings through the host (M2a-2 P13, Review Focus 3): real guests read only
//! their own declared settings, `options.get` never returns settings or sealed secrets
//! (even granted `options.read = ["*"]`), and a sealed value opens only for the plugin
//! and key it was sealed for.

use ferrite_core::ExtensionAdmin;
use ferrite_domain::Role;
use ferrite_http::{PluginRequest, RenderTrace, StoreHostServices};
use ferrite_plugin_host::{HostServices, Manifest, SettingSpec};
use serde_json::{Map, Value, json};

use crate::common::{SITE_URL, Site, plugin_manifest, plugin_wasm};
use crate::profile_tests;

profile_tests!(
    secrets_open_only_for_the_owner,
    type_changed_settings_fall_back_after_update
);

/// The `settings-probe` manifest's definition of `key`.
fn spec(key: &str) -> SettingSpec {
    Manifest::parse(&plugin_manifest("settings-probe"))
        .unwrap()
        .settings
        .remove(key)
        .unwrap()
}

/// `settings-probe`'s manifest named `name`, with `settings` as its `[settings]` table.
fn probe_manifest(name: &str, settings: Option<&str>) -> String {
    let manifest = plugin_manifest("settings-probe").replace(
        "name      = \"settings-probe\"",
        &format!("name      = \"{name}\""),
    );
    match settings {
        None => manifest,
        Some(table) => {
            let at = manifest.find("[settings]").unwrap();
            format!("{}[settings]\n{table}", &manifest[..at])
        }
    }
}

/// Every `settings-probe` meta of `html`, in page order.
fn probes(html: &str) -> Vec<String> {
    let start = "<meta name=\"settings-probe\" content=\"";
    html.match_indices(start)
        .filter_map(|(at, _)| html[at + start.len()..].split('"').next())
        .map(str::to_owned)
        .collect()
}

async fn head(site: &Site) -> String {
    let mut trace = RenderTrace::default();
    ferrite_http::page_head(
        &site.host,
        site.store.as_ref(),
        crate::SITE,
        ferrite_http::HeadPage {
            item: None,
            path: "/",
        },
        &PluginRequest::public("/", "en", None),
        &mut trace,
    )
    .await
}

fn patch(v: Value) -> Map<String, Value> {
    v.as_object().unwrap().clone()
}

async fn secrets_open_only_for_the_owner(site: Site) -> Site {
    let site = crate::with_generous_deadlines(site);
    let admin = Site::actor(&site.user("root", Role::Admin).await);
    let ext = ExtensionAdmin::new(site.store.clone(), site.key.clone());
    // One component installed as two plugins with the same settings keys.
    let hash = site.put_artifact(plugin_wasm("settings-probe")).await;
    for name in ["probe-a", "probe-b"] {
        let manifest = probe_manifest(name, None);
        assert!(manifest.contains(name));
        site.row(name, &manifest, hash.clone(), true).await;
    }
    assert_eq!(site.load().await.loaded, ["probe-a", "probe-b"]);
    let unset = "title=Probe;limit=-;key=-;undeclared=-;opt-a=-;opt-b=-";
    assert_eq!(probes(&head(&site).await), [unset, unset]);

    ext.set_plugin_settings(
        &admin,
        "probe-a",
        patch(json!({"api_key": "secret-a", "title": "A", "limit": 3})),
    )
    .await
    .unwrap();
    ext.set_plugin_settings(&admin, "probe-b", patch(json!({"api_key": "secret-b"})))
        .await
        .unwrap();
    // Each reads its own namespace; `options.get` (granted "*") sees no setting.
    assert_eq!(
        probes(&head(&site).await),
        [
            "title=A;limit=3;key=secret-a;undeclared=-;opt-a=-;opt-b=-",
            "title=Probe;limit=-;key=secret-b;undeclared=-;opt-a=-;opt-b=-",
        ]
    );
    let s = site.services.as_ref();
    assert_eq!(s.option_get("plugin.probe-a.settings.api_key").await, None);
    assert_eq!(
        s.setting_get("probe-a", "api_key", &spec("api_key"))
            .await
            .as_deref(),
        Some("\"secret-a\"")
    );

    // A sealed value moved to another plugin's (or key's) option does not open there:
    // the purpose binds plugin and key.
    let options = site.store.read().options();
    let sealed = options
        .get("plugin.probe-a.settings.api_key")
        .await
        .unwrap()
        .unwrap()
        .value;
    options
        .set("plugin.probe-b.settings.api_key", &sealed, false)
        .await
        .unwrap();
    options
        .set("plugin.probe-a.settings.limit", &sealed, false)
        .await
        .unwrap();
    assert_eq!(
        s.setting_get("probe-b", "api_key", &spec("api_key")).await,
        None
    );
    assert_eq!(
        s.setting_get("probe-a", "limit", &spec("limit")).await,
        None
    );
    assert_eq!(
        probes(&head(&site).await),
        [
            "title=A;limit=-;key=secret-a;undeclared=-;opt-a=-;opt-b=-",
            "title=Probe;limit=-;key=-;undeclared=-;opt-a=-;opt-b=-",
        ]
    );

    // Services without the site key open nothing (and read plain values as before).
    let keyless = StoreHostServices::new(site.store.clone(), site.reader.clone(), SITE_URL);
    assert_eq!(
        keyless
            .setting_get("probe-a", "api_key", &spec("api_key"))
            .await,
        None
    );
    assert_eq!(
        keyless
            .setting_get("probe-a", "title", &spec("title"))
            .await
            .as_deref(),
        Some("\"A\"")
    );
    site
}

/// Review R11: after an update changes a key's type, a stored value that does not fit
/// the new definition reaches neither the guest nor the admin view: text -> number
/// (`limit`), text -> secret (`api_key`, a plaintext never "set"), secret -> text
/// (`title`, the old secret never opened for a plain key). The view flags each key.
async fn type_changed_settings_fall_back_after_update(site: Site) -> Site {
    let site = crate::with_generous_deadlines(site);
    let admin = Site::actor(&site.user("root", Role::Admin).await);
    let ext = ExtensionAdmin::new(site.store.clone(), site.key.clone());
    let hash = site.put_artifact(plugin_wasm("settings-probe")).await;
    // Version 1: `title` secret, `limit` and `api_key` text.
    let v1 = probe_manifest(
        "probe-a",
        Some(
            "title = { type = \"secret\", label = \"T\" }\n\
             limit = { type = \"text\", label = \"L\" }\n\
             api_key = { type = \"text\", label = \"K\" }\n",
        ),
    );
    site.row("probe-a", &v1, hash.clone(), true).await;
    ext.set_plugin_settings(
        &admin,
        "probe-a",
        patch(json!({"title": "old-secret", "limit": "abc", "api_key": "plain"})),
    )
    .await
    .unwrap();
    let view = ext.plugin_settings(&admin, "probe-a").await.unwrap();
    assert!(view.invalid.is_empty(), "{view:?}");

    // Version 2 (the real manifest): `title` text, `limit` number, `api_key` secret.
    site.row("probe-a", &probe_manifest("probe-a", None), hash, true)
        .await;
    assert_eq!(site.load().await.loaded, ["probe-a"]);
    assert_eq!(
        probes(&head(&site).await),
        ["title=Probe;limit=-;key=-;undeclared=-;opt-a=-;opt-b=-"]
    );
    let s = site.services.as_ref();
    assert_eq!(
        s.setting_get("probe-a", "title", &spec("title"))
            .await
            .as_deref(),
        Some("\"Probe\""),
        "the declared default"
    );
    assert_eq!(
        s.setting_get("probe-a", "limit", &spec("limit")).await,
        None
    );
    assert_eq!(
        s.setting_get("probe-a", "api_key", &spec("api_key")).await,
        None
    );
    let view = ext.plugin_settings(&admin, "probe-a").await.unwrap();
    assert_eq!(
        Value::Object(view.values.clone()),
        json!({"title": "Probe", "limit": null})
    );
    assert!(!view.secrets["api_key"], "a plaintext is not a set secret");
    assert_eq!(view.invalid, ["api_key", "limit", "title"]);
    let shown = format!("{view:?}");
    assert!(
        !shown.contains("plain") && !shown.contains("old-secret"),
        "{shown}"
    );

    // Writing values that fit clears the flags; the guest reads them.
    let view = ext
        .set_plugin_settings(
            &admin,
            "probe-a",
            patch(json!({"title": "New", "limit": 4, "api_key": "sealed-now"})),
        )
        .await
        .unwrap();
    assert!(view.invalid.is_empty(), "{view:?}");
    assert_eq!(
        probes(&head(&site).await),
        ["title=New;limit=4;key=sealed-now;undeclared=-;opt-a=-;opt-b=-"]
    );
    site
}
