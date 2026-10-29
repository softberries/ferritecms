//! `/plugins`: inspection, install with an approval fingerprint, enable/disable and
//! delete (M2a-2 Task 4; P3, P4, P9, P10, P11; Review Focus 1).

use std::sync::atomic::Ordering;

use axum::{
    body::Body,
    http::{Method, StatusCode, header},
};
use ferrite_domain::{ArtifactHash, Event, NewNode, NodeId, Role};
use ferrite_http::extensions::UPLOAD_SLOTS;
use ferrite_store::{LOCK_EXTENSIONS, lock};
use serde_json::{Value, json};

use crate::{
    api_tests,
    harness::{
        Api, Client, EXTENSIONS_LOCK_WAIT, Resp, multipart_body, plugin_manifest, plugin_wasm,
    },
};

api_tests!(
    plugin_install_flow,
    install_needs_matching_approval,
    widening_update_without_approval_keeps_old_version,
    subscribing_to_request_served_needs_approval,
    editor_cannot_manage_plugins,
    multipart_limits,
    delete_with_purge,
    plugin_list_shows_node_state,
    broken_components_are_refused,
    reserved_name_is_refused,
    uploads_are_bounded_per_node,
    failed_install_leaves_blob_for_gc,
);

const PLUGINS: &str = "/api/v1/plugins";
const INSPECT: &str = "/api/v1/plugins/inspect";

async fn admin(api: &Api) -> Client {
    api.user("admin", Role::Admin, false).await;
    api.logged_in("admin").await
}

