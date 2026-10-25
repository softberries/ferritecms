//! M2b-1 Task 3: the per-user autosave slot under `/content/{type}/{id}/autosave`
//! (P15; F11, F12).

use axum::http::{Method, StatusCode};
use ferrite_domain::Role;
use serde_json::{Value, json};

use crate::{api_tests, harness::Api};

api_tests!(autosave_flow, autosave_requires_csrf);

const POSTS: &str = "/api/v1/content/post";

fn doc(text: &str) -> Value {
    json!({"type": "doc", "content": [{"type": "paragraph", "content": [
        {"type": "text", "text": text}
    ]}]})
}

/// PUT → GET → PATCH the item → GET 404, per user; validation and visibility.
async fn autosave_flow(api: Api) -> Api {
    let ann = api.user("ann", Role::Author, false).await;
    api.user("bob", Role::Author, false).await;
    let eve = api.user("eve", Role::Editor, false).await;
    let mut a = api.logged_in("ann").await;
    let mut b = api.logged_in("bob").await;
    let e = api.bearer(&eve, &["*"]).await;

    let r = a.post(&api, POSTS, json!({"title": "Draft"})).await;
    assert_eq!(r.status, StatusCode::CREATED, "{r:?}");
    let item = r.json();
    let id = item["id"].as_str().unwrap().to_owned();
    let version = item["version"].as_i64().unwrap();
    let path = format!("{POSTS}/{id}");
    let slot = format!("{path}/autosave");

    a.get(&api, &slot).await.assert_problem(404, "not-found");

    // No If-Match: per user, last write wins.
    let term = uuid::Uuid::now_v7();
    let input = json!({
        "base_version": version,
        "title": "Working title",
        "slug": "working",
        "menu_order": 3,
        "terms": [term],
        "body": doc("typed so far"),
        "fields": {"half": "done"},
    });
    let r = a.send(&api, Method::PUT, &slot, Some(&input), None).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    let put = r.json();
    for (k, v) in [
        ("base_version", json!(version)),
        ("title", json!("Working title")),
        ("excerpt", Value::Null),
        ("slug", json!("working")),
        ("parent_id", Value::Null),
        ("menu_order", json!(3)),
        ("terms", json!([term])),
        ("body", doc("typed so far")),
        ("fields", json!({"half": "done"})),
    ] {
        assert_eq!(put[k], v, "{k}: {put}");
    }
    assert!(put["saved_at"].as_str().unwrap().ends_with('Z'), "{put}");
    let r = a.get(&api, &slot).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert_eq!(r.json(), put);

    let second =
        json!({"base_version": version, "title": "Second", "body": doc("more"), "fields": {}});
    let r = a.send(&api, Method::PUT, &slot, Some(&second), None).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    let got = a.get(&api, &slot).await.json();
    assert_eq!(
        (&got["title"], &got["slug"], &got["terms"]),
        (&json!("Second"), &Value::Null, &Value::Null)
    );
    // Not a save: the item and its revisions are untouched.
    let now = a.get(&api, &path).await.json();
    assert_eq!(
        (&now["title"], &now["version"]),
        (&json!("Draft"), &json!(version))
    );
    let revs = a.get(&api, &format!("{path}/revisions")).await.json();
    assert_eq!(revs["items"].as_array().unwrap().len(), 1, "{revs}");

    // The editor keeps a slot of its own on the same item.
    let theirs = json!({"base_version": version, "title": "Eve's", "body": doc("e"), "fields": {}});
    let r = e.send(&api, Method::PUT, &slot, Some(&theirs), None).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");

    // Ann saves for real: her slot is gone, Eve's stays.
    let r = a
        .send(
            &api,
            Method::PATCH,
            &path,
            Some(&json!({"title": "Saved"})),
            Some(version),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    a.get(&api, &slot).await.assert_problem(404, "not-found");
    let r = e.get(&api, &slot).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert_eq!(r.json()["title"], "Eve's");
    // DELETE is 204, also when there is none.
    for _ in 0..2 {
        let r = e.send(&api, Method::DELETE, &slot, None, None).await;
        assert_eq!(r.status, StatusCode::NO_CONTENT, "{r:?}");
    }
    e.get(&api, &slot).await.assert_problem(404, "not-found");

    // Text is checked; the shape is required.
    let bad = json!({"base_version": 2, "title": "a\u{7}b", "body": doc("x"), "fields": {}});
    let p = a
        .send(&api, Method::PUT, &slot, Some(&bad), None)
        .await
        .assert_problem(422, "validation");
    assert_eq!(p["errors"][0]["field"], "title", "{p}");
    for bad in [
        json!({"base_version": 2, "title": "t", "body": null, "fields": {}}),
        json!({"base_version": 2, "title": "t", "fields": {}}),
        json!({"title": "t", "body": {}, "fields": {}}),
    ] {
        a.send(&api, Method::PUT, &slot, Some(&bad), None)
            .await
            .assert_problem(400, "malformed-json");
    }

    // Another author cannot see ann's draft, so not its slot either.
    b.get(&api, &slot).await.assert_problem(404, "not-found");
    b.send(&api, Method::PUT, &slot, Some(&second), None)
        .await
        .assert_problem(404, "not-found");
    b.send(&api, Method::DELETE, &slot, None, None)
        .await
        .assert_problem(404, "not-found");
    // Wrong type, media, malformed id, unauthenticated.
    a.get(&api, &format!("/api/v1/content/page/{id}/autosave"))
        .await
        .assert_problem(404, "not-found");
    a.get(&api, &format!("/api/v1/content/media/{id}/autosave"))
        .await
        .assert_problem(404, "not-found");
    a.get(&api, &format!("{POSTS}/nope/autosave"))
        .await
        .assert_problem(404, "not-found");
    api.get(&slot).await.assert_problem(401, "unauthenticated");
    // A read-only token reads but does not write (F12).
    let ro = api.bearer(&ann, &["content:read"]).await;
    ro.get(&api, &slot).await.assert_problem(404, "not-found");
    ro.send(&api, Method::PUT, &slot, Some(&second), None)
        .await
        .assert_problem(403, "forbidden");

    // Trashed: refused until restored.
    let r = a
        .send(&api, Method::DELETE, &path, None, Some(version + 1))
        .await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    let p = a
        .send(&api, Method::PUT, &slot, Some(&second), None)
        .await
        .assert_problem(422, "validation");
    assert!(
        p["detail"].as_str().unwrap().contains("restore first"),
        "{p}"
    );
    api
}

/// A session write without `X-CSRF-Token` is `403 csrf` (the sweep covers the route
/// too); reading needs none.
async fn autosave_requires_csrf(api: Api) -> Api {
    api.user("ann", Role::Author, false).await;
    let mut a = api.logged_in("ann").await;
    let r = a.post(&api, POSTS, json!({"title": "Guarded"})).await;
    assert_eq!(r.status, StatusCode::CREATED, "{r:?}");
    let slot = format!("{POSTS}/{}/autosave", r.json()["id"].as_str().unwrap());
    let body = json!({"base_version": 1, "title": "t", "body": doc("x"), "fields": {}});
    let token = a.csrf.take();
    a.send(&api, Method::PUT, &slot, Some(&body), None)
        .await
        .assert_problem(403, "csrf");
    a.send(&api, Method::DELETE, &slot, None, None)
        .await
        .assert_problem(403, "csrf");
    a.get(&api, &slot).await.assert_problem(404, "not-found");
    a.csrf = token;
    let r = a.send(&api, Method::PUT, &slot, Some(&body), None).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    api
}
