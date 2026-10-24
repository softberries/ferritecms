//! `/themes/{name}/settings` and `/plugins/{name}/settings` (M2a-2 Task 6; P13, P14;
//! Review Focus 3; pre-flight F10, F22): the schema the admin renders, validated
//! writes, write-only secrets, and the new values reaching the public site through the
//! outbox.

use std::sync::Arc;

use axum::http::{Method, StatusCode};
use ferrite_core::events::option_key;
use ferrite_domain::{Event, Role};
use serde_json::{Value, json};

use crate::{
    api_tests,
    harness::{Api, Client, Resp, plugin_manifest, plugin_wasm},
};

api_tests!(
    theme_settings_change_shows_on_page,
    plugin_settings_over_http,
    editor_cannot_patch_extension_settings,
);

const BLOG_SETTINGS: &str = "/api/v1/themes/blog/settings";
const PROBE: &str = "settings-probe";
const PROBE_SETTINGS: &str = "/api/v1/plugins/settings-probe/settings";
const SECRET: &str = "sk-live-0123456789abcdef";

async fn admin(api: &Api) -> Client {
    api.user("admin", Role::Admin, false).await;
    api.logged_in("admin").await
}

async fn patch(c: &mut Client, api: &Api, path: &str, body: Value) -> Resp {
    c.send(api, Method::PATCH, path, Some(&body), None).await
}

async fn max_seq(api: &Api) -> i64 {
    api.store.read().events().max_seq().await.unwrap()
}

async fn events_after(api: &Api, seq: i64) -> Vec<(String, Vec<String>)> {
    api.store
        .read()
        .events()
        .since(seq, 100)
        .await
        .unwrap()
        .into_iter()
        .map(|Event { kind, keys, .. }| (kind, keys))
        .collect()
}

async fn option(api: &Api, key: &str) -> Option<Value> {
    api.store
        .read()
        .options()
        .get(key)
        .await
        .unwrap()
        .map(|e| e.value)
}

/// The embedded `blog` theme's schema, as the API shows it (the row stores the same).
fn blog_schema() -> Value {
    json!([
        {"key": "accent_color", "type": "color",
         "label": "Accent colour (mid-tones keep links readable in light and dark mode)",
         "default": "#c2410c", "options": []},
        {"key": "show_author", "type": "bool", "label": "Show post authors",
         "default": true, "options": []},
        {"key": "show_excerpts", "type": "bool", "label": "Show excerpts in listings",
         "default": true, "options": []},
    ])
}

/// Swaps the harness node to the active row's theme and applies the outbox, as a node's
/// reload task and invalidation loop do on `theme.changed` (pre-flight F18).
async fn swap_to_active(api: &Api, poller: &mut ferrite_cache::InvalidationLoop) {
    let row = api.store.read().themes().active().await.unwrap().unwrap();
    let theme = api.api.installer.theme_from_artifact(&row).await.unwrap();
    api.app.swap_theme(Arc::new(theme));
    poller.poll_once().await.unwrap();
}