async fn inspect(c: &mut Client, api: &Api, manifest: &str, wasm: &[u8]) -> Resp {
    c.multipart(
        api,
        INSPECT,
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

async fn max_seq(api: &Api) -> i64 {
    api.store.read().events().max_seq().await.unwrap()
}

async fn kinds_after(api: &Api, seq: i64) -> Vec<(String, Vec<String>)> {
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

/// No plugin row, and no artifact row or blob for `artifact`.
async fn nothing_stored(api: &Api, artifact: &str) {
    assert!(api.store.read().plugins().list().await.unwrap().is_empty());
    let hash = ferrite_domain::ArtifactHash::parse(artifact).unwrap();
    assert_eq!(api.store.read().artifacts().get(&hash).await.unwrap(), None);
    assert_eq!(
        api.api.media.head(&hash).await.unwrap(),
        None,
        "blob stored"
    );
}

fn hello_meta(body: &str) -> bool {
    body.contains("<meta name=\"generator\" content=\"ferrite-hello\">")
}

/// The fixed report shape; returns the report.
fn assert_report(r: &Resp, version: &str, update: bool) -> Value {
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    let report = r.json();
    assert_eq!(report["plugin"], "hello", "{report}");
    assert_eq!(report["version"], version, "{report}");
    assert_eq!(report["artifact"].as_str().unwrap().len(), 64, "{report}");
    assert_eq!(
        report["fingerprint"].as_str().unwrap().len(),
        64,
        "{report}"
    );
    assert_eq!(report["update"], update, "{report}");
    let items = report["items"].as_array().unwrap();
    assert!(
        items.iter().any(|i| i["key"] == "render.head"
            && i["text"].is_string()
            && i["risk"] == "high"
            && i["high_risk"] == true),
        "render.head is high risk (R22): {report}"
    );
    assert!(report["limits"].is_array(), "{report}");
    report
}

async fn plugin_install_flow(api: Api) -> Api {
    let mut c = admin(&api).await;
    let (manifest, wasm) = (plugin_manifest("hello"), plugin_wasm("hello"));

    // Inspect: the report, nothing installed.
    let r = inspect(&mut c, &api, &manifest, &wasm).await;
    let report = assert_report(&r, "0.1.0", false);
    assert_eq!(report["added"], report["items"], "new: everything is added");
    assert!(!r.body.contains("\\u0000asm"), "no artifact bytes");
    let fingerprint = report["fingerprint"].as_str().unwrap().to_owned();
    nothing_stored(&api, report["artifact"].as_str().unwrap()).await;
    let r = c.get(&api, PLUGINS).await;
    assert_eq!(r.json(), json!({"items": [], "next_cursor": null}), "{r:?}");

    // Install with the fingerprint: 201, disabled, the stored grant's report.
    let r = install(&mut c, &api, &manifest, &wasm, &fingerprint).await;
    assert_eq!(r.status, StatusCode::CREATED, "{r:?}");
    assert_eq!(r.header("location"), Some("/api/v1/plugins/hello"));
    let dto = r.json();
    assert_eq!(dto["name"], "hello");
    assert_eq!(dto["version"], "0.1.0");
    assert_eq!(dto["enabled"], false);
    assert!(dto["disabled_reason"].is_null());
    assert!(dto["installed_at"].is_string() && dto["updated_at"].is_string());
    assert_eq!(dto["report"]["fingerprint"], fingerprint.as_str());
    assert_eq!(dto["report"]["added"], json!([]), "the grant covers it");
    assert_eq!(dto["nodes"], json!([]));
    assert_eq!(c.get(&api, "/api/v1/plugins/hello").await.json(), dto);
    let list = c.get(&api, PLUGINS).await.json();
    assert_eq!(list["items"], json!([dto]));
    assert!(list["next_cursor"].is_null());

    // Enable: `plugins.changed`, and after the reload that event triggers on a node
    // (F18) the hook runs on `/`.
    let r = api.get("/").await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert!(!hello_meta(&r.body));
    let mut poller = api.poller().await;
    let seq = max_seq(&api).await;
    let r = c.post_empty(&api, "/api/v1/plugins/hello/enable").await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert_eq!(r.json()["enabled"], true);
    assert_eq!(
        kinds_after(&api, seq).await,
        [("plugins.changed".to_owned(), vec!["hello".to_owned()])]
    );
    api.reloader.reload().await.unwrap();
    poller.poll_once().await.unwrap();
    let r = api.get("/").await;
    assert!(hello_meta(&r.body), "{}", r.body);

    // Disable: `plugins.changed`.
    let seq = max_seq(&api).await;
    let r = c.post_empty(&api, "/api/v1/plugins/hello/disable").await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert_eq!(r.json()["enabled"], false);
    assert_eq!(
        kinds_after(&api, seq).await,
        [("plugins.changed".to_owned(), vec!["hello".to_owned()])]
    );
    api.reloader.reload().await.unwrap();
    poller.poll_once().await.unwrap();
    assert!(!hello_meta(&api.get("/").await.body));

    // Delete: 204, `plugins.changed`, the row is gone; again: 404.
    let seq = max_seq(&api).await;
    let r = c
        .send(&api, Method::DELETE, "/api/v1/plugins/hello", None, None)
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT, "{r:?}");
    assert!(
        kinds_after(&api, seq)
            .await
            .contains(&("plugins.changed".to_owned(), vec!["hello".to_owned()])),
        "{:?}",
        kinds_after(&api, seq).await
    );
    assert_eq!(api.store.read().plugins().get("hello").await.unwrap(), None);
    c.get(&api, "/api/v1/plugins/hello")
        .await
        .assert_problem(404, "not-found");
    c.send(&api, Method::DELETE, "/api/v1/plugins/hello", None, None)
        .await
        .assert_problem(404, "not-found");
    c.post_empty(&api, "/api/v1/plugins/hello/enable")
        .await
        .assert_problem(404, "not-found");
    api
}

/// Review Focus 1: no approval, or the fingerprint of another upload, installs nothing.
async fn install_needs_matching_approval(api: Api) -> Api {
    let mut c = admin(&api).await;
    let (manifest, wasm) = (plugin_manifest("hello"), plugin_wasm("hello"));
    let artifact = inspect(&mut c, &api, &manifest, &wasm).await.json()["artifact"]
        .as_str()
        .unwrap()
        .to_owned();

    // No `approve` part.
    let r = c
        .multipart(
            &api,
            PLUGINS,
            &[("manifest", manifest.as_bytes()), ("component", &wasm)],
        )
        .await;
    let p = r.assert_problem(422, "validation");
    assert_eq!(p["errors"][0]["field"], "approve", "{p}");
    nothing_stored(&api, &artifact).await;

    // The fingerprint of another upload (another plugin) does not approve this one.
    let clock = plugin_wasm("clock");
    let other = inspect(&mut c, &api, &plugin_manifest("clock"), &clock).await;
    let other = other.json()["fingerprint"].as_str().unwrap().to_owned();
    for wrong in [other.as_str(), "", "0000"] {
        let r = install(&mut c, &api, &manifest, &wasm, wrong).await;
        let p = r.assert_problem(422, "validation");
        assert_eq!(p["errors"][0]["field"], "approve", "{p}");
        // The refusal carries this upload's report, so the admin can approve it.
        assert_eq!(p["report"]["artifact"], artifact.as_str(), "{p}");
        nothing_stored(&api, &artifact).await;
    }

    // `hello`'s fingerprint with another component under `hello`'s manifest: the
    // fingerprint binds the artifact, so nothing is stored.
    let fingerprint = inspect(&mut c, &api, &manifest, &wasm).await.json()["fingerprint"]
        .as_str()
        .unwrap()
        .to_owned();
    let r = install(&mut c, &api, &manifest, &clock, &fingerprint).await;
    let p = r.assert_problem(422, "validation");
    assert_eq!(p["errors"][0]["field"], "approve", "{p}");
    assert_ne!(p["report"]["fingerprint"], fingerprint.as_str(), "{p}");
    nothing_stored(&api, p["report"]["artifact"].as_str().unwrap()).await;
    nothing_stored(&api, &artifact).await;
    api
}

/// Review Focus 1 / R1: a widening update approved with the old fingerprint is refused,
/// nothing changes and the old version keeps running; its own fingerprint installs it.
async fn widening_update_without_approval_keeps_old_version(api: Api) -> Api {
    let mut c = admin(&api).await;
    let wasm = plugin_wasm("hello");
    let (v1, v2) = (plugin_manifest("hello"), plugin_manifest("hello-v2"));
    let fp1 = inspect(&mut c, &api, &v1, &wasm).await.json()["fingerprint"]
        .as_str()
        .unwrap()
        .to_owned();
    let r = install(&mut c, &api, &v1, &wasm, &fp1).await;
    assert_eq!(r.status, StatusCode::CREATED, "{r:?}");
    let r = c.post_empty(&api, "/api/v1/plugins/hello/enable").await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    api.reloader.reload().await.unwrap();
    let before = api
        .store
        .read()
        .plugins()
        .get("hello")
        .await
        .unwrap()
        .unwrap();

    // v2 asks for `kv` on top: the report of the update says so.
    let r = inspect(&mut c, &api, &v2, &wasm).await;
    let report = assert_report(&r, "0.2.0", true);
    let added: Vec<&str> = report["added"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["key"].as_str().unwrap())
        .collect();
    assert_eq!(added, ["kv"], "{report}");
    let fp2 = report["fingerprint"].as_str().unwrap().to_owned();
    assert_ne!(fp1, fp2);

    // The v1 fingerprint does not approve v2: 422, the row unchanged, v1 still runs.
    let seq = max_seq(&api).await;
    let r = install(&mut c, &api, &v2, &wasm, &fp1).await;
    let p = r.assert_problem(422, "validation");
    assert_eq!(p["errors"][0]["field"], "approve", "{p}");
    assert_eq!(p["report"]["fingerprint"], fp2.as_str(), "{p}");
    assert_eq!(p["report"]["added"][0]["key"], "kv", "{p}");
    let after = api
        .store
        .read()
        .plugins()
        .get("hello")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after, before);
    assert_eq!(after.version, "0.1.0");
    assert!(
        after.granted["capabilities"].get("kv").is_none(),
        "{}",
        after.granted
    );
    assert_eq!(kinds_after(&api, seq).await, []);
    assert!(hello_meta(&api.get("/").await.body), "v1 still runs");

    // v2's own fingerprint installs it; it keeps the enabled state; the new grant has kv.
    let r = install(&mut c, &api, &v2, &wasm, &fp2).await;
    assert_eq!(r.status, StatusCode::CREATED, "{r:?}");
    let dto = r.json();
    assert_eq!(
        (&dto["version"], &dto["enabled"]),
        (&json!("0.2.0"), &json!(true))
    );
    assert_eq!(dto["report"]["fingerprint"], fp2.as_str());
    assert_eq!(dto["report"]["added"], json!([]));
    let row = api
        .store
        .read()
        .plugins()
        .get("hello")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.granted["capabilities"]["kv"], true, "{}", row.granted);
    api
}

