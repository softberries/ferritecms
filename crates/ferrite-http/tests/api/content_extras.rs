//! M2b-1 Task 2: `author` and `allowed` on content and revision bodies, and
//! `GET /content/{type}/counts` (P13, P14).

use axum::http::{Method, StatusCode};
use ferrite_domain::Role;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{api_tests, harness::Api};

api_tests!(
    content_dto_has_author_and_allowed,
    list_items_have_author_and_allowed,
    counts_endpoint_shape_and_routing,
    revisions_carry_author,
);

const POSTS: &str = "/api/v1/content/post";

fn author(id: Uuid, login: &str) -> Value {
    json!({"id": id, "login": login, "display_name": login})
}

fn allowed(names: &[&str]) -> Value {
    let all = [
        "edit",
        "publish",
        "unpublish",
        "schedule",
        "submit",
        "trash",
        "restore",
        "delete",
        "trash_undoable",
    ];
    all.iter()
        .map(|n| ((*n).to_owned(), Value::Bool(names.contains(n))))
        .collect::<serde_json::Map<_, _>>()
        .into()
}

async fn content_dto_has_author_and_allowed(api: Api) -> Api {
    let ann = api.user("ann", Role::Author, false).await;
    let carl = api.user("carl", Role::Contributor, false).await;
    let eve = api.user("eve", Role::Editor, false).await;
    let (a, c, e) = (
        api.bearer(&ann, &["*"]).await,
        api.bearer(&carl, &["*"]).await,
        api.bearer(&eve, &["*"]).await,
    );

    // The author's own draft.
    let r = a
        .send(
            &api,
            Method::POST,
            POSTS,
            Some(&json!({"title": "Mine"})),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{r:?}");
    let item = r.json();
    assert_eq!(item["author"], author(ann.id.as_uuid(), "ann"));
    assert_eq!(
        item["allowed"],
        allowed(&[
            "edit",
            "publish",
            "schedule",
            "submit",
            "trash",
            "trash_undoable"
        ])
    );
    let path = format!("{POSTS}/{}", item["id"].as_str().unwrap());

    // The editor reads it with the same author and its own (full) rights.
    let got = e.get(&api, &path).await.json();
    assert_eq!(got["author"], author(ann.id.as_uuid(), "ann"));
    assert_eq!(
        got["allowed"],
        allowed(&[
            "edit",
            "publish",
            "schedule",
            "submit",
            "trash",
            "trash_undoable"
        ])
    );

    // Published: the contributor may read it but do nothing.
    let r = a
        .send(
            &api,
            Method::POST,
            &format!("{path}/publish"),
            None,
            Some(1),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert_eq!(
        r.json()["allowed"],
        allowed(&["edit", "unpublish", "trash", "trash_undoable"])
    );
    let got = c.get(&api, &path).await.json();
    assert_eq!(got["author"], author(ann.id.as_uuid(), "ann"));
    assert_eq!(got["allowed"], allowed(&[]));

    // A contributor's own draft: edit, submit and trash, never publish; the trash cannot
    // be undone (contributors never restore).
    let r = c
        .send(
            &api,
            Method::POST,
            POSTS,
            Some(&json!({"title": "Carl's"})),
            None,
        )
        .await;
    assert_eq!(r.json()["allowed"], allowed(&["edit", "submit", "trash"]));

    // Trashed (own, author): restore and delete only; the 412 `current` carries both.
    let r = a.send(&api, Method::DELETE, &path, None, Some(2)).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert_eq!(r.json()["allowed"], allowed(&["restore", "delete"]));
    let prob = a
        .send(
            &api,
            Method::POST,
            &format!("{path}/restore"),
            None,
            Some(1),
        )
        .await
        .assert_problem(412, "precondition-failed");
    assert_eq!(prob["current"]["author"], author(ann.id.as_uuid(), "ann"));
    assert_eq!(prob["current"]["allowed"], allowed(&["restore", "delete"]));

    // A read-only token: nothing allowed.
    let ro = api.bearer(&eve, &["content:read"]).await;
    assert_eq!(ro.get(&api, &path).await.json()["allowed"], allowed(&[]));
    api
}

async fn list_items_have_author_and_allowed(api: Api) -> Api {
    let ann = api.user("ann", Role::Author, false).await;
    let bob = api.user("bob", Role::Author, false).await;
    let (a, b) = (
        api.bearer(&ann, &["*"]).await,
        api.bearer(&bob, &["*"]).await,
    );
    for (t, title) in [(&a, "By ann"), (&b, "By bob")] {
        let r = t
            .send(
                &api,
                Method::POST,
                POSTS,
                Some(&json!({"title": title})),
                None,
            )
            .await;
        assert_eq!(r.status, StatusCode::CREATED, "{r:?}");
        let id = r.json()["id"].as_str().unwrap().to_owned();
        let r = t
            .send(
                &api,
                Method::POST,
                &format!("{POSTS}/{id}/publish"),
                None,
                Some(1),
            )
            .await;
        assert_eq!(r.status, StatusCode::OK, "{r:?}");
    }
    let r = a.get(&api, &format!("{POSTS}?order=title")).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    let items = r.json()["items"].as_array().unwrap().clone();
    assert_eq!(items.len(), 2, "{items:?}");
    assert_eq!(items[0]["title"], "By ann");
    assert_eq!(items[0]["author"], author(ann.id.as_uuid(), "ann"));
    assert_eq!(
        items[0]["allowed"],
        allowed(&["edit", "unpublish", "trash", "trash_undoable"])
    );
    assert_eq!(items[1]["title"], "By bob");
    assert_eq!(items[1]["author"], author(bob.id.as_uuid(), "bob"));
    assert_eq!(items[1]["allowed"], allowed(&[]));
    api
}

async fn counts_endpoint_shape_and_routing(api: Api) -> Api {
    let ann = api.user("ann", Role::Author, false).await;
    let bob = api.user("bob", Role::Author, false).await;
    let eve = api.user("eve", Role::Editor, false).await;
    let (a, b, e) = (
        api.bearer(&ann, &["*"]).await,
        api.bearer(&bob, &["*"]).await,
        api.bearer(&eve, &["*"]).await,
    );
    let mut first = String::new();
    for (t, title) in [(&a, "A1"), (&a, "A2"), (&b, "B1")] {
        let r = t
            .send(
                &api,
                Method::POST,
                POSTS,
                Some(&json!({"title": title})),
                None,
            )
            .await;
        assert_eq!(r.status, StatusCode::CREATED, "{r:?}");
        if first.is_empty() {
            first = r.json()["id"].as_str().unwrap().to_owned();
        }
    }
    let r = a
        .send(
            &api,
            Method::DELETE,
            &format!("{POSTS}/{first}"),
            None,
            Some(1),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");

    let counts = |draft: u64, trash: u64| json!({"draft": draft, "pending": 0, "scheduled": 0, "published": 0, "private": 0, "trash": trash});
    let r = a.get(&api, &format!("{POSTS}/counts")).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert_eq!(r.json(), counts(1, 1));
    assert_eq!(
        e.get(&api, &format!("{POSTS}/counts")).await.json(),
        counts(2, 1)
    );
    // `{id}` still reaches the item; a near miss of `counts` is an id (malformed: 404).
    let r = a.get(&api, &format!("{POSTS}/{first}")).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert_eq!(r.json()["id"], first.as_str());
    a.get(&api, &format!("{POSTS}/countsx"))
        .await
        .assert_problem(404, "not-found");
    a.get(&api, "/api/v1/content/nope/counts")
        .await
        .assert_problem(404, "not-found");
    a.get(&api, "/api/v1/content/media/counts")
        .await
        .assert_problem(404, "not-found");
    let no_scope = api.bearer(&eve, &["users:read"]).await;
    no_scope
        .get(&api, &format!("{POSTS}/counts"))
        .await
        .assert_problem(403, "forbidden");
    api.get(&format!("{POSTS}/counts"))
        .await
        .assert_problem(401, "unauthenticated");
    api
}

async fn revisions_carry_author(api: Api) -> Api {
    let ann = api.user("ann", Role::Author, false).await;
    let eve = api.user("eve", Role::Editor, false).await;
    let (a, e) = (
        api.bearer(&ann, &["*"]).await,
        api.bearer(&eve, &["*"]).await,
    );
    let r = a
        .send(
            &api,
            Method::POST,
            POSTS,
            Some(&json!({"title": "Rev"})),
            None,
        )
        .await;
    let path = format!("{POSTS}/{}", r.json()["id"].as_str().unwrap());
    let r = e
        .send(
            &api,
            Method::PATCH,
            &path,
            Some(&json!({"title": "Rev by eve"})),
            Some(1),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    let r = e.get(&api, &format!("{path}/revisions")).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    let revs = r.json()["items"].as_array().unwrap().clone();
    assert_eq!(revs.len(), 2, "{revs:?}");
    assert_eq!(revs[0]["author"], author(eve.id.as_uuid(), "eve"));
    assert_eq!(revs[0]["author_id"], eve.id.to_string());
    assert_eq!(revs[1]["author"], author(ann.id.as_uuid(), "ann"));
    let rid = revs[1]["id"].as_str().unwrap();
    let r = a.get(&api, &format!("{path}/revisions/{rid}")).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert_eq!(r.json()["author"], author(ann.id.as_uuid(), "ann"));
    api
}
