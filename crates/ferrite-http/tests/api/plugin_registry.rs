//! `/plugins/registry*` and `/plugins/publishers*` (M6-2, design 14 §7.3) over a signed
//! `file://` registry made with throwaway keys.

use std::sync::Arc;

use axum::{
    Router,
    body::Body,
    http::{Method, StatusCode},
};
use ferrite_domain::Role;
use ferrite_http::extensions::{RegistryTarget, TrustSettings};
use ferrite_http::registry_fetch::FileRegistry;
use serde_json::{Value, json};

use crate::{
    api_tests,
    harness::{Api, Client, Resp, json_body, plugin_manifest, plugin_wasm},
    registry_fixture::FileRegistryFixture,
};

api_tests!(
    get_registry_without_config_is_null,
    refresh_then_search,
    refresh_rate_limited,
    registry_inspect_then_install_201,
    registry_install_approval_mismatch_422,
    publishers_list_merges_site_and_registry_keys,
    revoke_compromised_disables_plugin,
    revoke_needs_manage_plugins,
    reserved_names_route_correctly,
    mcp_plugins_list_includes_trust,
);

const REGISTRY: &str = "/api/v1/plugins/registry";

fn fixture() -> FileRegistryFixture {
    let mut fx = FileRegistryFixture::new();
    let (m, w) = (plugin_manifest("hello"), plugin_wasm("hello"));
    fx.add_version("hello", "0.1.0", &m, &w, None);
    fx.add_version("hello", "0.2.0", &m, &w, None);
    fx.publish();
    fx
}

fn router(api: &Api, fx: &FileRegistryFixture) -> Router {
    api.router_with_trust(TrustSettings {
        registry: Some(RegistryTarget {
            url: url::Url::parse(&fx.url()).unwrap(),
            key: fx.registry_key.key.clone(),
        }),
        source: Some(Arc::new(FileRegistry::new(fx.path().to_path_buf()))),
        ..Default::default()
    })
}

async fn admin(api: &Api) -> Client {
    api.user("admin", Role::Admin, false).await;
    api.logged_in("admin").await
}

async fn get(c: &mut Client, r: &Router, path: &str) -> Resp {
    let req = c.builder(Method::GET, path).body(Body::empty()).unwrap();
    c.send_on(r, req).await
}

async fn post(c: &mut Client, r: &Router, path: &str, body: Value) -> Resp {
    let req = json_body(c.builder(Method::POST, path), &body);
    c.send_on(r, req).await
}

async fn post_empty(c: &mut Client, r: &Router, path: &str) -> Resp {
    let req = c.builder(Method::POST, path).body(Body::empty()).unwrap();
    c.send_on(r, req).await
}

/// Inspects `hello` `version` from the registry and installs it.
async fn install(c: &mut Client, r: &Router, version: &str) -> Resp {
    let report = post(
        c,
        r,
        &format!("{REGISTRY}/inspect"),
        json!({"name": "hello", "version": version}),
    )
    .await;
    assert_eq!(report.status, StatusCode::OK, "{report:?}");
    let fp = report.json()["fingerprint"].as_str().unwrap().to_owned();
    post(
        c,
        r,
        &format!("{REGISTRY}/install"),
        json!({"name": "hello", "version": version, "approve": fp}),
    )
    .await
}

async fn get_registry_without_config_is_null(api: Api) -> Api {
    let mut c = admin(&api).await;
    let r = c.get(&api, REGISTRY).await;
    assert_eq!(
        r.json(),
        json!({"registry": null, "plugins": [], "collisions": []}),
        "{r:?}"
    );
    let r = c.post_empty(&api, &format!("{REGISTRY}/refresh")).await;
    r.assert_problem(404, "not-found");
    let r = c
        .post(
            &api,
            &format!("{REGISTRY}/inspect"),
            json!({"name": "hello"}),
        )
        .await;
    r.assert_problem(404, "not-found");
    api
}

async fn refresh_then_search(api: Api) -> Api {
    let fx = fixture();
    let r = router(&api, &fx);
    let mut c = admin(&api).await;
    let before = get(&mut c, &r, REGISTRY).await.json();
    assert_eq!(before["registry"]["serial"], Value::Null, "{before}");
    let after = post_empty(&mut c, &r, &format!("{REGISTRY}/refresh")).await;
    assert_eq!(after.status, StatusCode::OK, "{after:?}");
    let body = after.json();
    assert_eq!(body["registry"]["serial"], 1);
    assert_eq!(body["registry"]["name"], "Test registry");
    let found = get(&mut c, &r, &format!("{REGISTRY}?q=HELLO")).await.json();
    assert_eq!(found["plugins"][0]["name"], "hello", "{found}");
    assert_eq!(found["plugins"][0]["latest"], "0.2.0");
    assert_eq!(found["plugins"][0]["publisher"]["id"], "ferritecms.dev");
    assert_eq!(found["plugins"][0]["versions"][0]["version"], "0.2.0");
    let none = get(&mut c, &r, &format!("{REGISTRY}?q=zzz")).await.json();
    assert_eq!(none["plugins"], json!([]));
    api
}

