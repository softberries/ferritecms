//! Task 6: the `/api/v1` router, problem documents, extractors, the OpenAPI document
//! and its snapshot, and the public site behind `build_site`.

use axum::{
    Json, Router,
    body::Body,
    http::{Method, Request, StatusCode, header},
    routing::{get, post},
};
use ferrite_http::api::{self, ApiJson, ApiQuery, MAX_JSON_BODY, PageParams, problem::Problem};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{
    api_tests,
    harness::{Api, SITE_URL, request, send},
};

api_tests!(
    openapi_json_is_valid_3_1,
    meta_reports_the_site,
    unknown_api_path_is_problem_404,
    wrong_method_is_problem_405,
    public_site_still_served,
);

async fn openapi_json_is_valid_3_1(api: Api) -> Api {
    let r = api.get("/api/v1/openapi.json").await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert_eq!(r.header("content-type"), Some("application/json"));
    assert_eq!(r.header("cache-control"), Some("no-store"));
    let doc = r.json();
    assert_eq!(doc["openapi"], "3.1.0");
    assert_eq!(doc["info"]["title"], "FerriteCMS API");
    assert_eq!(doc["info"]["version"], "1");
    let schemes = &doc["components"]["securitySchemes"];
    // P23: the scheme names the https cookie and documents the http one.
    assert_eq!(schemes["session"]["type"], "apiKey");
    assert_eq!(schemes["session"]["in"], "cookie");
    assert_eq!(schemes["session"]["name"], "__Host-ferrite_session");
    let about = schemes["session"]["description"]
        .as_str()
        .unwrap_or_default();
    assert!(
        about.contains("`__Host-ferrite_session`") && about.contains("`ferrite_session`"),
        "{about}"
    );
    assert_eq!(schemes["bearer"]["type"], "http");
    assert_eq!(schemes["bearer"]["scheme"], "bearer");
    assert!(doc["paths"]["/api/v1/meta"]["get"].is_object(), "{doc}");
    assert!(doc["paths"]["/api/v1/openapi.json"]["get"].is_object());
    // Every `$ref` names a schema the document defines (the admin's client generator
    // refuses a dangling one).
    let mut dangling = Vec::new();
    collect_dangling_refs(&doc, &doc["components"]["schemas"], &mut dangling);
    assert_eq!(dangling, Vec::<String>::new(), "dangling $refs");
    // What is served is the document the snapshot holds.
    assert_eq!(doc, serde_json::to_value(api::openapi()).unwrap());
    api
}

fn collect_dangling_refs(
    node: &serde_json::Value,
    schemas: &serde_json::Value,
    out: &mut Vec<String>,
) {
    match node {
        serde_json::Value::Object(map) => {
            for (key, value) in map {
                if key == "$ref"
                    && let Some(r) = value.as_str()
                {
                    let known = r
                        .strip_prefix("#/components/schemas/")
                        .is_some_and(|name| schemas.get(name).is_some());
                    if !known {
                        out.push(r.to_owned());
                    }
                } else {
                    collect_dangling_refs(value, schemas, out);
                }
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                collect_dangling_refs(item, schemas, out);
            }
        }
        _ => {}
    }
}

async fn meta_reports_the_site(api: Api) -> Api {
    let r = api.get("/api/v1/meta").await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert_eq!(r.header("content-type"), Some("application/json"));
    assert_eq!(r.header("cache-control"), Some("no-store"));
    // The public site's layers wrap the API too.
    assert_eq!(r.header("x-content-type-options"), Some("nosniff"));
    assert_eq!(r.header("x-frame-options"), Some("SAMEORIGIN"));
    assert_eq!(
        r.json(),
        json!({"api": "v1", "version": env!("CARGO_PKG_VERSION"), "site_url": SITE_URL})
    );
    let head = api.send(request(Method::HEAD, "/api/v1/meta")).await;
    assert_eq!(head.status, StatusCode::OK);
    assert!(head.body.is_empty());
    // The page cache is never touched by the API.
    assert_eq!(api.app.cache.len().await, 0);
    api
}