/// Ruling R23: subscribing to `request.served` (site-wide traffic metadata) is part of
/// the grant. The same name, version and component with that one hook added has its own
/// fingerprint, shows the hook in `added`, and the earlier approval does not install it.
async fn subscribing_to_request_served_needs_approval(api: Api) -> Api {
    let mut c = admin(&api).await;
    let wasm = plugin_wasm("hello");
    let v1 = plugin_manifest("hello");
    let served = v1.replace("[hooks]\n", "[hooks]\n\"request.served\" = 5\n");
    assert_ne!(served, v1);
    let fp1 = inspect(&mut c, &api, &v1, &wasm).await.json()["fingerprint"]
        .as_str()
        .unwrap()
        .to_owned();
    let r = install(&mut c, &api, &v1, &wasm, &fp1).await;
    assert_eq!(r.status, StatusCode::CREATED, "{r:?}");
    let before = api
        .store
        .read()
        .plugins()
        .get("hello")
        .await
        .unwrap()
        .unwrap();
    assert!(before.granted.get("hooks").is_none(), "{}", before.granted);

    let r = inspect(&mut c, &api, &served, &wasm).await;
    let report = assert_report(&r, "0.1.0", true);
    assert_eq!(
        report["added"],
        json!([{
            "key": "hooks",
            "text": report["added"][0]["text"],
            "detail": ["request.served"],
            "risk": "medium",
            "high_risk": false,
        }]),
        "{report}"
    );
    let fp2 = report["fingerprint"].as_str().unwrap().to_owned();
    assert_ne!(fp1, fp2, "the subscription is in the fingerprint");

    // The earlier approval does not cover it (R1): 422, nothing changes.
    let r = install(&mut c, &api, &served, &wasm, &fp1).await;
    let p = r.assert_problem(422, "validation");
    assert_eq!(p["errors"][0]["field"], "approve", "{p}");
    let after = api
        .store
        .read()
        .plugins()
        .get("hello")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after, before);

    // Its own fingerprint installs it, and the stored grant records the subscription.
    let r = install(&mut c, &api, &served, &wasm, &fp2).await;
    assert_eq!(r.status, StatusCode::CREATED, "{r:?}");
    assert_eq!(r.json()["report"]["added"], json!([]));
    let row = api
        .store
        .read()
        .plugins()
        .get("hello")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        row.granted["hooks"],
        json!(["request.served"]),
        "{}",
        row.granted
    );
    api
}