async fn refresh_rate_limited(api: Api) -> Api {
    let fx = fixture();
    let r = router(&api, &fx);
    let mut c = admin(&api).await;
    let first = post_empty(&mut c, &r, &format!("{REGISTRY}/refresh")).await;
    assert_eq!(first.status, StatusCode::OK, "{first:?}");
    let second = post_empty(&mut c, &r, &format!("{REGISTRY}/refresh")).await;
    second.assert_problem(429, "rate-limited");
    assert!(second.header("retry-after").is_some());
    api
}

async fn registry_inspect_then_install_201(api: Api) -> Api {
    let fx = fixture();
    let r = router(&api, &fx);
    let mut c = admin(&api).await;
    post_empty(&mut c, &r, &format!("{REGISTRY}/refresh")).await;
    let report = post(
        &mut c,
        &r,
        &format!("{REGISTRY}/inspect"),
        json!({"name": "hello"}),
    )
    .await;
    let body = report.json();
    assert_eq!(body["version"], "0.2.0", "{body}");
    assert_eq!(body["trust"]["source"]["kind"], "registry");
    assert_eq!(body["trust"]["signer"]["source"], "registry");
    assert_eq!(body["trust"]["signer"]["publisher"]["name"], "FerriteCMS");
    let installed = install(&mut c, &r, "0.1.0").await;
    assert_eq!(installed.status, StatusCode::CREATED, "{installed:?}");
    let body = installed.json();
    assert_eq!(body["trust"]["state"], "verified", "{body}");
    assert_eq!(body["trust"]["update"]["version"], "0.2.0");
    assert_eq!(body["trust"]["update"]["publisher_matches"], true);
    let list = get(&mut c, &r, REGISTRY).await.json();
    assert_eq!(list["plugins"][0]["installed"]["update_available"], true);
    // The update.
    let updated = install(&mut c, &r, "0.2.0").await;
    assert_eq!(updated.status, StatusCode::CREATED, "{updated:?}");
    assert_eq!(updated.json()["trust"]["update"], Value::Null);
    api
}

async fn registry_install_approval_mismatch_422(api: Api) -> Api {
    let fx = fixture();
    let r = router(&api, &fx);
    let mut c = admin(&api).await;
    post_empty(&mut c, &r, &format!("{REGISTRY}/refresh")).await;
    let resp = post(
        &mut c,
        &r,
        &format!("{REGISTRY}/install"),
        json!({"name": "hello", "version": "0.2.0", "approve": "nope"}),
    )
    .await;
    let p = resp.assert_problem(422, "validation");
    assert_eq!(p["errors"][0]["field"], "approve");
    assert_eq!(p["report"]["trust"]["source"]["kind"], "registry");
    assert!(api.store.read().plugins().list().await.unwrap().is_empty());
    api
}

async fn publishers_list_merges_site_and_registry_keys(api: Api) -> Api {
    let fx = fixture();
    let r = router(&api, &fx);
    let mut c = admin(&api).await;
    post_empty(&mut c, &r, &format!("{REGISTRY}/refresh")).await;
    // A site key, approved at an upload.
    let other = crate::registry_fixture::TestKey::new();
    let (m, w) = (plugin_manifest("clock"), plugin_wasm("clock"));
    let sig = other.sign(ferrite_plugin_host::trust::signing_payload(m.as_bytes(), &w).as_bytes());
    let public = other.public_file();
    let parts: [(&str, &[u8]); 4] = [
        ("manifest", m.as_bytes()),
        ("component", &w),
        ("signature", sig.as_bytes()),
        ("publisher_key", public.as_bytes()),
    ];
    let report = c.multipart_on(&r, "/api/v1/plugins/inspect", &parts).await;
    let fp = report.json()["fingerprint"].as_str().unwrap().to_owned();
    let mut all = parts.to_vec();
    all.push(("approve", fp.as_bytes()));
    let installed = c.multipart_on(&r, "/api/v1/plugins", &all).await;
    assert_eq!(installed.status, StatusCode::CREATED, "{installed:?}");
    let list = get(&mut c, &r, "/api/v1/plugins/publishers").await.json();
    let items = list["items"].as_array().unwrap();
    assert_eq!(list["policy"], "any");
    let by = |id: &str| items.iter().find(|k| k["key_id"] == id).cloned().unwrap();
    let site = by(&other.key.key_id.to_string());
    assert_eq!(site["source"], "approved");
    assert_eq!(site["plugins"], json!(["clock"]));
    let reg = by(&fx.publisher_key.key.key_id.to_string());
    assert_eq!(reg["source"], "registry");
    assert_eq!(reg["registry_publisher"]["id"], "ferritecms.dev");
    assert_eq!(reg["trusted_at"], Value::Null);
    api
}

