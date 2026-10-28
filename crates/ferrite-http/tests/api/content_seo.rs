//! M2b-2 Task 1: `seo` on content items and in the autosave slot, and the body root
//! check, under `/api/v1/content/{type}` (P28).

use axum::http::{Method, StatusCode};
use ferrite_domain::Role;
use serde_json::{Value, json};

use crate::{
    api_tests,
    harness::{Api, Bearer},
};

api_tests!(
    content_dto_has_seo,
    patch_seo_with_if_match,
    head_after_seo_only_patch
);

const POSTS: &str = "/api/v1/content/post";

async fn admin(api: &Api) -> Bearer {
    let ada = api.user("ada", Role::Admin, false).await;
    api.bearer(&ada, &["*"]).await
}

async fn create(api: &Api, b: &Bearer, body: Value) -> Value {
    let r = b.send(api, Method::POST, POSTS, Some(&body), None).await;
    assert_eq!(r.status, StatusCode::CREATED, "{r:?}");
    r.json()
}

fn path(item: &Value) -> String {
    format!("{POSTS}/{}", item["id"].as_str().unwrap())
}

/// Items carry `seo` (both `null` until set), list entries do not; create sets it
/// (trimmed); limits, control characters and a body that is not a document are `422`
/// naming the member; the autosave slot keeps `seo` as sent.
async fn content_dto_has_seo(api: Api) -> Api {
    let b = admin(&api).await;
    let plain = create(&api, &b, json!({"title": "Plain"})).await;
    assert_eq!(plain["seo"], json!({"title": null, "description": null}));
    let got = b.get(&api, &path(&plain)).await.json();
    assert_eq!(got["seo"], json!({"title": null, "description": null}));

    let item = create(
        &api,
        &b,
        json!({"title": "With SEO", "seo": {"title": " Search title ", "description": "About"}}),
    )
    .await;
    assert_eq!(
        item["seo"],
        json!({"title": "Search title", "description": "About"})
    );
    assert_eq!(b.get(&api, &path(&item)).await.json()["seo"], item["seo"]);
    let list = b.get(&api, POSTS).await.json();
    let entries = list["items"].as_array().unwrap();
    assert_eq!(entries.len(), 2, "{list}");
    assert!(entries.iter().all(|e| e.get("seo").is_none()), "{list}");

    // Problems name the member.
    let long = "t".repeat(201);
    for (body, field) in [
        (json!({"title": "x", "seo": {"title": long}}), "seo.title"),
        (
            json!({"title": "x", "seo": {"description": "d".repeat(501)}}),
            "seo.description",
        ),
        // (NUL never reaches core: the JSON extractor refuses it for any member.)
        (
            json!({"title": "x", "seo": {"description": "a\nb"}}),
            "seo.description",
        ),
        (json!({"title": "x", "body": {"type": "paragraph"}}), "body"),
        (json!({"title": "x", "body": []}), "body"),
        (json!({"title": "x", "body": "x"}), "body"),
    ] {
        let r = b.send(&api, Method::POST, POSTS, Some(&body), None).await;
        let p = r.assert_problem(422, "validation");
        assert_eq!(p["errors"][0]["field"], field, "{p}");
    }
    let r = b
        .send(
            &api,
            Method::PATCH,
            &path(&item),
            Some(&json!({"body": {"type": "paragraph"}})),
            Some(1),
        )
        .await;
    assert_eq!(
        r.assert_problem(422, "validation")["errors"][0]["field"],
        "body"
    );

    // The autosave slot: `seo` as sent; absent or `null` is `null` (unchanged).
    let slot = format!("{}/autosave", path(&item));
    let input = |seo: Value| {
        json!({
            "base_version": 1, "title": "Working",
            "body": {"type": "doc", "content": []}, "fields": {}, "seo": seo,
        })
    };
    let r = b
        .send(
            &api,
            Method::PUT,
            &slot,
            Some(&input(json!({"title": null, "description": " typed "}))),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert_eq!(
        r.json()["seo"],
        json!({"title": null, "description": " typed "})
    );
    assert_eq!(b.get(&api, &slot).await.json()["seo"], r.json()["seo"]);
    let r = b
        .send(&api, Method::PUT, &slot, Some(&input(Value::Null)), None)
        .await;
    assert_eq!(r.json()["seo"], Value::Null, "{r:?}");
    let r = b
        .send(
            &api,
            Method::PUT,
            &slot,
            Some(&input(json!({"title": long}))),
            None,
        )
        .await;
    assert_eq!(
        r.assert_problem(422, "validation")["errors"][0]["field"],
        "seo.title"
    );
    let mut bad = input(Value::Null);
    bad["body"] = json!({"type": "paragraph"});
    let r = b.send(&api, Method::PUT, &slot, Some(&bad), None).await;
    assert_eq!(
        r.assert_problem(422, "validation")["errors"][0]["field"],
        "body"
    );
    // The item itself is untouched by all of it.
    assert_eq!(b.get(&api, &path(&item)).await.json()["version"], 1);
    api
}

/// A `PATCH` of `seo` alone is a full save: `If-Match` is required (`428` without, as
/// for any `PATCH`), a stale one is `412`, and it writes a new version and revision.
async fn patch_seo_with_if_match(api: Api) -> Api {
    let b = admin(&api).await;
    let item = create(&api, &b, json!({"title": "Guarded"})).await;
    let p = path(&item);
    let change = json!({"seo": {"title": "New search title"}});

    let r = b.send(&api, Method::PATCH, &p, Some(&change), None).await;
    r.assert_problem(428, "precondition-required");

    let r = b
        .send(&api, Method::PATCH, &p, Some(&change), Some(1))
        .await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert_eq!(r.header("etag"), Some("\"2\""));
    let saved = r.json();
    assert_eq!(saved["version"], 2);
    assert_eq!(
        saved["seo"],
        json!({"title": "New search title", "description": null})
    );
    assert_eq!(saved["title"], "Guarded");
    let revs = b.get(&api, &format!("{p}/revisions")).await.json();
    assert_eq!(revs["items"].as_array().unwrap().len(), 2, "{revs}");

    // Stale: 412 with the item as stored now, SEO included; nothing written.
    let r = b
        .send(
            &api,
            Method::PATCH,
            &p,
            Some(&json!({"seo": {"title": null}})),
            Some(1),
        )
        .await;
    let problem = r.assert_problem(412, "precondition-failed");
    assert_eq!(problem["current"]["seo"]["title"], "New search title");

    // `null` deletes; `seo: null` itself is malformed.
    let r = b
        .send(
            &api,
            Method::PATCH,
            &p,
            Some(&json!({"seo": {"title": null}})),
            Some(2),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert_eq!(r.json()["seo"]["title"], Value::Null);
    let r = b
        .send(
            &api,
            Method::PATCH,
            &p,
            Some(&json!({"seo": null})),
            Some(3),
        )
        .await;
    let problem = r.assert_problem(400, "malformed-json");
    assert_eq!(problem["errors"][0]["field"], "seo", "{problem}");
    api
}

/// A `PATCH` of `seo` alone reaches the public page's head through the cache
/// invalidation: `<title>`, the meta description and `og:title` show the new values.
async fn head_after_seo_only_patch(api: Api) -> Api {
    let b = admin(&api).await;
    let item = create(
        &api,
        &b,
        json!({"title": "Plain title", "excerpt": "The excerpt"}),
    )
    .await;
    let p = path(&item);
    let r = b
        .send(&api, Method::POST, &format!("{p}/publish"), None, Some(1))
        .await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    let live = r.json();
    let public = live["permalink"].as_str().unwrap().to_owned();
    let mut poller = api.poller().await;
    let page = api.get(&public).await;
    assert_eq!(page.status, StatusCode::OK, "{page:?}");
    assert!(page.body.contains("<title>Plain title – "), "{}", page.body);
    assert!(
        page.body
            .contains("<meta name=\"description\" content=\"The excerpt\">"),
        "{}",
        page.body
    );
    assert!(api.app.cache.len().await >= 1, "the page is cached");

    let r = b
        .send(
            &api,
            Method::PATCH,
            &p,
            Some(&json!({"seo": {"title": "Search title", "description": "Search text"}})),
            Some(live["version"].as_i64().unwrap()),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    poller.poll_once().await.unwrap();
    let page = api.get(&public).await;
    assert!(
        page.body.contains("<title>Search title</title>"),
        "{}",
        page.body
    );
    assert!(
        page.body
            .contains("<meta name=\"description\" content=\"Search text\">"),
        "{}",
        page.body
    );
    assert!(
        page.body
            .contains("<meta property=\"og:title\" content=\"Search title\">"),
        "{}",
        page.body
    );
    api
}
