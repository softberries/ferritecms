//! M6-5 Task 8: change sets over MCP (design 17 §3.1, §10): every write tool records a
//! change, `changes_list` shows the calling connection's own sets, an OAuth refresh keeps
//! one set, and REST writes with the same token record nothing.

use std::collections::BTreeSet;

use axum::http::{Method, StatusCode};
use ferrite_domain::{CommentStatus, Role, Timestamp, User};
use serde_json::{Value, json};

use crate::{
    api_tests,
    comments_admin::{comment, post},
    harness::{Api, Bearer},
    mcp::{mcp_call, tool},
    mcp_admin::install_plugin,
    mcp_structure::{b64, png},
};

api_tests!(
    changes_list_shows_only_own_agent,
    changes_list_has_review_url_and_no_images,
    oauth_refresh_keeps_one_set,
    rest_write_with_same_token_records_nothing,
    every_write_tool_records_a_change,
    instructions_mention_change_sets,
);

async fn admin(api: &Api) -> (User, Bearer) {
    let u = api.user("ada", Role::Admin, false).await;
    let t = api.bearer(&u, &["*"]).await;
    (u, t)
}

fn ok(r: &Value) -> &Value {
    assert_eq!(r["isError"], false, "{r}");
    &r["structuredContent"]
}

/// Every change recorded so far.
async fn total(api: &Api) -> u64 {
    api.store
        .read()
        .change_sets()
        .changes_since(None, Timestamp::from_micros(0).unwrap())
        .await
        .unwrap()
}

async fn changes_list_shows_only_own_agent(api: Api) -> Api {
    let (ada, a) = admin(&api).await;
    let b = api.bearer(&ada, &["*"]).await;
    let r = tool(
        &api,
        &a.0,
        "content_create",
        json!({"type": "post", "title": "Mine", "body_markdown": "x"}),
    )
    .await;
    ok(&r);
    let r = tool(&api, &b.0, "changes_list", json!({})).await;
    assert_eq!(
        ok(&r)["sets"],
        json!([]),
        "another token of the same user: {r}"
    );
    let r = tool(&api, &a.0, "changes_list", json!({"limit": 3})).await;
    let sets = ok(&r)["sets"].as_array().unwrap().clone();
    assert_eq!(sets.len(), 1, "{r}");
    assert_eq!(sets[0]["items"][0]["op"], "content.create");
    assert_eq!(sets[0]["items"][0]["label"], "Mine");
    // A set id of another connection reads as nothing.
    let id = sets[0]["id"].as_str().unwrap();
    let r = tool(&api, &b.0, "changes_list", json!({"set": id})).await;
    assert_eq!(ok(&r)["sets"], json!([]));
    let r = tool(&api, &a.0, "changes_list", json!({"limit": 21})).await;
    assert_eq!(r["isError"], true, "{r}");
    api
}