/// P10: an editor sees no plugin list and cannot upload; the refusal comes before the
/// upload is compiled (or even read).
async fn editor_cannot_manage_plugins(api: Api) -> Api {
    api.user("ed", Role::Editor, false).await;
    let mut c = api.logged_in("ed").await;
    let (manifest, wasm) = (plugin_manifest("hello"), plugin_wasm("hello"));
    let before = api.compiles.load(Ordering::SeqCst);
    for r in [
        inspect(&mut c, &api, &manifest, &wasm).await,
        install(&mut c, &api, &manifest, &wasm, "x").await,
    ] {
        let p = r.assert_problem(403, "forbidden");
        assert!(
            p["detail"].as_str().unwrap().contains("manage_plugins"),
            "{p}"
        );
    }
    assert_eq!(
        api.compiles.load(Ordering::SeqCst),
        before,
        "nothing compiled"
    );
    // Not even the content type is looked at first.
    let r = c.post(&api, INSPECT, json!({})).await;
    r.assert_problem(403, "forbidden");
    for path in [PLUGINS, "/api/v1/plugins/hello"] {
        c.get(&api, path).await.assert_problem(403, "forbidden");
    }
    for path in [
        "/api/v1/plugins/hello/enable",
        "/api/v1/plugins/hello/disable",
    ] {
        c.post_empty(&api, path)
            .await
            .assert_problem(403, "forbidden");
    }
    c.send(&api, Method::DELETE, "/api/v1/plugins/hello", None, None)
        .await
        .assert_problem(403, "forbidden");
    // Authorized before the query is read.
    c.send(
        &api,
        Method::DELETE,
        "/api/v1/plugins/hello?purge=yes",
        None,
        None,
    )
    .await
    .assert_problem(403, "forbidden");

    // A bearer token of an admin without `extensions:write` cannot install either.
    let admin = api.user("admin", Role::Admin, false).await;
    let token = api.bearer(&admin, &["content:write"]).await;
    let (ct, body) = multipart_body(&[("manifest", manifest.as_bytes()), ("component", &wasm)]);
    let req = token
        .builder(Method::POST, INSPECT)
        .header(header::CONTENT_TYPE, ct)
        .body(Body::from(body))
        .unwrap();
    api.send(req).await.assert_problem(403, "forbidden");
    assert_eq!(
        api.compiles.load(Ordering::SeqCst),
        before,
        "nothing compiled"
    );
    api
}

