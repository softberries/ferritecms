//! M5 Task 8: the comment, settings, appearance, plugin, user and system tools of the
//! MCP endpoint (design 12 §5.1, §5.2, §5.4): moderation and replies, validated settings
//! writes with the `mcp.enabled` switch out of reach, theme activation and settings,
//! plugin enable/disable with write-only secrets, users without any sign-in data, the
//! admin-only system reads with their scopes, annotations and the audit rows.

use ferrite_domain::{AuditQuery, CommentStatus, Role, Secret, User};
use serde_json::{Value, json};

use crate::{
    api_tests,
    comments_admin::{comment, post},
    harness::{Api, Bearer, plugin_manifest, plugin_wasm},
    mcp::{V_CURRENT, mcp_call, rpc, text, tool},
};

api_tests!(
    moderate_and_reply_comment,
    comment_tools_are_scoped,
    settings_update_validates,
    mcp_enabled_not_writable_via_mcp,
    mcp_settings_not_writable_via_mcp,
    theme_activate_and_settings,
    plugin_secret_settings_are_write_only,
    plugin_enable_disable,
    users_list_hides_secrets,
    system_status_and_audit_need_scope,
    admin_tools_are_annotated,
    admin_writes_are_audited,
);

async fn admin(api: &Api) -> (User, Bearer) {
    let u = api.user("ada", Role::Admin, false).await;
    let t = api.bearer(&u, &["*"]).await;
    (u, t)
}

/// The structured content of a successful result.
fn ok(r: &Value) -> &Value {
    assert_eq!(r["isError"], false, "{r}");
    &r["structuredContent"]
}

/// The problem of a failed result.
fn problem(r: &Value) -> &Value {
    assert_eq!(r["isError"], true, "{r}");
    &r["structuredContent"]
}

/// The whole result as one string (structured content and every text block).
fn everything(r: &Value) -> String {
    r.to_string()
}