async fn unknown_api_path_is_problem_404(api: Api) -> Api {
    for path in [
        "/api/v1",
        "/api/v1/",
        "/api/v1/nope",
        "/api/v1/meta/extra",
        "/api/v1/a/b/c?x=1",
    ] {
        let body = api.get(path).await.assert_problem(404, "not-found");
        assert_eq!(body["title"], "Not found", "{path}");
    }
    let r = api.send(request(Method::POST, "/api/v1/nope/deeper")).await;
    r.assert_problem(404, "not-found");
    assert_eq!(
        api.app.cache.len().await,
        0,
        "no shared 404 page was rendered"
    );
    api
}

async fn wrong_method_is_problem_405(api: Api) -> Api {
    for method in [Method::POST, Method::DELETE, Method::PUT, Method::PATCH] {
        let r = api.send(request(method.clone(), "/api/v1/meta")).await;
        r.assert_problem(405, "method-not-allowed");
        assert_eq!(r.header("allow"), Some("GET,HEAD"), "{method}");
    }
    api
}

async fn public_site_still_served(api: Api) -> Api {
    let r = api.get("/").await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert_eq!(r.header("content-type"), Some("text/html; charset=utf-8"));
    assert!(r.header("etag").is_some_and(|e| e.starts_with("W/")));
    assert_ne!(r.header("cache-control"), Some("no-store"));
    assert_eq!(api.app.cache.len().await, 1, "the front page is cached");
    // Outside `/api/v1` the public rules apply: the canonical trailing slash first,
    // then the shared 404 page.
    for (path, canonical) in [
        ("/api", "/api/"),
        ("/api/v2/meta", "/api/v2/meta/"),
        ("/api/v1x", "/api/v1x/"),
    ] {
        let r = api.get(path).await;
        assert_eq!(r.status, StatusCode::MOVED_PERMANENTLY, "{path}");
        assert_eq!(r.header("location"), Some(canonical), "{path}");
        assert_ne!(r.header("cache-control"), Some("no-store"), "{path}");
    }
    for path in ["/api/", "/api/v2/meta/", "/api/v1x/"] {
        let r = api.get(path).await;
        assert_eq!(r.status, StatusCode::NOT_FOUND, "{path}");
        assert_eq!(
            r.header("content-type"),
            Some("text/html; charset=utf-8"),
            "{path}: the public 404 page"
        );
        assert!(r.body.contains("Nothing here (404)"), "{path}");
    }
    // The public site keeps its own 405.
    let r = api.send(request(Method::POST, "/")).await;
    assert_eq!(r.status, StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(r.header("allow"), Some("GET, HEAD"));
    assert_eq!(r.header("content-type"), Some("text/plain; charset=utf-8"));
    api
}

// ---- the committed OpenAPI snapshot ------------------------------------------------

#[test]
fn openapi_snapshot_is_current() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("openapi.json");
    let current = api::openapi_snapshot();
    if std::env::var_os("FERRITE_UPDATE_OPENAPI").is_some() {
        std::fs::write(&path, &current).unwrap();
        return;
    }
    let committed = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        committed == current,
        "crates/ferrite-http/openapi.json is out of date: run \
         `FERRITE_UPDATE_OPENAPI=1 cargo test -p ferrite-http --test api openapi`"
    );
}

// ---- extractors over a test router -------------------------------------------------

#[derive(Debug, Serialize, Deserialize)]
struct Echo {
    n: u32,
    #[serde(default)]
    s: String,
}

async fn echo(ApiJson(v): ApiJson<Echo>) -> Json<Echo> {
    Json(v)
}

async fn page(ApiQuery(p): ApiQuery<PageParams>) -> Result<String, Problem> {
    Ok(format!("{} {:?}", p.limit()?, p.cursor))
}

fn extractor_router() -> Router {
    Router::new()
        .route("/echo", post(echo))
        .route("/page", get(page))
        .layer(api::body_limit())
}

fn post_json(content_type: Option<&str>, body: impl Into<Body>) -> Request<Body> {
    let mut b = Request::builder().method(Method::POST).uri("/echo");
    if let Some(ct) = content_type {
        b = b.header(header::CONTENT_TYPE, ct);
    }
    b.body(body.into()).unwrap()
}

