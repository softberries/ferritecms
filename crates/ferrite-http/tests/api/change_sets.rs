//! M6-5 Task 7: `/api/v1/change-sets` (design 17 §9): sets after MCP writes, their
//! changes and images, and revert (session only, `409 change-conflict`, force, dry run).

use axum::http::{Method, StatusCode};
use ferrite_domain::{Role, User};
use serde_json::{Value, json};

use crate::{
    api_tests,
    harness::{Api, Bearer},
    mcp::tool,
};

api_tests!(
    own_sets_listed_after_mcp_writes,
    others_need_manage_users_and_all_users,
    detail_lists_changes_newest_first,
    change_detail_shows_images_or_hidden,
    secret_is_redacted_in_change_detail,
    revert_requires_session_not_token,
    revert_conflict_is_409_with_list,
    revert_force_and_dry_run,
    revert_bad_seq_list_is_422,
    rest_writes_are_never_recorded,
);

async fn person(api: &Api, login: &str, role: Role) -> (User, Bearer) {
    let u = api.user(login, role, false).await;
    let t = api.bearer(&u, &["*"]).await;
    (u, t)
}

fn ok(r: &Value) -> &Value {
    assert_eq!(r["isError"], false, "{r}");
    &r["structuredContent"]
}

/// Creates a post through MCP; its id and version.
async fn mcp_post(api: &Api, token: &Bearer, title: &str) -> (String, i64) {
    let r = tool(
        api,
        &token.0,
        "content_create",
        json!({"type": "post", "title": title, "body_markdown": "Hello."}),
    )
    .await;
    let out = ok(&r);
    (
        out["id"].as_str().unwrap().to_owned(),
        out["version"].as_i64().unwrap(),
    )
}

async fn sets(api: &Api, login: &str, query: &str) -> Value {
    let mut c = api.logged_in(login).await;
    let r = c.get(api, &format!("/api/v1/change-sets{query}")).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    r.json()
}