/// P11: the per-route body limit (33 MiB) and the per-part limits; unknown, repeated
/// and missing parts; the media type.
async fn multipart_limits(api: Api) -> Api {
    let mut c = admin(&api).await;
    let manifest = plugin_manifest("hello");
    let before = api.compiles.load(Ordering::SeqCst);

    // A 33 MiB component: over the part limit and the route's body limit.
    let big = vec![0u8; 33 * 1024 * 1024];
    let r = inspect(&mut c, &api, &manifest, &big).await;
    r.assert_problem(413, "too-large");
    // Just over the component's 32 MiB (within the route limit): the part limit.
    let over = vec![0u8; 32 * 1024 * 1024 + 1];
    let p = inspect(&mut c, &api, &manifest, &over)
        .await
        .assert_problem(413, "too-large");
    assert!(p["detail"].as_str().unwrap().contains("component"), "{p}");
    // A manifest over 64 KiB.
    let long = format!("{manifest}\n# {}\n", "x".repeat(64 * 1024));
    let p = inspect(&mut c, &api, &long, b"x")
        .await
        .assert_problem(413, "too-large");
    assert!(p["detail"].as_str().unwrap().contains("manifest"), "{p}");
    // A declared length over the route limit is refused before any of it is read.
    let (ct, body) = multipart_body(&[("manifest", manifest.as_bytes())]);
    let req = c
        .builder(Method::POST, INSPECT)
        .header(header::CONTENT_TYPE, ct)
        .header(header::CONTENT_LENGTH, 40 * 1024 * 1024)
        .body(Body::from(body))
        .unwrap();
    c.send_on(&api.router, req)
        .await
        .assert_problem(413, "too-large");

    // An unknown part, a repeated part.
    let p = c
        .multipart(
            &api,
            INSPECT,
            &[("manifest", manifest.as_bytes()), ("extra", b"x")],
        )
        .await
        .assert_problem(400, "malformed-body");
    assert!(p["detail"].as_str().unwrap().contains("extra"), "{p}");
    let p = c
        .multipart(
            &api,
            INSPECT,
            &[("manifest", manifest.as_bytes()), ("manifest", b"x")],
        )
        .await
        .assert_problem(400, "malformed-body");
    assert!(p["detail"].as_str().unwrap().contains("manifest"), "{p}");
    // `approve` is not a part of an inspection.
    c.multipart(
        &api,
        INSPECT,
        &[
            ("manifest", manifest.as_bytes()),
            ("component", b"x"),
            ("approve", b"x"),
        ],
    )
    .await
    .assert_problem(400, "malformed-body");
    // A missing part names it.
    let p = c
        .multipart(&api, INSPECT, &[("manifest", manifest.as_bytes())])
        .await
        .assert_problem(422, "validation");
    assert_eq!(p["errors"][0]["field"], "component", "{p}");
    // Not multipart; a broken body.
    c.post(&api, INSPECT, json!({}))
        .await
        .assert_problem(415, "unsupported-media-type");
    let req = c
        .builder(Method::POST, INSPECT)
        .header(
            header::CONTENT_TYPE,
            "multipart/form-data; boundary=nope-not-here",
        )
        .body(Body::from("garbage"))
        .unwrap();
    c.send_on(&api.router, req)
        .await
        .assert_problem(400, "malformed-body");
    // A manifest that is not UTF-8.
    let p = c
        .multipart(
            &api,
            INSPECT,
            &[("manifest", b"\xff\xfe"), ("component", b"x")],
        )
        .await
        .assert_problem(422, "validation");
    assert_eq!(p["errors"][0]["field"], "manifest", "{p}");

    assert_eq!(
        api.compiles.load(Ordering::SeqCst),
        before,
        "nothing compiled"
    );
    assert!(api.store.read().plugins().list().await.unwrap().is_empty());
    api
}

