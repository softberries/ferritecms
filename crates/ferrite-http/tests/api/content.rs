//! Task 8: content, lifecycle and revisions under `/api/v1/content/{type}` (P11, P13,
//! Review Focus 2 and 5).

use axum::{
    body::Body,
    http::{Method, StatusCode, header},
};
use ferrite_core::Actor;
use ferrite_domain::{Role, User};
use serde_json::{Value, json};

use crate::{
    api_tests,
    harness::{Api, Bearer, Resp, json_body},
};

api_tests!(
    content_crud_roundtrip,
    content_list_filters,
    stale_if_match_is_412_with_current,
    missing_if_match_is_428,
    delete_trashes_then_force_deletes,
    lifecycle_endpoints,
    token_scope_limits_writes,
    type_mismatch_is_404,
    revisions_over_http,
    others_unpublished_items_are_hidden,
    others_items_are_403,
    json_problems_are_no_store,
);

const POSTS: &str = "/api/v1/content/post";
const PAGES: &str = "/api/v1/content/page";

fn doc(text: &str) -> Value {
    json!({"type": "doc", "content": [
        {"type": "paragraph", "content": [{"type": "text", "text": text}]}
    ]})
}

/// An admin and a token of theirs with every scope.
async fn admin(api: &Api) -> (User, Bearer) {
    let ada = api.user("ada", Role::Admin, false).await;
    let token = api.bearer(&ada, &["*"]).await;
    (ada, token)
}

async fn create(api: &Api, b: &Bearer, base: &str, body: Value) -> Value {
    let r = b.send(api, Method::POST, base, Some(&body), None).await;
    assert_eq!(r.status, StatusCode::CREATED, "{r:?}");
    r.json()
}

fn item_path(base: &str, item: &Value) -> String {
    format!("{base}/{}", item["id"].as_str().unwrap())
}

fn version(item: &Value) -> i64 {
    item["version"].as_i64().unwrap()
}

/// 200 with `ETag: "<version>"` matching the body; returns the body.
fn ok_item(r: &Resp) -> Value {
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    let body = r.json();
    assert_eq!(
        r.header("etag"),
        Some(format!("\"{}\"", version(&body)).as_str()),
        "{r:?}"
    );
    body
}

