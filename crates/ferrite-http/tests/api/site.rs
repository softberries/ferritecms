//! Task 10: menus, redirects (with CSV import and export) and settings under
//! `/api/v1/menus`, `/api/v1/redirects` and `/api/v1/settings` (P11, P19, P29).

use axum::{
    body::Body,
    http::{Method, StatusCode, header},
};
use ferrite_domain::{Role, User};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{
    api_tests,
    harness::{Api, Bearer, Resp},
};

api_tests!(
    menus_put_get_delete,
    redirects_crud_and_csv_roundtrip,
    csv_errors_name_lines,
    settings_patch_over_http,
    author_cannot_patch_settings,
    settings_round_trip_locales,
);

const MENUS: &str = "/api/v1/menus";
const REDIRECTS: &str = "/api/v1/redirects";
const SETTINGS: &str = "/api/v1/settings";

async fn admin(api: &Api) -> (User, Bearer) {
    let ada = api.user("ada", Role::Admin, false).await;
    let token = api.bearer(&ada, &["*"]).await;
    (ada, token)
}

async fn csv_import(api: &Api, b: &Bearer, content_type: &str, body: &str) -> Resp {
    let req = b
        .builder(Method::POST, &format!("{REDIRECTS}/import"))
        .header(header::CONTENT_TYPE, content_type)
        .body(Body::from(body.to_owned()))
        .unwrap();
    api.send(req).await
}

/// `(source, target, code)` of every redirect, by source.
async fn all_redirects(api: &Api, b: &Bearer) -> Vec<(String, String, u64)> {
    let r = b.get(api, &format!("{REDIRECTS}?limit=200")).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    r.json()["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| {
            (
                i["source"].as_str().unwrap().to_owned(),
                i["target"].as_str().unwrap().to_owned(),
                i["code"].as_u64().unwrap(),
            )
        })
        .collect()
}

async fn menus_put_get_delete(api: Api) -> Api {
    let (_, b) = admin(&api).await;
    let r = b.get(&api, MENUS).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    let list = r.json();
    assert_eq!(list["items"], json!([]));
    assert_eq!(list["next_cursor"], Value::Null);

    // The public header shows menu `primary`; a PUT flushes the cached home page.
    let mut poller = api.poller().await;
    let home = api.get("/").await;
    assert_eq!(home.status, StatusCode::OK);
    assert!(!home.body.contains("class=\"menu\""), "{}", home.body);
    let root = Uuid::now_v7();
    let child = Uuid::now_v7();
    let body = json!({
        "label": "Main",
        "items": [
            {"id": child, "parent_id": root, "position": 0, "kind": "url", "url": "/about/", "label": "About"},
            {"id": root, "position": 0, "kind": "url", "url": "https://example.org/", "label": "Elsewhere"},
        ]
    });
    let path = format!("{MENUS}/primary");
    let r = b.send(&api, Method::PUT, &path, Some(&body), None).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    let menu = r.json();
    assert_eq!(menu["name"], "primary");
    assert_eq!(menu["label"], "Main");
    // Tree order: the root first.
    assert_eq!(menu["items"][0]["id"], json!(root));
    assert_eq!(menu["items"][1]["parent_id"], json!(root));
    assert_eq!(menu["items"][1]["kind"], "url");
    assert_eq!(b.get(&api, &path).await.json(), menu);
    poller.poll_once().await.unwrap();
    let home = api.get("/").await;
    assert!(home.body.contains(">Elsewhere</a>"), "{}", home.body);

    let list = b.get(&api, MENUS).await.json();
    assert_eq!(
        list["items"],
        json!([{"name": "primary", "label": "Main", "locale": "en", "translation_of": null}])
    );

    // Validation: a bad URL, a parent outside the menu, a name that does not match.
    let bad = json!({"label": "x", "items": [
        {"id": Uuid::now_v7(), "kind": "url", "url": "javascript:alert(1)", "label": "x"}
    ]});
    b.send(&api, Method::PUT, &path, Some(&bad), None)
        .await
        .assert_problem(422, "validation");
    let orphan = json!({"label": "x", "items": [
        {"id": Uuid::now_v7(), "parent_id": Uuid::now_v7(), "kind": "url", "url": "/x", "label": "x"}
    ]});
    b.send(&api, Method::PUT, &path, Some(&orphan), None)
        .await
        .assert_problem(422, "validation");
    let p = b
        .send(
            &api,
            Method::PUT,
            &path,
            Some(&json!({"name": "other", "label": "x"})),
            None,
        )
        .await
        .assert_problem(422, "validation");
    assert_eq!(p["errors"][0]["field"], "name");
    let p = b
        .send(
            &api,
            Method::PUT,
            &path,
            Some(&json!({"label": "x".repeat(201)})),
            None,
        )
        .await
        .assert_problem(422, "validation");
    assert_eq!(p["errors"][0]["field"], "label");
    // An entry id another menu uses: 409 naming `items`, no constraint name.
    let r = b
        .send(
            &api,
            Method::PUT,
            &format!("{MENUS}/footer"),
            Some(&json!({"label": "F", "items": [
                {"id": root, "kind": "url", "url": "/f", "label": "F"}
            ]})),
            None,
        )
        .await;
    let p = r.assert_problem(409, "conflict");
    assert_eq!(p["errors"][0]["field"], "items");
    assert!(!r.body.contains("pkey"), "{}", r.body);
    assert_eq!(b.get(&api, &path).await.json(), menu, "unchanged");

    // A read-only token can read but not write; an author cannot write.
    let ada = api.user("reader", Role::Admin, false).await;
    let ro = api.bearer(&ada, &["menus:read"]).await;
    assert_eq!(ro.get(&api, &path).await.status, StatusCode::OK);
    ro.send(&api, Method::DELETE, &path, None, None)
        .await
        .assert_problem(403, "forbidden");
    let author = api.user("arthur", Role::Author, false).await;
    let ab = api.bearer(&author, &["*"]).await;
    let p = ab
        .send(&api, Method::PUT, &path, Some(&body), None)
        .await
        .assert_problem(403, "forbidden");
    assert!(
        p["detail"].as_str().unwrap().contains("manage_menus"),
        "{p}"
    );

    // Delete: 204, then 404; the header loses the menu.
    let r = b.send(&api, Method::DELETE, &path, None, None).await;
    assert_eq!(r.status, StatusCode::NO_CONTENT, "{r:?}");
    b.get(&api, &path).await.assert_problem(404, "not-found");
    b.send(&api, Method::DELETE, &path, None, None)
        .await
        .assert_problem(404, "not-found");
    poller.poll_once().await.unwrap();
    assert!(!api.get("/").await.body.contains("Elsewhere"));
    api
}