async fn mcp_rows(api: &Api) -> Vec<ferrite_domain::AuditEntry> {
    api.store
        .read()
        .audit()
        .list(&AuditQuery::first(200))
        .await
        .unwrap()
        .items
        .into_iter()
        .filter(|r| r.action.starts_with("mcp."))
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

/// Installs and enables plugin `name` through the REST API (the MCP tools cannot).
pub(crate) async fn install_plugin(api: &Api, name: &str) {
    let mut c = api.logged_in("ada").await;
    let manifest = plugin_manifest(name);
    let wasm = plugin_wasm(name);
    let r = c
        .multipart(
            api,
            "/api/v1/plugins/inspect",
            &[("manifest", manifest.as_bytes()), ("component", &wasm)],
        )
        .await;
    assert_eq!(r.status, 200, "{r:?}");
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
    assert_eq!(r.status, 201, "{r:?}");
}

// ---- comments --------------------------------------------------------------------------

async fn moderate_and_reply_comment(api: Api) -> Api {
    let (ada, t) = admin(&api).await;
    let item = post(&api, &ada, "Hello", "hello", true).await;
    let pending = comment(&api, item, CommentStatus::Pending).await;
    let spam = comment(&api, item, CommentStatus::Spam).await;

    // The queue: newest first with the item and the counts of every status.
    let r = tool(&api, &t.0, "comments_list", json!({"status": "pending"})).await;
    let out = ok(&r);
    let items = out["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "{out}");
    assert_eq!(items[0]["id"], pending.to_string(), "{out}");
    assert_eq!(items[0]["item"]["title"], "Hello", "{out}");
    assert_eq!(items[0]["item"]["permalink"], "/hello/", "{out}");
    assert_eq!(items[0]["body"], "Hello", "{out}");
    assert_eq!(items[0]["author_name"], "Ann", "{out}");
    // No visitor e-mail, no address hash, in any tool output.
    let shown = out.to_string();
    for needle in ["ann@example.com", "author_email", "ip_hash"] {
        assert!(!shown.contains(needle), "{needle} in {out}");
    }
    assert_eq!(
        out["counts"],
        json!({"pending": 1, "approved": 0, "spam": 1, "trash": 0}),
        "{out}"
    );
    assert!(out["next_cursor"].is_null(), "{out}");
    let all = tool(
        &api,
        &t.0,
        "comments_list",
        json!({"item": item.to_string()}),
    )
    .await;
    assert_eq!(ok(&all)["items"].as_array().unwrap().len(), 2);

    // Approve it: the result is the comment with its new status.
    let r = tool(
        &api,
        &t.0,
        "comment_moderate",
        json!({"id": pending.to_string(), "status": "approved"}),
    )
    .await;
    assert_eq!(ok(&r)["status"], "approved", "{r}");
    assert_eq!(ok(&r)["item_id"], item.to_string(), "{r}");
    assert!(text(&r).contains("approved"), "{}", text(&r));
    assert!(
        text(&r).contains(&pending.to_string()),
        "named by id: {}",
        text(&r)
    );
    assert!(
        !text(&r).contains("Ann"),
        "not by the visitor's name: {}",
        text(&r)
    );
    for needle in ["ann@example.com", "author_email", "ip_hash"] {
        assert!(!r.to_string().contains(needle), "{needle} in {r}");
    }
    let counts = tool(&api, &t.0, "comments_list", json!({})).await;
    assert_eq!(ok(&counts)["counts"]["approved"], 1, "{counts}");

    // Spam and trash are statuses too; an unknown one or an unknown id is a tool error.
    for status in ["spam", "trash", "pending"] {
        let r = tool(
            &api,
            &t.0,
            "comment_moderate",
            json!({"id": spam.to_string(), "status": status}),
        )
        .await;
        assert_eq!(ok(&r)["status"], status, "{r}");
    }
    let r = tool(
        &api,
        &t.0,
        "comment_moderate",
        json!({"id": pending.to_string(), "status": "deleted"}),
    )
    .await;
    assert_eq!(r["isError"], true, "{r}");
    let r = tool(
        &api,
        &t.0,
        "comment_moderate",
        json!({"id": uuid::Uuid::new_v4().to_string(), "status": "spam"}),
    )
    .await;
    assert_eq!(problem(&r)["status"], 404, "{r}");

    // An admin's reply to the approved comment is approved at once and shown on the post.
    let r = tool(
        &api,
        &t.0,
        "comment_reply",
        json!({"item": item.to_string(), "parent_id": pending.to_string(),
               "body": "Thanks, glad you liked it.\nSee you."}),
    )
    .await;
    let reply = ok(&r);
    assert_eq!(reply["status"], "approved", "{r}");
    assert_eq!(reply["parent_id"], pending.to_string(), "{r}");
    assert_eq!(reply["author_name"], "ada", "{r}");
    assert!(reply["html"].as_str().unwrap().contains("<br>"), "{r}");
    assert!(reply.get("author_email").is_none(), "public view: {r}");
    let id = uuid::Uuid::parse_str(reply["id"].as_str().unwrap()).unwrap();
    let stored = api
        .store
        .read()
        .comments()
        .get(ferrite_domain::CommentId::from_uuid(id))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.status, CommentStatus::Approved);
    assert_eq!(stored.user_id, Some(ada.id), "{stored:?}");
    assert_eq!(stored.body, "Thanks, glad you liked it.\nSee you.");

    // The parent must be an approved comment of the same post; the post must take
    // comments; a body is needed.
    let held = comment(&api, item, CommentStatus::Pending).await;
    let r = tool(
        &api,
        &t.0,
        "comment_reply",
        json!({"item": item.to_string(), "parent_id": held.to_string(), "body": "x"}),
    )
    .await;
    assert_eq!(problem(&r)["errors"][0]["field"], "parent_id", "{r}");
    let r = tool(
        &api,
        &t.0,
        "comment_reply",
        json!({"item": item.to_string(), "body": "   "}),
    )
    .await;
    assert_eq!(problem(&r)["errors"][0]["field"], "body", "{r}");
    let draft = post(&api, &ada, "Draft", "draft", false).await;
    let r = tool(
        &api,
        &t.0,
        "comment_reply",
        json!({"item": draft.to_string(), "body": "hi"}),
    )
    .await;
    assert_eq!(problem(&r)["status"], 404, "{r}");
    let r = tool(
        &api,
        &t.0,
        "comment_reply",
        json!({"item": item.to_string(), "body": "hi", "author_email": "a@b.c"}),
    )
    .await;
    assert_eq!(r["isError"], true, "unknown fields are refused: {r}");
    api
}

async fn comment_tools_are_scoped(api: Api) -> Api {
    let (ada, _) = admin(&api).await;
    let item = post(&api, &ada, "Hello", "hello", true).await;
    let c = comment(&api, item, CommentStatus::Pending).await;

    // An editor moderates; a token with only `comments:read` reads but cannot write.
    let ed = api.user("eddie", Role::Editor, false).await;
    let read = api.bearer(&ed, &["comments:read"]).await;
    let r = tool(&api, &read.0, "comments_list", json!({})).await;
    assert_eq!(ok(&r)["items"].as_array().unwrap().len(), 1, "{r}");
    for (name, args) in [
        (
            "comment_moderate",
            json!({"id": c.to_string(), "status": "approved"}),
        ),
        (
            "comment_reply",
            json!({"item": item.to_string(), "body": "hi"}),
        ),
    ] {
        let r = tool(&api, &read.0, name, args).await;
        assert_eq!(problem(&r)["status"], 403, "{name}: {r}");
        assert!(text(&r).contains("comments:write"), "{name}: {}", text(&r));
    }
    // A token without any comments scope reads nothing.
    let none = api.bearer(&ed, &["content:read"]).await;
    let r = tool(&api, &none.0, "comments_list", json!({})).await;
    assert!(text(&r).contains("comments:read"), "{}", text(&r));
    // An author cannot moderate; their own reply is held.
    let au = api.user("amy", Role::Author, false).await;
    let au_t = api.bearer(&au, &["*"]).await;
    let r = tool(&api, &au_t.0, "comments_list", json!({})).await;
    assert_eq!(problem(&r)["status"], 403, "{r}");
    let r = tool(
        &api,
        &au_t.0,
        "comment_reply",
        json!({"item": item.to_string(), "body": "from an author"}),
    )
    .await;
    assert_eq!(ok(&r)["status"], "pending", "{r}");
    assert!(text(&r).contains("held"), "{}", text(&r));
    // Nothing above was written by the refused calls.
    assert_eq!(
        api.store
            .read()
            .comments()
            .get(c)
            .await
            .unwrap()
            .unwrap()
            .status,
        CommentStatus::Pending
    );
    api
}

// ---- settings --------------------------------------------------------------------------

async fn settings_update_validates(api: Api) -> Api {
    let (ada, t) = admin(&api).await;
    let r = tool(&api, &t.0, "settings_get", json!({})).await;
    let before = ok(&r).clone();
    assert!(before.get("site.title").is_some(), "{before}");
    assert_eq!(before["mcp.enabled"], Value::Null, "unset: {before}");

    // A good change: the answer is every setting as stored.
    let r = tool(
        &api,
        &t.0,
        "settings_update",
        json!({"changes": {"site.title": "Renamed", "reading.per_page": 7}}),
    )
    .await;
    assert_eq!(ok(&r)["site.title"], "Renamed", "{r}");
    assert_eq!(ok(&r)["reading.per_page"], 7, "{r}");
    assert_eq!(option(&api, "site.title").await, Some(json!("Renamed")));

    // Refusals name the key; nothing of the call is written.
    for (changes, field) in [
        (json!({"reading.per_page": 0}), "reading.per_page"),
        (json!({"reading.per_page": "many"}), "reading.per_page"),
        (json!({"site.title": 5}), "site.title"),
        (
            json!({"comments.moderation": "never"}),
            "comments.moderation",
        ),
        (json!({"permalink.page": "/%slug%/"}), "permalink.page"),
        (json!({"nope.key": 1}), "nope.key"),
    ] {
        let r = tool(&api, &t.0, "settings_update", json!({"changes": changes})).await;
        let p = problem(&r);
        assert_eq!(p["status"], 422, "{changes}: {r}");
        assert!(
            p["errors"].to_string().contains(field) || p["detail"].to_string().contains(field),
            "{changes}: {r}"
        );
    }
    let r = tool(
        &api,
        &t.0,
        "settings_update",
        json!({"changes": {"site.title": "Half", "reading.per_page": 0}}),
    )
    .await;
    assert_eq!(r["isError"], true, "{r}");
    assert_eq!(option(&api, "site.title").await, Some(json!("Renamed")));

    // `null` resets to the default; nothing to change, or a wrong shape, is an error.
    let r = tool(
        &api,
        &t.0,
        "settings_update",
        json!({"changes": {"reading.per_page": null}}),
    )
    .await;
    assert_eq!(ok(&r)["reading.per_page"], Value::Null, "{r}");
    assert_eq!(option(&api, "reading.per_page").await, None);
    let r = tool(&api, &t.0, "settings_update", json!({"changes": {}})).await;
    assert_eq!(problem(&r)["errors"][0]["field"], "changes", "{r}");
    let r = tool(&api, &t.0, "settings_update", json!({"site.title": "x"})).await;
    assert_eq!(r["isError"], true, "{r}");
    let r = tool(&api, &t.0, "settings_update", json!({"changes": ["x"]})).await;
    assert_eq!(r["isError"], true, "{r}");

    // Scopes: reading needs `settings:read`, writing `settings:write` and the role.
    let ed = api.user("eddie", Role::Editor, false).await;
    let ed_t = api.bearer(&ed, &["*"]).await;
    let r = tool(&api, &ed_t.0, "settings_get", json!({})).await;
    assert_eq!(ok(&r)["site.title"], "Renamed", "{r}");
    let r = tool(
        &api,
        &ed_t.0,
        "settings_update",
        json!({"changes": {"site.title": "By editor"}}),
    )
    .await;
    assert_eq!(problem(&r)["status"], 403, "{r}");
    let ro = api.bearer(&ada, &["settings:read"]).await;
    let r = tool(
        &api,
        &ro.0,
        "settings_update",
        json!({"changes": {"site.title": "By read scope"}}),
    )
    .await;
    assert!(text(&r).contains("settings:write"), "{}", text(&r));
    assert_eq!(option(&api, "site.title").await, Some(json!("Renamed")));
    api
}

async fn mcp_enabled_not_writable_via_mcp(api: Api) -> Api {
    let (_, t) = admin(&api).await;
    for value in [json!(false), json!(true), Value::Null] {
        let r = tool(
            &api,
            &t.0,
            "settings_update",
            json!({"changes": {"mcp.enabled": value}}),
        )
        .await;
        let p = problem(&r);
        assert_eq!(p["errors"][0]["field"], "mcp.enabled", "{r}");
        assert!(
            text(&r).contains("change this in the admin"),
            "{}",
            text(&r)
        );
        assert_eq!(option(&api, "mcp.enabled").await, None, "{r}");
    }
    // Together with a valid key: the whole call is refused, nothing is written.
    let r = tool(
        &api,
        &t.0,
        "settings_update",
        json!({"changes": {"mcp.enabled": false, "site.title": "Sneaky"}}),
    )
    .await;
    assert_eq!(r["isError"], true, "{r}");
    assert_ne!(option(&api, "site.title").await, Some(json!("Sneaky")));
    // The endpoint still answers, and the setting can still be read.
    let r = tool(&api, &t.0, "settings_get", json!({})).await;
    assert!(ok(&r).get("mcp.enabled").is_some(), "{r}");
    assert!(
        mcp_rows(&api).await.is_empty(),
        "refused calls are not audited"
    );
    // The REST API (a person's session or token) still can: the rule is MCP's.
    let mut c = api.logged_in("ada").await;
    let r = c
        .send(
            &api,
            axum::http::Method::PATCH,
            "/api/v1/settings",
            Some(&json!({"mcp.enabled": false})),
            None,
        )
        .await;
    assert_eq!(r.status, 200, "{r:?}");
    let r = api
        .send(rpc(Some(&t.0), V_CURRENT, "tools/list", json!({})))
        .await;
    assert_eq!(r.status, 404, "off switch: {r:?}");
    api
}

// ---- appearance ------------------------------------------------------------------------

async fn theme_activate_and_settings(api: Api) -> Api {
    let (ada, t) = admin(&api).await;
    let r = tool(&api, &t.0, "themes_list", json!({})).await;
    let items = ok(&r)["items"].as_array().unwrap();
    let names: Vec<&str> = items.iter().filter_map(|i| i["name"].as_str()).collect();
    assert_eq!(names, ["blog", "business", "docs"], "{r}");
    let blog = items.iter().find(|i| i["name"] == "blog").unwrap();
    assert_eq!(blog["active"], true, "the default theme: {r}");
    assert_eq!(blog["origin"], "embedded", "{r}");

    // Activate another built-in theme: it becomes active, the others do not.
    let r = tool(&api, &t.0, "theme_activate", json!({"name": "docs"})).await;
    assert_eq!(ok(&r)["name"], "docs", "{r}");
    assert_eq!(ok(&r)["active"], true, "{r}");
    let row = api.store.read().themes().active().await.unwrap().unwrap();
    assert_eq!(row.name, "docs");
    let r = tool(&api, &t.0, "themes_list", json!({})).await;
    let active: Vec<&str> = ok(&r)["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|i| i["active"] == true)
        .filter_map(|i| i["name"].as_str())
        .collect();
    assert_eq!(active, ["docs"], "{r}");
    assert!(text(&r).contains("active: docs"), "{}", text(&r));

    // An unknown theme, and a name that cannot be one.
    for name in ["nope", "../etc", "Blog", ""] {
        let r = tool(&api, &t.0, "theme_activate", json!({"name": name})).await;
        assert_eq!(problem(&r)["status"], 404, "{name}: {r}");
    }
    assert_eq!(
        api.store
            .read()
            .themes()
            .active()
            .await
            .unwrap()
            .unwrap()
            .name,
        "docs"
    );

    // Settings: the schema and values, a change, refusals.
    let r = tool(&api, &t.0, "theme_activate", json!({"name": "blog"})).await;
    assert_eq!(ok(&r)["active"], true, "{r}");
    let r = tool(&api, &t.0, "theme_settings_get", json!({"name": "blog"})).await;
    let keys: Vec<&str> = ok(&r)["schema"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|s| s["key"].as_str())
        .collect();
    assert_eq!(
        keys,
        ["accent_color", "show_author", "show_excerpts"],
        "{r}"
    );
    assert_eq!(ok(&r)["values"]["accent_color"], "#c2410c", "{r}");

    let r = tool(
        &api,
        &t.0,
        "theme_settings_update",
        json!({"name": "blog", "changes": {"accent_color": "#123456", "show_author": false}}),
    )
    .await;
    assert_eq!(ok(&r)["values"]["accent_color"], "#123456", "{r}");
    assert_eq!(ok(&r)["values"]["show_author"], false, "{r}");
    assert_eq!(ok(&r)["values"]["show_excerpts"], true, "{r}");
    assert_eq!(
        option(&api, "theme.blog.settings").await.unwrap()["accent_color"],
        "#123456"
    );
    for (changes, field) in [
        (json!({"accent_color": "red"}), "accent_color"),
        (json!({"undeclared": 1}), "undeclared"),
        (json!({"show_author": "yes"}), "show_author"),
    ] {
        let r = tool(
            &api,
            &t.0,
            "theme_settings_update",
            json!({"name": "blog", "changes": changes}),
        )
        .await;
        assert_eq!(problem(&r)["status"], 422, "{changes}: {r}");
        assert!(r.to_string().contains(field), "{changes}: {r}");
    }
    let r = tool(
        &api,
        &t.0,
        "theme_settings_update",
        json!({"name": "blog", "changes": {"accent_color": null}}),
    )
    .await;
    assert_eq!(ok(&r)["values"]["accent_color"], "#c2410c", "reset: {r}");
    let r = tool(
        &api,
        &t.0,
        "theme_settings_update",
        json!({"name": "blog", "changes": {}}),
    )
    .await;
    assert_eq!(problem(&r)["errors"][0]["field"], "changes", "{r}");
    let r = tool(&api, &t.0, "theme_settings_get", json!({"name": "nope"})).await;
    assert_eq!(problem(&r)["status"], 404, "{r}");

    // An editor has no say over themes, and a read scope reads only.
    let ed = api.user("eddie", Role::Editor, false).await;
    let ed_t = api.bearer(&ed, &["*"]).await;
    for (name, args) in [
        ("themes_list", json!({})),
        ("theme_activate", json!({"name": "blog"})),
        ("theme_settings_get", json!({"name": "blog"})),
    ] {
        let r = tool(&api, &ed_t.0, name, args).await;
        assert_eq!(problem(&r)["status"], 403, "{name}: {r}");
    }
    let ro = api.bearer(&ada, &["extensions:read"]).await;
    let r = tool(&api, &ro.0, "themes_list", json!({})).await;
    assert_eq!(r["isError"], false, "{r}");
    let r = tool(&api, &ro.0, "theme_activate", json!({"name": "docs"})).await;
    assert!(text(&r).contains("extensions:write"), "{}", text(&r));
    let r = tool(
        &api,
        &ro.0,
        "theme_settings_update",
        json!({"name": "blog", "changes": {"show_author": true}}),
    )
    .await;
    assert!(text(&r).contains("extensions:write"), "{}", text(&r));
    api
}

// ---- plugins ---------------------------------------------------------------------------

const SECRET: &str = "sk-live-0123456789abcdef";

async fn plugin_secret_settings_are_write_only(api: Api) -> Api {
    let (_, t) = admin(&api).await;
    install_plugin(&api, "settings-probe").await;

    let r = tool(
        &api,
        &t.0,
        "plugin_settings_get",
        json!({"name": "settings-probe"}),
    )
    .await;
    let keys: Vec<&str> = ok(&r)["schema"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|s| s["key"].as_str())
        .collect();
    assert_eq!(keys, ["api_key", "limit", "title"], "{r}");
    assert_eq!(ok(&r)["secrets"], json!({"api_key": false}), "{r}");
    assert_eq!(
        ok(&r)["values"],
        json!({"limit": null, "title": "Probe"}),
        "{r}"
    );

    // Set a secret and a value: the answer says only that the secret is set.
    let r = tool(
        &api,
        &t.0,
        "plugin_settings_update",
        json!({"name": "settings-probe", "changes": {"api_key": SECRET, "title": "Hello"}}),
    )
    .await;
    assert_eq!(ok(&r)["secrets"], json!({"api_key": true}), "{r}");
    assert_eq!(ok(&r)["values"]["title"], "Hello", "{r}");
    assert!(ok(&r)["values"].get("api_key").is_none(), "{r}");
    assert!(!everything(&r).contains(SECRET), "echoed: {r}");

    // Never returned again by any tool, and never in the audit log.
    let r = tool(
        &api,
        &t.0,
        "plugin_settings_get",
        json!({"name": "settings-probe"}),
    )
    .await;
    assert_eq!(ok(&r)["secrets"], json!({"api_key": true}), "{r}");
    assert!(!everything(&r).contains(SECRET), "{r}");
    let r = tool(&api, &t.0, "plugins_list", json!({})).await;
    assert!(!everything(&r).contains(SECRET), "{r}");
    // One page, so no cursor field at all.
    assert!(ok(&r).get("next_cursor").is_none(), "{r}");
    let r = tool(&api, &t.0, "audit_list", json!({"per_page": 100})).await;
    assert!(!everything(&r).contains(SECRET), "{r}");
    let rows = mcp_rows(&api).await;
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].action, "mcp.plugin_settings_update");
    assert_eq!(rows[0].target_kind, "plugin");
    assert_eq!(rows[0].target_id.as_deref(), Some("settings-probe"));
    assert_eq!(rows[0].details["keys"], json!(["api_key", "title"]));
    assert!(!rows[0].details.to_string().contains(SECRET));
    assert!(!rows[0].details.to_string().contains("Hello"), "no values");
    let all = api
        .store
        .read()
        .audit()
        .list(&AuditQuery::first(200))
        .await
        .unwrap();
    assert!(
        !serde_json::to_string(
            &all.items
                .iter()
                .map(|e| e.details.clone())
                .collect::<Vec<_>>()
        )
        .unwrap()
        .contains(SECRET)
    );

    // `null` clears the secret; refusals write nothing.
    let r = tool(
        &api,
        &t.0,
        "plugin_settings_update",
        json!({"name": "settings-probe", "changes": {"api_key": null}}),
    )
    .await;
    assert_eq!(ok(&r)["secrets"], json!({"api_key": false}), "{r}");
    for (changes, field) in [
        (json!({"api_key": 5}), "api_key"),
        (json!({"limit": "ten"}), "limit"),
        (json!({"undeclared": SECRET}), "undeclared"),
    ] {
        let r = tool(
            &api,
            &t.0,
            "plugin_settings_update",
            json!({"name": "settings-probe", "changes": changes}),
        )
        .await;
        assert_eq!(problem(&r)["status"], 422, "{changes}: {r}");
        assert!(r.to_string().contains(field), "{changes}: {r}");
        assert!(!everything(&r).contains(SECRET), "{r}");
    }
    let r = tool(
        &api,
        &t.0,
        "plugin_settings_update",
        json!({"name": "nope", "changes": {"a": 1}}),
    )
    .await;
    assert_eq!(problem(&r)["status"], 404, "{r}");
    let r = tool(
        &api,
        &t.0,
        "plugin_settings_update",
        json!({"name": "settings-probe", "changes": {}}),
    )
    .await;
    assert_eq!(problem(&r)["errors"][0]["field"], "changes", "{r}");

    // An editor cannot touch plugins at all.
    let ed = api.user("eddie", Role::Editor, false).await;
    let ed_t = api.bearer(&ed, &["*"]).await;
    let r = tool(
        &api,
        &ed_t.0,
        "plugin_settings_get",
        json!({"name": "settings-probe"}),
    )
    .await;
    assert_eq!(problem(&r)["status"], 403, "{r}");
    api
}

