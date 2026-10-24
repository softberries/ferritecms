//! `/themes`: list, raw-tar upload and activation that refuses a theme that does not load
//! (M2a-2 Task 5; P11, P12, P30; Review Focus 5; pre-flight F11, F12, F18, F20, F21).

use std::{path::PathBuf, sync::Arc};

use axum::{
    body::Body,
    http::{Method, StatusCode, header},
};
use ferrite_domain::{ArtifactHash, Event, NewNode, NodeId, Role};
use ferrite_http::extensions::UPLOAD_SLOTS;
use ferrite_store::{LOCK_EXTENSIONS, lock};
use ferrite_theme::{MAX_BUNDLE_BYTES, ThemeSource, pack};
use serde_json::{Value, json};

use crate::{
    api_tests,
    harness::{Api, Client, EXTENSIONS_LOCK_WAIT, Resp},
};

api_tests!(
    theme_upload_and_activate,
    activation_refuses_broken_theme,
    embedded_theme_activates_without_a_row,
    broken_upload_lists_template_errors,
    theme_upload_limits,
    theme_list_shows_node_state,
    editor_cannot_manage_themes,
    theme_uploads_are_bounded_per_node,
    failed_theme_install_discards_unreferenced_blob,
    activation_race_is_a_conflict,
    hostile_bundles_are_refused,
);

const THEMES: &str = "/api/v1/themes";
const TAR: &str = "application/x-tar";
/// What the harness theme (`tests/fixtures/app-theme`) renders on `/`.
const APP_THEME_MARK: &str = "data-tpl=\"index\"";
/// What `tests/fixtures/upload-theme` renders on `/`.
const UPLOADED_MARK: &str = "UPLOADED-THEME";
/// What the embedded `blog` theme renders on every page.
const BLOG_MARK: &str = "class=\"skip-link\"";

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

/// The packed bundle of the fixture folder `name`.
fn bundle(name: &str) -> Vec<u8> {
    pack(&ThemeSource::Dir(fixture(name))).expect("pack fixture")
}

fn sha256(bytes: &[u8]) -> ArtifactHash {
    use sha2::{Digest, Sha256};
    let hex: String = Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    ArtifactHash::parse(&hex).unwrap()
}

async fn admin(api: &Api) -> Client {
    api.user("admin", Role::Admin, false).await;
    api.logged_in("admin").await
}

async fn upload(c: &mut Client, api: &Api, path: &str, body: Vec<u8>) -> Resp {
    c.raw(api, path, TAR, body).await
}

