//! Task 9: content types, taxonomies and terms under `/api/v1/types`,
//! `/api/v1/taxonomies` and `/api/v1/terms` (P11, P18).

use axum::http::{Method, StatusCode};
use ferrite_core::{PermalinkRules, events::TYPES_CHANGED};
use ferrite_domain::{Role, User};
use ferrite_http::load_route_table;
use serde_json::{Value, json};

use crate::{
    api_tests,
    harness::{Api, Bearer},
};

api_tests!(
    types_crud_over_http,
    terms_crud_over_http,
    editor_cannot_create_types
);

const TYPES: &str = "/api/v1/types";
const TAXONOMIES: &str = "/api/v1/taxonomies";

async fn admin(api: &Api) -> (User, Bearer) {
    let ada = api.user("ada", Role::Admin, false).await;
    let token = api.bearer(&ada, &["*"]).await;
    (ada, token)
}

fn names(list: &Value) -> Vec<String> {
    list["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_owned())
        .collect()
}

async fn types_crud_over_http(api: Api) -> Api {
    let (_, b) = admin(&api).await;
    let r = b.get(&api, TYPES).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert_eq!(r.header("cache-control"), Some("no-store"));
    let list = r.json();
    assert_eq!(names(&list), ["media", "page", "post"]);
    assert_eq!(list["next_cursor"], Value::Null);
    let post = &list["items"][2];
    assert_eq!(post["built_in"], true);
    assert_eq!(post["fields"], json!([]));

    // Create with a schema: 201, Location, the DTO; a page in the public cache is
    // flushed once the event is applied.
    let mut poller = api.poller().await;
    assert_eq!(api.get("/").await.status, StatusCode::OK);
    assert!(api.app.cache.len().await >= 1, "home is cached");
    let before = api.store.read().events().max_seq().await.unwrap();
    let body = json!({
        "name": "event", "label": "Events", "has_archive": true,
        "fields": [
            {"name": "venue", "label": "Venue", "kind": "text", "required": true},
            {"name": "kind", "kind": "select", "options": ["talk", "workshop"]},
            {"name": "slots", "kind": "repeater", "fields": [{"name": "at", "kind": "date"}]}
        ]
    });
    let r = b.send(&api, Method::POST, TYPES, Some(&body), None).await;
    assert_eq!(r.status, StatusCode::CREATED, "{r:?}");
    assert_eq!(r.header("location"), Some("/api/v1/types/event"));
    let created = r.json();
    assert_eq!(created["name"], "event");
    assert_eq!(created["built_in"], false);
    assert_eq!(created["hierarchical"], false);
    assert_eq!(created["fields"][0]["kind"], "text");
    assert_eq!(created["fields"][0]["required"], true);
    assert_eq!(created["fields"][1]["options"], json!(["talk", "workshop"]));
    assert_eq!(created["fields"][2]["fields"][0]["name"], "at");
    let kinds: Vec<String> = api
        .store
        .read()
        .events()
        .since(before, 10)
        .await
        .unwrap()
        .into_iter()
        .map(|e| e.kind)
        .collect();
    assert_eq!(kinds, [TYPES_CHANGED, "invalidate"]);
    poller.poll_once().await.unwrap();
    assert!(
        api.app.cache.is_empty().await,
        "type change flushes the cache"
    );
    let r = b.get(&api, "/api/v1/types/event").await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json(), created);

    // Validation problems name the member.
    for (body, field) in [
        (json!({"name": "Bad Name", "label": "x"}), "name"),
        (json!({"name": "author", "label": "x"}), "name"),
        (json!({"name": "event", "label": "again"}), "name"),
        (json!({"name": "news", "label": " "}), "label"),
        (
            json!({"name": "news", "label": "N", "fields": [{"name": "s", "kind": "select"}]}),
            "fields",
        ),
    ] {
        let r = b.send(&api, Method::POST, TYPES, Some(&body), None).await;
        let p = r.assert_problem(422, "validation");
        assert_eq!(p["errors"][0]["field"], field, "{body}: {p}");
    }
    let r = b
        .send(&api, Method::POST, TYPES, Some(&json!({"name": "x"})), None)
        .await;
    r.assert_problem(400, "malformed-json");

    // Items of the type are validated against its schema.
    let items = "/api/v1/content/event";
    let r = b
        .send(
            &api,
            Method::POST,
            items,
            Some(&json!({"title": "No venue"})),
            None,
        )
        .await;
    let p = r.assert_problem(422, "validation");
    assert_eq!(p["errors"][0]["field"], "fields");
    assert_eq!(p["detail"], "fields.venue: required");
    let r = b
        .send(
            &api,
            Method::POST,
            items,
            Some(&json!({"title": "RustConf", "slug": "rustconf", "fields": {"venue": "Hall A", "kind": "talk"}})),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{r:?}");
    let item = r.json();
    assert_eq!(item["permalink"], "/event/rustconf/");

    // The type's permalink is served on the public site once the node rebuilt its route
    // table (the reloader does this on `types.changed`; here the harness swaps it).
    let path = format!("{items}/{}", item["id"].as_str().unwrap());
    let r = b
        .send(
            &api,
            Method::POST,
            &format!("{path}/publish"),
            None,
            Some(1),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    let table = load_route_table(api.store.as_ref(), &PermalinkRules::default())
        .await
        .unwrap();
    api.app.swap_routes(table);
    poller.poll_once().await.unwrap();
    let page = api.get("/event/rustconf/").await;
    assert_eq!(page.status, StatusCode::OK, "{page:?}");
    assert!(page.body.contains("RustConf"));
    assert_eq!(api.get("/event/").await.status, StatusCode::OK);

    // PATCH: label and has_archive change, null is refused, unknown name is 404.
    let r = b
        .send(
            &api,
            Method::PATCH,
            "/api/v1/types/event",
            Some(&json!({"label": "Meetups", "has_archive": false, "hierarchical": true})),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    let patched = r.json();
    assert_eq!(patched["label"], "Meetups");
    assert_eq!(patched["has_archive"], false);
    assert_eq!(patched["hierarchical"], false, "hierarchical cannot change");
    assert_eq!(patched["fields"], created["fields"]);
    let r = b
        .send(
            &api,
            Method::PATCH,
            "/api/v1/types/event",
            Some(&json!({"label": null})),
            None,
        )
        .await;
    r.assert_problem(400, "malformed-json");
    let r = b
        .send(
            &api,
            Method::PATCH,
            "/api/v1/types/nope",
            Some(&json!({})),
            None,
        )
        .await;
    r.assert_problem(404, "not-found");

    // DELETE: built in and with content are 422; an empty type is 204, then 404.
    let r = b
        .send(&api, Method::DELETE, "/api/v1/types/post", None, None)
        .await;
    assert_eq!(
        r.assert_problem(422, "validation")["detail"],
        "built-in type"
    );
    let r = b
        .send(&api, Method::DELETE, "/api/v1/types/event", None, None)
        .await;
    assert_eq!(
        r.assert_problem(422, "validation")["detail"],
        "type has content"
    );
    let r = b
        .send(
            &api,
            Method::POST,
            TYPES,
            Some(&json!({"name": "draft-type", "label": "Temp"})),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{r:?}");
    let r = b
        .send(&api, Method::DELETE, "/api/v1/types/draft-type", None, None)
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT, "{r:?}");
    let r = b.get(&api, "/api/v1/types/draft-type").await;
    r.assert_problem(404, "not-found");

    // The type check of /content/{type} now goes through the type service.
    let r = b.get(&api, "/api/v1/content/draft-type").await;
    r.assert_problem(404, "not-found");
    api
}

async fn terms_crud_over_http(api: Api) -> Api {
    let (_, b) = admin(&api).await;
    // Register a hierarchical taxonomy for posts.
    let r = b
        .send(
            &api,
            Method::POST,
            TAXONOMIES,
            Some(&json!({"name": "genre", "label": "Genres", "hierarchical": true, "content_types": ["post"]})),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{r:?}");
    assert_eq!(r.header("location"), Some("/api/v1/taxonomies/genre"));
    assert_eq!(
        r.json(),
        json!({"name": "genre", "label": "Genres", "hierarchical": true, "content_types": ["post"], "built_in": false})
    );
    let r = b.get(&api, TAXONOMIES).await;
    assert_eq!(names(&r.json()), ["category", "genre", "tag"]);
    let r = b
        .send(
            &api,
            Method::POST,
            TAXONOMIES,
            Some(&json!({"name": "topic", "label": "T", "content_types": ["nope"]})),
            None,
        )
        .await;
    let p = r.assert_problem(422, "validation");
    assert_eq!(p["errors"][0]["field"], "content_types");
    let r = b
        .send(
            &api,
            Method::PATCH,
            "/api/v1/taxonomies/genre",
            Some(&json!({"label": "Kinds"})),
            None,
        )
        .await;
    assert_eq!(r.json()["label"], "Kinds");
    assert_eq!(r.json()["content_types"], json!(["post"]));

    // Terms: create (201 + Location), list, get.
    let terms = "/api/v1/taxonomies/genre/terms";
    let r = b
        .send(
            &api,
            Method::POST,
            terms,
            Some(&json!({"name": "Jazz"})),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{r:?}");
    let jazz = r.json();
    let jazz_path = format!("/api/v1/terms/{}", jazz["id"].as_str().unwrap());
    assert_eq!(r.header("location"), Some(jazz_path.as_str()));
    assert_eq!(jazz["slug"], "jazz");
    assert_eq!(jazz["taxonomy"], "genre");
    assert_eq!(jazz["description"], "");
    assert_eq!(jazz["parent_id"], Value::Null);
    let r = b
        .send(
            &api,
            Method::POST,
            terms,
            Some(&json!({"name": "Bebop", "slug": "jazz", "parent_id": jazz["id"], "description": "fast"})),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{r:?}");
    let bebop = r.json();
    assert_eq!(bebop["slug"], "jazz-2", "a taken slug gets -2");
    assert_eq!(bebop["parent_id"], jazz["id"]);
    let bebop_path = format!("/api/v1/terms/{}", bebop["id"].as_str().unwrap());
    let r = b.get(&api, terms).await;
    assert_eq!(r.json()["items"].as_array().unwrap().len(), 2);
    assert_eq!(b.get(&api, &jazz_path).await.json(), jazz);
    b.get(&api, "/api/v1/taxonomies/nope/terms")
        .await
        .assert_problem(404, "not-found");
    b.send(
        &api,
        Method::POST,
        "/api/v1/taxonomies/nope/terms",
        Some(&json!({"name": "x"})),
        None,
    )
    .await
    .assert_problem(404, "not-found");
    b.get(&api, "/api/v1/terms/not-a-uuid")
        .await
        .assert_problem(404, "not-found");
    let r = b
        .send(
            &api,
            Method::POST,
            terms,
            Some(&json!({"name": "x", "slug": "a/b"})),
            None,
        )
        .await;
    assert_eq!(
        r.assert_problem(422, "validation")["errors"][0]["field"],
        "slug"
    );

    // PATCH merges: absent members keep, `parent_id: null` moves to the top level.
    let r = b
        .send(
            &api,
            Method::PATCH,
            &bebop_path,
            Some(&json!({"name": "Be-bop"})),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    let renamed = r.json();
    assert_eq!(renamed["name"], "Be-bop");
    assert_eq!(renamed["slug"], "jazz-2");
    assert_eq!(renamed["description"], "fast");
    assert_eq!(renamed["parent_id"], jazz["id"]);
    let r = b
        .send(
            &api,
            Method::PATCH,
            &bebop_path,
            Some(&json!({"parent_id": null, "slug": "bebop"})),
            None,
        )
        .await;
    let moved = r.json();
    assert_eq!(moved["parent_id"], Value::Null);
    assert_eq!(moved["slug"], "bebop");
    assert_eq!(moved["name"], "Be-bop");
    b.send(
        &api,
        Method::PATCH,
        &bebop_path,
        Some(&json!({"name": null})),
        None,
    )
    .await
    .assert_problem(400, "malformed-json");

    // DELETE: a term with children is 422; leaves first, then the taxonomy.
    let r = b
        .send(
            &api,
            Method::PATCH,
            &bebop_path,
            Some(&json!({"parent_id": jazz["id"]})),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    let r = b.send(&api, Method::DELETE, &jazz_path, None, None).await;
    assert_eq!(
        r.assert_problem(422, "validation")["detail"],
        "has children"
    );
    let r = b
        .send(&api, Method::DELETE, "/api/v1/taxonomies/genre", None, None)
        .await;
    assert_eq!(
        r.assert_problem(422, "validation")["detail"],
        "taxonomy has terms"
    );
    for path in [&bebop_path, &jazz_path] {
        let r = b.send(&api, Method::DELETE, path, None, None).await;
        assert_eq!(r.status, StatusCode::NO_CONTENT, "{r:?}");
    }
    b.get(&api, &jazz_path)
        .await
        .assert_problem(404, "not-found");
    let r = b
        .send(
            &api,
            Method::DELETE,
            "/api/v1/taxonomies/category",
            None,
            None,
        )
        .await;
    assert_eq!(
        r.assert_problem(422, "validation")["detail"],
        "built-in taxonomy"
    );
    let r = b
        .send(&api, Method::DELETE, "/api/v1/taxonomies/genre", None, None)
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT, "{r:?}");
    b.get(&api, "/api/v1/taxonomies/genre")
        .await
        .assert_problem(404, "not-found");

    // A terms token reads and writes terms, but not taxonomies or types.
    let ada = api.user("tia", Role::Editor, false).await;
    let t = api.bearer(&ada, &["terms:write"]).await;
    let r = t
        .send(
            &api,
            Method::POST,
            "/api/v1/taxonomies/category/terms",
            Some(&json!({"name": "News"})),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{r:?}");
    assert_eq!(
        t.get(&api, "/api/v1/taxonomies/category/terms")
            .await
            .status,
        StatusCode::OK
    );
    let p = t
        .get(&api, TAXONOMIES)
        .await
        .assert_problem(403, "forbidden");
    assert!(
        p["detail"].as_str().unwrap().contains("content:read"),
        "{p}"
    );
    let c = api.bearer(&ada, &["content:read"]).await;
    let p = c
        .get(&api, "/api/v1/taxonomies/category/terms")
        .await
        .assert_problem(403, "forbidden");
    assert!(p["detail"].as_str().unwrap().contains("terms:read"), "{p}");
    api
}

async fn editor_cannot_create_types(api: Api) -> Api {
    let (ada, _) = admin(&api).await;
    api.user("eve", Role::Editor, false).await;
    let mut eve = api.logged_in("eve").await;
    let body = json!({"name": "event", "label": "Events"});
    for (method, path, body) in [
        (Method::POST, TYPES, Some(&body)),
        (
            Method::PATCH,
            "/api/v1/types/post",
            Some(&json!({"label": "x"})),
        ),
        (Method::DELETE, "/api/v1/types/post", None),
        (Method::POST, TAXONOMIES, Some(&body)),
        (
            Method::PATCH,
            "/api/v1/taxonomies/tag",
            Some(&json!({"label": "x"})),
        ),
        (Method::DELETE, "/api/v1/taxonomies/tag", None),
    ] {
        let r = eve.send(&api, method.clone(), path, body, None).await;
        let p = r.assert_problem(403, "forbidden");
        assert!(
            p["detail"].as_str().unwrap().contains("manage_settings"),
            "{method} {path}: {p}"
        );
    }
    // Editors read types and manage terms.
    let r = eve.get(&api, TYPES).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    let r = eve
        .post(
            &api,
            "/api/v1/taxonomies/tag/terms",
            json!({"name": "rust"}),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{r:?}");
    // An admin's settings:read token is narrowed too; an author cannot manage terms.
    let narrow = api.bearer(&ada, &["settings:read", "content:read"]).await;
    let p = narrow
        .send(&api, Method::POST, TYPES, Some(&body), None)
        .await
        .assert_problem(403, "forbidden");
    assert!(
        p["detail"].as_str().unwrap().contains("settings:write"),
        "{p}"
    );
    let au = api.user("abe", Role::Author, false).await;
    let abe = api.bearer(&au, &["*"]).await;
    let p = abe
        .send(
            &api,
            Method::POST,
            "/api/v1/taxonomies/tag/terms",
            Some(&json!({"name": "x"})),
            None,
        )
        .await
        .assert_problem(403, "forbidden");
    assert!(
        p["detail"].as_str().unwrap().contains("manage_terms"),
        "{p}"
    );
    // Nothing was registered.
    assert!(
        api.store
            .read()
            .types()
            .get("event")
            .await
            .unwrap()
            .is_none()
    );
    api
}