async fn theme_settings_change_shows_on_page(api: Api) -> Api {
    let mut c = admin(&api).await;

    // An embedded theme without a row: its schema comes from the embedded files.
    let r = c.get(&api, BLOG_SETTINGS).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    let dto = r.json();
    assert_eq!(dto["schema"], blog_schema(), "{dto}");
    assert_eq!(
        dto["values"],
        json!({"accent_color": "#c2410c", "show_author": true, "show_excerpts": true})
    );

    // Activated: the row stores the schema (pre-flight F10), and `/` shows the default.
    let r = c.post_empty(&api, "/api/v1/themes/blog/activate").await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    let row = api
        .store
        .read()
        .themes()
        .get("blog")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.manifest["settings"], blog_schema(), "{}", row.manifest);
    let mut poller = api.poller().await;
    swap_to_active(&api, &mut poller).await;
    assert!(api.get("/").await.body.contains("--accent:#c2410c"));
    // Cached now: the second view is a hit and still shows the default.
    assert!(api.get("/").await.body.contains("--accent:#c2410c"));

    // PATCH: one `options.changed` naming the dependency key every page records; after
    // one poll `/` shows the new colour.
    let seq = max_seq(&api).await;
    let r = patch(
        &mut c,
        &api,
        BLOG_SETTINGS,
        json!({"accent_color": "#123456"}),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    let dto = r.json();
    assert_eq!(dto["schema"], blog_schema());
    assert_eq!(dto["values"]["accent_color"], "#123456", "{dto}");
    assert_eq!(dto["values"]["show_author"], true, "{dto}");
    assert_eq!(
        events_after(&api, seq).await,
        [(
            "options.changed".to_owned(),
            vec![option_key("theme.blog.settings")]
        )]
    );
    assert_eq!(
        option(&api, "theme.blog.settings").await,
        Some(json!({"accent_color": "#123456"}))
    );
    poller.poll_once().await.unwrap();
    let page = api.get("/").await.body;
    assert!(page.contains("--accent:#123456"), "{page}");
    assert_eq!(c.get(&api, BLOG_SETTINGS).await.json(), dto);

    // Refusals: a bad value names the key; an unknown key; nothing is written.
    let seq = max_seq(&api).await;
    let p = patch(
        &mut c,
        &api,
        BLOG_SETTINGS,
        json!({"accent_color": "red;}"}),
    )
    .await
    .assert_problem(422, "validation");
    assert_eq!(p["errors"][0]["field"], "accent_color", "{p}");
    patch(&mut c, &api, BLOG_SETTINGS, json!({"nope": true}))
        .await
        .assert_problem(422, "validation");
    patch(&mut c, &api, BLOG_SETTINGS, json!(["accent_color"]))
        .await
        .assert_problem(400, "malformed-json");
    assert_eq!(events_after(&api, seq).await, []);

    // `null` resets to the default.
    let r = patch(&mut c, &api, BLOG_SETTINGS, json!({"accent_color": null})).await;
    assert_eq!(r.json()["values"]["accent_color"], "#c2410c");
    poller.poll_once().await.unwrap();
    assert!(api.get("/").await.body.contains("--accent:#c2410c"));

    // A row stored before rows carried the schema (M1.5): the schema comes from the
    // row's artifact (pre-flight F22).
    let mut legacy = row.clone();
    legacy.manifest = json!({"name": "blog", "version": "0.1.0"});
    api.store.read().themes().upsert(&legacy).await.unwrap();
    let dto = c.get(&api, BLOG_SETTINGS).await.json();
    assert_eq!(dto["schema"], blog_schema(), "{dto}");
    let r = patch(&mut c, &api, BLOG_SETTINGS, json!({"show_author": false})).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");

    // An uploaded theme without settings: an empty schema; unknown themes are 404.
    let bundle = ferrite_theme::pack(&ferrite_theme::ThemeSource::Dir(
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/upload-theme"),
    ))
    .unwrap();
    let r = c
        .raw(&api, "/api/v1/themes", "application/x-tar", bundle)
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{r:?}");
    let dto = c.get(&api, "/api/v1/themes/uploaded/settings").await.json();
    assert_eq!(dto, json!({"schema": [], "values": {}}));
    for name in ["nope", "Blog", "a.b"] {
        c.get(&api, &format!("/api/v1/themes/{name}/settings"))
            .await
            .assert_problem(404, "not-found");
        patch(
            &mut c,
            &api,
            &format!("/api/v1/themes/{name}/settings"),
            json!({}),
        )
        .await
        .assert_problem(404, "not-found");
    }
    api
}