async fn activate(c: &mut Client, api: &Api, name: &str) -> Resp {
    c.post_empty(api, &format!("{THEMES}/{name}/activate"))
        .await
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

async fn active_row(api: &Api) -> Option<String> {
    api.store
        .read()
        .themes()
        .active()
        .await
        .unwrap()
        .map(|r| r.name)
}

/// The list entry named `name`.
fn entry(list: &Value, name: &str) -> Value {
    let items = list["items"].as_array().unwrap();
    let found: Vec<&Value> = items.iter().filter(|i| i["name"] == name).collect();
    assert_eq!(found.len(), 1, "one {name} entry: {list}");
    found[0].clone()
}

/// Swaps the theme of the harness node to the active row's artifact and applies the
/// outbox, as a node's reload task and invalidation loop do on `theme.changed` (F18).
async fn swap_to_active(api: &Api, poller: &mut ferrite_cache::InvalidationLoop) {
    let row = api.store.read().themes().active().await.unwrap().unwrap();
    let theme = api.api.installer.theme_from_artifact(&row).await.unwrap();
    api.app.swap_theme(Arc::new(theme));
    poller.poll_once().await.unwrap();
}

/// No theme row, and no artifact row or blob for `hash`.
async fn nothing_stored(api: &Api, hash: &ArtifactHash) {
    assert!(api.store.read().themes().list().await.unwrap().is_empty());
    assert_eq!(api.store.read().artifacts().get(hash).await.unwrap(), None);
    assert_eq!(api.api.media.head(hash).await.unwrap(), None, "blob stored");
}

async fn theme_upload_and_activate(api: Api) -> Api {
    let mut c = admin(&api).await;

    // Nothing installed: the embedded `blog` is what nodes serve (F21).
    let list = c.get(&api, THEMES).await;
    assert_eq!(list.status, StatusCode::OK, "{list:?}");
    let list = list.json();
    assert!(list["next_cursor"].is_null(), "{list}");
    let blog = entry(&list, "blog");
    assert_eq!(blog["origin"], "embedded", "{blog}");
    assert_eq!(blog["active"], true, "{blog}");
    assert!(blog["installed_at"].is_null(), "{blog}");
    assert_eq!(blog["version"], "0.1.0", "{blog}");
    assert!(blog["description"].is_string(), "{blog}");

    // Upload: 201, installed and not active; no `theme.changed`.
    let seq = max_seq(&api).await;
    let bytes = bundle("upload-theme");
    let hash = sha256(&bytes);
    let r = upload(&mut c, &api, THEMES, bytes).await;
    assert_eq!(r.status, StatusCode::CREATED, "{r:?}");
    let dto = r.json();
    assert_eq!(dto["name"], "uploaded", "{dto}");
    assert_eq!(dto["version"], "1.2.0", "{dto}");
    assert_eq!(dto["description"], "A theme uploaded in the API tests.");
    assert_eq!(dto["origin"], "installed");
    assert_eq!(dto["active"], false);
    assert!(dto["installed_at"].is_string(), "{dto}");
    assert_eq!(dto["nodes"], json!([]));
    assert_eq!(events_after(&api, seq).await, []);
    let row = api.store.read().themes().get("uploaded").await.unwrap();
    assert_eq!(row.unwrap().artifact_hash, hash);
    assert!(
        api.store
            .read()
            .artifacts()
            .get(&hash)
            .await
            .unwrap()
            .is_some()
    );
    assert_eq!(entry(&c.get(&api, THEMES).await.json(), "uploaded"), dto);

    // Activate: `theme.changed`, and after the swap that event triggers on a node (F18)
    // `/` renders the new theme.
    let r = api.get("/").await;
    assert!(r.body.contains(APP_THEME_MARK), "{}", r.body);
    let mut poller = api.poller().await;
    let seq = max_seq(&api).await;
    let r = activate(&mut c, &api, "uploaded").await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert_eq!(r.json()["active"], true, "{r:?}");
    assert_eq!(r.json()["origin"], "installed", "{r:?}");
    assert_eq!(
        events_after(&api, seq).await,
        [("theme.changed".to_owned(), vec!["uploaded".to_owned()])]
    );
    swap_to_active(&api, &mut poller).await;
    let r = api.get("/").await;
    assert!(r.body.contains(UPLOADED_MARK), "{}", r.body);

    // The list follows: `uploaded` active, the embedded `blog` not.
    let list = c.get(&api, THEMES).await.json();
    assert_eq!(entry(&list, "uploaded")["active"], true, "{list}");
    assert_eq!(entry(&list, "blog")["active"], false, "{list}");

    // Activating the active theme again: 200, no event.
    let seq = max_seq(&api).await;
    let r = activate(&mut c, &api, "uploaded").await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert_eq!(events_after(&api, seq).await, []);
    api
}

/// Review Focus 5: a row whose artifact no longer loads is refused, the active theme
/// stays and keeps serving, and no `theme.changed` is written.
async fn activation_refuses_broken_theme(api: Api) -> Api {
    let mut c = admin(&api).await;
    let mut poller = api.poller().await;

    // The current theme: `blog`, installed on activation and served.
    let r = activate(&mut c, &api, "blog").await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    swap_to_active(&api, &mut poller).await;
    assert!(api.get("/").await.body.contains(BLOG_MARK));

    // A theme whose stored bundle was tampered with: 422, nothing changes.
    let bytes = bundle("upload-theme");
    let hash = sha256(&bytes);
    let r = upload(&mut c, &api, THEMES, bytes.clone()).await;
    assert_eq!(r.status, StatusCode::CREATED, "{r:?}");
    api.tamper(&hash, b"tampered");
    let seq = max_seq(&api).await;
    let p = activate(&mut c, &api, "uploaded")
        .await
        .assert_problem(422, "validation");
    assert!(
        p["detail"].as_str().unwrap().contains("hash mismatch"),
        "{p}"
    );
    assert_eq!(events_after(&api, seq).await, []);
    assert_eq!(active_row(&api).await.as_deref(), Some("blog"));
    poller.poll_once().await.unwrap();
    let r = api.get("/").await;
    assert!(
        r.body.contains(BLOG_MARK) && !r.body.contains(UPLOADED_MARK),
        "{}",
        r.body
    );
    let list = c.get(&api, THEMES).await.json();
    assert_eq!(entry(&list, "blog")["active"], true, "{list}");
    assert_eq!(entry(&list, "uploaded")["active"], false, "{list}");

    // A missing blob: also 422, nothing changes.
    api.api.media.delete(&hash).await.unwrap();
    let seq = max_seq(&api).await;
    activate(&mut c, &api, "uploaded")
        .await
        .assert_problem(422, "validation");
    assert_eq!(events_after(&api, seq).await, []);
    assert_eq!(active_row(&api).await.as_deref(), Some("blog"));

    // Uploading the same bundle again restores the blob; it then activates.
    let r = upload(&mut c, &api, THEMES, bytes).await;
    assert_eq!(r.status, StatusCode::CREATED, "{r:?}");
    let r = activate(&mut c, &api, "uploaded").await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert_eq!(active_row(&api).await.as_deref(), Some("uploaded"));
    api
}

/// P12: an embedded theme without a row is installed on activation; `?activate=true`
/// activates an upload at once; unknown names are 404.
async fn embedded_theme_activates_without_a_row(api: Api) -> Api {
    let mut c = admin(&api).await;
    let seq = max_seq(&api).await;
    let r = upload(
        &mut c,
        &api,
        &format!("{THEMES}?activate=true"),
        bundle("upload-theme"),
    )
    .await;
    assert_eq!(r.status, StatusCode::CREATED, "{r:?}");
    assert_eq!(r.json()["active"], true, "{r:?}");
    assert_eq!(
        events_after(&api, seq).await,
        [("theme.changed".to_owned(), vec!["uploaded".to_owned()])]
    );
    assert_eq!(api.store.read().themes().get("blog").await.unwrap(), None);

    let seq = max_seq(&api).await;
    let r = activate(&mut c, &api, "blog").await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    let dto = r.json();
    assert_eq!(dto["name"], "blog", "{dto}");
    assert_eq!(dto["origin"], "installed", "{dto}");
    assert_eq!(dto["active"], true, "{dto}");
    assert!(dto["installed_at"].is_string(), "{dto}");
    let row = api
        .store
        .read()
        .themes()
        .get("blog")
        .await
        .unwrap()
        .unwrap();
    assert!(row.active);
    let bundle = ferrite_theme::pack_embedded("blog").unwrap();
    assert_eq!(row.artifact_hash, sha256(&bundle), "the embedded files");
    assert_eq!(
        events_after(&api, seq).await,
        [("theme.changed".to_owned(), vec!["blog".to_owned()])]
    );
    // `blog` is listed once, as installed.
    let list = c.get(&api, THEMES).await.json();
    assert_eq!(entry(&list, "blog")["origin"], "installed", "{list}");
    assert_eq!(entry(&list, "uploaded")["active"], false, "{list}");

    // Back to `uploaded` (a row now): no reinstall.
    let r = activate(&mut c, &api, "uploaded").await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert_eq!(active_row(&api).await.as_deref(), Some("uploaded"));

    for name in ["nope", "Blog", "no%20such", "a.b"] {
        activate(&mut c, &api, name)
            .await
            .assert_problem(404, "not-found");
    }
    assert_eq!(active_row(&api).await.as_deref(), Some("uploaded"));
    api
}

/// A bundle whose templates do not compile: one `errors[]` entry per template problem,
/// nothing stored.
async fn broken_upload_lists_template_errors(api: Api) -> Api {
    let mut c = admin(&api).await;
    let bytes = bundle("broken-theme");
    let hash = sha256(&bytes);
    let seq = max_seq(&api).await;
    let p = upload(&mut c, &api, THEMES, bytes)
        .await
        .assert_problem(422, "validation");
    let errors = p["errors"].as_array().unwrap();
    assert_eq!(errors.len(), 2, "{p}");
    for (e, file) in errors
        .iter()
        .zip(["templates/index.html:", "templates/single.html:"])
    {
        let detail = e["detail"].as_str().unwrap();
        assert!(detail.starts_with(file), "{p}");
        assert!(e["field"].is_null(), "{p}");
    }
    nothing_stored(&api, &hash).await;
    assert_eq!(events_after(&api, seq).await, []);

    // A malformed package lists its problems the same way.
    let mut files = tar::Builder::new(Vec::new());
    let mut header = tar::Header::new_ustar();
    header.set_size(3);
    header.set_mode(0o644);
    files
        .append_data(&mut header, "assets/x.css", &b"a{}"[..])
        .unwrap();
    let bytes = files.into_inner().unwrap();
    let p = upload(&mut c, &api, THEMES, bytes.clone())
        .await
        .assert_problem(422, "validation");
    let details: Vec<&str> = p["errors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["detail"].as_str().unwrap())
        .collect();
    assert!(
        details.iter().any(|d| d.contains("theme.toml is missing"))
            && details.iter().any(|d| d.contains("index.html is required")),
        "{p}"
    );
    nothing_stored(&api, &sha256(&bytes)).await;

    // Not a tar at all.
    let p = upload(&mut c, &api, THEMES, b"not a tar".to_vec())
        .await
        .assert_problem(422, "validation");
    assert!(!p["errors"].as_array().unwrap().is_empty(), "{p}");
    nothing_stored(&api, &sha256(b"not a tar")).await;
    api
}

/// P11: a raw `application/x-tar` body of at most `MAX_BUNDLE_BYTES`.
async fn theme_upload_limits(api: Api) -> Api {
    let mut c = admin(&api).await;
    let limit = usize::try_from(MAX_BUNDLE_BYTES).unwrap();

    // Over the limit: 413, before parsing (not 422) and with nothing stored.
    let big = vec![0u8; limit + 1];
    let hash = sha256(&big);
    upload(&mut c, &api, THEMES, big)
        .await
        .assert_problem(413, "too-large");
    nothing_stored(&api, &hash).await;

    // A declared length over the limit: 413 before reading.
    let req = c
        .builder(Method::POST, THEMES)
        .header(header::CONTENT_TYPE, TAR)
        .header(header::CONTENT_LENGTH, (limit + 1).to_string())
        .body(Body::from(vec![0u8; 16]))
        .unwrap();
    c.send_on(&api.router, req)
        .await
        .assert_problem(413, "too-large");

    // Exactly the limit is not refused for its size (it is not a theme: 422).
    upload(&mut c, &api, THEMES, vec![0u8; limit])
        .await
        .assert_problem(422, "validation");

    // Another media type: 415, also for multipart and JSON.
    for ct in [
        "application/json",
        "multipart/form-data; boundary=x",
        "application/gzip",
    ] {
        c.raw(&api, THEMES, ct, bundle("upload-theme"))
            .await
            .assert_problem(415, "unsupported-media-type");
    }
    let req = c
        .builder(Method::POST, THEMES)
        .body(Body::from(bundle("upload-theme")))
        .unwrap();
    c.send_on(&api.router, req)
        .await
        .assert_problem(415, "unsupported-media-type");
    // A parameter on the media type is fine.
    let r = c
        .raw(
            &api,
            THEMES,
            "application/x-tar; charset=binary",
            bundle("upload-theme"),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{r:?}");

    // `activate` must be a boolean.
    upload(
        &mut c,
        &api,
        &format!("{THEMES}?activate=yes"),
        bundle("upload-theme"),
    )
    .await
    .assert_problem(400, "malformed-query");
    api
}

/// `nodes` on the active entry: what each live node reports in `nodes.meta.theme`.
async fn theme_list_shows_node_state(api: Api) -> Api {
    let mut c = admin(&api).await;
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
                "theme": {
                    "name": "blog",
                    "hash": "0123456789abcdef",
                    "origin": "embedded",
                    "error": null,
                    "fallback": false
                }
            }),
        })
        .await
        .unwrap();
    let list = c.get(&api, THEMES).await.json();
    let want = json!([{
        "node": id.as_uuid().to_string(),
        "node_name": "web-1",
        "node_version": "0.0.0",
        "name": "blog",
        "hash": "0123456789abcdef",
        "origin": "embedded",
        "error": null,
        "fallback": false
    }]);
    assert_eq!(entry(&list, "blog")["nodes"], want, "{list}");

    // Only on the active entry.
    let r = upload(
        &mut c,
        &api,
        &format!("{THEMES}?activate=true"),
        bundle("upload-theme"),
    )
    .await;
    assert_eq!(r.status, StatusCode::CREATED, "{r:?}");
    assert_eq!(r.json()["nodes"], want, "{r:?}");
    let list = c.get(&api, THEMES).await.json();
    assert_eq!(entry(&list, "blog")["nodes"], json!([]), "{list}");
    assert_eq!(entry(&list, "uploaded")["nodes"], want, "{list}");

    // A node serving a fallback after a failed swap, with an artifact origin.
    let row = api
        .store
        .read()
        .themes()
        .get("uploaded")
        .await
        .unwrap()
        .unwrap();
    api.store
        .read()
        .nodes()
        .heartbeat(
            id,
            &json!({
                "theme": {
                    "name": "uploaded",
                    "hash": "fedcba9876543210",
                    "origin": {"artifact": row.artifact_hash.as_str()},
                    "error": "theme \"uploaded\": hash mismatch",
                    "fallback": true
                }
            }),
        )
        .await
        .unwrap();
    let list = c.get(&api, THEMES).await.json();
    let node = &entry(&list, "uploaded")["nodes"][0];
    assert_eq!(node["origin"], "artifact", "{list}");
    assert_eq!(node["fallback"], true, "{list}");
    assert_eq!(node["error"], "theme \"uploaded\": hash mismatch", "{list}");
    api
}

