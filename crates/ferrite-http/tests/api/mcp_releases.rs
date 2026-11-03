//! M6-3 task 13: the release and review MCP tools, gated publishing over MCP and the
//! signed release preview link.

use axum::http::{Method, StatusCode};
use ferrite_domain::Role;
use serde_json::{Value, json};

use crate::{
    api_tests,
    harness::Api,
    mcp::{mcp_call, text, tool},
};

api_tests!(
    stage_check_apply_round_trip,
    contributor_cannot_apply_a_publish,
    workflow_publish_tells_to_submit,
    no_override_parameter,
    release_preview_link_opens_without_session,
);

fn ok(r: &Value) -> &Value {
    assert_eq!(r["isError"], false, "{r}");
    &r["structuredContent"]
}

fn err(r: &Value) -> &Value {
    assert_eq!(r["isError"], true, "{r}");
    &r["structuredContent"]
}

async fn token(api: &Api, login: &str, role: Role) -> String {
    let u = api.user(login, role, false).await;
    api.bearer(&u, &["*"]).await.0
}

async fn draft(api: &Api, t: &str, title: &str) -> String {
    let r = tool(
        api,
        t,
        "content_create",
        json!({"type": "post", "title": title, "body_markdown": "Text."}),
    )
    .await;
    ok(&r)["id"].as_str().unwrap().to_owned()
}

async fn release(api: &Api, t: &str) -> (String, i64) {
    let r = tool(api, t, "release_create", json!({"title": "Launch"})).await;
    let v = ok(&r);
    (
        v["id"].as_str().unwrap().to_owned(),
        v["version"].as_i64().unwrap(),
    )
}

async fn stage_check_apply_round_trip(api: Api) -> Api {
    let t = token(&api, "eddie", Role::Editor).await;
    let item = draft(&api, &t, "Draft").await;
    let (rel, v) = release(&api, &t).await;
    let r = tool(
        &api,
        &t,
        "release_stage",
        json!({"id": rel, "version": v, "item": item, "transition": "publish",
               "body_markdown": "## New\n\nStaged text."}),
    )
    .await;
    let staged = ok(&r);
    assert!(
        staged["entries"][0]["changes"]["body_markdown"]
            .as_str()
            .unwrap()
            .contains("Staged text"),
        "{staged}"
    );
    let v = staged["version"].as_i64().unwrap();
    let check = tool(&api, &t, "release_check", json!({"id": rel})).await;
    assert_eq!(ok(&check)["clean"], true);
    let listed = tool(&api, &t, "releases_list", json!({})).await;
    assert_eq!(ok(&listed)["items"].as_array().unwrap().len(), 1);
    let applied = tool(&api, &t, "release_apply", json!({"id": rel, "version": v})).await;
    assert_eq!(ok(&applied)["status"], "applied");
    let got = tool(&api, &t, "content_get", json!({"type": "post", "id": item})).await;
    assert_eq!(ok(&got)["status"], "published");
    api
}

async fn contributor_cannot_apply_a_publish(api: Api) -> Api {
    let c = token(&api, "carl", Role::Contributor).await;
    let item = draft(&api, &c, "Mine").await;
    let (rel, v) = release(&api, &c).await;
    let r = tool(
        &api,
        &c,
        "release_stage",
        json!({"id": rel, "version": v, "item": item, "transition": "publish"}),
    )
    .await;
    err(&r);
    assert!(text(&r).contains("publish_own"), "{}", text(&r));
    api
}

async fn workflow_publish_tells_to_submit(api: Api) -> Api {
    let admin = api.user("root", Role::Admin, false).await;
    let b = api.bearer(&admin, &["*"]).await;
    let steps = json!({"steps": [{"key": "review", "label": "Editor review",
        "approvers": {"roles": ["editor", "admin"]}, "required": 1}]});
    let r = b
        .send(
            &api,
            Method::PUT,
            "/api/v1/workflows/post",
            Some(&steps),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    let t = token(&api, "eddie", Role::Editor).await;
    let item = draft(&api, &t, "Gated").await;
    let r = tool(
        &api,
        &t,
        "content_publish",
        json!({"type": "post", "id": item, "version": 1}),
    )
    .await;
    assert!(
        err(&r)["type"]
            .as_str()
            .unwrap()
            .ends_with("approval-required")
    );
    assert!(
        text(&r).contains("content_submit_for_review"),
        "{}",
        text(&r)
    );
    let r = tool(
        &api,
        &t,
        "content_submit_for_review",
        json!({"type": "post", "id": item, "version": 1, "note": "please"}),
    )
    .await;
    ok(&r);
    let queue = tool(&api, &t, "reviews_list", json!({})).await;
    assert_eq!(ok(&queue)["items"][0]["item_title"], "Gated");
    api
}

async fn no_override_parameter(api: Api) -> Api {
    let t = token(&api, "root", Role::Admin).await;
    let (rel, v) = release(&api, &t).await;
    let (status, body) = mcp_call(
        &api,
        &t,
        "tools/call",
        json!({"name": "release_apply",
               "arguments": {"id": rel, "version": v, "override_reason": "x"}}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let refused = body.get("error").is_some() || body["result"]["isError"] == true;
    assert!(refused, "{body}");
    api
}

async fn release_preview_link_opens_without_session(api: Api) -> Api {
    let t = token(&api, "eddie", Role::Editor).await;
    let item = draft(&api, &t, "Coming soon").await;
    let (rel, v) = release(&api, &t).await;
    let r = tool(
        &api,
        &t,
        "release_stage",
        json!({"id": rel, "version": v, "item": item, "transition": "publish"}),
    )
    .await;
    ok(&r);
    let r = tool(&api, &t, "release_preview_link", json!({"id": rel})).await;
    let url = ok(&r)["url"].as_str().unwrap().to_owned();
    let path = url.strip_prefix("https://example.com").unwrap();
    let res = api.get(path).await;
    assert_eq!(res.status, StatusCode::SEE_OTHER, "{res:?}");
    assert!(
        res.header("set-cookie")
            .is_some_and(|c| c.starts_with("__Host-ferrite_preview=ferp_"))
    );
    api
}