/// P9 / R2: `?purge=true` deletes the KV and settings at once; without it they stay.
async fn delete_with_purge(api: Api) -> Api {
    let mut c = admin(&api).await;
    let wasm = plugin_wasm("hello");
    let manifest = format!(
        "{}\n[settings.greeting]\ntype = \"text\"\nlabel = \"Greeting\"\n",
        plugin_manifest("hello")
    );
    let kv = api.store.read().plugin_kv();
    let options = api.store.read().options();
    let setting = "plugin.hello.settings.greeting";
    for purge in [false, true] {
        let fp = inspect(&mut c, &api, &manifest, &wasm).await.json()["fingerprint"]
            .as_str()
            .unwrap()
            .to_owned();
        let r = install(&mut c, &api, &manifest, &wasm, &fp).await;
        assert_eq!(r.status, StatusCode::CREATED, "{r:?}");
        kv.set("hello", "k", b"v").await.unwrap();
        options.set(setting, &json!("hi"), false).await.unwrap();

        let path = format!("/api/v1/plugins/hello?purge={purge}");
        let r = c.send(&api, Method::DELETE, &path, None, None).await;
        assert_eq!(r.status, StatusCode::NO_CONTENT, "{r:?}");
        assert_eq!(api.store.read().plugins().get("hello").await.unwrap(), None);
        let kept = kv.get("hello", "k").await.unwrap();
        let setting_kept = options.get(setting).await.unwrap().map(|o| o.value);
        if purge {
            assert_eq!(kept, None);
            assert_eq!(setting_kept, None);
        } else {
            assert_eq!(kept.as_deref(), Some(&b"v"[..]));
            assert_eq!(setting_kept, Some(json!("hi")));
        }
    }
    // A bad `purge` value.
    c.send(
        &api,
        Method::DELETE,
        "/api/v1/plugins/hello?purge=yes",
        None,
        None,
    )
    .await
    .assert_problem(400, "malformed-query");
    api
}

/// `nodes` lists what live nodes report for the plugin (`nodes.meta.plugins`).
async fn plugin_list_shows_node_state(api: Api) -> Api {
    let mut c = admin(&api).await;
    let (manifest, wasm) = (plugin_manifest("hello"), plugin_wasm("hello"));
    let fp = inspect(&mut c, &api, &manifest, &wasm).await.json()["fingerprint"]
        .as_str()
        .unwrap()
        .to_owned();
    install(&mut c, &api, &manifest, &wasm, &fp).await;
    let id = NodeId::new();
    api.store
        .read()
        .nodes()
        .register(&NewNode {
            id,
            name: "web-1".into(),
            version: "0.0.0".into(),
            schema_version: 1,
            meta: json!({
                "plugins": {
                    "hello": {
                        "version": "0.1.0",
                        "hash": "ab",
                        "status": "failed",
                        "error": "link error"
                    }
                }
            }),
        })
        .await
        .unwrap();
    let dto = c.get(&api, "/api/v1/plugins/hello").await.json();
    assert_eq!(
        dto["nodes"],
        json!([{
            "node": id.as_uuid().to_string(),
            "name": "web-1",
            "version": "0.1.0",
            "status": "failed",
            "error": "link error"
        }]),
        "{dto}"
    );
    api
}