async fn editor_cannot_manage_themes(api: Api) -> Api {
    api.user("ed", Role::Editor, false).await;
    let mut c = api.logged_in("ed").await;
    let bytes = bundle("upload-theme");
    let hash = sha256(&bytes);
    let p = upload(&mut c, &api, THEMES, bytes.clone())
        .await
        .assert_problem(403, "forbidden");
    assert!(
        p["detail"].as_str().unwrap().contains("manage_themes"),
        "{p}"
    );
    nothing_stored(&api, &hash).await;
    // Not even the content type or the query is looked at first.
    c.post(&api, THEMES, json!({}))
        .await
        .assert_problem(403, "forbidden");
    upload(
        &mut c,
        &api,
        &format!("{THEMES}?activate=yes"),
        bytes.clone(),
    )
    .await
    .assert_problem(403, "forbidden");
    c.get(&api, THEMES).await.assert_problem(403, "forbidden");
    activate(&mut c, &api, "blog")
        .await
        .assert_problem(403, "forbidden");
    assert!(api.store.read().themes().list().await.unwrap().is_empty());

    // An admin token without `extensions:write` can list but not write.
    let admin = api.user("admin", Role::Admin, false).await;
    let token = api.bearer(&admin, &["extensions:read"]).await;
    let r = token.get(&api, THEMES).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    let req = token
        .builder(Method::POST, THEMES)
        .header(header::CONTENT_TYPE, TAR)
        .body(Body::from(bytes))
        .unwrap();
    api.send(req).await.assert_problem(403, "forbidden");
    token
        .send(
            &api,
            Method::POST,
            &format!("{THEMES}/blog/activate"),
            None,
            None,
        )
        .await
        .assert_problem(403, "forbidden");
    nothing_stored(&api, &hash).await;
    // Without `extensions:read` not even the list.
    let token = api.bearer(&admin, &["content:write"]).await;
    token
        .get(&api, THEMES)
        .await
        .assert_problem(403, "forbidden");
    api
}

