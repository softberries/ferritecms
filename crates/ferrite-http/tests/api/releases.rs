//! M6-3 tasks 11 and 12: `/api/v1/releases`, preview links, `/workflows`, `/reviews` and
//! the content changes (submit body, override, `allowed` flags, `review`, `releases`).

use axum::http::{Method, StatusCode};
use ferrite_domain::Role;
use serde_json::{Value, json};

use crate::{
    api_tests,
    harness::{Api, Bearer, Client},
};

api_tests!(
    release_crud_with_if_match,
    stage_apply_and_conflict,
    schedule_locks_and_unschedule,
    read_scope_cannot_stage,
    preview_link_token_shown_once,
    session_writes_need_csrf,
    workflow_put_validates_and_is_admin_only,
    gated_publish_review_and_override,
    staging_required_for_live_items,
);

const RELEASES: &str = "/api/v1/releases";

fn doc(text: &str) -> Value {
    json!({"type": "doc", "content": [
        {"type": "paragraph", "content": [{"type": "text", "text": text}]}
    ]})
}

async fn bearer(api: &Api, login: &str, role: Role, scopes: &[&str]) -> Bearer {
    let u = api.user(login, role, false).await;
    api.bearer(&u, scopes).await
}

/// A draft post; returns its id and version.
async fn post(api: &Api, b: &Bearer, title: &str) -> (String, i64) {
    let r = b
        .send(
            api,
            Method::POST,
            "/api/v1/content/post",
            Some(&json!({"title": title, "body": doc(title)})),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{r:?}");
    let v = r.json();
    (
        v["id"].as_str().unwrap().to_owned(),
        v["version"].as_i64().unwrap(),
    )
}

async fn release(api: &Api, b: &Bearer) -> (String, i64) {
    let r = b
        .send(
            api,
            Method::POST,
            RELEASES,
            Some(&json!({"title": "Spring launch"})),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{r:?}");
    let v = r.json();
    (
        v["id"].as_str().unwrap().to_owned(),
        v["version"].as_i64().unwrap(),
    )
}

async fn stage(api: &Api, b: &Bearer, rel: &str, version: i64, body: Value) -> Value {
    let r = b
        .send(
            api,
            Method::POST,
            &format!("{RELEASES}/{rel}/items"),
            Some(&body),
            Some(version),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    r.json()
}

async fn release_crud_with_if_match(api: Api) -> Api {
    let b = bearer(&api, "ed", Role::Editor, &["*"]).await;
    let (id, v) = release(&api, &b).await;
    let got = b.get(&api, &format!("{RELEASES}/{id}")).await;
    assert_eq!(got.header("etag"), Some(format!("\"{v}\"").as_str()));
    assert_eq!(got.json()["status"], "open");
    let r = b
        .send(
            &api,
            Method::PATCH,
            &format!("{RELEASES}/{id}"),
            Some(&json!({"title": "Summer"})),
            Some(v),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert_eq!(r.json()["title"], "Summer");
    // Stale version: 412 with `current`.
    let r = b
        .send(
            &api,
            Method::PATCH,
            &format!("{RELEASES}/{id}"),
            Some(&json!({"title": "x"})),
            Some(v),
        )
        .await;
    assert_eq!(r.status, StatusCode::PRECONDITION_FAILED, "{r:?}");
    assert_eq!(r.json()["current"]["title"], "Summer");
    let list = b.get(&api, RELEASES).await;
    assert_eq!(list.json()["items"].as_array().unwrap().len(), 1);
    let r = b
        .send(
            &api,
            Method::DELETE,
            &format!("{RELEASES}/{id}"),
            None,
            Some(v + 1),
        )
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT, "{r:?}");
    api
}

async fn stage_apply_and_conflict(api: Api) -> Api {
    let b = bearer(&api, "ed", Role::Editor, &["*"]).await;
    let (a, _) = post(&api, &b, "A").await;
    let (c, cv) = post(&api, &b, "C").await;
    let (rel, v) = release(&api, &b).await;
    let staged = stage(
        &api,
        &b,
        &rel,
        v,
        json!({"item_id": a, "transition": "publish",
               "changes": {"title": "A staged", "body": doc("A staged")}}),
    )
    .await;
    let v = staged["version"].as_i64().unwrap();
    assert_eq!(staged["entries"][0]["has_changes"], true);
    let staged = stage(
        &api,
        &b,
        &rel,
        v,
        json!({"item_id": c, "transition": "publish"}),
    )
    .await;
    let v = staged["version"].as_i64().unwrap();
    // C moves elsewhere: the check reports it, apply is 409 with entries.
    let r = b
        .send(
            &api,
            Method::PATCH,
            &format!("/api/v1/content/post/{c}"),
            Some(&json!({"title": "C moved"})),
            Some(cv),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    let check = b.get(&api, &format!("{RELEASES}/{rel}/check")).await.json();
    assert_eq!(check["clean"], false);
    assert_eq!(check["entries"][1]["outcome"], "conflict");
    let r = b
        .send(
            &api,
            Method::POST,
            &format!("{RELEASES}/{rel}/apply"),
            None,
            Some(v),
        )
        .await;
    r.assert_problem(409, "release-conflict");
    assert_eq!(r.json()["entries"][0]["item"], c.as_str());
    let rebased = b
        .send(
            &api,
            Method::POST,
            &format!("{RELEASES}/{rel}/items/{c}/rebase"),
            None,
            Some(v),
        )
        .await;
    assert_eq!(rebased.status, StatusCode::OK, "{rebased:?}");
    let v = rebased.json()["version"].as_i64().unwrap();
    let r = b
        .send(
            &api,
            Method::POST,
            &format!("{RELEASES}/{rel}/apply"),
            None,
            Some(v),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert_eq!(r.json()["status"], "applied");
    let got = b
        .get(&api, &format!("/api/v1/content/post/{a}"))
        .await
        .json();
    assert_eq!(
        (got["status"].as_str(), got["title"].as_str()),
        (Some("published"), Some("A staged"))
    );
    api
}

async fn schedule_locks_and_unschedule(api: Api) -> Api {
    let b = bearer(&api, "ed", Role::Editor, &["*"]).await;
    let (a, av) = post(&api, &b, "A").await;
    let (rel, v) = release(&api, &b).await;
    let v = stage(
        &api,
        &b,
        &rel,
        v,
        json!({"item_id": a, "transition": "publish"}),
    )
    .await["version"]
        .as_i64()
        .unwrap();
    let at = (chrono::Utc::now() + chrono::Duration::hours(2)).to_rfc3339();
    let r = b
        .send(
            &api,
            Method::POST,
            &format!("{RELEASES}/{rel}/schedule"),
            Some(&json!({"at": at})),
            Some(v),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    let v = r.json()["version"].as_i64().unwrap();
    let r = b
        .send(
            &api,
            Method::PATCH,
            &format!("/api/v1/content/post/{a}"),
            Some(&json!({"title": "nope"})),
            Some(av),
        )
        .await;
    r.assert_problem(409, "release-locked");
    let item = b
        .get(&api, &format!("/api/v1/content/post/{a}"))
        .await
        .json();
    assert_eq!(item["releases"][0]["status"], "scheduled");
    let r = b
        .send(
            &api,
            Method::POST,
            &format!("{RELEASES}/{rel}/unschedule"),
            None,
            Some(v),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    let r = b
        .send(
            &api,
            Method::PATCH,
            &format!("/api/v1/content/post/{a}"),
            Some(&json!({"title": "free"})),
            Some(av),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    api
}

async fn read_scope_cannot_stage(api: Api) -> Api {
    let ed = api.user("ed", Role::Editor, false).await;
    let full = api.bearer(&ed, &["*"]).await;
    let read = api.bearer(&ed, &["content:read"]).await;
    let (a, _) = post(&api, &full, "A").await;
    let (rel, v) = release(&api, &full).await;
    let r = read
        .send(
            &api,
            Method::POST,
            &format!("{RELEASES}/{rel}/items"),
            Some(&json!({"item_id": a, "transition": "publish"})),
            Some(v),
        )
        .await;
    r.assert_problem(403, "forbidden");
    assert_eq!(
        read.get(&api, &format!("{RELEASES}/{rel}")).await.status,
        StatusCode::OK
    );
    api
}

async fn preview_link_token_shown_once(api: Api) -> Api {
    let b = bearer(&api, "ed", Role::Editor, &["*"]).await;
    let (a, _) = post(&api, &b, "A").await;
    let path = format!("/api/v1/content/post/{a}/preview-links");
    let r = b
        .send(
            &api,
            Method::POST,
            &path,
            Some(&json!({"expires_in_hours": 2})),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{r:?}");
    let created = r.json();
    let token = created["token"].as_str().unwrap().to_owned();
    assert!(token.starts_with("ferp_"));
    assert!(
        created["url"]
            .as_str()
            .unwrap()
            .ends_with(&format!("/_preview/link/{token}"))
    );
    let list = b.get(&api, &path).await;
    assert!(!list.body.contains(&token), "{}", list.body);
    let id = list.json()["items"][0]["id"].as_str().unwrap().to_owned();
    let r = b
        .send(
            &api,
            Method::DELETE,
            &format!("/api/v1/preview-links/{id}"),
            None,
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    let r = b
        .send(
            &api,
            Method::POST,
            &path,
            Some(&json!({"expires_in_hours": 500})),
            None,
        )
        .await;
    r.assert_problem(422, "validation");
    api
}

async fn session_writes_need_csrf(api: Api) -> Api {
    api.user("ada", Role::Editor, false).await;
    let mut c = api.logged_in("ada").await;
    let r = c.post(&api, RELEASES, json!({"title": "R"})).await;
    assert_eq!(r.status, StatusCode::CREATED, "{r:?}");
    let mut no_csrf = Client::new();
    no_csrf.cookie = c.cookie.clone();
    no_csrf
        .post(&api, RELEASES, json!({"title": "R2"}))
        .await
        .assert_problem(403, "csrf");
    api
}

async fn workflow_put_validates_and_is_admin_only(api: Api) -> Api {
    let admin = bearer(&api, "root", Role::Admin, &["*"]).await;
    let ed = bearer(&api, "ed", Role::Editor, &["*"]).await;
    let steps = json!({"steps": [{"key": "review", "label": "Editor review",
        "approvers": {"roles": ["editor", "admin"], "users": []}, "required": 1}]});
    ed.send(
        &api,
        Method::PUT,
        "/api/v1/workflows/post",
        Some(&steps),
        None,
    )
    .await
    .assert_problem(403, "forbidden");
    let bad = json!({"steps": [{"key": "Bad Key", "label": "x",
        "approvers": {"roles": ["editor"]}, "required": 1}]});
    let r = admin
        .send(
            &api,
            Method::PUT,
            "/api/v1/workflows/post",
            Some(&bad),
            None,
        )
        .await;
    r.assert_problem(422, "validation");
    assert_eq!(r.json()["errors"][0]["field"], "steps");
    let r = admin
        .send(
            &api,
            Method::PUT,
            "/api/v1/workflows/post",
            Some(&steps),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert_eq!(r.header("etag"), Some("\"1\""));
    let list = ed.get(&api, "/api/v1/workflows").await.json();
    assert_eq!(list["items"][0]["type"], "post");
    let r = admin
        .send(
            &api,
            Method::DELETE,
            "/api/v1/workflows/post",
            None,
            Some(1),
        )
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    api
}

async fn with_workflow(api: &Api) -> Bearer {
    let admin = bearer(api, "root", Role::Admin, &["*"]).await;
    let steps = json!({"steps": [{"key": "review", "label": "Editor review",
        "approvers": {"roles": ["editor", "admin"], "users": []}, "required": 1}]});
    let r = admin
        .send(
            api,
            Method::PUT,
            "/api/v1/workflows/post",
            Some(&steps),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    admin
}

async fn gated_publish_review_and_override(api: Api) -> Api {
    let admin = with_workflow(&api).await;
    let author = bearer(&api, "au", Role::Author, &["*"]).await;
    let editor = bearer(&api, "ed", Role::Editor, &["*"]).await;
    let (a, v) = post(&api, &author, "A").await;
    let item = author
        .get(&api, &format!("/api/v1/content/post/{a}"))
        .await
        .json();
    assert_eq!(item["allowed"]["publish"], false);
    assert_eq!(item["allowed"]["needs_approval"], true);
    author
        .send(
            &api,
            Method::POST,
            &format!("/api/v1/content/post/{a}/publish"),
            None,
            Some(v),
        )
        .await
        .assert_problem(409, "approval-required");
    let r = author
        .send(
            &api,
            Method::POST,
            &format!("/api/v1/content/post/{a}/submit"),
            Some(&json!({"note": "please"})),
            Some(v),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    let v = r.json()["version"].as_i64().unwrap();
    assert_eq!(r.json()["review"]["status"], "in_review");
    let queue = editor.get(&api, "/api/v1/reviews").await.json();
    let review = queue["items"][0]["id"].as_str().unwrap().to_owned();
    assert_eq!(queue["items"][0]["item_title"], "A");
    let rv = queue["items"][0]["version"].as_i64().unwrap();
    let r = editor
        .send(
            &api,
            Method::POST,
            &format!("/api/v1/reviews/{review}/decisions"),
            Some(&json!({"decision": "approve", "comment": "ok"})),
            Some(rv),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert_eq!(r.json()["status"], "approved");
    assert_eq!(r.json()["decisions"][0]["comment"], "ok");
    let r = author
        .send(
            &api,
            Method::POST,
            &format!("/api/v1/content/post/{a}/publish"),
            None,
            Some(v),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    // An admin's override, audited.
    let (b, bv) = post(&api, &admin, "B").await;
    let r = admin
        .send(
            &api,
            Method::POST,
            &format!("/api/v1/content/post/{b}/publish"),
            Some(&json!({"override_reason": "legal deadline"})),
            Some(bv),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    let audit = admin.get(&api, "/api/v1/system/audit").await;
    assert!(audit.body.contains("workflow.override"), "{}", audit.body);
    api
}

async fn staging_required_for_live_items(api: Api) -> Api {
    let admin = with_workflow(&api).await;
    let author = bearer(&api, "au", Role::Author, &["*"]).await;
    let (a, v) = post(&api, &admin, "A").await;
    let r = admin
        .send(
            &api,
            Method::POST,
            &format!("/api/v1/content/post/{a}/publish"),
            Some(&json!({"override_reason": "seed"})),
            Some(v),
        )
        .await;
    let v = r.json()["version"].as_i64().unwrap();
    // An author's own live post (made by the admin here) is not theirs; use an
    // author item published by override instead.
    let (mine, mv) = post(&api, &author, "Mine").await;
    let r = admin
        .send(
            &api,
            Method::POST,
            &format!("/api/v1/content/post/{mine}/publish"),
            Some(&json!({"override_reason": "seed"})),
            Some(mv),
        )
        .await;
    let mv = r.json()["version"].as_i64().unwrap();
    author
        .send(
            &api,
            Method::PATCH,
            &format!("/api/v1/content/post/{mine}"),
            Some(&json!({"title": "typo"})),
            Some(mv),
        )
        .await
        .assert_problem(409, "staging-required");
    let item = author
        .get(&api, &format!("/api/v1/content/post/{mine}"))
        .await
        .json();
    assert_eq!(
        (
            item["allowed"]["edit"].as_bool(),
            item["allowed"]["stage"].as_bool()
        ),
        (Some(false), Some(true))
    );
    // The admin approver edits in place (P7).
    let r = admin
        .send(
            &api,
            Method::PATCH,
            &format!("/api/v1/content/post/{a}"),
            Some(&json!({"title": "typo fixed"})),
            Some(v),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    api
}
