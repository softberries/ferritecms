//! M2b-3 Task 2: theme and plugin manifest facts in the DTOs, retained plugin data
//! (`GET /plugins/retained`, `POST /plugins/{name}/purge`; P47) and the committed
//! `blog-min.tar` fixture (`ferrite theme pack`'s output; P42) installing over the API.

use std::path::PathBuf;

use axum::http::{Method, StatusCode};
use chrono::{DateTime, Utc};
use ferrite_domain::Role;
use serde_json::{Value, json};

use crate::{
    api_tests,
    harness::{Api, Client, Resp, plugin_manifest, plugin_wasm},
};

api_tests!(
    theme_dto_has_manifest_facts,
    plugin_dto_has_manifest_facts,
    retained_list_and_purge,
    blog_min_fixture_installs,
);

const THEMES: &str = "/api/v1/themes";
const PLUGINS: &str = "/api/v1/plugins";
const RETAINED: &str = "/api/v1/plugins/retained";

async fn admin(api: &Api) -> Client {
    api.user("admin", Role::Admin, false).await;
    api.logged_in("admin").await
}

fn theme<'a>(list: &'a Value, name: &str) -> &'a Value {
    list["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == name)
        .unwrap_or_else(|| panic!("no theme {name}: {list}"))
}

fn blog_min_tar() -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../admin/src/appearance/__fixtures__/blog-min.tar");
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn sha256(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The facts `themes/blog/theme.toml` declares, whatever the entry's origin.
fn assert_blog_facts(blog: &Value) {
    assert_eq!(blog["author"], "FerriteCMS", "{blog}");
    assert_eq!(blog["license"], "MIT", "{blog}");
    assert_eq!(
        blog["homepage"], "https://github.com/softberries/ferritecms",
        "{blog}"
    );
    assert_eq!(blog["has_settings"], true, "{blog}");
    assert_eq!(
        blog["menus"],
        json!([
            {"name": "footer", "label": "Footer"},
            {"name": "primary", "label": "Primary navigation"},
        ]),
        "{blog}"
    );
    assert_eq!(
        blog["images"],
        json!([
            {"name": "card", "width": 640, "height": 360, "crop": true},
            {"name": "hero", "width": 1280, "height": 640, "crop": true},
        ]),
        "{blog}"
    );
}

/// `ThemeDto` carries the manifest's author, license, homepage, menus, `[images]` and
/// whether it has settings, for an embedded theme (no `hash`) and for a row (its
/// artifact's hash).
async fn theme_dto_has_manifest_facts(api: Api) -> Api {
    let mut c = admin(&api).await;
    let list = c.get(&api, THEMES).await.json();
    let blog = theme(&list, "blog");
    assert_eq!(blog["origin"], "embedded", "{blog}");
    assert_blog_facts(blog);
    assert_eq!(blog["hash"], Value::Null, "{blog}");

    // Activated, the embedded theme becomes a row: the same facts, plus its artifact.
    let r = c.post_empty(&api, &format!("{THEMES}/blog/activate")).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    let dto = r.json();
    assert_eq!(dto["origin"], "installed", "{dto}");
    assert_blog_facts(&dto);
    let row = api
        .store
        .read()
        .themes()
        .get("blog")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(dto["hash"], row.artifact_hash.as_str(), "{dto}");
    let list = c.get(&api, THEMES).await.json();
    assert_eq!(theme(&list, "blog"), &dto);

    // A theme that declares none of them.
    let r = c
        .raw(&api, THEMES, "application/x-tar", blog_min_tar())
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{r:?}");
    let min = r.json();
    for key in ["author", "license", "homepage"] {
        assert_eq!(min[key], Value::Null, "{key}: {min}");
    }
    assert_eq!(min["has_settings"], false, "{min}");
    assert_eq!(min["menus"], json!([]), "{min}");
    assert_eq!(min["images"], json!([]), "{min}");
    api
}

/// The `hello` manifest with the optional `[plugin]` keys set.
fn described_manifest() -> String {
    plugin_manifest("hello").replace(
        "[capabilities]",
        "description = \"Says <b>hello</b>\"\nlicense = \"MIT\"\n\
         homepage = \"https://example.dev/hello\"\n\n[capabilities]",
    )
}

fn assert_plugin_facts(v: &Value) {
    assert_eq!(v["description"], "Says <b>hello</b>", "{v}");
    assert_eq!(v["publisher"], "ferritecms.dev", "{v}");
    assert_eq!(v["license"], "MIT", "{v}");
    assert_eq!(v["homepage"], "https://example.dev/hello", "{v}");
}

/// `PluginDto` (from the stored manifest) and `PermissionReportDto` (from the upload)
/// carry description, publisher, license and homepage; unset ones are `null`.
async fn plugin_dto_has_manifest_facts(api: Api) -> Api {
    let mut c = admin(&api).await;
    let wasm = plugin_wasm("hello");

    // Unset optional keys are null; the publisher is required.
    let plain = plugin_manifest("hello");
    let report = inspect(&mut c, &api, &plain, &wasm).await.json();
    assert_eq!(report["publisher"], "ferritecms.dev", "{report}");
    for key in ["description", "license", "homepage"] {
        assert_eq!(report[key], Value::Null, "{key}: {report}");
    }

    let manifest = described_manifest();
    let report = inspect(&mut c, &api, &manifest, &wasm).await.json();
    assert_plugin_facts(&report);

    // The refusal's report is the same DTO.
    let p = install(&mut c, &api, &manifest, &wasm, &"0".repeat(64))
        .await
        .assert_problem(422, "validation");
    assert_plugin_facts(&p["report"]);

    let fp = report["fingerprint"].as_str().unwrap();
    let r = install(&mut c, &api, &manifest, &wasm, fp).await;
    assert_eq!(r.status, StatusCode::CREATED, "{r:?}");
    assert_plugin_facts(&r.json());
    assert_plugin_facts(&c.get(&api, &format!("{PLUGINS}/hello")).await.json());
    let list = c.get(&api, PLUGINS).await.json();
    assert_plugin_facts(&list["items"][0]);
    api
}

async fn inspect(c: &mut Client, api: &Api, manifest: &str, wasm: &[u8]) -> Resp {
    c.multipart(
        api,
        "/api/v1/plugins/inspect",
        &[("manifest", manifest.as_bytes()), ("component", wasm)],
    )
    .await
}

async fn install(c: &mut Client, api: &Api, manifest: &str, wasm: &[u8], approve: &str) -> Resp {
    c.multipart(
        api,
        PLUGINS,
        &[
            ("manifest", manifest.as_bytes()),
            ("component", wasm),
            ("approve", approve.as_bytes()),
        ],
    )
    .await
}

async fn install_hello(c: &mut Client, api: &Api, manifest: &str) {
    let wasm = plugin_wasm("hello");
    let fp = inspect(c, api, manifest, &wasm).await.json()["fingerprint"]
        .as_str()
        .unwrap()
        .to_owned();
    let r = install(c, api, manifest, &wasm, &fp).await;
    assert_eq!(r.status, StatusCode::CREATED, "{r:?}");
}

fn at(v: &Value) -> DateTime<Utc> {
    v.as_str().unwrap().parse().unwrap()
}

/// P47: a plugin deleted without purge is listed with its purge date until
/// `POST /plugins/{name}/purge` erases its KV and settings (audited `plugin.purge`);
/// nothing retained is `404`, an installed plugin of that name `422 name`.
async fn retained_list_and_purge(api: Api) -> Api {
    let mut c = admin(&api).await;
    let manifest = format!(
        "{}\n[settings.greeting]\ntype = \"text\"\nlabel = \"Greeting\"\n",
        plugin_manifest("hello")
    );
    install_hello(&mut c, &api, &manifest).await;
    let kv = api.store.read().plugin_kv();
    let options = api.store.read().options();
    let setting = "plugin.hello.settings.greeting";
    kv.set("hello", "k", b"v").await.unwrap();
    options.set(setting, &json!("hi"), false).await.unwrap();

    let r = c.get(&api, RETAINED).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert_eq!(r.json(), json!({"items": []}));

    // Installed: nothing to purge yet.
    let p = c
        .post_empty(&api, &format!("{PLUGINS}/hello/purge"))
        .await
        .assert_problem(422, "validation");
    assert_eq!(p["errors"][0]["field"], "name", "{p}");

    let r = c
        .send(
            &api,
            Method::DELETE,
            &format!("{PLUGINS}/hello"),
            None,
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT, "{r:?}");
    let list = c.get(&api, RETAINED).await.json();
    let items = list["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "{list}");
    assert_eq!(items[0]["name"], "hello", "{list}");
    let (deleted, purge) = (at(&items[0]["deleted_at"]), at(&items[0]["purge_after"]));
    assert_eq!(purge - deleted, chrono::Duration::days(30), "{list}");
    assert!((Utc::now() - deleted).num_seconds().abs() < 60, "{list}");
    assert_eq!(
        kv.get("hello", "k").await.unwrap().as_deref(),
        Some(&b"v"[..])
    );

    // An editor reads no plugin data.
    api.user("ed", Role::Editor, false).await;
    let mut ed = api.logged_in("ed").await;
    ed.get(&api, RETAINED)
        .await
        .assert_problem(403, "forbidden");
    ed.post_empty(&api, &format!("{PLUGINS}/hello/purge"))
        .await
        .assert_problem(403, "forbidden");

    let r = c.post_empty(&api, &format!("{PLUGINS}/hello/purge")).await;
    assert_eq!(r.status, StatusCode::NO_CONTENT, "{r:?}");
    assert_eq!(kv.get("hello", "k").await.unwrap(), None);
    assert_eq!(options.get(setting).await.unwrap(), None);
    assert_eq!(
        options.get("plugin.hello.deleted_at").await.unwrap(),
        None,
        "the marker goes too"
    );
    assert_eq!(c.get(&api, RETAINED).await.json(), json!({"items": []}));
    let audit = api.store.read().audit().list(10, None).await.unwrap().items;
    let entry = audit
        .iter()
        .find(|a| a.action == "plugin.purge")
        .expect("plugin.purge audited");
    assert_eq!(entry.target_kind, "plugin");
    assert_eq!(entry.target_id.as_deref(), Some("hello"));
    assert_eq!(entry.details, json!({"name": "hello"}));

    // Nothing retained any more, and never for an unknown name.
    for name in ["hello", "nope", "a.settings"] {
        c.post_empty(&api, &format!("{PLUGINS}/{name}/purge"))
            .await
            .assert_problem(404, "not-found");
    }

    // A reinstall within the retention takes the data back: nothing is listed.
    install_hello(&mut c, &api, &manifest).await;
    kv.set("hello", "k2", b"v").await.unwrap();
    c.send(
        &api,
        Method::DELETE,
        &format!("{PLUGINS}/hello"),
        None,
        None,
    )
    .await;
    install_hello(&mut c, &api, &manifest).await;
    assert_eq!(c.get(&api, RETAINED).await.json(), json!({"items": []}));
    assert_eq!(
        kv.get("hello", "k2").await.unwrap().as_deref(),
        Some(&b"v"[..])
    );
    api
}

/// The committed `blog-min.tar` (written by `ferrite theme pack`) installs as it is,
/// stored as the very bytes uploaded.
async fn blog_min_fixture_installs(api: Api) -> Api {
    let mut c = admin(&api).await;
    let tar = blog_min_tar();
    let r = c.raw(&api, THEMES, "application/x-tar", tar.clone()).await;
    assert_eq!(r.status, StatusCode::CREATED, "{r:?}");
    let dto = r.json();
    assert_eq!(dto["name"], "blog-min", "{dto}");
    assert_eq!(dto["hash"], sha256(&tar), "{dto}");
    api
}
