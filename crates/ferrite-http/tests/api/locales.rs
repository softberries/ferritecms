//! M6-1 Task 13: locales over the REST API: `/meta`, content `locale` and
//! `translation_of`, `POST /content/{type}/{id}/translations`, list and count filters,
//! terms and menus per locale.

use axum::http::{Method, StatusCode};
use ferrite_domain::Role;
use serde_json::{Value, json};

use crate::{
    api_tests,
    harness::{Api, Bearer},
    mcp::{mcp_call, tool},
};

api_tests!(
    meta_lists_locales,
    content_translate_and_filter,
    terms_and_menus_per_locale,
    mcp_translate_then_update,
    patch_with_refused_move_saves_nothing,
);

const POSTS: &str = "/api/v1/content/post";

async fn bilingual(api: &Api) -> Bearer {
    let ada = api.user("ada", Role::Admin, false).await;
    let b = api.bearer(&ada, &["*"]).await;
    let r = b
        .send(
            api,
            Method::PATCH,
            "/api/v1/settings",
            Some(
                &json!({"site.locales": [{"tag": "en"}, {"tag": "de-AT", "label": "Österreich"}]}),
            ),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    b
}

async fn post(api: &Api, b: &Bearer, path: &str, body: Value) -> Value {
    let r = b.send(api, Method::POST, path, Some(&body), None).await;
    assert_eq!(r.status, StatusCode::CREATED, "{r:?}");
    r.json()
}

async fn meta_lists_locales(api: Api) -> Api {
    let r = api.get("/api/v1/meta").await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert_eq!(r.json()["default_locale"], "en");
    assert_eq!(
        r.json()["locales"],
        json!([{"tag": "en", "label": "English", "prefix": null}])
    );
    let _b = bilingual(&api).await;
    let m = api.get("/api/v1/meta").await.json();
    assert_eq!(
        m["locales"],
        json!([
            {"tag": "en", "label": "English", "prefix": null},
            {"tag": "de-AT", "label": "Österreich", "prefix": "de-at"},
        ])
    );
    api
}

async fn content_translate_and_filter(api: Api) -> Api {
    let b = bilingual(&api).await;
    let en = post(&api, &b, POSTS, json!({"title": "Hello"})).await;
    assert_eq!(en["locale"], "en");
    assert_eq!(en["translation_group"], en["id"]);
    let id = en["id"].as_str().unwrap();

    // An unknown locale is refused on create; on the list filter it is `400`.
    let p = b
        .send(
            &api,
            Method::POST,
            POSTS,
            Some(&json!({"title": "x", "locale": "fr"})),
            None,
        )
        .await;
    assert!(p.status.is_client_error(), "{p:?}");
    b.get(&api, &format!("{POSTS}?locale=fr"))
        .await
        .assert_problem(400, "invalid-locale");

    let t = post(
        &api,
        &b,
        &format!("{POSTS}/{id}/translations"),
        json!({"locale": "de-AT"}),
    )
    .await;
    let de = &t["item"];
    assert_eq!(de["locale"], "de-AT");
    assert_eq!(de["status"], "draft");
    assert_eq!(de["translation_group"], en["translation_group"]);
    assert!(t["notes"].is_array(), "{t}");
    // No permalink assertion: the API harness runs no reloader, so its permalink rules
    // keep the one-locale list it started with (`serve` reloads on `site.locales`; the
    // prefixed URLs are covered by the app and serve tests).

    // A second translation into the same locale is a conflict.
    b.send(
        &api,
        Method::POST,
        &format!("{POSTS}/{id}/translations"),
        Some(&json!({"locale": "de-AT"})),
        None,
    )
    .await
    .assert_problem(409, "conflict");

    // The source lists its translation.
    let src = b.get(&api, &format!("{POSTS}/{id}")).await.json();
    let refs = src["translations"].as_array().unwrap();
    assert!(
        refs.iter()
            .any(|r| r["locale"] == "de-AT" && r["id"] == de["id"]),
        "{src}"
    );

    // The list filters by locale, and its rows name the other locales.
    let only_de = b.get(&api, &format!("{POSTS}?locale=de-AT")).await.json();
    let items = only_de["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "{only_de}");
    assert_eq!(items[0]["id"], de["id"]);
    let all = b.get(&api, POSTS).await.json();
    assert_eq!(all["items"].as_array().unwrap().len(), 2, "{all}");
    let counts = b
        .get(&api, &format!("{POSTS}/counts?locale=en"))
        .await
        .json();
    assert_eq!(counts["draft"], 1, "{counts}");

    // `translation_of` joins an existing group on create.
    let other = post(&api, &b, POSTS, json!({"title": "Other"})).await;
    let joined = post(
        &api,
        &b,
        POSTS,
        json!({"title": "Andere", "locale": "de-AT", "translation_of": other["id"]}),
    )
    .await;
    assert_eq!(joined["translation_group"], other["translation_group"]);
    api
}

async fn terms_and_menus_per_locale(api: Api) -> Api {
    let b = bilingual(&api).await;
    let terms = "/api/v1/taxonomies/category/terms";
    let news = post(&api, &b, terms, json!({"name": "News"})).await;
    assert_eq!(news["locale"], "en");
    let nid = news["id"].as_str().unwrap();
    let de = post(
        &api,
        &b,
        &format!("/api/v1/terms/{nid}/translations"),
        json!({"locale": "de-AT"}),
    )
    .await;
    assert_eq!(de["locale"], "de-AT");
    assert_eq!(de["translation_group"], news["translation_group"]);
    assert_eq!(de["slug"], news["slug"]);
    let made = post(&api, &b, terms, json!({"name": "Sport", "locale": "de-AT"})).await;
    assert_eq!(made["locale"], "de-AT");

    let list = b.get(&api, &format!("{terms}?locale=de-AT")).await.json();
    let names: Vec<_> = list["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(names.len(), 2, "{list}");
    assert!(names.contains(&"Sport".to_owned()), "{list}");
    b.get(&api, &format!("{terms}?locale=xx"))
        .await
        .assert_problem(400, "invalid-locale");

    let body = json!({"label": "Main", "items": []});
    let r = b
        .send(
            &api,
            Method::PUT,
            "/api/v1/menus/primary",
            Some(&body),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert_eq!(r.json()["locale"], "en");
    assert_eq!(r.json()["translation_of"], Value::Null);
    let body =
        json!({"label": "Haupt", "items": [], "locale": "de-AT", "translation_of": "primary"});
    let r = b
        .send(
            &api,
            Method::PUT,
            "/api/v1/menus/primary-de",
            Some(&body),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert_eq!(r.json()["locale"], "de-AT");
    assert_eq!(r.json()["translation_of"], "primary");
    // A PUT without `locale` keeps the stored one.
    let r = b
        .send(
            &api,
            Method::PUT,
            "/api/v1/menus/primary-de",
            Some(&json!({"label": "Haupt 2", "items": [], "translation_of": "primary"})),
            None,
        )
        .await;
    assert_eq!(r.json()["locale"], "de-AT", "{r:?}");
    let list = b.get(&api, "/api/v1/menus").await.json();
    assert!(
        list["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["name"] == "primary-de" && m["locale"] == "de-AT"),
        "{list}"
    );
    api
}

/// MCP: `content_translate` makes the draft, `content_update` saves the translation,
/// `content_list` filters by locale, `site_overview` lists the locales and the
/// `translate` prompt is offered.
async fn mcp_translate_then_update(api: Api) -> Api {
    let b = bilingual(&api).await;
    let made = tool(
        &api,
        &b.0,
        "content_create",
        json!({"type": "post", "title": "Hello", "body_markdown": "Good morning."}),
    )
    .await;
    assert_eq!(made["isError"], false, "{made}");
    let id = made["structuredContent"]["id"].clone();
    let r = tool(
        &api,
        &b.0,
        "content_translate",
        json!({"type": "post", "id": id, "locale": "de-AT"}),
    )
    .await;
    assert_eq!(r["isError"], false, "{r}");
    let de = r["structuredContent"].clone();
    assert_eq!(de["locale"], "de-AT", "{de}");
    assert_eq!(de["status"], "draft", "{de}");
    assert!(
        de["body_markdown"]
            .as_str()
            .unwrap()
            .contains("Good morning."),
        "{de}"
    );
    let again = tool(
        &api,
        &b.0,
        "content_translate",
        json!({"type": "post", "id": id, "locale": "de-AT"}),
    )
    .await;
    assert_eq!(again["isError"], true, "{again}");

    let u = tool(
        &api,
        &b.0,
        "content_update",
        json!({
            "type": "post", "id": de["id"], "version": de["version"],
            "title": "Hallo", "body_markdown": "Guten Morgen.",
        }),
    )
    .await;
    assert_eq!(u["isError"], false, "{u}");
    assert_eq!(u["structuredContent"]["locale"], "de-AT");

    let l = tool(
        &api,
        &b.0,
        "content_list",
        json!({"type": "post", "locale": "de-AT"}),
    )
    .await;
    let items = l["structuredContent"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "{l}");
    assert_eq!(items[0]["title"], "Hallo");

    let o = tool(&api, &b.0, "site_overview", json!({})).await;
    assert_eq!(
        o["structuredContent"]["site"]["default_locale"], "en",
        "{o}"
    );
    assert_eq!(
        o["structuredContent"]["site"]["locales"][1]["tag"], "de-AT",
        "{o}"
    );

    let (_, body) = mcp_call(&api, &b.0, "prompts/list", json!({})).await;
    let prompts = body["result"]["prompts"].as_array().unwrap();
    assert!(prompts.iter().any(|p| p["name"] == "translate"), "{body}");
    let (_, body) = mcp_call(
        &api,
        &b.0,
        "prompts/get",
        json!({"name": "translate", "arguments": {"type": "post", "id": id, "locale": "de-AT"}}),
    )
    .await;
    let text = body["result"]["messages"][0]["content"]["text"]
        .as_str()
        .unwrap_or_default();
    assert!(text.contains("content_translate"), "{body}");
    api
}

/// Review fix: a `PATCH` whose locale move is refused saves none of its other changes
/// either (one transaction).
async fn patch_with_refused_move_saves_nothing(api: Api) -> Api {
    let b = bilingual(&api).await;
    let en = post(&api, &b, POSTS, json!({"title": "Hello"})).await;
    let id = en["id"].as_str().unwrap().to_owned();
    post(
        &api,
        &b,
        &format!("{POSTS}/{id}/translations"),
        json!({"locale": "de-AT"}),
    )
    .await;
    // The group has a German member already: moving this one there is refused.
    b.send(
        &api,
        Method::PATCH,
        &format!("{POSTS}/{id}"),
        Some(&json!({"title": "Changed", "locale": "de-AT"})),
        Some(en["version"].as_i64().unwrap()),
    )
    .await
    .assert_problem(409, "conflict");
    let now = b.get(&api, &format!("{POSTS}/{id}")).await.json();
    assert_eq!(now["title"], "Hello", "{now}");
    assert_eq!(now["locale"], "en");
    assert_eq!(now["version"], en["version"]);
    api
}