async fn redirects_crud_and_csv_roundtrip(api: Api) -> Api {
    let (_, b) = admin(&api).await;
    // Create: 201 with Location; the public site follows it.
    let r = b
        .send(
            &api,
            Method::POST,
            REDIRECTS,
            Some(&json!({"source": "/old", "target": "/new/"})),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{r:?}");
    let created = r.json();
    assert_eq!(created["code"], 301, "default code");
    assert_eq!(created["origin"], "manual");
    assert_eq!(created["hits"], 0);
    let id = created["id"].as_str().unwrap().to_owned();
    assert_eq!(
        r.header("location"),
        Some(format!("{REDIRECTS}/{id}").as_str())
    );
    let public = api.get("/old").await;
    assert_eq!(public.status, StatusCode::MOVED_PERMANENTLY);
    assert_eq!(public.header("location"), Some("/new/"));
    assert_eq!(
        b.get(&api, &format!("{REDIRECTS}/{id}")).await.json()["source"],
        "/old"
    );

    // PATCH: move the source, keep the id.
    let r = b
        .send(
            &api,
            Method::PATCH,
            &format!("{REDIRECTS}/{id}"),
            Some(&json!({"source": "/older", "code": 302})),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    let moved = r.json();
    assert_eq!(
        (
            &moved["id"],
            &moved["source"],
            &moved["target"],
            &moved["code"]
        ),
        (&json!(id), &json!("/older"), &json!("/new/"), &json!(302))
    );
    // No redirect left for "/old" (the canonical slash 301 leads to a 404).
    assert_eq!(api.get("/old/").await.status, StatusCode::NOT_FOUND);
    assert_eq!(api.get("/older").await.status, StatusCode::FOUND);
    // A field error names the member; `null` is refused.
    let p = b
        .send(
            &api,
            Method::POST,
            REDIRECTS,
            Some(&json!({"source": "/loop", "target": "/loop/"})),
            None,
        )
        .await
        .assert_problem(422, "validation");
    assert_eq!(p["errors"][0]["field"], "target");
    let p = b
        .send(
            &api,
            Method::POST,
            REDIRECTS,
            Some(&json!({"source": "nope", "target": "/x"})),
            None,
        )
        .await
        .assert_problem(422, "validation");
    assert_eq!(p["errors"][0]["field"], "source");
    b.send(
        &api,
        Method::PATCH,
        &format!("{REDIRECTS}/{id}"),
        Some(&json!({"target": null})),
        None,
    )
    .await
    .assert_problem(400, "malformed-json");
    b.get(&api, &format!("{REDIRECTS}/{}", Uuid::now_v7()))
        .await
        .assert_problem(404, "not-found");
    b.get(&api, &format!("{REDIRECTS}?cursor=junk"))
        .await
        .assert_problem(400, "validation");

    // Import (with a header row, CRLF, a BOM and a quoted comma), then export.
    let file = "\u{feff}source,target,code\r\n\
                /a,/b,301\r\n\
                /c,\"https://example.com/?q=1,2\",308\r\n\
                /older,/newest/,307\r\n";
    let r = csv_import(&api, &b, "text/csv; charset=utf-8", file).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert_eq!(r.json(), json!({"created": 2, "updated": 1}));
    let before = all_redirects(&api, &b).await;
    assert_eq!(
        before,
        [
            ("/a".into(), "/b".into(), 301),
            ("/c".into(), "https://example.com/?q=1,2".into(), 308),
            ("/older".into(), "/newest/".into(), 307),
        ]
    );
    let item = b.get(&api, &format!("{REDIRECTS}/{id}")).await.json();
    assert_eq!(item["origin"], "import", "the import replaced it");

    let r = b.get(&api, &format!("{REDIRECTS}/export")).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert_eq!(r.header("content-type"), Some("text/csv; charset=utf-8"));
    assert_eq!(r.header("cache-control"), Some("no-store"));
    assert!(
        r.header("content-disposition")
            .is_some_and(|d| d.contains("redirects.csv"))
    );
    assert_eq!(
        r.body,
        "source,target,code\r\n/a,/b,301\r\n/c,\"https://example.com/?q=1,2\",308\r\n/older,/newest/,307\r\n"
    );
    // Export -> delete everything -> import: the same rows.
    for i in b.get(&api, REDIRECTS).await.json()["items"]
        .as_array()
        .unwrap()
    {
        let path = format!("{REDIRECTS}/{}", i["id"].as_str().unwrap());
        let d = b.send(&api, Method::DELETE, &path, None, None).await;
        assert_eq!(d.status, StatusCode::NO_CONTENT, "{d:?}");
    }
    assert!(all_redirects(&api, &b).await.is_empty());
    let back = csv_import(&api, &b, "text/csv", &r.body).await;
    assert_eq!(back.json(), json!({"created": 3, "updated": 0}), "{back:?}");
    assert_eq!(all_redirects(&api, &b).await, before);

    // A redirects:read token can list, read and export, but not write.
    let ada = api.user("reader", Role::Admin, false).await;
    let ro = api.bearer(&ada, &["redirects:read"]).await;
    let r = ro.get(&api, &format!("{REDIRECTS}/export")).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert!(
        r.body.starts_with("source,target,code\r\n/a,/b,301\r\n"),
        "{}",
        r.body
    );
    assert_eq!(ro.get(&api, REDIRECTS).await.status, StatusCode::OK);
    let p = csv_import(&api, &ro, "text/csv", "/x,/y,301\n")
        .await
        .assert_problem(403, "forbidden");
    assert!(
        p["detail"].as_str().unwrap().contains("redirects:write"),
        "{p}"
    );
    // A router-owned source is a field error.
    let p = b
        .send(
            &api,
            Method::POST,
            REDIRECTS,
            Some(&json!({"source": "/sitemap.xml", "target": "/x"})),
            None,
        )
        .await
        .assert_problem(422, "validation");
    assert_eq!(p["errors"][0]["field"], "source");

    // An editor cannot touch redirects.
    let ed = api.user("eddie", Role::Editor, false).await;
    let eb = api.bearer(&ed, &["*"]).await;
    eb.get(&api, REDIRECTS)
        .await
        .assert_problem(403, "forbidden");
    eb.get(&api, &format!("{REDIRECTS}/export"))
        .await
        .assert_problem(403, "forbidden");
    csv_import(&api, &eb, "text/csv", "/x,/y,301\n")
        .await
        .assert_problem(403, "forbidden");
    api
}

async fn csv_errors_name_lines(api: Api) -> Api {
    let (_, b) = admin(&api).await;
    b.send(
        &api,
        Method::POST,
        REDIRECTS,
        Some(&json!({"source": "/existing", "target": "/a"})),
        None,
    )
    .await;

    // Not CSV, too large.
    csv_import(&api, &b, "application/json", "[]")
        .await
        .assert_problem(415, "unsupported-media-type");
    let big = "/x,/y,301\n".repeat(110_000);
    csv_import(&api, &b, "text/csv", &big)
        .await
        .assert_problem(413, "too-large");
    // More than 10 000 rows (under 1 MiB).
    let many: String = (0..10_001).map(|i| format!("/{i},/t,301\n")).collect();
    let p = csv_import(&api, &b, "text/csv", &many)
        .await
        .assert_problem(422, "validation");
    assert!(p["detail"].as_str().unwrap().contains("10000"), "{p}");

    // A syntax error names its line (a quoted field spanning lines counts both).
    let p = csv_import(&api, &b, "text/csv", "/a,\"x\ny\",301\n/b,c\"d,301\n")
        .await
        .assert_problem(422, "validation");
    assert_eq!(p["errors"][0]["field"], "line 3", "{p}");

    // Row errors: every bad line is listed, nothing is written.
    let file = "source,target,code\n\
                /ok,/fine,301\n\
                /short,/x\n\
                /code,/x,abc\n";
    let p = csv_import(&api, &b, "text/csv", file)
        .await
        .assert_problem(422, "validation");
    let fields: Vec<&str> = p["errors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["field"].as_str().unwrap())
        .collect();
    assert_eq!(fields, ["line 3", "line 4"], "{p}");
    let file = "/ok,/fine,301\n\
                /bad source,/x,301\n\
                /ok,/dup,301\n\
                /a,/existing,301\n\
                /gone,/x,200\n";
    let p = csv_import(&api, &b, "text/csv", file)
        .await
        .assert_problem(422, "validation");
    let errors = p["errors"].as_array().unwrap();
    let fields: Vec<&str> = errors
        .iter()
        .map(|e| e["field"].as_str().unwrap())
        .collect();
    assert_eq!(fields, ["line 2", "line 3", "line 4", "line 5"], "{p}");
    assert!(errors[1]["detail"].as_str().unwrap().contains("duplicate"));
    assert!(errors[2]["detail"].as_str().unwrap().contains("loop"));
    let left = all_redirects(&api, &b).await;
    assert_eq!(left, [("/existing".into(), "/a".into(), 301)]);
    // Not UTF-8.
    let req = b
        .builder(Method::POST, &format!("{REDIRECTS}/import"))
        .header(header::CONTENT_TYPE, "text/csv")
        .body(Body::from(vec![b'/', 0xff, b',', b'/', b',', b'1']))
        .unwrap();
    api.send(req).await.assert_problem(400, "malformed-body");
    api
}

async fn settings_patch_over_http(api: Api) -> Api {
    let (_, b) = admin(&api).await;
    let r = b.get(&api, SETTINGS).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    let all = r.json();
    let keys: Vec<&String> = all.as_object().unwrap().keys().collect();
    assert_eq!(keys.len(), 17, "{all}");
    assert_eq!(all["mcp.enabled"], serde_json::Value::Null, "{all}");
    assert!(all.get("comments.enabled").is_some() && all.get("comments.moderation").is_some());
    assert_eq!(
        all["comments.per_page"],
        serde_json::Value::Null,
        "unset until set (F21)"
    );
    let p = b
        .send(
            &api,
            Method::PATCH,
            SETTINGS,
            Some(&json!({"comments.moderation": "nobody"})),
            None,
        )
        .await
        .assert_problem(422, "validation");
    assert_eq!(p["errors"][0]["field"], "comments.moderation");
    assert!(
        all.as_object().unwrap().values().all(Value::is_null),
        "{all}"
    );

    let mut poller = api.poller().await;
    let home = api.get("/").await;
    assert!(home.body.contains("Test Site"), "{}", home.body);
    let r = b
        .send(
            &api,
            Method::PATCH,
            SETTINGS,
            Some(&json!({"site.title": "Renamed", "reading.per_page": 3})),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    let saved = r.json();
    assert_eq!(saved["site.title"], "Renamed");
    assert_eq!(saved["reading.per_page"], 3);
    assert_eq!(saved["site.locale"], Value::Null);
    assert_eq!(b.get(&api, SETTINGS).await.json(), saved);
    // On `/` within one poll.
    poller.poll_once().await.unwrap();
    let home = api.get("/").await;
    assert!(home.body.contains("Renamed"), "{}", home.body);

    // null resets; an unknown key and a bad value are 422 and write nothing.
    let r = b
        .send(
            &api,
            Method::PATCH,
            SETTINGS,
            Some(&json!({"reading.per_page": null})),
            None,
        )
        .await;
    assert_eq!(r.json()["reading.per_page"], Value::Null);
    let p = b
        .send(
            &api,
            Method::PATCH,
            SETTINGS,
            Some(&json!({"site.title": "X", "site.url": "https://evil"})),
            None,
        )
        .await
        .assert_problem(422, "validation");
    assert!(p["detail"].as_str().unwrap().contains("site.url"), "{p}");
    let p = b
        .send(
            &api,
            Method::PATCH,
            SETTINGS,
            Some(&json!({"site.title": "X", "reading.per_page": 500})),
            None,
        )
        .await
        .assert_problem(422, "validation");
    assert_eq!(p["errors"][0]["field"], "reading.per_page");
    let p = b
        .send(
            &api,
            Method::PATCH,
            SETTINGS,
            Some(&json!({"permalink.post": "/post/%slug%/"})),
            None,
        )
        .await
        .assert_problem(422, "validation");
    assert_eq!(p["errors"][0]["field"], "permalink.post");
    assert!(
        p["detail"].as_str().unwrap().contains("content type"),
        "{p}"
    );
    assert_eq!(b.get(&api, SETTINGS).await.json()["site.title"], "Renamed");
    b.send(&api, Method::PATCH, SETTINGS, Some(&json!(["x"])), None)
        .await
        .assert_problem(400, "malformed-json");
    api
}

async fn author_cannot_patch_settings(api: Api) -> Api {
    api.user("arthur", Role::Author, false).await;
    let mut c = api.logged_in("arthur").await;
    // Reading is allowed (read_admin + settings:read); writing needs manage_settings.
    assert_eq!(c.get(&api, SETTINGS).await.status, StatusCode::OK);
    let p = c
        .send(
            &api,
            Method::PATCH,
            SETTINGS,
            Some(&json!({"site.title": "Hacked"})),
            None,
        )
        .await
        .assert_problem(403, "forbidden");
    assert!(
        p["detail"].as_str().unwrap().contains("manage_settings"),
        "{p}"
    );
    // An admin's settings:read token cannot write either.
    let (ada, _) = admin(&api).await;
    let ro = api.bearer(&ada, &["settings:read"]).await;
    assert_eq!(ro.get(&api, SETTINGS).await.status, StatusCode::OK);
    let p = ro
        .send(
            &api,
            Method::PATCH,
            SETTINGS,
            Some(&json!({"site.title": "Hacked"})),
            None,
        )
        .await
        .assert_problem(403, "forbidden");
    assert!(
        p["detail"].as_str().unwrap().contains("settings:write"),
        "{p}"
    );
    // A token without settings scope cannot read.
    let content = api.bearer(&ada, &["content:read"]).await;
    content
        .get(&api, SETTINGS)
        .await
        .assert_problem(403, "forbidden");
    assert!(
        api.store
            .read()
            .options()
            .get("site.title")
            .await
            .unwrap()
            .is_none()
    );
    api
}

/// `site.locales` and `site.locale_texts` through `PATCH /settings` (M6-1 Task 5): the
/// list comes back as written, `site.locale` follows the default, a removal of a locale
/// that holds content is `409`, a clashing prefix `422`.
async fn settings_round_trip_locales(api: Api) -> Api {
    let (ada, b) = admin(&api).await;
    let r = b
        .send(
            &api,
            Method::PATCH,
            SETTINGS,
            Some(&json!({
                "site.locales": [{"tag": "en"}, {"tag": "de", "label": "Deutsch"}],
                "site.locale_texts": {"de": {"title": "Mein Garten"}},
            })),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    let saved = r.json();
    assert_eq!(
        saved["site.locales"],
        json!([{"tag": "en", "label": "English"}, {"tag": "de", "label": "Deutsch"}])
    );
    assert_eq!(saved["site.locale"], "en");
    assert_eq!(saved["site.locale_texts"]["de"]["title"], "Mein Garten");

    let p = b
        .send(
            &api,
            Method::PATCH,
            SETTINGS,
            Some(&json!({"site.locales": [{"tag": "en"}, {"tag": "api"}]})),
            None,
        )
        .await
        .assert_problem(422, "validation");
    assert_eq!(p["errors"][0]["field"], "site.locales", "{p}");

    api.store
        .read()
        .content()
        .insert(&ferrite_domain::NewContent {
            locale: "de".into(),
            ..ferrite_store_tests::fixtures::new_post(&ada, "hallo")
        })
        .await
        .unwrap();
    let p = b
        .send(
            &api,
            Method::PATCH,
            SETTINGS,
            Some(&json!({"site.locales": [{"tag": "en"}]})),
            None,
        )
        .await
        .assert_problem(409, "conflict");
    assert!(
        p["detail"].as_str().unwrap().contains("1 content item"),
        "{p}"
    );
    api
}
