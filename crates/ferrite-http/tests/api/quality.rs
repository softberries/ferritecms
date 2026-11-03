//! M6-4 task 5: `POST /content/{type}/check`, suppressions, the type's `quality_gate` and
//! the `422 content-quality` of a gated publish (design 16 §5, §10).

use axum::http::{Method, StatusCode};
use ferrite_domain::Role;
use serde_json::{Value, json};

use crate::{
    api_tests,
    harness::{Api, Bearer},
};

api_tests!(
    check_draft_returns_findings_with_paths,
    check_existing_item_applies_suppressions,
    check_needs_content_read_scope,
    check_static_segment_does_not_shadow_items,
    check_flags_broken_internal_link,
    publish_on_gated_type_is_422_with_findings,
    bulk_publish_path_gated,
    suppress_and_unsuppress,
    type_patch_quality_gate,
);

fn no_alt() -> Value {
    json!({"type": "doc", "content": [
        {"type": "paragraph", "content": [{"type": "text", "text": "Intro"}]},
        {"type": "image", "attrs": {"src": "/a.png"}}
    ]})
}

async fn editor(api: &Api) -> Bearer {
    let ed = api.user("ed", Role::Editor, false).await;
    api.bearer(&ed, &["*"]).await
}

async fn admin(api: &Api) -> Bearer {
    let ada = api.user("ada", Role::Admin, false).await;
    api.bearer(&ada, &["*"]).await
}