async fn content_crud_roundtrip(api: Api) -> Api {
    let (ada, b) = admin(&api).await;

    // Create: 201, Location, ETag "1", every column but body_html, terms and permalink.
    let r = b
        .send(
            &api,
            Method::POST,
            POSTS,
            Some(&json!({"title": "Hello API", "body": doc("hello body")})),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{r:?}");
    assert_eq!(r.header("etag"), Some("\"1\""));
    assert_eq!(r.header("cache-control"), Some("no-store"));
    let created = r.json();
    let path = item_path(POSTS, &created);
    assert_eq!(r.header("location"), Some(path.as_str()));
    assert_eq!(created["type"], "post");
    assert_eq!(created["status"], "draft");
    assert_eq!(created["slug"], "hello-api");
    assert_eq!(created["author_id"], ada.id.to_string());
    assert_eq!(created["title"], "Hello API");
    assert_eq!(created["excerpt"], "hello body");
    assert_eq!(created["body"], doc("hello body"));
    assert_eq!(created["fields"], json!({}));
    assert_eq!(created["menu_order"], 0);
    assert_eq!(created["terms"], json!([]));
    assert_eq!(created["parent_id"], Value::Null);
    assert_eq!(created["published_at"], Value::Null);
    assert_eq!(created["permalink"], "/hello-api/");
    assert_eq!(version(&created), 1);
    assert!(created.get("body_html").is_none(), "{created}");
    for ts in ["created_at", "updated_at"] {
        let t = created[ts].as_str().unwrap();
        assert!(
            t.ends_with('Z') && chrono::DateTime::parse_from_rfc3339(t).is_ok(),
            "{t}"
        );
    }

    // Read it back.
    let got = ok_item(&b.get(&api, &path).await);
    assert_eq!(got, created);

    // PATCH the title: ETag "2", everything else kept.
    let r = b
        .send(
            &api,
            Method::PATCH,
            &path,
            Some(&json!({"title": "Hello again"})),
            Some(1),
        )
        .await;
    let patched = ok_item(&r);
    assert_eq!(r.header("etag"), Some("\"2\""));
    assert_eq!(patched["title"], "Hello again");
    assert_eq!(patched["slug"], "hello-api");
    assert_eq!(patched["body"], doc("hello body"));
    assert_eq!(patched["excerpt"], "hello body");
    assert_eq!(patched["permalink"], "/hello-api/");

    // `slug: null` is refused (P13); so is `null` for any non-nullable field.
    for field in ["slug", "title", "body", "fields", "menu_order", "terms"] {
        let r = b
            .send(
                &api,
                Method::PATCH,
                &path,
                Some(&json!({ field: null })),
                Some(2),
            )
            .await;
        let p = r.assert_problem(400, "malformed-json");
        assert_eq!(p["errors"][0]["field"], field, "{p}");
    }
    assert_eq!(version(&b.get(&api, &path).await.json()), 2);

    // Defaults: empty doc body, `{}` fields, no terms; a taken slug gets `-2`.
    let minimal = create(&api, &b, POSTS, json!({"title": "Hello API"})).await;
    assert_eq!(minimal["slug"], "hello-api-2");
    assert_eq!(minimal["body"], json!({"type": "doc", "content": []}));
    assert_eq!(minimal["excerpt"], "");

    // Terms, an explicit excerpt and fields on create.
    let news = api
        .api
        .taxonomy
        .create_term(
            &Actor::new(ada.id, ada.role),
            "category",
            "News",
            Some("news".into()),
            None,
            "",
        )
        .await
        .unwrap();
    let full = create(
        &api,
        &b,
        POSTS,
        json!({
            "title": "Full", "slug": "full-post", "excerpt": "Custom",
            "body": doc("full body"), "fields": {"featured_image": null},
            "menu_order": 3, "terms": [news.id],
        }),
    )
    .await;
    assert_eq!(full["slug"], "full-post");
    assert_eq!(full["excerpt"], "Custom");
    assert_eq!(full["terms"], json!([news.id]));
    assert_eq!(full["menu_order"], 3);
    assert_eq!(full["fields"], json!({"featured_image": null}));
    let full_path = item_path(POSTS, &full);
    // Excerpt: absent keeps the stored text, `null` re-derives it from the body (F20);
    // terms: `[]` clears them.
    let r = b
        .send(
            &api,
            Method::PATCH,
            &full_path,
            Some(&json!({"body": doc("new body")})),
            Some(1),
        )
        .await;
    assert_eq!(ok_item(&r)["excerpt"], "Custom");
    let r = b
        .send(
            &api,
            Method::PATCH,
            &full_path,
            Some(&json!({"excerpt": null, "terms": []})),
            Some(2),
        )
        .await;
    let p = ok_item(&r);
    assert_eq!(p["excerpt"], "new body");
    assert_eq!(p["terms"], json!([]));
    assert_eq!(p["menu_order"], 3, "absent keeps");

    // Core validation is 422 naming the member: unknown term, reserved or bad slug, a
    // parent on a non-hierarchical type or an unknown parent.
    for (body, field) in [
        (
            json!({"title": "x", "terms": [uuid::Uuid::now_v7()]}),
            "terms",
        ),
        (json!({"title": "x", "slug": "admin"}), "slug"),
        (json!({"title": "x", "slug": "a b"}), "slug"),
        (
            json!({"title": "x", "parent_id": created["id"]}),
            "parent_id",
        ),
    ] {
        let p = b
            .send(&api, Method::POST, POSTS, Some(&body), None)
            .await
            .assert_problem(422, "validation");
        assert_eq!(p["errors"][0]["field"], field, "{p}");
    }
    let p = b
        .send(
            &api,
            Method::POST,
            PAGES,
            Some(&json!({"title": "x", "parent_id": uuid::Uuid::now_v7()})),
            None,
        )
        .await
        .assert_problem(422, "validation");
    assert_eq!(p["errors"][0]["field"], "parent_id", "{p}");

    // Pages: `parent_id` absent keeps the parent, `null` moves to the top level (F20).
    let parent = create(&api, &b, PAGES, json!({"title": "Parent"})).await;
    let child = create(
        &api,
        &b,
        PAGES,
        json!({"title": "Child", "parent_id": parent["id"]}),
    )
    .await;
    assert_eq!(child["parent_id"], parent["id"]);
    assert_eq!(child["permalink"], "/parent/child/");
    let child_path = item_path(PAGES, &child);
    let r = b
        .send(
            &api,
            Method::PATCH,
            &child_path,
            Some(&json!({"title": "Child 2"})),
            Some(1),
        )
        .await;
    assert_eq!(ok_item(&r)["parent_id"], parent["id"]);
    let r = b
        .send(
            &api,
            Method::PATCH,
            &child_path,
            Some(&json!({"parent_id": null})),
            Some(2),
        )
        .await;
    let moved = ok_item(&r);
    assert_eq!(moved["parent_id"], Value::Null);
    assert_eq!(moved["permalink"], "/child/");

    // An account that must change its password reads nothing (F13).
    api.user("newbie", Role::Editor, true).await;
    let mut newbie = api.logged_in("newbie").await;
    newbie
        .get(&api, POSTS)
        .await
        .assert_problem(403, "password-reset-required");
    api
}

async fn content_list_filters(api: Api) -> Api {
    let (ada, b) = admin(&api).await;
    let eve = api.user("eve", Role::Author, false).await;
    let eb = api.bearer(&eve, &["*"]).await;
    let a = create(
        &api,
        &b,
        POSTS,
        json!({"title": "Alpha", "body": doc("quokka")}),
    )
    .await;
    let e = create(
        &api,
        &eb,
        POSTS,
        json!({"title": "Eve's", "body": doc("plain")}),
    )
    .await;
    let t = create(&api, &b, POSTS, json!({"title": "Binned"})).await;
    let r = b
        .send(&api, Method::DELETE, &item_path(POSTS, &t), None, Some(1))
        .await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    create(&api, &b, PAGES, json!({"title": "A page"})).await;

    let ids = |r: &Resp| -> Vec<String> {
        assert_eq!(r.status, StatusCode::OK, "{r:?}");
        r.json()["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["id"].as_str().unwrap().to_owned())
            .collect()
    };
    let id = |v: &Value| v["id"].as_str().unwrap().to_owned();

    // Default: non-trash posts, newest update first; summaries have no body.
    let r = b.get(&api, POSTS).await;
    assert_eq!(ids(&r), [id(&e), id(&a)]);
    let list = r.json();
    assert_eq!(list["next_cursor"], Value::Null);
    let first = &list["items"][0];
    assert!(first.get("body").is_none() && first.get("body_html").is_none());
    assert_eq!(first["title"], "Eve's");
    assert_eq!(first["type"], "post");
    assert_eq!(first["version"], 1);

    let (api_ref, b_ref) = (&api, &b);
    let get = move |q: &str| {
        let path = format!("{POSTS}?{q}");
        async move { b_ref.get(api_ref, &path).await }
    };
    assert_eq!(ids(&get("status=trash").await), [id(&t)]);
    assert_eq!(ids(&get("status=draft,trash&order=title").await).len(), 3);
    assert_eq!(ids(&get(&format!("author={}", eve.id)).await), [id(&e)]);
    assert_eq!(ids(&get(&format!("author={}", ada.id)).await), [id(&a)]);
    assert_eq!(ids(&get("q=quokka").await), [id(&a)]);
    assert_eq!(ids(&get("parent=root&order=menu").await).len(), 2);

    // Pages of one: disjoint, then the end.
    let r = get("limit=1").await;
    let page1 = ids(&r);
    let cursor = r.json()["next_cursor"].as_str().unwrap().to_owned();
    let r = get(&format!("limit=1&cursor={cursor}")).await;
    let page2 = ids(&r);
    assert_eq!(page1.len() + page2.len(), 2);
    assert_ne!(page1, page2);
    assert_eq!(r.json()["next_cursor"], Value::Null);

    // A malformed cursor, or one of another order, is 400 with field `cursor`.
    for bad in [
        "cursor=nonsense".to_owned(),
        format!("cursor={cursor}&order=title"),
        format!("cursor={cursor}&q=quokka"),
    ] {
        let p = get(&bad).await.assert_problem(400, "validation");
        assert_eq!(p["errors"][0]["field"], "cursor", "{bad}: {p}");
    }
    // Other bad parameters.
    for (q, field) in [
        ("status=draft,nope", "status"),
        ("parent=nope", "parent"),
        ("limit=0", "limit"),
        ("limit=201", "limit"),
    ] {
        let p = get(q).await.assert_problem(400, "validation");
        assert_eq!(p["errors"][0]["field"], field, "{q}: {p}");
    }
    for q in ["order=random", "author=nope", "limit=x"] {
        get(q).await.assert_problem(400, "malformed-query");
    }
    // `q` with author, term or parent: 422 (F19).
    for q in [
        format!("q=quokka&author={}", ada.id),
        format!("q=quokka&term={}", uuid::Uuid::now_v7()),
        "q=quokka&parent=root".to_owned(),
    ] {
        get(&q).await.assert_problem(422, "validation");
    }
    api
}

async fn stale_if_match_is_412_with_current(api: Api) -> Api {
    let (_, b) = admin(&api).await;
    let item = create(&api, &b, POSTS, json!({"title": "Shared"})).await;
    let path = item_path(POSTS, &item);
    // Another tab saves first.
    let r = b
        .send(
            &api,
            Method::PATCH,
            &path,
            Some(&json!({"title": "Saved by tab A"})),
            Some(1),
        )
        .await;
    let current = ok_item(&r);
    let rev = b.get(&api, &format!("{path}/revisions")).await.json()["items"][1]["id"]
        .as_str()
        .unwrap()
        .to_owned();

    // The stale tab (still at version 1): every write is 412 with the current item.
    let attempts: Vec<(Method, String, Option<Value>)> = vec![
        (
            Method::PATCH,
            path.clone(),
            Some(json!({"title": "Saved by tab B"})),
        ),
        (Method::DELETE, path.clone(), None),
        (Method::DELETE, format!("{path}?force=true"), None),
        (Method::POST, format!("{path}/publish"), None),
        (Method::POST, format!("{path}/submit"), None),
        (
            Method::POST,
            format!("{path}/schedule"),
            Some(json!({"at": "2999-01-01T00:00:00Z"})),
        ),
        (
            Method::POST,
            format!("{path}/revisions/{rev}/restore"),
            None,
        ),
    ];
    for (method, p, body) in attempts {
        let r = b
            .send(&api, method.clone(), &p, body.as_ref(), Some(1))
            .await;
        let problem = r.assert_problem(412, "precondition-failed");
        assert_eq!(problem["current"], current, "{method} {p}");
    }
    // Nothing was written.
    let now = ok_item(&b.get(&api, &path).await);
    assert_eq!(now, current);
    let revs = b.get(&api, &format!("{path}/revisions")).await.json();
    assert_eq!(revs["items"].as_array().unwrap().len(), 2);
    api
}

async fn missing_if_match_is_428(api: Api) -> Api {
    let (_, b) = admin(&api).await;
    let item = create(&api, &b, POSTS, json!({"title": "Guarded"})).await;
    let path = item_path(POSTS, &item);
    let rev = b.get(&api, &format!("{path}/revisions")).await.json()["items"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let attempts: Vec<(Method, String, Option<Value>)> = vec![
        (Method::PATCH, path.clone(), Some(json!({"title": "Nope"}))),
        (Method::DELETE, path.clone(), None),
        (Method::POST, format!("{path}/publish"), None),
        (Method::POST, format!("{path}/unpublish"), None),
        (Method::POST, format!("{path}/submit"), None),
        (Method::POST, format!("{path}/restore"), None),
        (
            Method::POST,
            format!("{path}/schedule"),
            Some(json!({"at": "2999-01-01T00:00:00Z"})),
        ),
        (
            Method::POST,
            format!("{path}/revisions/{rev}/restore"),
            None,
        ),
    ];
    for (method, p, body) in attempts {
        let r = b.send(&api, method, &p, body.as_ref(), None).await;
        r.assert_problem(428, "precondition-required");
    }
    // A malformed `If-Match` is 400.
    let req = json_body(
        b.builder(Method::PATCH, &path)
            .header(header::IF_MATCH, "W/\"1\""),
        &json!({"title": "Nope"}),
    );
    api.send(req).await.assert_problem(400, "malformed-header");
    let now = ok_item(&b.get(&api, &path).await);
    assert_eq!(now, item);
    let revs = b.get(&api, &format!("{path}/revisions")).await.json();
    assert_eq!(revs["items"].as_array().unwrap().len(), 1);
    api
}

async fn delete_trashes_then_force_deletes(api: Api) -> Api {
    let (_, b) = admin(&api).await;
    let item = create(&api, &b, POSTS, json!({"title": "Doomed"})).await;
    let path = item_path(POSTS, &item);

    // `force` on an item that is not trashed: 422, nothing written.
    b.send(
        &api,
        Method::DELETE,
        &format!("{path}?force=true"),
        None,
        Some(1),
    )
    .await
    .assert_problem(422, "validation");

    // DELETE trashes (200 with the item).
    let r = b.send(&api, Method::DELETE, &path, None, Some(1)).await;
    let trashed = ok_item(&r);
    assert_eq!(trashed["status"], "trash");
    assert_eq!(version(&trashed), 2);
    // Trashing again is an illegal transition.
    b.send(&api, Method::DELETE, &path, None, Some(2))
        .await
        .assert_problem(422, "validation");
    // A bad `force` value is a malformed query.
    b.send(
        &api,
        Method::DELETE,
        &format!("{path}?force=yes"),
        None,
        Some(2),
    )
    .await
    .assert_problem(400, "malformed-query");

    // `?force=true` deletes the trashed item: 204, then 404.
    let r = b
        .send(
            &api,
            Method::DELETE,
            &format!("{path}?force=true"),
            None,
            Some(2),
        )
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT, "{r:?}");
    assert_eq!(r.header("cache-control"), Some("no-store"));
    assert!(r.body.is_empty());
    b.get(&api, &path).await.assert_problem(404, "not-found");
    b.send(&api, Method::DELETE, &path, None, Some(2))
        .await
        .assert_problem(404, "not-found");
    api
}

async fn lifecycle_endpoints(api: Api) -> Api {
    let (_, b) = admin(&api).await;
    let item = create(
        &api,
        &b,
        POSTS,
        json!({"title": "Going Live", "body": doc("live body")}),
    )
    .await;
    let path = item_path(POSTS, &item);
    let public = item["permalink"].as_str().unwrap().to_owned();
    let mut poller = api.poller().await;
    assert_eq!(api.get(&public).await.status, StatusCode::NOT_FOUND);

    // Publish: the public page renders after one poll.
    let r = b
        .send(
            &api,
            Method::POST,
            &format!("{path}/publish"),
            None,
            Some(1),
        )
        .await;
    let live = ok_item(&r);
    assert_eq!(live["status"], "published");
    assert!(live["published_at"].is_string());
    poller.poll_once().await.unwrap();
    let page = api.get(&public).await;
    assert_eq!(page.status, StatusCode::OK, "{page:?}");
    assert!(page.body.contains("Going Live"));

    // An API edit reaches the public page through the cache invalidation (the same
    // events as the core services).
    assert!(api.app.cache.len().await >= 1, "the page is cached");
    let r = b
        .send(
            &api,
            Method::PATCH,
            &path,
            Some(&json!({"title": "Edited Live"})),
            Some(version(&live)),
        )
        .await;
    let edited = ok_item(&r);
    assert!(api.get(&public).await.body.contains("Going Live"), "cached");
    poller.poll_once().await.unwrap();
    let page = api.get(&public).await;
    assert!(page.body.contains("Edited Live"), "{page:?}");

    // Unpublish, submit, schedule (a past time is 422), publish now from scheduled.
    let r = b
        .send(
            &api,
            Method::POST,
            &format!("{path}/unpublish"),
            None,
            Some(version(&edited)),
        )
        .await;
    let draft = ok_item(&r);
    assert_eq!(draft["status"], "draft");
    poller.poll_once().await.unwrap();
    assert_eq!(api.get(&public).await.status, StatusCode::NOT_FOUND);
    let r = b
        .send(
            &api,
            Method::POST,
            &format!("{path}/submit"),
            None,
            Some(version(&draft)),
        )
        .await;
    let pending = ok_item(&r);
    assert_eq!(pending["status"], "pending");
    let schedule = format!("{path}/schedule");
    b.send(
        &api,
        Method::POST,
        &schedule,
        Some(&json!({"at": "2001-01-01T00:00:00Z"})),
        Some(version(&pending)),
    )
    .await
    .assert_problem(422, "validation");
    for bad in [json!({"at": "tomorrow"}), json!({})] {
        b.send(
            &api,
            Method::POST,
            &schedule,
            Some(&bad),
            Some(version(&pending)),
        )
        .await
        .assert_problem(400, "malformed-json");
    }
    let r = b
        .send(
            &api,
            Method::POST,
            &schedule,
            Some(&json!({"at": "2999-01-01T00:00:00Z"})),
            Some(version(&pending)),
        )
        .await;
    let scheduled = ok_item(&r);
    assert_eq!(scheduled["status"], "scheduled");
    assert_eq!(scheduled["scheduled_at"], "2999-01-01T00:00:00Z");
    let r = b
        .send(
            &api,
            Method::POST,
            &format!("{path}/publish"),
            None,
            Some(version(&scheduled)),
        )
        .await;
    let now = ok_item(&r);
    assert_eq!(now["status"], "published");
    assert_eq!(now["scheduled_at"], Value::Null);
    // Illegal transitions are 422; trash, then restore.
    b.send(
        &api,
        Method::POST,
        &format!("{path}/submit"),
        None,
        Some(version(&now)),
    )
    .await
    .assert_problem(422, "validation");
    let r = b
        .send(&api, Method::DELETE, &path, None, Some(version(&now)))
        .await;
    let trashed = ok_item(&r);
    let r = b
        .send(
            &api,
            Method::POST,
            &format!("{path}/restore"),
            None,
            Some(version(&trashed)),
        )
        .await;
    assert_eq!(ok_item(&r)["status"], "draft");
    api
}

async fn token_scope_limits_writes(api: Api) -> Api {
    let (ada, full) = admin(&api).await;
    let item = create(&api, &full, POSTS, json!({"title": "A draft"})).await;
    let path = item_path(POSTS, &item);

    // An admin's `content:read` token lists drafts but cannot write (Review Focus 5).
    let read = api.bearer(&ada, &["content:read"]).await;
    let r = read.get(&api, POSTS).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert_eq!(r.json()["items"][0]["status"], "draft");
    ok_item(&read.get(&api, &path).await);
    let p = read
        .send(
            &api,
            Method::POST,
            POSTS,
            Some(&json!({"title": "x"})),
            None,
        )
        .await
        .assert_problem(403, "forbidden");
    assert!(
        p["detail"].as_str().unwrap().contains("content:write"),
        "{p}"
    );
    for (method, p, body) in [
        (Method::PATCH, path.clone(), Some(json!({"title": "x"}))),
        (Method::POST, format!("{path}/publish"), None),
        (Method::DELETE, path.clone(), None),
    ] {
        let prob = read
            .send(&api, method, &p, body.as_ref(), Some(1))
            .await
            .assert_problem(403, "forbidden");
        assert!(
            prob["detail"].as_str().unwrap().contains("content:write"),
            "{prob}"
        );
    }
    // A token without any content scope reads nothing.
    let users = api.bearer(&ada, &["users:read"]).await;
    let p = users
        .get(&api, POSTS)
        .await
        .assert_problem(403, "forbidden");
    assert!(
        p["detail"].as_str().unwrap().contains("content:read"),
        "{p}"
    );
    users
        .get(&api, &path)
        .await
        .assert_problem(403, "forbidden");
    // Nothing was written.
    assert_eq!(ok_item(&full.get(&api, &path).await), item);

    // A contributor session creates its own draft but cannot publish it.
    let cora_user = api.user("cora", Role::Contributor, false).await;
    let mut cora = api.logged_in("cora").await;
    let r = cora
        .send(
            &api,
            Method::POST,
            POSTS,
            Some(&json!({"title": "Cora's"})),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{r:?}");
    let own = r.json();
    let r = cora
        .send(
            &api,
            Method::POST,
            &format!("{}/publish", item_path(POSTS, &own)),
            None,
            Some(1),
        )
        .await;
    let p = r.assert_problem(403, "forbidden");
    assert!(p["detail"].as_str().unwrap().contains("publish_own"), "{p}");
    // A contributor's full-scope token cannot publish either.
    let token = api.bearer(&cora_user, &["*"]).await;
    let p = token
        .send(
            &api,
            Method::POST,
            &format!("{}/publish", item_path(POSTS, &own)),
            None,
            Some(1),
        )
        .await
        .assert_problem(403, "forbidden");
    assert!(p["detail"].as_str().unwrap().contains("publish_own"), "{p}");
    api
}

async fn type_mismatch_is_404(api: Api) -> Api {
    let (_, b) = admin(&api).await;
    let page = create(&api, &b, PAGES, json!({"title": "A page"})).await;
    let id = page["id"].as_str().unwrap();
    let wrong = format!("{POSTS}/{id}");
    let rev = b.get(&api, &format!("{PAGES}/{id}/revisions")).await.json()["items"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();

    // The page through the post path: 404 for every endpoint, nothing written.
    let attempts: Vec<(Method, String, Option<Value>)> = vec![
        (Method::GET, wrong.clone(), None),
        (Method::PATCH, wrong.clone(), Some(json!({"title": "x"}))),
        (Method::DELETE, wrong.clone(), None),
        (Method::POST, format!("{wrong}/publish"), None),
        (Method::GET, format!("{wrong}/revisions"), None),
        (Method::GET, format!("{wrong}/revisions/{rev}"), None),
        (
            Method::POST,
            format!("{wrong}/revisions/{rev}/restore"),
            None,
        ),
    ];
    for (method, p, body) in attempts {
        b.send(&api, method.clone(), &p, body.as_ref(), Some(1))
            .await
            .assert_problem(404, "not-found");
    }
    assert_eq!(ok_item(&b.get(&api, &format!("{PAGES}/{id}")).await), page);

    // Unknown types, `media` (F21) and a malformed id: 404.
    for p in [
        "/api/v1/content/nope".to_owned(),
        "/api/v1/content/media".to_owned(),
        format!("/api/v1/content/nope/{id}"),
        format!("/api/v1/content/media/{id}"),
        format!("{POSTS}/not-a-uuid"),
        format!("{POSTS}/not-a-uuid/revisions"),
    ] {
        b.get(&api, &p).await.assert_problem(404, "not-found");
    }
    for p in ["/api/v1/content/nope", "/api/v1/content/media"] {
        b.send(&api, Method::POST, p, Some(&json!({"title": "x"})), None)
            .await
            .assert_problem(404, "not-found");
    }
    // Unauthenticated: 401 before the type is looked at.
    let r = api
        .send(
            axum::http::Request::builder()
                .uri("/api/v1/content/nope")
                .header(header::HOST, "example.com")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    r.assert_problem(401, "unauthenticated");
    api
}

async fn revisions_over_http(api: Api) -> Api {
    let (ada, b) = admin(&api).await;
    let item = create(
        &api,
        &b,
        POSTS,
        json!({"title": "Rev 1", "body": doc("first"), "fields": {"a": 1}}),
    )
    .await;
    let path = item_path(POSTS, &item);
    let mut v = 1;
    for title in ["Rev 2", "Rev 3"] {
        let r = b
            .send(
                &api,
                Method::PATCH,
                &path,
                Some(&json!({"title": title, "body": doc(title), "fields": {}})),
                Some(v),
            )
            .await;
        v = version(&ok_item(&r));
    }

    // Newest first, summaries.
    let r = b.get(&api, &format!("{path}/revisions")).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    let list = r.json();
    let items = list["items"].as_array().unwrap();
    let titles: Vec<&str> = items.iter().map(|i| i["title"].as_str().unwrap()).collect();
    assert_eq!(titles, ["Rev 3", "Rev 2", "Rev 1"]);
    assert_eq!(list["next_cursor"], Value::Null);
    assert!(items[0].get("body").is_none(), "{list}");
    assert_eq!(items[0]["author_id"], ada.id.to_string());
    assert_eq!(items[0]["note"], Value::Null);
    let first = items[2]["id"].as_str().unwrap().to_owned();
    let r = b.get(&api, &format!("{path}/revisions?limit=1")).await;
    assert_eq!(r.json()["items"].as_array().unwrap().len(), 1);
    for bad in ["limit=0", "limit=51"] {
        let p = b
            .get(&api, &format!("{path}/revisions?{bad}"))
            .await
            .assert_problem(400, "validation");
        assert_eq!(p["errors"][0]["field"], "limit");
    }

    // One revision in full.
    let r = b.get(&api, &format!("{path}/revisions/{first}")).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    let rev = r.json();
    assert_eq!(rev["id"], first.as_str());
    assert_eq!(rev["title"], "Rev 1");
    assert_eq!(rev["body"], doc("first"));
    assert_eq!(rev["excerpt"], "first");
    assert_eq!(rev["fields"], json!({"a": 1}));
    assert!(rev["created_at"].as_str().unwrap().ends_with('Z'));
    assert!(rev.get("meta").is_none(), "{rev}");

    // Restore it: title, body and fields back, version +1, a noted revision.
    let r = b
        .send(
            &api,
            Method::POST,
            &format!("{path}/revisions/{first}/restore"),
            None,
            Some(v),
        )
        .await;
    let restored = ok_item(&r);
    assert_eq!(version(&restored), v + 1);
    assert_eq!(restored["title"], "Rev 1");
    assert_eq!(restored["body"], doc("first"));
    assert_eq!(restored["fields"], json!({"a": 1}));
    let list = b.get(&api, &format!("{path}/revisions")).await.json();
    assert_eq!(
        list["items"][0]["note"],
        format!("restored from {first}").as_str()
    );

    // Another item's revision, or an unknown one: 404.
    let other = create(&api, &b, POSTS, json!({"title": "Other"})).await;
    let other_path = item_path(POSTS, &other);
    for p in [
        format!("{other_path}/revisions/{first}"),
        format!("{path}/revisions/{}", uuid::Uuid::now_v7()),
        format!("{path}/revisions/not-a-uuid"),
    ] {
        b.get(&api, &p).await.assert_problem(404, "not-found");
    }
    b.send(
        &api,
        Method::POST,
        &format!("{other_path}/revisions/{first}/restore"),
        None,
        Some(1),
    )
    .await
    .assert_problem(404, "not-found");
    api
}

/// The first `ApiJson` resource endpoint: 413, 415 and 400 problems through
/// `build_site` carry `no-store` (Task 6 carry).
async fn json_problems_are_no_store(api: Api) -> Api {
    let (_, b) = admin(&api).await;
    let big = format!(r#"{{"title": "{}"}}"#, "a".repeat(5 * 1024 * 1024));
    let req = b
        .builder(Method::POST, POSTS)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(big))
        .unwrap();
    api.send(req).await.assert_problem(413, "too-large");
    let req = b
        .builder(Method::POST, POSTS)
        .header(header::CONTENT_TYPE, "text/plain")
        .body(Body::from(r#"{"title": "x"}"#))
        .unwrap();
    api.send(req)
        .await
        .assert_problem(415, "unsupported-media-type");
    let req = b
        .builder(Method::POST, POSTS)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from("{"))
        .unwrap();
    api.send(req).await.assert_problem(400, "malformed-json");
    let p = b
        .send(&api, Method::POST, POSTS, Some(&json!({"body": {}})), None)
        .await
        .assert_problem(400, "malformed-json");
    assert!(p["detail"].is_string());
    // Nothing was created.
    assert_eq!(b.get(&api, POSTS).await.json()["items"], json!([]));
    api
}

/// Marks item `id` private, bypassing the lifecycle (no transition sets it in M2a-1).
async fn make_private(api: &Api, id: &str) {
    let id: ferrite_domain::ContentId = id.parse().unwrap();
    let mut item = api.store.read().content().get(id).await.unwrap().unwrap();
    item.status = ferrite_domain::ContentStatus::Private;
    api.store.read().content().update(&item).await.unwrap();
}

/// R17: a contributor (or author) sees published items and its own, never others'
/// draft, pending, scheduled, private or trashed items or their revisions; an editor sees
/// everything. Hidden items are 404, not 403.
async fn others_unpublished_items_are_hidden(api: Api) -> Api {
    let ed = api.user("ed", Role::Editor, false).await;
    let eb = api.bearer(&ed, &["*"]).await;
    let cora = api.user("cora", Role::Contributor, false).await;
    let cb = api.bearer(&cora, &["*"]).await;
    let ann = api.user("ann", Role::Author, false).await;
    let ab = api.bearer(&ann, &["*"]).await;

    let body = |t: &str| json!({"title": t, "body": doc("secret narwhal plans")});
    let draft = create(&api, &eb, POSTS, body("Ed draft")).await;
    let pending = create(&api, &eb, POSTS, body("Ed pending")).await;
    let r = eb
        .send(
            &api,
            Method::POST,
            &format!("{}/submit", item_path(POSTS, &pending)),
            None,
            Some(1),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    let private = create(&api, &eb, POSTS, body("Ed private")).await;
    make_private(&api, private["id"].as_str().unwrap()).await;
    let scheduled = create(&api, &eb, POSTS, body("Ed scheduled")).await;
    let r = eb
        .send(
            &api,
            Method::POST,
            &format!("{}/schedule", item_path(POSTS, &scheduled)),
            Some(&json!({"at": "2999-01-01T00:00:00Z"})),
            Some(1),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    let live = create(&api, &eb, POSTS, body("Ed live")).await;
    let r = eb
        .send(
            &api,
            Method::POST,
            &format!("{}/publish", item_path(POSTS, &live)),
            None,
            Some(1),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    let own = create(&api, &cb, POSTS, body("Cora own")).await;
    let hidden = [&draft, &pending, &private, &scheduled];

    let ids = |r: &Resp| -> Vec<String> {
        assert_eq!(r.status, StatusCode::OK, "{r:?}");
        let mut v: Vec<String> = r.json()["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["id"].as_str().unwrap().to_owned())
            .collect();
        v.sort();
        v
    };
    let id = |v: &Value| v["id"].as_str().unwrap().to_owned();
    let sorted = |mut v: Vec<String>| {
        v.sort();
        v
    };

    for (who, b) in [("contributor", &cb), ("author", &ab)] {
        for item in hidden {
            let path = item_path(POSTS, item);
            b.get(&api, &path).await.assert_problem(404, "not-found");
            b.get(&api, &format!("{path}/revisions"))
                .await
                .assert_problem(404, "not-found");
            let rev = eb.get(&api, &format!("{path}/revisions")).await.json()["items"][0]["id"]
                .as_str()
                .unwrap()
                .to_owned();
            b.get(&api, &format!("{path}/revisions/{rev}"))
                .await
                .assert_problem(404, "not-found");
        }
        let mine = if who == "contributor" {
            vec![id(&own)]
        } else {
            vec![]
        };
        let mut visible = mine.clone();
        visible.push(id(&live));
        assert_eq!(
            ids(&b.get(&api, POSTS).await),
            sorted(visible.clone()),
            "{who}"
        );
        assert_eq!(
            ids(&b
                .get(
                    &api,
                    &format!("{POSTS}?status=draft,pending,private,scheduled,trash")
                )
                .await),
            sorted(mine.clone()),
            "{who}"
        );
        assert_eq!(
            ids(&b.get(&api, &format!("{POSTS}?author={}", ed.id)).await),
            [id(&live)],
            "{who}"
        );
        assert_eq!(
            ids(&b.get(&api, &format!("{POSTS}?q=narwhal")).await),
            sorted(visible),
            "{who}"
        );
        assert_eq!(
            ids(&b
                .get(&api, &format!("{POSTS}?q=narwhal&status=private"))
                .await),
            Vec::<String>::new(),
            "{who}"
        );
        // Writes to a hidden item are 404 too.
        let p = item_path(POSTS, &draft);
        b.send(
            &api,
            Method::PATCH,
            &p,
            Some(&json!({"title": "x"})),
            Some(1),
        )
        .await
        .assert_problem(404, "not-found");
    }
    // The contributor reads its own item and the published one.
    ok_item(&cb.get(&api, &item_path(POSTS, &own)).await);
    ok_item(&cb.get(&api, &item_path(POSTS, &live)).await);

    // An editor reads everything, in every mode.
    let ed2 = api.user("ed2", Role::Editor, false).await;
    let e2 = api.bearer(&ed2, &["content:read"]).await;
    for item in hidden {
        ok_item(&e2.get(&api, &item_path(POSTS, item)).await);
        let r = e2
            .get(&api, &format!("{}/revisions", item_path(POSTS, item)))
            .await;
        assert_eq!(r.status, StatusCode::OK, "{r:?}");
    }
    let mut all: Vec<String> = hidden.iter().map(|i| id(i)).collect();
    all.push(id(&live));
    all.push(id(&own));
    assert_eq!(ids(&e2.get(&api, POSTS).await), sorted(all.clone()));
    assert_eq!(
        ids(&e2.get(&api, &format!("{POSTS}?q=narwhal")).await),
        sorted(all)
    );
    api
}

/// Writes to others' items (readable because published) and the contributor limits on
/// own items are 403 naming the capability; nothing is written (Review Focus).
async fn others_items_are_403(api: Api) -> Api {
    let ed = api.user("ed", Role::Editor, false).await;
    let eb = api.bearer(&ed, &["*"]).await;
    let amy = api.user("amy", Role::Author, false).await;
    let amy_b = api.bearer(&amy, &["*"]).await;
    let bob = api.user("bob", Role::Author, false).await;
    let bob_b = api.bearer(&bob, &["*"]).await;

    let item = create(&api, &amy_b, POSTS, json!({"title": "Amy's"})).await;
    let path = item_path(POSTS, &item);
    let r = amy_b
        .send(
            &api,
            Method::POST,
            &format!("{path}/publish"),
            None,
            Some(1),
        )
        .await;
    let live = ok_item(&r);
    let v = version(&live);
    let rev = amy_b.get(&api, &format!("{path}/revisions")).await.json()["items"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    for (method, p, body, cap) in [
        (
            Method::PATCH,
            path.clone(),
            Some(json!({"title": "Bob's now"})),
            "edit_others",
        ),
        (
            Method::POST,
            format!("{path}/unpublish"),
            None,
            "publish_others",
        ),
        (Method::DELETE, path.clone(), None, "edit_others"),
        (
            Method::POST,
            format!("{path}/revisions/{rev}/restore"),
            None,
            "edit_others",
        ),
    ] {
        let prob = bob_b
            .send(&api, method.clone(), &p, body.as_ref(), Some(v))
            .await
            .assert_problem(403, "forbidden");
        assert!(
            prob["detail"].as_str().unwrap().contains(cap),
            "{method} {p}: {prob}"
        );
        // A stale version does not change the answer: permission comes first.
        let prob = bob_b
            .send(&api, method.clone(), &p, body.as_ref(), Some(v - 1))
            .await
            .assert_problem(403, "forbidden");
        assert!(
            prob["detail"].as_str().unwrap().contains(cap),
            "{method} {p}: {prob}"
        );
    }
    assert_eq!(ok_item(&amy_b.get(&api, &path).await), live);
    // A read-only token: 403 before the version, also on PATCH.
    let ro = api.bearer(&amy, &["content:read"]).await;
    let prob = ro
        .send(
            &api,
            Method::PATCH,
            &path,
            Some(&json!({"title": "x"})),
            Some(v - 1),
        )
        .await
        .assert_problem(403, "forbidden");
    assert!(
        prob["detail"].as_str().unwrap().contains("content:write"),
        "{prob}"
    );

    // Contributor: may trash its own draft, but not restore or delete it (R8), and may
    // not edit its own published item.
    let cora = api.user("cora", Role::Contributor, false).await;
    let cb = api.bearer(&cora, &["*"]).await;
    let own = create(&api, &cb, POSTS, json!({"title": "Cora's"})).await;
    let own_path = item_path(POSTS, &own);
    let trashed = ok_item(
        &cb.send(&api, Method::DELETE, &own_path, None, Some(1))
            .await,
    );
    let tv = version(&trashed);
    for (method, p) in [
        (Method::POST, format!("{own_path}/restore")),
        (Method::DELETE, format!("{own_path}?force=true")),
    ] {
        let prob = cb
            .send(&api, method.clone(), &p, None, Some(tv))
            .await
            .assert_problem(403, "forbidden");
        assert!(
            prob["detail"].as_str().unwrap().contains("edit_own"),
            "{method} {p}: {prob}"
        );
    }
    assert_eq!(ok_item(&cb.get(&api, &own_path).await), trashed);
    let own2 = create(&api, &cb, POSTS, json!({"title": "Cora's live"})).await;
    let own2_path = item_path(POSTS, &own2);
    let live2 = ok_item(
        &eb.send(
            &api,
            Method::POST,
            &format!("{own2_path}/publish"),
            None,
            Some(1),
        )
        .await,
    );
    let prob = cb
        .send(
            &api,
            Method::PATCH,
            &own2_path,
            Some(&json!({"title": "Changed"})),
            Some(version(&live2)),
        )
        .await
        .assert_problem(403, "forbidden");
    assert!(
        prob["detail"].as_str().unwrap().contains("edit_own"),
        "{prob}"
    );
    assert_eq!(ok_item(&cb.get(&api, &own2_path).await), live2);
    api
}