/// The node's upload slots are shared with plugin uploads: all taken → `503 busy`.
async fn theme_uploads_are_bounded_per_node(api: Api) -> Api {
    let mut c = admin(&api).await;
    let held: Vec<_> = (0..UPLOAD_SLOTS)
        .map(|_| api.api.installer.try_upload_slot().unwrap())
        .collect();
    let bytes = bundle("upload-theme");
    let r = upload(&mut c, &api, THEMES, bytes.clone()).await;
    r.assert_problem(503, "busy");
    assert!(r.header("retry-after").is_some(), "{r:?}");
    nothing_stored(&api, &sha256(&bytes)).await;
    drop(held);
    let r = upload(&mut c, &api, THEMES, bytes).await;
    assert_eq!(r.status, StatusCode::CREATED, "{r:?}");
    api
}

/// A failed install after staging deletes the staged blob nothing names; activation
/// of an embedded theme too.
async fn failed_theme_install_discards_unreferenced_blob(api: Api) -> Api {
    let mut c = admin(&api).await;
    for (what, bytes) in [
        ("upload", bundle("upload-theme")),
        ("embedded", ferrite_theme::pack_embedded("blog").unwrap()),
    ] {
        let hash = sha256(&bytes);
        let guard = lock(api.store.as_ref(), LOCK_EXTENSIONS, EXTENSIONS_LOCK_WAIT)
            .await
            .unwrap();
        let media = api.api.media.clone();
        let watched = hash.clone();
        let releaser = tokio::spawn(async move {
            while media.head(&watched).await.unwrap().is_none() {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
            tokio::time::sleep(EXTENSIONS_LOCK_WAIT * 2).await;
            guard.release().await.unwrap();
        });
        let r = match what {
            "upload" => upload(&mut c, &api, THEMES, bytes).await,
            _ => activate(&mut c, &api, "blog").await,
        };
        r.assert_problem(503, "busy");
        releaser.await.unwrap();
        nothing_stored(&api, &hash).await;
    }
    api
}

/// R10: a row replaced between loading its bundle and activating it is not activated
/// (`409 conflict`): only the bundle just checked can become active.
async fn activation_race_is_a_conflict(api: Api) -> Api {
    let mut c = admin(&api).await;
    let r = activate(&mut c, &api, "blog").await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    let blog = api
        .store
        .read()
        .themes()
        .get("blog")
        .await
        .unwrap()
        .unwrap();
    let r = upload(&mut c, &api, THEMES, bundle("upload-theme")).await;
    assert_eq!(r.status, StatusCode::CREATED, "{r:?}");
    let checked = api
        .store
        .read()
        .themes()
        .get("uploaded")
        .await
        .unwrap()
        .unwrap();

    // Right after the handler loaded `uploaded`, its row is replaced by another version
    // (as a concurrent upload would): the row now names a bundle this request did not
    // check.
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("templates")).unwrap();
    std::fs::write(
        dir.path().join("theme.toml"),
        "[theme]\nname = \"uploaded\"\nversion = \"1.3.0\"\n",
    )
    .unwrap();
    std::fs::write(dir.path().join("templates/index.html"), "v2").unwrap();
    let v2 = pack(&ThemeSource::Dir(dir.path().to_owned())).unwrap();
    let (artifact, v2_row) = api.api.installer.stage_theme(v2.into()).await.unwrap();
    api.store
        .read()
        .artifacts()
        .insert_if_absent(&artifact)
        .await
        .unwrap();
    assert_ne!(v2_row.artifact_hash, checked.artifact_hash);
    assert_ne!(v2_row.artifact_hash, blog.artifact_hash);
    *api.replace_after_theme_load.lock().unwrap() = Some(v2_row.clone());
    let seq = max_seq(&api).await;
    let p = activate(&mut c, &api, "uploaded")
        .await
        .assert_problem(409, "conflict");
    assert!(p["detail"].as_str().unwrap().contains("replaced"), "{p}");
    assert_eq!(events_after(&api, seq).await, []);
    assert_eq!(active_row(&api).await.as_deref(), Some("blog"));
    assert!(
        api.replace_after_theme_load.lock().unwrap().is_none(),
        "hook ran"
    );

    // A retry checks the row as it is now (v2), and activates that.
    let r = activate(&mut c, &api, "uploaded").await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert_eq!(r.json()["version"], "1.3.0", "{r:?}");
    let active = api.store.read().themes().active().await.unwrap().unwrap();
    assert_eq!(active.artifact_hash, v2_row.artifact_hash);
    api
}