async fn plugin_enable_disable(api: Api) -> Api {
    let (ada, t) = admin(&api).await;
    let r = tool(&api, &t.0, "plugins_list", json!({})).await;
    assert_eq!(ok(&r)["items"], json!([]), "{r}");
    install_plugin(&api, "hello").await;

    let r = tool(&api, &t.0, "plugins_list", json!({})).await;
    let p = &ok(&r)["items"][0];
    assert_eq!(p["name"], "hello", "{r}");
    assert_eq!(
        p["enabled"], false,
        "a new plugin is installed disabled: {r}"
    );
    assert!(
        p["report"]["items"].is_array(),
        "the approved permissions: {r}"
    );
    assert!(text(&r).contains("1 plugins, 0 enabled"), "{}", text(&r));
    assert!(
        p["report"].get("fingerprint").is_none(),
        "no approval hash: {r}"
    );
    assert!(!r.to_string().contains("fingerprint"), "{r}");

    let seq = api.store.read().events().max_seq().await.unwrap();
    let r = tool(&api, &t.0, "plugin_enable", json!({"name": "hello"})).await;
    assert_eq!(ok(&r)["enabled"], true, "{r}");
    assert!(!r.to_string().contains("fingerprint"), "{r}");
    let events = api.store.read().events().since(seq, 100).await.unwrap();
    assert!(
        events
            .iter()
            .any(|e| e.kind == "plugins.changed" && e.keys == ["hello"]),
        "{events:?}"
    );
    // Idempotent.
    let r = tool(&api, &t.0, "plugin_enable", json!({"name": "hello"})).await;
    assert_eq!(ok(&r)["enabled"], true, "{r}");
    let r = tool(&api, &t.0, "plugin_disable", json!({"name": "hello"})).await;
    assert_eq!(ok(&r)["enabled"], false, "{r}");
    assert!(
        api.store
            .read()
            .plugins()
            .get("hello")
            .await
            .unwrap()
            .is_some()
    );

    for name in ["plugin_enable", "plugin_disable"] {
        let r = tool(&api, &t.0, name, json!({"name": "nope"})).await;
        assert_eq!(problem(&r)["status"], 404, "{name}: {r}");
    }

    // Only an administrator with `extensions:write` switches plugins.
    let ed = api.user("eddie", Role::Editor, false).await;
    let ed_t = api.bearer(&ed, &["*"]).await;
    let r = tool(&api, &ed_t.0, "plugin_enable", json!({"name": "hello"})).await;
    assert_eq!(problem(&r)["status"], 403, "{r}");
    let r = tool(&api, &ed_t.0, "plugins_list", json!({})).await;
    assert_eq!(problem(&r)["status"], 403, "{r}");
    let ro = api.bearer(&ada, &["extensions:read"]).await;
    let r = tool(&api, &ro.0, "plugins_list", json!({})).await;
    assert_eq!(r["isError"], false, "{r}");
    let r = tool(&api, &ro.0, "plugin_enable", json!({"name": "hello"})).await;
    assert!(text(&r).contains("extensions:write"), "{}", text(&r));
    let row = api
        .store
        .read()
        .plugins()
        .get("hello")
        .await
        .unwrap()
        .unwrap();
    assert!(!row.enabled);

    let rows: Vec<_> = mcp_rows(&api).await.into_iter().map(|r| r.action).collect();
    assert_eq!(
        rows.iter().filter(|a| *a == "mcp.plugin_enable").count(),
        2,
        "{rows:?}"
    );
    assert_eq!(
        rows.iter().filter(|a| *a == "mcp.plugin_disable").count(),
        1
    );
    api
}