/// Problems built by extractors carry `no-store` only through the API layers, so the
/// test router checks status, media type and type.
async fn problem(
    router: &Router,
    req: Request<Body>,
    status: u16,
    slug: &str,
) -> serde_json::Value {
    let r = send(router, req).await;
    assert_eq!(r.status.as_u16(), status, "{r:?}");
    assert_eq!(r.header("content-type"), Some("application/problem+json"));
    let body = r.json();
    assert_eq!(body["type"], format!("/api/v1/problems/{slug}"), "{body}");
    body
}

#[tokio::test]
async fn json_extractor_rejections() {
    let router = extractor_router();
    let ok = send(
        &router,
        post_json(
            Some("application/json; charset=utf-8"),
            r#"{"n": 3, "s": "x"}"#,
        ),
    )
    .await;
    assert_eq!(ok.status, StatusCode::OK, "{ok:?}");
    assert_eq!(ok.json(), json!({"n": 3, "s": "x"}));

    for ct in [
        None,
        Some("text/plain"),
        Some("application/x-www-form-urlencoded"),
    ] {
        problem(
            &router,
            post_json(ct, r#"{"n": 1}"#),
            415,
            "unsupported-media-type",
        )
        .await;
    }

    let body = problem(
        &router,
        post_json(Some("application/json"), "{"),
        400,
        "malformed-json",
    )
    .await;
    assert!(body["errors"][0]["field"].is_string(), "{body}");
    let body = problem(
        &router,
        post_json(Some("application/json"), r#"{"n": "seven"}"#),
        400,
        "malformed-json",
    )
    .await;
    assert_eq!(body["errors"][0]["field"], "n", "{body}");
    problem(
        &router,
        post_json(Some("application/json"), r#"{"n": 1} x"#),
        400,
        "malformed-json",
    )
    .await;

    // A NUL anywhere in the document, also in a field the target ignores.
    for doc in [
        r#"{"n": 1, "s": "a\u0000b"}"#,
        r#"{"n": 1, "other": ["\u0000"]}"#,
        r#"{"n": 1, "k\u0000": 1}"#,
    ] {
        problem(
            &router,
            post_json(Some("application/json"), doc),
            422,
            "validation",
        )
        .await;
    }
    // An escaped backslash before `u0000` is text, not a NUL.
    let ok = send(
        &router,
        post_json(Some("application/json"), r#"{"n": 1, "s": "a\\u0000b"}"#),
    )
    .await;
    assert_eq!(ok.status, StatusCode::OK, "{ok:?}");
    assert_eq!(ok.json()["s"], "a\\u0000b");

    // 4 MiB is the limit: 3 MiB passes, 5 MiB is 413.
    let padded = |size: usize| format!(r#"{{"n": 1, "s": "{}"}}"#, "a".repeat(size));
    let ok = send(
        &router,
        post_json(Some("application/json"), padded(3 * 1024 * 1024)),
    )
    .await;
    assert_eq!(ok.status, StatusCode::OK);
    assert_eq!(MAX_JSON_BODY, 4 * 1024 * 1024);
    problem(
        &router,
        post_json(Some("application/json"), padded(5 * 1024 * 1024)),
        413,
        "too-large",
    )
    .await;
}

#[tokio::test]
async fn limit_bounds() {
    let router = extractor_router();
    let get = |uri: &str| Request::builder().uri(uri).body(Body::empty()).unwrap();
    let r = send(&router, get("/page")).await;
    assert_eq!(r.body, "50 None");
    let r = send(&router, get("/page?limit=1&cursor=abc")).await;
    assert_eq!(r.body, "1 Some(\"abc\")");
    let r = send(&router, get("/page?limit=200")).await;
    assert_eq!(r.body, "200 None");
    for bad in ["/page?limit=0", "/page?limit=201"] {
        let body = problem(&router, get(bad), 400, "validation").await;
        assert_eq!(body["errors"][0]["field"], "limit", "{body}");
    }
    for bad in ["/page?limit=-1", "/page?limit=ten"] {
        problem(&router, get(bad), 400, "malformed-query").await;
    }
}