async fn revoke_compromised_disables_plugin(api: Api) -> Api {
    let fx = fixture();
    let r = router(&api, &fx);
    let mut c = admin(&api).await;
    post_empty(&mut c, &r, &format!("{REGISTRY}/refresh")).await;
    assert_eq!(
        install(&mut c, &r, "0.2.0").await.status,
        StatusCode::CREATED
    );
    post_empty(&mut c, &r, "/api/v1/plugins/hello/enable").await;
    let id = fx.publisher_key.key.key_id.to_string();
    let path = format!("/api/v1/plugins/publishers/{id}/revoke");
    let bad = post(&mut c, &r, &path, json!({"reason": "  "})).await;
    assert_eq!(
        bad.assert_problem(422, "validation")["errors"][0]["field"],
        "reason"
    );
    let resp = post(
        &mut c,
        &r,
        &path,
        json!({"reason": "leaked", "compromised": true}),
    )
    .await;
    assert_eq!(resp.status, StatusCode::OK, "{resp:?}");
    let body = resp.json();
    assert_eq!(body["compromised"], true);
    assert_eq!(body["revoked_reason"], "leaked");
    let plugin = get(&mut c, &r, "/api/v1/plugins/hello").await.json();
    assert_eq!(plugin["enabled"], false, "{plugin}");
    assert_eq!(plugin["trust"]["state"], "revoked");
    assert!(
        plugin["disabled_reason"]
            .as_str()
            .unwrap()
            .contains("publisher key revoked")
    );
    // An install signed by it is refused now.
    let report = post(
        &mut c,
        &r,
        &format!("{REGISTRY}/inspect"),
        json!({"name": "hello"}),
    )
    .await;
    assert_eq!(
        report.assert_problem(422, "validation")["errors"][0]["field"],
        "signature"
    );
    let unknown = post(
        &mut c,
        &r,
        "/api/v1/plugins/publishers/0000000000000000/revoke",
        json!({"reason": "x"}),
    )
    .await;
    unknown.assert_problem(404, "not-found");
    api
}

async fn revoke_needs_manage_plugins(api: Api) -> Api {
    let fx = fixture();
    let r = router(&api, &fx);
    api.user("ed", Role::Editor, false).await;
    let mut ed = api.logged_in("ed").await;
    let id = fx.publisher_key.key.key_id.to_string();
    let path = format!("/api/v1/plugins/publishers/{id}/revoke");
    post(&mut ed, &r, &path, json!({"reason": "x"}))
        .await
        .assert_problem(403, "forbidden");
    get(&mut ed, &r, "/api/v1/plugins/publishers")
        .await
        .assert_problem(403, "forbidden");
    get(&mut ed, &r, REGISTRY)
        .await
        .assert_problem(403, "forbidden");
    let admin = api.user("adm", Role::Admin, false).await;
    let ro = api.bearer(&admin, &["extensions:read"]).await;
    let req = json_body(ro.builder(Method::POST, &path), &json!({"reason": "x"}));
    crate::harness::send(&r, req)
        .await
        .assert_problem(403, "forbidden");
    let req = ro
        .builder(Method::POST, &format!("{REGISTRY}/refresh"))
        .body(Body::empty())
        .unwrap();
    crate::harness::send(&r, req)
        .await
        .assert_problem(403, "forbidden");
    let req = ro
        .builder(Method::GET, REGISTRY)
        .body(Body::empty())
        .unwrap();
    assert_eq!(crate::harness::send(&r, req).await.status, StatusCode::OK);
    api
}

async fn reserved_names_route_correctly(api: Api) -> Api {
    let mut c = admin(&api).await;
    // `/plugins/registry` is the registry, not a plugin named `registry`.
    let r = c.get(&api, REGISTRY).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert!(r.json().get("registry").is_some());
    let r = c.get(&api, "/api/v1/plugins/publishers").await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert!(r.json().get("items").is_some());
    // An upload named `registry` is refused.
    let m = plugin_manifest("hello").replace("name      = \"hello\"", "name      = \"registry\"");
    let w = plugin_wasm("hello");
    let r = c
        .multipart(
            &api,
            "/api/v1/plugins/inspect",
            &[("manifest", m.as_bytes()), ("component", &w)],
        )
        .await;
    let p = r.assert_problem(422, "validation");
    assert_eq!(p["errors"][0]["field"], "plugin.name");
    api
}

async fn mcp_plugins_list_includes_trust(api: Api) -> Api {
    let mut c = admin(&api).await;
    let (m, w) = (plugin_manifest("hello"), plugin_wasm("hello"));
    let report = c
        .multipart(
            &api,
            "/api/v1/plugins/inspect",
            &[("manifest", m.as_bytes()), ("component", &w)],
        )
        .await;
    let fp = report.json()["fingerprint"].as_str().unwrap().to_owned();
    c.multipart(
        &api,
        "/api/v1/plugins",
        &[
            ("manifest", m.as_bytes()),
            ("component", &w),
            ("approve", fp.as_bytes()),
        ],
    )
    .await;
    let u = api.user("ada", Role::Admin, false).await;
    let t = api.bearer(&u, &["*"]).await;
    let r = crate::mcp::tool(&api, &t.0, "plugins_list", json!({})).await;
    assert_eq!(r["isError"], false, "{r}");
    let item = &r["structuredContent"]["items"][0];
    assert_eq!(item["trust"]["state"], "unsigned", "{r}");
    api
}