async fn changes_list_has_review_url_and_no_images(api: Api) -> Api {
    let (_, t) = admin(&api).await;
    tool(
        &api,
        &t.0,
        "content_create",
        json!({"type": "post", "title": "Secret body", "body_markdown": "very private words"}),
    )
    .await;
    let r = tool(&api, &t.0, "changes_list", json!({})).await;
    let set = &ok(&r)["sets"][0];
    let id = set["id"].as_str().unwrap();
    assert_eq!(
        set["review_url"],
        format!("https://example.com/admin/ai-activity/{id}")
    );
    assert_eq!(set["open"], true);
    assert!(!r.to_string().contains("very private words"), "{r}");
    // The tool is read-only and idempotent.
    let (_, body) = mcp_call(&api, &t.0, "tools/list", json!({})).await;
    let listed = body["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "changes_list")
        .cloned()
        .unwrap();
    assert_eq!(listed["annotations"]["readOnlyHint"], true);
    assert_eq!(listed["annotations"]["idempotentHint"], true);
    api
}

async fn oauth_refresh_keeps_one_set(api: Api) -> Api {
    let (_, client_id, access, refresh_token) =
        crate::oauth::connected(&api, &["content:read", "content:write"]).await;
    let r = tool(
        &api,
        &access,
        "content_create",
        json!({"type": "post", "title": "First", "body_markdown": "x"}),
    )
    .await;
    ok(&r);
    let r = crate::oauth::refresh(&api, &client_id, &refresh_token).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    let next = r.json()["access_token"].as_str().unwrap().to_owned();
    assert_ne!(next, access);
    let r = tool(
        &api,
        &next,
        "content_create",
        json!({"type": "post", "title": "Second", "body_markdown": "x"}),
    )
    .await;
    ok(&r);
    let r = tool(&api, &next, "changes_list", json!({})).await;
    let sets = ok(&r)["sets"].as_array().unwrap().clone();
    assert_eq!(sets.len(), 1, "one set per grant: {r}");
    assert_eq!(sets[0]["changes"], 2);
    let page = api
        .store
        .read()
        .change_sets()
        .list(&ferrite_domain::ChangeSetQuery {
            user: None,
            agent_id: None,
            open: None,
            idle_since: Timestamp::now(),
            limit: 10,
            cursor: None,
        })
        .await
        .unwrap();
    assert_eq!(page.items[0].agent_kind, ferrite_domain::AgentKind::OAuth);
    api
}

async fn rest_write_with_same_token_records_nothing(api: Api) -> Api {
    let (_, t) = admin(&api).await;
    let r = t
        .send(
            &api,
            Method::POST,
            "/api/v1/content/post",
            Some(&json!({"title": "Script", "body": {"type": "doc", "content": []}})),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{r:?}");
    assert_eq!(total(&api).await, 0);
    let r = tool(
        &api,
        &t.0,
        "content_create",
        json!({"type": "post", "title": "Assistant", "body_markdown": "x"}),
    )
    .await;
    ok(&r);
    assert_eq!(total(&api).await, 1, "the same token over /mcp is an agent");
    api
}

async fn instructions_mention_change_sets(api: Api) -> Api {
    let (_, t) = admin(&api).await;
    let (_, body) = mcp_call(
        &api,
        &t.0,
        "initialize",
        json!({"protocolVersion": crate::mcp::V_CURRENT, "capabilities": {},
               "clientInfo": {"name": "test", "version": "1"}}),
    )
    .await;
    let text = body["result"]["instructions"].as_str().unwrap_or_default();
    assert!(
        text.contains("changes_list") && text.contains("change sets"),
        "{body}"
    );
    api
}

/// Calls tool `name` and asserts it succeeded and recorded at least one more change.
async fn write(
    api: &Api,
    t: &Bearer,
    covered: &mut BTreeSet<String>,
    name: &str,
    args: Value,
) -> Value {
    let before = total(api).await;
    let r = tool(api, &t.0, name, args).await;
    let out = ok(&r).clone();
    let after = total(api).await;
    assert!(
        after > before,
        "{name} recorded no change ({before} -> {after}): {r}"
    );
    covered.insert(name.to_owned());
    out
}

fn version(v: &Value) -> i64 {
    v["version"].as_i64().unwrap()
}

fn id(v: &Value) -> String {
    v["id"].as_str().unwrap().to_owned()
}

/// Every tool whose annotations say `readOnlyHint: false` records a change when it
/// writes (design 17 §3.3): a new write tool fails here until it captures (or is listed
/// as deliberately excluded, of which there are none).
async fn every_write_tool_records_a_change(api: Api) -> Api {
    let (ada, t) = admin(&api).await;
    // Two languages for content_translate (a person's setting, over REST).
    let mut c = api.logged_in("ada").await;
    let r = c
        .send(
            &api,
            Method::PATCH,
            "/api/v1/settings",
            Some(&json!({"site.locales": [{"tag": "en"}, {"tag": "de"}]})),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    install_plugin(&api, "settings-probe").await;
    let mut covered = BTreeSet::new();
    let w = &mut covered;

    // Content.
    let item = write(
        &api,
        &t,
        w,
        "content_create",
        json!({"type": "post", "title": "A", "body_markdown": "Hello."}),
    )
    .await;
    let pid = id(&item);
    let item = write(
        &api,
        &t,
        w,
        "content_update",
        json!({"type": "post", "id": pid, "version": version(&item), "title": "B"}),
    )
    .await;
    let item = write(
        &api,
        &t,
        w,
        "content_publish",
        json!({"type": "post", "id": pid, "version": version(&item)}),
    )
    .await;
    let item = write(
        &api,
        &t,
        w,
        "content_unpublish",
        json!({"type": "post", "id": pid, "version": version(&item)}),
    )
    .await;
    let item = write(
        &api,
        &t,
        w,
        "content_submit_for_review",
        json!({"type": "post", "id": pid, "version": version(&item)}),
    )
    .await;
    let item = write(
        &api,
        &t,
        w,
        "content_schedule",
        json!({"type": "post", "id": pid, "version": version(&item), "at": "2999-01-01T00:00:00Z"}),
    )
    .await;
    let item = write(
        &api,
        &t,
        w,
        "content_trash",
        json!({"type": "post", "id": pid, "version": version(&item)}),
    )
    .await;
    let item = write(
        &api,
        &t,
        w,
        "content_restore",
        json!({"type": "post", "id": pid, "version": version(&item)}),
    )
    .await;
    let r = tool(
        &api,
        &t.0,
        "content_revisions",
        json!({"type": "post", "id": pid}),
    )
    .await;
    let revisions = ok(&r)["items"].as_array().unwrap().clone();
    let oldest = revisions.last().unwrap()["id"].as_str().unwrap().to_owned();
    write(
        &api,
        &t,
        w,
        "content_restore_revision",
        json!({"type": "post", "id": pid, "revision": oldest, "version": version(&item)}),
    )
    .await;
    write(
        &api,
        &t,
        w,
        "content_translate",
        json!({"type": "post", "id": pid, "locale": "de"}),
    )
    .await;

    // Media.
    let m = write(
        &api,
        &t,
        w,
        "media_upload",
        json!({"data_base64": b64(&png("guard", 1000)), "filename": "guard.png", "alt": "A"}),
    )
    .await;
    let mid = id(&m);
    let m = write(
        &api,
        &t,
        w,
        "media_update",
        json!({"id": mid, "version": version(&m), "alt": "B"}),
    )
    .await;
    let m = write(
        &api,
        &t,
        w,
        "media_trash",
        json!({"id": mid, "version": version(&m)}),
    )
    .await;
    write(
        &api,
        &t,
        w,
        "media_restore",
        json!({"id": mid, "version": version(&m)}),
    )
    .await;

    // Terms, menus, types, redirects.
    let term = write(
        &api,
        &t,
        w,
        "term_create",
        json!({"taxonomy": "category", "name": "News"}),
    )
    .await;
    write(
        &api,
        &t,
        w,
        "term_update",
        json!({"id": id(&term), "name": "Latest"}),
    )
    .await;
    write(&api, &t, w, "term_delete", json!({"id": id(&term)})).await;
    write(
        &api,
        &t,
        w,
        "menu_set",
        json!({"name": "primary", "items": [{"kind": "url", "label": "Home", "url": "/"}]}),
    )
    .await;
    write(&api, &t, w, "content_type_create",
        json!({"name": "case_study", "label": "Case study", "hierarchical": false, "has_archive": true})).await;
    write(
        &api,
        &t,
        w,
        "content_type_update",
        json!({"name": "case_study", "label": "Case studies"}),
    )
    .await;
    let red = write(
        &api,
        &t,
        w,
        "redirect_create",
        json!({"source": "/old/", "target": "/new/"}),
    )
    .await;
    write(
        &api,
        &t,
        w,
        "redirect_update",
        json!({"id": id(&red), "target": "/newer/", "code": 308}),
    )
    .await;
    write(&api, &t, w, "redirect_delete", json!({"id": id(&red)})).await;

    // Comments.
    let published = post(&api, &ada, "Talk", "talk", true).await;
    let pending = comment(&api, published, CommentStatus::Pending).await;
    write(
        &api,
        &t,
        w,
        "comment_moderate",
        json!({"id": pending.to_string(), "status": "approved"}),
    )
    .await;
    write(
        &api,
        &t,
        w,
        "comment_reply",
        json!({"item": published.to_string(), "body": "Thanks."}),
    )
    .await;

    // Settings and extensions.
    write(
        &api,
        &t,
        w,
        "settings_update",
        json!({"changes": {"site.title": "AI title"}}),
    )
    .await;
    write(&api, &t, w, "theme_activate", json!({"name": "docs"})).await;
    write(
        &api,
        &t,
        w,
        "theme_settings_update",
        json!({"name": "blog", "changes": {"accent_color": "#123456"}}),
    )
    .await;
    write(
        &api,
        &t,
        w,
        "plugin_enable",
        json!({"name": "settings-probe"}),
    )
    .await;
    write(
        &api,
        &t,
        w,
        "plugin_disable",
        json!({"name": "settings-probe"}),
    )
    .await;
    write(
        &api,
        &t,
        w,
        "plugin_settings_update",
        json!({"name": "settings-probe", "changes": {"title": "Hello"}}),
    )
    .await;

    // Releases (M6-3): applying records each item it changes.
    let rel = ok(&tool(&api, &t.0, "release_create", json!({"title": "Batch"})).await).clone();
    let draft = post(&api, &ada, "Batched", "batched", false).await;
    let staged = ok(&tool(
        &api,
        &t.0,
        "release_stage",
        json!({"id": id(&rel), "version": version(&rel), "item": draft.to_string(),
               "transition": "publish"}),
    )
    .await)
    .clone();
    write(
        &api,
        &t,
        w,
        "release_apply",
        json!({"id": id(&rel), "version": version(&staged)}),
    )
    .await;

    // The guard: every write tool the server lists is covered above, except those that
    // change no site object a change set can hold: release rows and preview links
    // (M6-3; reverting content cannot undo them, and they publish nothing).
    let excluded = [
        "release_create",
        "release_stage",
        "release_unstage",
        "release_schedule",
        "release_preview_link",
        "content_preview_link",
    ];
    covered.extend(excluded.iter().map(|s| (*s).to_owned()));
    let (_, body) = mcp_call(&api, &t.0, "tools/list", json!({})).await;
    let writes: BTreeSet<String> = body["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|tool| tool["annotations"]["readOnlyHint"] != true)
        .map(|tool| tool["name"].as_str().unwrap().to_owned())
        .collect();
    let missing: Vec<&String> = writes.difference(&covered).collect();
    assert!(
        missing.is_empty(),
        "write tools without a capture check: {missing:?}"
    );
    assert_eq!(covered.len(), 38, "{covered:?}");
    api
}
