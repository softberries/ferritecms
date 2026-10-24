//! Plugin settings through the host (M2a-2 P13, Review Focus 3): real guests read only
//! their own declared settings, `options.get` never returns settings or sealed secrets
//! (even granted `options.read = ["*"]`), and a sealed value opens only for the plugin
//! and key it was sealed for.

use ferrite_core::ExtensionAdmin;
use ferrite_domain::Role;
use ferrite_http::{PluginRequest, RenderTrace, StoreHostServices};
use ferrite_plugin_host::HostServices;
use serde_json::{Map, Value, json};

use crate::common::{SITE_URL, Site, plugin_manifest, plugin_wasm};
use crate::profile_tests;

profile_tests!(secrets_open_only_for_the_owner);

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
    let admin = Site::actor(&site.user("root", Role::Admin).await);
    let ext = ExtensionAdmin::new(site.store.clone(), site.key.clone());
    // One component installed as two plugins with the same settings keys.
    let hash = site.put_artifact(plugin_wasm("settings-probe")).await;
    for name in ["probe-a", "probe-b"] {
        let manifest = plugin_manifest("settings-probe").replace(
            "name      = \"settings-probe\"",
            &format!("name      = \"{name}\""),
        );
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
        s.setting_get("probe-a", "api_key").await.as_deref(),
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
    assert_eq!(s.setting_get("probe-b", "api_key").await, None);
    assert_eq!(s.setting_get("probe-a", "limit").await, None);
    assert_eq!(
        probes(&head(&site).await),
        [
            "title=A;limit=-;key=secret-a;undeclared=-;opt-a=-;opt-b=-",
            "title=Probe;limit=-;key=-;undeclared=-;opt-a=-;opt-b=-",
        ]
    );

    // Services without the site key open nothing (and read plain values as before).
    let keyless = StoreHostServices::new(site.store.clone(), site.reader.clone(), SITE_URL);
    assert_eq!(keyless.setting_get("probe-a", "api_key").await, None);
    assert_eq!(
        keyless.setting_get("probe-a", "title").await.as_deref(),
        Some("\"A\"")
    );
    site
}