/// Appends one ustar entry with raw `path` bytes, type flag `kind`, `link` and a
/// declared `size` (the data written is `data`).
fn tar_entry(out: &mut Vec<u8>, path: &[u8], kind: u8, link: &[u8], size: u64, data: &[u8]) {
    let mut h = tar::Header::new_ustar();
    {
        let old = h.as_old_mut();
        old.name[..path.len()].copy_from_slice(path);
        old.linkname[..link.len()].copy_from_slice(link);
    }
    h.set_size(size);
    h.set_mode(0o644);
    h.set_entry_type(tar::EntryType::new(kind));
    h.set_cksum();
    out.extend_from_slice(h.as_bytes());
    out.extend_from_slice(data);
    out.extend(std::iter::repeat_n(0u8, (512 - data.len() % 512) % 512));
}

/// Hostile bundles over HTTP: a valid theme plus a symlink, a hard link, a `..` path,
/// an absolute path or an entry declaring more than the bundle may hold is `422`, the
/// problem naming the entry, and nothing is stored.
async fn hostile_bundles_are_refused(api: Api) -> Api {
    let mut c = admin(&api).await;
    let base = || {
        let mut out = Vec::new();
        tar_entry(&mut out, b"templates/index.html", b'0', b"", 5, b"index");
        let toml = b"[theme]\nname = \"t\"\nversion = \"0.1.0\"\n";
        tar_entry(&mut out, b"theme.toml", b'0', b"", toml.len() as u64, toml);
        out
    };
    let end = |mut tar: Vec<u8>| {
        tar.extend_from_slice(&[0u8; 1024]);
        tar
    };
    let oversize = ferrite_theme::MAX_TOTAL_BYTES + 1;
    let hostile: [(&[u8], u8, &[u8], u64); 6] = [
        (b"assets/link", b'2', b"../../../etc/passwd", 0),
        (b"assets/hl", b'1', b"theme.toml", 0),
        (b"../x", b'0', b"", 1),
        (b"assets/../../x", b'0', b"", 1),
        (b"/etc/x", b'0', b"", 1),
        (b"assets/big.css", b'0', b"", oversize),
    ];
    for (path, kind, link, size) in hostile {
        let mut tar = base();
        let data: &[u8] = if size == 1 { b"x" } else { b"" };
        tar_entry(&mut tar, path, kind, link, size, data);
        let bytes = end(tar);
        let hash = sha256(&bytes);
        let p = upload(&mut c, &api, THEMES, bytes)
            .await
            .assert_problem(422, "validation");
        let name = String::from_utf8_lossy(path);
        assert!(p.to_string().contains(&*name), "{name}: {p}");
        nothing_stored(&api, &hash).await;
    }
    // The base bundle itself is a theme (the refusals are about the added entry).
    let r = upload(&mut c, &api, THEMES, end(base())).await;
    assert_eq!(r.status, StatusCode::CREATED, "{r:?}");
    api
}