fn sha256(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// A component that is not wasm, or that imports what the host does not provide, is
/// `422` with `field = component` at inspection and at install; nothing is stored.
async fn broken_components_are_refused(api: Api) -> Api {
    let mut c = admin(&api).await;
    let hostile = plugin_wasm("undeclared-import");
    let cases: [(String, Vec<u8>); 2] = [
        (plugin_manifest("hello"), b"not wasm at all".to_vec()),
        (plugin_manifest("undeclared-import"), hostile),
    ];
    for (manifest, component) in cases {
        for r in [
            inspect(&mut c, &api, &manifest, &component).await,
            install(&mut c, &api, &manifest, &component, &"0".repeat(64)).await,
        ] {
            let p = r.assert_problem(422, "validation");
            assert_eq!(p["errors"][0]["field"], "component", "{p}");
            assert!(p.get("report").is_none(), "{p}");
        }
        nothing_stored(&api, &sha256(&component)).await;
    }
    api
}

/// R9: `inspect` is a static path under `/plugins/`, so no plugin may be named so.
async fn reserved_name_is_refused(api: Api) -> Api {
    let mut c = admin(&api).await;
    let manifest = plugin_manifest("hello").replace("\"hello\"", "\"inspect\"");
    let wasm = plugin_wasm("hello");
    let p = inspect(&mut c, &api, &manifest, &wasm)
        .await
        .assert_problem(422, "validation");
    assert_eq!(p["errors"][0]["field"], "plugin.name", "{p}");
    let p = install(&mut c, &api, &manifest, &wasm, &"0".repeat(64))
        .await
        .assert_problem(422, "validation");
    assert_eq!(p["errors"][0]["field"], "plugin.name", "{p}");
    nothing_stored(&api, &sha256(&wasm)).await;
    api
}

/// At most `UPLOAD_SLOTS` uploads at once per node: one more is `503 busy` before its
/// body is read.
async fn uploads_are_bounded_per_node(api: Api) -> Api {
    let mut c = admin(&api).await;
    let (manifest, wasm) = (plugin_manifest("hello"), plugin_wasm("hello"));
    let held: Vec<_> = (0..UPLOAD_SLOTS)
        .map(|_| api.api.installer.try_upload_slot().expect("free slot"))
        .collect();
    let before = api.compiles.load(Ordering::SeqCst);
    for r in [
        inspect(&mut c, &api, &manifest, &wasm).await,
        install(&mut c, &api, &manifest, &wasm, "x").await,
    ] {
        r.assert_problem(503, "busy");
        assert_eq!(r.header("retry-after"), Some("5"), "{r:?}");
    }
    assert_eq!(
        api.compiles.load(Ordering::SeqCst),
        before,
        "nothing compiled"
    );
    drop(held);
    let r = inspect(&mut c, &api, &manifest, &wasm).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    api
}

/// A staged component whose rows are not recorded stays in the store, with no row, for
/// the artifacts GC (M4-1 P12: nothing deletes it in the request, so a concurrent install
/// of the same bytes never loses its blob).
async fn failed_install_leaves_blob_for_gc(api: Api) -> Api {
    let mut c = admin(&api).await;
    let (manifest, wasm) = (plugin_manifest("hello"), plugin_wasm("hello"));
    let fingerprint = inspect(&mut c, &api, &manifest, &wasm).await.json()["fingerprint"]
        .as_str()
        .unwrap()
        .to_owned();
    let hash = ArtifactHash::parse(&sha256(&wasm)).unwrap();

    // The `extensions` lock is held while the install records: `busy`.
    let guard = lock(api.store.as_ref(), LOCK_EXTENSIONS, EXTENSIONS_LOCK_WAIT)
        .await
        .unwrap();
    let r = install(&mut c, &api, &manifest, &wasm, &fingerprint).await;
    r.assert_problem(503, "busy");
    guard.release().await.unwrap();
    assert!(api.store.read().plugins().list().await.unwrap().is_empty());
    assert_eq!(api.store.read().artifacts().get(&hash).await.unwrap(), None);
    assert_eq!(
        api.api.media.head(&hash).await.unwrap(),
        Some(wasm.len() as u64),
        "left for the GC"
    );
    api
}