// ---- users -----------------------------------------------------------------------------

async fn users_list_hides_secrets(api: Api) -> Api {
    let (ada, t) = admin(&api).await;
    let tess = api.user("tess", Role::Author, false).await;
    let bob = api.user("bob", Role::Editor, true).await;
    // Tokens and sessions exist for these users; none may show up in a result.
    let tess_token = api.bearer(&tess, &["content:read"]).await;
    let _session = api.logged_in("tess").await;
    api.store
        .read()
        .users()
        .set_totp(tess.id, Some(&Secret::new("sealed-totp-secret")))
        .await
        .unwrap();

    let r = tool(&api, &t.0, "users_list", json!({})).await;
    let items = ok(&r)["items"].as_array().unwrap();
    assert_eq!(items.len(), 3, "{r}");
    let expected = [
        "created_at",
        "display_name",
        "email",
        "id",
        "last_login_at",
        "login",
        "role",
        "status",
    ];
    for item in items {
        let mut keys: Vec<&str> = item
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(keys, expected, "{item}");
    }
    let tess_item = items.iter().find(|i| i["login"] == "tess").unwrap();
    assert_eq!(tess_item["role"], "author", "{r}");
    assert_eq!(tess_item["status"], "active", "{r}");
    assert_eq!(tess_item["email"], "tess@example.com", "{r}");
    assert!(tess_item["last_login_at"].is_string(), "logged in: {r}");
    let shown = everything(&r);
    for needle in [
        "sealed-totp-secret",
        "totp",
        "must_reset",
        "password",
        "$argon2",
        "token",
        "session",
        "recovery",
        &tess_token.0,
    ] {
        assert!(
            !shown.to_lowercase().contains(&needle.to_lowercase()),
            "{needle} in {shown}"
        );
    }

    // One user, with the same shape; bob must reset his password but it is not said.
    let r = tool(&api, &t.0, "user_get", json!({"id": bob.id.to_string()})).await;
    let mut keys: Vec<&str> = ok(&r)
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(keys, expected, "{r}");
    assert_eq!(ok(&r)["login"], "bob", "{r}");
    assert!(!everything(&r).contains("must_reset"), "{r}");
    let r = tool(&api, &t.0, "user_get", json!({"id": tess.id.to_string()})).await;
    assert!(!everything(&r).to_lowercase().contains("totp"), "{r}");
    let r = tool(
        &api,
        &t.0,
        "user_get",
        json!({"id": uuid::Uuid::new_v4().to_string()}),
    )
    .await;
    assert_eq!(problem(&r)["status"], 404, "{r}");

    // Filters and paging.
    let r = tool(&api, &t.0, "users_list", json!({"role": "editor"})).await;
    let logins: Vec<&str> = ok(&r)["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|i| i["login"].as_str())
        .collect();
    assert_eq!(logins, ["bob"], "{r}");
    let r = tool(&api, &t.0, "users_list", json!({"q": "TES"})).await;
    assert_eq!(ok(&r)["items"].as_array().unwrap().len(), 1, "{r}");
    let r = tool(&api, &t.0, "users_list", json!({"status": "disabled"})).await;
    assert_eq!(ok(&r)["items"], json!([]), "{r}");
    let r = tool(&api, &t.0, "users_list", json!({"per_page": 2})).await;
    assert_eq!(ok(&r)["items"].as_array().unwrap().len(), 2, "{r}");
    let cursor = ok(&r)["next_cursor"].as_str().unwrap().to_owned();
    let r = tool(
        &api,
        &t.0,
        "users_list",
        json!({"per_page": 2, "cursor": cursor}),
    )
    .await;
    assert_eq!(ok(&r)["items"].as_array().unwrap().len(), 1, "{r}");
    assert!(ok(&r)["next_cursor"].is_null(), "{r}");
    for bad in [
        json!({"role": "owner"}),
        json!({"per_page": 0}),
        json!({"per_page": 101}),
        json!({"cursor": "garbage"}),
    ] {
        let r = tool(&api, &t.0, "users_list", bad.clone()).await;
        assert_eq!(r["isError"], true, "{bad}: {r}");
    }

    // An editor lists nobody and reads only themselves; a token needs `users:read`.
    let bob_t = api.bearer(&bob, &["*"]).await;
    let r = tool(&api, &bob_t.0, "users_list", json!({})).await;
    assert_eq!(problem(&r)["status"], 403, "{r}");
    let r = tool(
        &api,
        &bob_t.0,
        "user_get",
        json!({"id": ada.id.to_string()}),
    )
    .await;
    assert_eq!(problem(&r)["status"], 403, "{r}");
    let r = tool(
        &api,
        &bob_t.0,
        "user_get",
        json!({"id": bob.id.to_string()}),
    )
    .await;
    assert_eq!(ok(&r)["login"], "bob", "{r}");
    let no_scope = api.bearer(&ada, &["content:read"]).await;
    let r = tool(&api, &no_scope.0, "users_list", json!({})).await;
    assert!(text(&r).contains("users:read"), "{}", text(&r));
    api
}

// ---- system ----------------------------------------------------------------------------

async fn system_status_and_audit_need_scope(api: Api) -> Api {
    let (ada, t) = admin(&api).await;

    let r = tool(&api, &t.0, "system_status", json!({})).await;
    let s = ok(&r);
    assert_eq!(s["version"], env!("CARGO_PKG_VERSION"), "{r}");
    assert_eq!(s["schema"]["verdict"], "up_to_date", "{r}");
    assert!(
        s["jobs"]["dead"].is_number() && s["nodes"]["live"].is_number(),
        "{r}"
    );
    assert!(s["mail"].get("smtp_url").is_none(), "{r}");
    assert!(text(&r).contains("FerriteCMS"), "{}", text(&r));

    let r = tool(&api, &t.0, "jobs_list", json!({})).await;
    assert!(ok(&r)["items"].is_array(), "{r}");
    let r = tool(
        &api,
        &t.0,
        "jobs_list",
        json!({"status": "dead", "kind": "media.variants"}),
    )
    .await;
    assert_eq!(ok(&r)["items"], json!([]), "{r}");
    for bad in [
        json!({"status": "stuck"}),
        json!({"kind": "BAD KIND!"}),
        json!({"per_page": 0}),
        json!({"cursor": "garbage"}),
    ] {
        let r = tool(&api, &t.0, "jobs_list", bad.clone()).await;
        assert_eq!(r["isError"], true, "{bad}: {r}");
    }

    // The audit log: what an assistant did shows with the actor's login and the client.
    tool(
        &api,
        &t.0,
        "settings_update",
        json!({"changes": {"site.title": "Audited"}}),
    )
    .await;
    let r = tool(&api, &t.0, "audit_list", json!({"action": "mcp."})).await;
    let rows = ok(&r)["items"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "{r}");
    assert_eq!(rows[0]["action"], "mcp.settings_update", "{r}");
    assert_eq!(rows[0]["actor"]["login"], "ada", "{r}");
    assert_eq!(rows[0]["details"]["client"], "API token test", "{r}");
    let r = tool(
        &api,
        &t.0,
        "audit_list",
        json!({"actor": ada.id.to_string(), "target_kind": "settings", "per_page": 1}),
    )
    .await;
    assert_eq!(ok(&r)["items"].as_array().unwrap().len(), 1, "{r}");
    for bad in [
        json!({"actor": "ada"}),
        json!({"target_id": "x"}),
        json!({"action": ""}),
        json!({"per_page": 101}),
    ] {
        let r = tool(&api, &t.0, "audit_list", bad.clone()).await;
        assert_eq!(r["isError"], true, "{bad}: {r}");
    }

    // Each needs the `system:read` scope and an administrator.
    let no_scope = api.bearer(&ada, &["content:read", "extensions:read"]).await;
    let only = api.bearer(&ada, &["system:read"]).await;
    let ed = api.user("eddie", Role::Editor, false).await;
    let ed_t = api.bearer(&ed, &["*"]).await;
    for name in ["system_status", "jobs_list", "audit_list"] {
        let r = tool(&api, &no_scope.0, name, json!({})).await;
        assert_eq!(problem(&r)["status"], 403, "{name}: {r}");
        assert!(text(&r).contains("system:read"), "{name}: {}", text(&r));
        assert!(
            !everything(&r).contains(env!("CARGO_PKG_VERSION")),
            "{name}: {r}"
        );
        let r = tool(&api, &only.0, name, json!({})).await;
        assert_eq!(r["isError"], false, "{name}: {r}");
        let r = tool(&api, &ed_t.0, name, json!({})).await;
        assert_eq!(problem(&r)["status"], 403, "{name}: {r}");
        assert!(
            !everything(&r).contains(env!("CARGO_PKG_VERSION")),
            "{name}: {r}"
        );
    }
    api
}

// ---- cross-cutting ---------------------------------------------------------------------

const READ_ONLY: &[&str] = &[
    "comments_list",
    "settings_get",
    "themes_list",
    "theme_settings_get",
    "plugins_list",
    "plugin_settings_get",
    "users_list",
    "user_get",
    "system_status",
    "jobs_list",
    "audit_list",
];
const WRITES: &[&str] = &[
    "comment_moderate",
    "comment_reply",
    "settings_update",
    "theme_activate",
    "theme_settings_update",
    "plugin_enable",
    "plugin_disable",
    "plugin_settings_update",
];

async fn admin_tools_are_annotated(api: Api) -> Api {
    let (_, t) = admin(&api).await;
    let (_, body) = mcp_call(&api, &t.0, "tools/list", json!({})).await;
    let tools = body["result"]["tools"].as_array().unwrap();
    let find = |name: &str| {
        tools
            .iter()
            .find(|t| t["name"] == name)
            .unwrap_or_else(|| panic!("no tool {name}"))
    };
    for name in READ_ONLY {
        let tool = find(name);
        assert_eq!(tool["annotations"]["readOnlyHint"], true, "{name}");
        assert_eq!(tool["annotations"]["openWorldHint"], false, "{name}");
    }
    for name in WRITES {
        let tool = find(name);
        assert_eq!(tool["annotations"]["readOnlyHint"], false, "{name}");
        assert_eq!(tool["annotations"]["openWorldHint"], false, "{name}");
    }
    // Settings writes, moderation (trash), plugin switches and secrets are destructive,
    // as design 12 §5.1 says; a reply and an activation add something and are not.
    for name in [
        "settings_update",
        "comment_moderate",
        "plugin_disable",
        "plugin_settings_update",
        "theme_settings_update",
    ] {
        assert_eq!(find(name)["annotations"]["destructiveHint"], true, "{name}");
    }
    for name in ["comment_reply", "theme_activate", "plugin_enable"] {
        assert_eq!(
            find(name)["annotations"]["destructiveHint"],
            false,
            "{name}"
        );
    }
    assert_eq!(
        find("comment_reply")["annotations"]["idempotentHint"],
        false
    );
    for name in READ_ONLY.iter().chain(WRITES) {
        let tool = find(name);
        let description = tool["description"].as_str().unwrap_or_default();
        assert!(
            description.len() > 60,
            "{name} needs a description for a model"
        );
        assert_eq!(tool["inputSchema"]["type"], "object", "{name}");
        if tool["inputSchema"]["properties"]
            .as_object()
            .is_some_and(|p| !p.is_empty())
        {
            assert_eq!(
                tool["inputSchema"]["additionalProperties"], false,
                "{name} refuses unknown fields"
            );
        }
    }
    // The settings tools tell a model what `null` does.
    for name in [
        "settings_update",
        "theme_settings_update",
        "plugin_settings_update",
    ] {
        let schema = find(name)["inputSchema"].to_string();
        assert!(schema.contains("`null`"), "{name}: {schema}");
    }
    // The prompt of the site-health summary names tools that exist now.
    let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
    for name in ["system_status", "jobs_list", "plugins_list"] {
        assert!(names.contains(&name), "{name}");
    }
    api
}

async fn admin_writes_are_audited(api: Api) -> Api {
    let (ada, t) = admin(&api).await;
    let item = post(&api, &ada, "Hello", "hello", true).await;
    let c = comment(&api, item, CommentStatus::Pending).await;
    install_plugin(&api, "settings-probe").await;

    for (name, args) in [
        (
            "comment_moderate",
            json!({"id": c.to_string(), "status": "approved"}),
        ),
        (
            "comment_reply",
            json!({"item": item.to_string(), "parent_id": c.to_string(), "body": "ok"}),
        ),
        ("settings_update", json!({"changes": {"site.title": "T"}})),
        ("theme_activate", json!({"name": "blog"})),
        (
            "theme_settings_update",
            json!({"name": "blog", "changes": {"show_author": false}}),
        ),
        ("plugin_enable", json!({"name": "settings-probe"})),
        ("plugin_disable", json!({"name": "settings-probe"})),
        (
            "plugin_settings_update",
            json!({"name": "settings-probe", "changes": {"title": "x"}}),
        ),
    ] {
        let r = tool(&api, &t.0, name, args).await;
        assert_eq!(r["isError"], false, "{name}: {r}");
    }
    // Reads and refused writes leave no row.
    for (name, args) in [
        ("comments_list", json!({})),
        ("settings_get", json!({})),
        ("system_status", json!({})),
        (
            "settings_update",
            json!({"changes": {"reading.per_page": 0}}),
        ),
        ("theme_activate", json!({"name": "nope"})),
        ("plugin_enable", json!({"name": "nope"})),
    ] {
        tool(&api, &t.0, name, args).await;
    }
    let rows = mcp_rows(&api).await;
    let mut actions: Vec<&str> = rows.iter().map(|r| r.action.as_str()).collect();
    actions.sort_unstable();
    assert_eq!(
        actions,
        [
            "mcp.comment_moderate",
            "mcp.comment_reply",
            "mcp.plugin_disable",
            "mcp.plugin_enable",
            "mcp.plugin_settings_update",
            "mcp.settings_update",
            "mcp.theme_activate",
            "mcp.theme_settings_update",
        ],
        "{rows:?}"
    );
    for row in &rows {
        assert_eq!(row.actor_id, Some(ada.id), "{row:?}");
        assert_eq!(row.details["client"], "API token test", "{row:?}");
    }
    api
}

/// M6-5 (design 17 §10, §12): no `mcp.*` key is writable over MCP, so an assistant
/// cannot shorten the retention of its own change record.
async fn mcp_settings_not_writable_via_mcp(api: Api) -> Api {
    let (_, t) = admin(&api).await;
    let r = tool(
        &api,
        &t.0,
        "settings_update",
        json!({"changes": {"mcp.change_sets_days": 1}}),
    )
    .await;
    let p = problem(&r);
    assert_eq!(p["errors"][0]["field"], "mcp.change_sets_days", "{r}");
    assert_eq!(option(&api, "mcp.change_sets_days").await, None);
    // A person can, over REST.
    let mut c = api.logged_in("ada").await;
    let r = c
        .send(
            &api,
            axum::http::Method::PATCH,
            "/api/v1/settings",
            Some(&json!({"mcp.change_sets_days": 30})),
            None,
        )
        .await;
    assert_eq!(r.status, axum::http::StatusCode::OK, "{r:?}");
    assert_eq!(option(&api, "mcp.change_sets_days").await, Some(json!(30)));
    api
}