/// Installs `settings-probe` through the API (inspect, then install with the
/// fingerprint) and enables it.
async fn install_probe(c: &mut Client, api: &Api) {
    let manifest = plugin_manifest(PROBE);
    let wasm = plugin_wasm(PROBE);
    let r = c
        .multipart(
            api,
            "/api/v1/plugins/inspect",
            &[("manifest", manifest.as_bytes()), ("component", &wasm)],
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    let fingerprint = r.json()["fingerprint"].as_str().unwrap().to_owned();
    let r = c
        .multipart(
            api,
            "/api/v1/plugins",
            &[
                ("manifest", manifest.as_bytes()),
                ("component", &wasm),
                ("approve", fingerprint.as_bytes()),
            ],
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{r:?}");
    let r = c
        .post_empty(api, &format!("/api/v1/plugins/{PROBE}/enable"))
        .await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
}

/// The `settings-probe` meta of a page.
fn probe_meta(html: &str) -> Option<&str> {
    let start = "<meta name=\"settings-probe\" content=\"";
    let rest = &html[html.find(start)? + start.len()..];
    rest.split('"').next()
}

async fn plugin_settings_over_http(api: Api) -> Api {
    let mut c = admin(&api).await;
    install_probe(&mut c, &api).await;
    api.reloader.reload().await.unwrap();
    let mut poller = api.poller().await;
    poller.poll_once().await.unwrap();

    // The schema from the manifest's `[settings]`; nothing set.
    let r = c.get(&api, PROBE_SETTINGS).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    let dto = r.json();
    assert_eq!(
        dto["schema"],
        json!([
            {"key": "api_key", "type": "secret", "label": "API key", "default": null, "options": []},
            {"key": "limit", "type": "number", "label": "Limit", "default": null, "options": []},
            {"key": "title", "type": "text", "label": "Title", "default": "Probe", "options": []},
        ]),
        "{dto}"
    );
    assert_eq!(dto["values"], json!({"limit": null, "title": "Probe"}));
    assert_eq!(dto["secrets"], json!({"api_key": false}));
    let page = api.get("/").await.body;
    assert_eq!(
        probe_meta(&page),
        Some("title=Probe;limit=-;key=-;undeclared=-;opt-a=-;opt-b=-"),
        "{page}"
    );

    // PATCH a secret and a value: the response says only that the secret is set.
    let seq = max_seq(&api).await;
    let r = patch(
        &mut c,
        &api,
        PROBE_SETTINGS,
        json!({"api_key": SECRET, "title": "Hello"}),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert!(!r.body.contains(SECRET), "{}", r.body);
    let dto = r.json();
    assert_eq!(dto["secrets"], json!({"api_key": true}));
    assert_eq!(dto["values"], json!({"limit": null, "title": "Hello"}));
    let mut events = events_after(&api, seq).await;
    events.sort();
    assert_eq!(
        events,
        [
            (
                "options.changed".to_owned(),
                vec![option_key("plugin.settings-probe.settings.api_key")]
            ),
            (
                "options.changed".to_owned(),
                vec![option_key("plugin.settings-probe.settings.title")]
            ),
        ]
    );
    // Never returned again, by any read; never in the audit.
    for path in [
        PROBE_SETTINGS,
        "/api/v1/plugins/settings-probe",
        "/api/v1/plugins",
    ] {
        let r = c.get(&api, path).await;
        assert_eq!(r.status, StatusCode::OK, "{r:?}");
        assert!(!r.body.contains(SECRET), "{path}: {}", r.body);
    }
    let audit = api.store.read().audit().list(50, None).await.unwrap().items;
    assert_eq!(audit[0].action, "plugin.settings");
    assert_eq!(
        audit[0].details,
        json!({"name": PROBE, "keys": ["api_key", "title"]})
    );
    assert!(
        audit
            .iter()
            .all(|a| !a.details.to_string().contains(SECRET))
    );
    let stored = option(&api, "plugin.settings-probe.settings.api_key")
        .await
        .unwrap();
    assert!(stored["sealed"].as_str().unwrap().starts_with("v1."));

    // After one poll the page shows what the plugin now reads: its own secret, opened
    // only by its own `settings.get`.
    poller.poll_once().await.unwrap();
    let page = api.get("/").await.body;
    assert_eq!(
        probe_meta(&page),
        Some(format!("title=Hello;limit=-;key={SECRET};undeclared=-;opt-a=-;opt-b=-").as_str()),
        "{page}"
    );

    // Refusals name the key and never echo the value; nothing is written.
    let seq = max_seq(&api).await;
    let long = "x".repeat(4_097);
    let r = patch(&mut c, &api, PROBE_SETTINGS, json!({"api_key": long})).await;
    let p = r.assert_problem(422, "validation");
    assert_eq!(p["errors"][0]["field"], "api_key", "{p}");
    assert!(!r.body.contains(&long));
    let p = patch(&mut c, &api, PROBE_SETTINGS, json!({"limit": "five"}))
        .await
        .assert_problem(422, "validation");
    assert_eq!(p["errors"][0]["field"], "limit", "{p}");
    patch(&mut c, &api, PROBE_SETTINGS, json!({"undeclared": "x"}))
        .await
        .assert_problem(422, "validation");
    patch(
        &mut c,
        &api,
        "/api/v1/plugins/none/settings",
        json!({"title": "x"}),
    )
    .await
    .assert_problem(404, "not-found");
    c.get(&api, "/api/v1/plugins/none/settings")
        .await
        .assert_problem(404, "not-found");
    assert_eq!(events_after(&api, seq).await, []);

    // `null` clears the secret; the plugin then reads nothing.
    let r = patch(
        &mut c,
        &api,
        PROBE_SETTINGS,
        json!({"api_key": null, "limit": 3}),
    )
    .await;
    assert_eq!(r.json()["secrets"], json!({"api_key": false}));
    assert_eq!(
        option(&api, "plugin.settings-probe.settings.api_key").await,
        None
    );
    poller.poll_once().await.unwrap();
    assert_eq!(
        probe_meta(&api.get("/").await.body),
        Some("title=Hello;limit=3;key=-;undeclared=-;opt-a=-;opt-b=-")
    );
    api
}

async fn editor_cannot_patch_extension_settings(api: Api) -> Api {
    let mut root = admin(&api).await;
    install_probe(&mut root, &api).await;
    api.user("ed", Role::Editor, false).await;
    let mut ed = api.logged_in("ed").await;
    let seq = max_seq(&api).await;
    for path in [BLOG_SETTINGS, PROBE_SETTINGS] {
        ed.get(&api, path).await.assert_problem(403, "forbidden");
        patch(
            &mut ed,
            &api,
            path,
            json!({"title": "x", "accent_color": "#000"}),
        )
        .await
        .assert_problem(403, "forbidden");
    }

    // An admin token with `extensions:read` reads but cannot write.
    let user = api.user("reader", Role::Admin, false).await;
    let bearer = api.bearer(&user, &["extensions:read"]).await;
    for (path, body) in [
        (BLOG_SETTINGS, json!({"accent_color": "#000"})),
        (PROBE_SETTINGS, json!({"title": "x"})),
    ] {
        let r = bearer.get(&api, path).await;
        assert_eq!(r.status, StatusCode::OK, "{r:?}");
        let p = bearer
            .send(&api, Method::PATCH, path, Some(&body), None)
            .await
            .assert_problem(403, "forbidden");
        assert!(
            p["detail"].as_str().unwrap().contains("extensions:write"),
            "{p}"
        );
    }
    assert_eq!(events_after(&api, seq).await, []);
    assert_eq!(option(&api, "theme.blog.settings").await, None);
    assert_eq!(
        option(&api, "plugin.settings-probe.settings.title").await,
        None
    );
    api
}