async fn own_sets_listed_after_mcp_writes(api: Api) -> Api {
    let (_, t) = person(&api, "ada", Role::Admin).await;
    mcp_post(&api, &t, "From the assistant").await;
    let list = sets(&api, "ada", "").await;
    let items = list["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "{list}");
    let s = &items[0];
    assert_eq!(s["agent"]["kind"], "token");
    assert_eq!(s["agent"]["client"], "API token test");
    assert_eq!(s["agent"]["connected"], true);
    assert_eq!(s["user"]["login"], "ada");
    assert_eq!(s["changes"], 1);
    assert_eq!(s["status"], "open");
    assert_eq!(s["open"], true);
    api
}

async fn others_need_manage_users_and_all_users(api: Api) -> Api {
    let (_, admin) = person(&api, "ada", Role::Admin).await;
    let (eddie, editor) = person(&api, "eddie", Role::Editor).await;
    mcp_post(&api, &editor, "Eddie's").await;
    mcp_post(&api, &admin, "Ada's").await;
    let mut c = api.logged_in("eddie").await;
    let r = c.get(&api, "/api/v1/change-sets?user=all").await;
    r.assert_problem(403, "forbidden");
    let r = c.get(&api, "/api/v1/change-sets").await;
    assert_eq!(r.json()["items"].as_array().unwrap().len(), 1);
    let all = sets(&api, "ada", "?user=all").await;
    assert_eq!(all["items"].as_array().unwrap().len(), 2);
    let theirs = sets(&api, "ada", &format!("?user={}", eddie.id)).await;
    assert_eq!(theirs["items"][0]["user"]["login"], "eddie");
    let mut c = api.logged_in("ada").await;
    let r = c.get(&api, "/api/v1/change-sets?user=nope").await;
    r.assert_problem(422, "validation");
    api
}

async fn detail_lists_changes_newest_first(api: Api) -> Api {
    let (_, t) = person(&api, "ada", Role::Admin).await;
    let (id, version) = mcp_post(&api, &t, "One").await;
    let r = tool(
        &api,
        &t.0,
        "content_update",
        json!({"type": "post", "id": id, "version": version, "title": "Two"}),
    )
    .await;
    ok(&r);
    let list = sets(&api, "ada", "").await;
    let set = list["items"][0]["id"].as_str().unwrap().to_owned();
    let mut c = api.logged_in("ada").await;
    let r = c.get(&api, &format!("/api/v1/change-sets/{set}")).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    let d = r.json();
    let ops: Vec<&str> = d["changes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["op"].as_str().unwrap())
        .collect();
    assert_eq!(ops, ["content.update", "content.create"]);
    assert_eq!(d["changes"][1]["created"], true);
    assert_eq!(
        d["changes"][0]["object"]["admin_path"],
        format!("/content/post/{id}")
    );
    assert_eq!(d["changes"][0]["object"]["label"], "Two");
    api
}

async fn change_detail_shows_images_or_hidden(api: Api) -> Api {
    let (ada, t) = person(&api, "ada", Role::Admin).await;
    let r = tool(
        &api,
        &t.0,
        "settings_update",
        json!({"changes": {"site.title": "AI title"}}),
    )
    .await;
    ok(&r);
    let list = sets(&api, "ada", "").await;
    let set = list["items"][0]["id"].as_str().unwrap().to_owned();
    let mut c = api.logged_in("ada").await;
    let r = c
        .get(&api, &format!("/api/v1/change-sets/{set}/changes/1"))
        .await;
    let d = r.json();
    assert_eq!(d["images"], "shown", "{d}");
    assert_eq!(d["after"], json!({"value": "AI title"}));
    // Ada demoted to editor (by another admin): settings images are hidden now.
    let root = api.user("root", Role::Admin, false).await;
    api.api
        .users
        .update(
            &ferrite_core::Actor::new(root.id, Role::Admin),
            ada.id,
            ferrite_core::UserPatch {
                role: Some(Role::Editor),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let mut c = api.logged_in("ada").await;
    let r = c
        .get(&api, &format!("/api/v1/change-sets/{set}/changes/1"))
        .await;
    let d = r.json();
    assert_eq!(d["images"], "hidden", "{d}");
    assert_eq!(d["before"], Value::Null);
    let r = c
        .get(&api, &format!("/api/v1/change-sets/{set}/changes/9"))
        .await;
    r.assert_problem(404, "not-found");
    api
}

async fn secret_is_redacted_in_change_detail(api: Api) -> Api {
    let (ada, _) = person(&api, "ada", Role::Admin).await;
    let admin = ferrite_core::Actor::new(ada.id, Role::Admin);
    let artifact = ferrite_domain::Artifact {
        hash: ferrite_domain::ArtifactHash::parse(&"0c".repeat(32)).unwrap(),
        kind: ferrite_domain::ArtifactKind::Plugin,
        bytes: 42,
        mime: "application/wasm".into(),
        created_at: ferrite_domain::Timestamp::now(),
        orphaned_at: None,
    };
    let now = ferrite_domain::Timestamp::now();
    api.api
        .extensions
        .install_plugin(
            &admin,
            &artifact,
            &ferrite_domain::InstalledPlugin {
                name: "seo".into(),
                version: "0.1.0".into(),
                artifact_hash: artifact.hash.clone(),
                manifest: json!({
                    "plugin": {"name": "seo", "version": "0.1.0"},
                    "settings": {"api_key": {"type": "secret", "label": "API key"}},
                }),
                granted: json!({}),
                enabled: false,
                disabled_reason: None,
                installed_at: now,
                updated_at: now,
            },
        )
        .await
        .unwrap();
    let agent = admin.with_agent(ferrite_core::Agent::Token(ferrite_domain::ApiTokenId::new()));
    api.api
        .extensions
        .set_plugin_settings(
            &agent,
            "seo",
            json!({"api_key": "sk-live-secret"})
                .as_object()
                .unwrap()
                .clone(),
        )
        .await
        .unwrap();
    let list = sets(&api, "ada", "").await;
    let set = list["items"][0]["id"].as_str().unwrap().to_owned();
    let mut c = api.logged_in("ada").await;
    let r = c
        .get(&api, &format!("/api/v1/change-sets/{set}/changes/1"))
        .await;
    let d = r.json();
    assert_eq!(d["after"], json!({"value": {"secret": "set"}}), "{d}");
    let text = d.to_string();
    assert!(
        !text.contains("sealed") && !text.contains("sk-live"),
        "{text}"
    );
    assert_eq!(d["change"]["object"]["admin_path"], "/plugins/seo");
    api
}

async fn revert_requires_session_not_token(api: Api) -> Api {
    let (_, t) = person(&api, "ada", Role::Admin).await;
    mcp_post(&api, &t, "AI").await;
    let list = sets(&api, "ada", "").await;
    let set = list["items"][0]["id"].as_str().unwrap().to_owned();
    let r = t
        .send(
            &api,
            Method::POST,
            &format!("/api/v1/change-sets/{set}/revert"),
            Some(&json!({})),
            None,
        )
        .await;
    let p = r.assert_problem(403, "forbidden");
    assert!(p["detail"].as_str().unwrap().contains("session"), "{p}");
    let mut c = api.logged_in("ada").await;
    let r = c
        .post(
            &api,
            &format!("/api/v1/change-sets/{set}/revert"),
            json!({}),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert_eq!(r.json()["reverted"], json!([1]));
    api
}

async fn revert_conflict_is_409_with_list(api: Api) -> Api {
    let (ada, t) = person(&api, "ada", Role::Admin).await;
    let (id, version) = mcp_post(&api, &t, "AI").await;
    // A person edits it afterwards (REST, never recorded).
    let mut c = api.logged_in("ada").await;
    let r = c
        .send(
            &api,
            Method::PATCH,
            &format!("/api/v1/content/post/{id}"),
            Some(&json!({"title": "Person"})),
            Some(version),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    let list = sets(&api, "ada", "").await;
    let set = list["items"][0]["id"].as_str().unwrap().to_owned();
    let r = c
        .post(
            &api,
            &format!("/api/v1/change-sets/{set}/revert"),
            json!({}),
        )
        .await;
    let p = r.assert_problem(409, "change-conflict");
    let conflicts = p["report"]["conflicts"].as_array().unwrap();
    assert_eq!(conflicts.len(), 1, "{p}");
    assert_eq!(conflicts[0]["reason"], "changed_since");
    assert_eq!(conflicts[0]["by"], ada.id.to_string());
    assert_eq!(conflicts[0]["object"]["kind"], "content");
    api
}

async fn revert_force_and_dry_run(api: Api) -> Api {
    let (_, t) = person(&api, "ada", Role::Admin).await;
    let (id, version) = mcp_post(&api, &t, "AI").await;
    let mut c = api.logged_in("ada").await;
    c.send(
        &api,
        Method::PATCH,
        &format!("/api/v1/content/post/{id}"),
        Some(&json!({"title": "Person"})),
        Some(version),
    )
    .await;
    let list = sets(&api, "ada", "").await;
    let set = list["items"][0]["id"].as_str().unwrap().to_owned();
    let path = format!("/api/v1/change-sets/{set}/revert");
    let r = c
        .post(&api, &path, json!({"force": true, "dry_run": true}))
        .await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    let d = r.json();
    assert_eq!(
        (d["dry_run"].clone(), d["forced"].clone()),
        (json!(true), json!([1]))
    );
    let item = c
        .get(&api, &format!("/api/v1/content/post/{id}"))
        .await
        .json();
    assert_eq!(item["status"], "draft", "a dry run writes nothing");
    let r = c.post(&api, &path, json!({"force": true})).await;
    assert_eq!(r.json()["forced"], json!([1]));
    let item = c
        .get(&api, &format!("/api/v1/content/post/{id}"))
        .await
        .json();
    assert_eq!(item["status"], "trash");
    let detail = c
        .get(&api, &format!("/api/v1/change-sets/{set}"))
        .await
        .json();
    assert_eq!(detail["set"]["status"], "reverted");
    assert_eq!(detail["changes"][0]["forced"], true);
    api
}

async fn revert_bad_seq_list_is_422(api: Api) -> Api {
    let (_, t) = person(&api, "ada", Role::Admin).await;
    mcp_post(&api, &t, "AI").await;
    let list = sets(&api, "ada", "").await;
    let set = list["items"][0]["id"].as_str().unwrap().to_owned();
    let path = format!("/api/v1/change-sets/{set}/revert");
    let mut c = api.logged_in("ada").await;
    let r = c.post(&api, &path, json!({"changes": [7]})).await;
    let p = r.assert_problem(422, "validation");
    assert_eq!(p["errors"][0]["field"], "changes");
    let many: Vec<u32> = (1..=501).collect();
    let r = c.post(&api, &path, json!({"changes": many})).await;
    r.assert_problem(422, "validation");
    let r = c.post(&api, &path, json!({"changes": [1, 1]})).await;
    assert_eq!(r.json()["reverted"], json!([1]));
    let r = c.post(&api, &path, json!({"nope": true})).await;
    assert!(matches!(r.status.as_u16(), 400 | 422), "{r:?}");
    api
}

async fn rest_writes_are_never_recorded(api: Api) -> Api {
    let (_, t) = person(&api, "ada", Role::Admin).await;
    let r = t
        .send(
            &api,
            Method::POST,
            "/api/v1/content/post",
            Some(&json!({"title": "By a script", "body": {"type": "doc", "content": []}})),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{r:?}");
    let list = sets(&api, "ada", "").await;
    assert_eq!(
        list["items"],
        json!([]),
        "the same token over REST is a script"
    );
    api
}