async fn create(api: &Api, b: &Bearer, body: Value) -> Value {
    let r = b
        .send(
            api,
            Method::POST,
            "/api/v1/content/post",
            Some(&json!({"title": "Hello", "body": body})),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{r:?}");
    r.json()
}

async fn gate_on(api: &Api) {
    let a = admin(api).await;
    let r = a
        .send(
            api,
            Method::PATCH,
            "/api/v1/types/post",
            Some(&json!({"quality_gate": "errors"})),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert_eq!(r.json()["quality_gate"], "errors");
}

async fn check_draft_returns_findings_with_paths(api: Api) -> Api {
    let b = editor(&api).await;
    let r = b
        .send(
            &api,
            Method::POST,
            "/api/v1/content/post/check",
            Some(&json!({"title": "T", "body": no_alt()})),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    let body = r.json();
    let f = &body["findings"][0];
    assert_eq!(f["rule"], "image-alt-missing");
    assert_eq!(f["severity"], "error");
    assert_eq!(f["category"], "accessibility");
    assert_eq!(f["wcag"], json!(["1.1.1"]));
    assert_eq!(f["location"], json!({"kind": "body", "path": [1]}));
    assert_eq!(f["key"], "/a.png");
    assert_eq!(f["suppressed"], Value::Null);
    assert_eq!(
        body["counts"],
        json!({"error": 1, "warning": 0, "notice": 0, "suppressed": 0})
    );
    assert_eq!(body["gate"], json!({"mode": "off", "blocked": false}));
    // Unknown members and unknown types.
    let r = b
        .send(
            &api,
            Method::POST,
            "/api/v1/content/post/check",
            Some(&json!({"body": {}, "x": 1})),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "{r:?}");
    let r = b
        .send(
            &api,
            Method::POST,
            "/api/v1/content/nope/check",
            Some(&json!({"body": {}})),
            None,
        )
        .await;
    r.assert_problem(404, "not-found");
    api
}

async fn check_existing_item_applies_suppressions(api: Api) -> Api {
    let b = editor(&api).await;
    let item = create(&api, &b, no_alt()).await;
    let id = item["id"].as_str().unwrap();
    let r = b
        .send(
            &api,
            Method::POST,
            &format!("/api/v1/content/post/{id}/suppressions"),
            Some(&json!({"rule": "image-alt-missing", "key": "/a.png", "reason": "a divider"})),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{r:?}");
    let r = b
        .send(
            &api,
            Method::POST,
            "/api/v1/content/post/check",
            Some(&json!({"id": id, "title": "T", "body": no_alt()})),
            None,
        )
        .await;
    let body = r.json();
    assert_eq!(body["findings"][0]["suppressed"]["reason"], "a divider");
    assert_eq!(body["counts"]["suppressed"], 1);
    assert_eq!(body["counts"]["error"], 0);
    api
}

async fn check_needs_content_read_scope(api: Api) -> Api {
    let ed = api.user("ed", Role::Editor, false).await;
    let b = api.bearer(&ed, &["media:read"]).await;
    let r = b
        .send(
            &api,
            Method::POST,
            "/api/v1/content/post/check",
            Some(&json!({"body": no_alt()})),
            None,
        )
        .await;
    r.assert_problem(403, "forbidden");
    let r = api
        .send(crate::harness::json_body(
            crate::harness::builder(Method::POST, "/api/v1/content/post/check"),
            &json!({"body": no_alt()}),
        ))
        .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    api
}

async fn check_static_segment_does_not_shadow_items(api: Api) -> Api {
    let b = editor(&api).await;
    let item = create(&api, &b, no_alt()).await;
    let id = item["id"].as_str().unwrap();
    let r = b.get(&api, &format!("/api/v1/content/post/{id}")).await;
    assert_eq!(r.status, StatusCode::OK);
    // GET of `check` is not an item.
    let r = b.get(&api, "/api/v1/content/post/check").await;
    assert_ne!(r.status, StatusCode::OK);
    api
}

async fn check_flags_broken_internal_link(api: Api) -> Api {
    let b = editor(&api).await;
    let body = json!({"type": "doc", "content": [{"type": "paragraph", "content": [
        {"type": "text", "text": "A missing page", "marks": [{"type": "link", "attrs": {"href": "/no-such-page/"}}]}
    ]}]});
    let r = b
        .send(
            &api,
            Method::POST,
            "/api/v1/content/post/check",
            Some(&json!({"body": body})),
            None,
        )
        .await;
    let body = r.json();
    let rules: Vec<&str> = body["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["rule"].as_str().unwrap())
        .collect();
    assert_eq!(rules, ["link-internal-broken"], "{body}");
    assert_eq!(body["findings"][0]["detail"]["status"], 404);
    api
}

async fn publish_on_gated_type_is_422_with_findings(api: Api) -> Api {
    gate_on(&api).await;
    let b = editor(&api).await;
    let item = create(&api, &b, no_alt()).await;
    let id = item["id"].as_str().unwrap();
    let v = item["version"].as_i64().unwrap();
    let r = b
        .send(
            &api,
            Method::POST,
            &format!("/api/v1/content/post/{id}/publish"),
            None,
            Some(v),
        )
        .await;
    let p = r.assert_problem(422, "content-quality");
    assert_eq!(p["title"], "This item has accessibility errors");
    assert_eq!(p["findings"][0]["rule"], "image-alt-missing");
    // The check says so too.
    let r = b
        .send(
            &api,
            Method::POST,
            "/api/v1/content/post/check",
            Some(&json!({"id": id, "body": no_alt()})),
            None,
        )
        .await;
    assert_eq!(r.json()["gate"], json!({"mode": "errors", "blocked": true}));
    api
}

/// The admin's bulk bar publishes through the same per-item endpoint, so a selection of
/// two is refused for the gated item and published for the other.
async fn bulk_publish_path_gated(api: Api) -> Api {
    gate_on(&api).await;
    let b = editor(&api).await;
    let bad = create(&api, &b, no_alt()).await;
    let good = create(&api, &b, json!({"type": "doc", "content": []})).await;
    let mut statuses = Vec::new();
    for item in [&bad, &good] {
        let id = item["id"].as_str().unwrap();
        let v = item["version"].as_i64().unwrap();
        let r = b
            .send(
                &api,
                Method::POST,
                &format!("/api/v1/content/post/{id}/publish"),
                None,
                Some(v),
            )
            .await;
        statuses.push(r.status.as_u16());
    }
    assert_eq!(statuses, [422, 200]);
    api
}

async fn suppress_and_unsuppress(api: Api) -> Api {
    let b = editor(&api).await;
    let item = create(&api, &b, no_alt()).await;
    let id = item["id"].as_str().unwrap();
    let base = format!("/api/v1/content/post/{id}/suppressions");
    let r = b
        .send(
            &api,
            Method::POST,
            &base,
            Some(&json!({"rule": "nope", "reason": "x"})),
            None,
        )
        .await;
    let p = r.assert_problem(422, "validation");
    assert_eq!(p["errors"][0]["field"], "rule");
    let r = b
        .send(
            &api,
            Method::POST,
            &base,
            Some(&json!({"rule": "image-alt-missing", "reason": ""})),
            None,
        )
        .await;
    assert_eq!(
        r.assert_problem(422, "validation")["errors"][0]["field"],
        "reason"
    );
    let r = b
        .send(
            &api,
            Method::POST,
            &base,
            Some(&json!({"rule": "image-alt-missing", "reason": "whole rule"})),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{r:?}");
    assert_eq!(r.json()["key"], "");
    let list = b.get(&api, &base).await.json();
    assert_eq!(list["items"].as_array().unwrap().len(), 1);
    let r = b
        .send(
            &api,
            Method::DELETE,
            &format!("{base}/image-alt-missing"),
            None,
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT, "{r:?}");
    let r = b
        .send(
            &api,
            Method::DELETE,
            &format!("{base}/image-alt-missing?key=x"),
            None,
            None,
        )
        .await;
    r.assert_problem(404, "not-found");
    assert!(
        b.get(&api, &base).await.json()["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    api
}

async fn type_patch_quality_gate(api: Api) -> Api {
    let a = admin(&api).await;
    let got = a.get(&api, "/api/v1/types/page").await.json();
    assert_eq!(got["quality_gate"], "off");
    let r = a
        .send(
            &api,
            Method::PATCH,
            "/api/v1/types/media",
            Some(&json!({"quality_gate": "errors"})),
            None,
        )
        .await;
    assert_eq!(
        r.assert_problem(422, "validation")["errors"][0]["field"],
        "quality_gate"
    );
    let r = a
        .send(
            &api,
            Method::PATCH,
            "/api/v1/types/page",
            Some(&json!({"quality_gate": "bogus"})),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "{r:?}");
    let r = a
        .send(
            &api,
            Method::POST,
            "/api/v1/types",
            Some(&json!({"name": "guide", "label": "Guides", "quality_gate": "errors"})),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{r:?}");
    assert_eq!(r.json()["quality_gate"], "errors");
    api
}
