//! Route sweep over the generated OpenAPI document (M2a-1 R26, M7): every operation with
//! an unsafe method refuses a request without credentials (`401`) and a cookie request
//! without `X-CSRF-Token` (`403 csrf`), so a future handler that forgets `Authed` fails
//! here. Path parameters get dummy values, both well-formed and malformed, and the body
//! is not valid JSON: authentication must refuse before any path, query or body
//! extractor runs.

use axum::{
    body::Body,
    http::{Method, header},
};
use ferrite_domain::Role;
use ferrite_http::api;
use serde_json::Value;

use crate::{
    api_tests,
    harness::{Api, Resp, builder},
};

api_tests!(unsafe_operations_need_auth_and_csrf);

/// The operations that are reachable without credentials by design: they check the
/// origin instead (`SameOrigin`, login CSRF).
const UNAUTHENTICATED: [&str; 2] = ["POST /api/v1/auth/login", "POST /api/v1/auth/login/totp"];

/// Every `(method, path template)` with an unsafe method in the OpenAPI document.
fn unsafe_operations() -> Vec<(Method, String)> {
    let doc = serde_json::to_value(api::openapi()).unwrap();
    let mut ops = Vec::new();
    for (path, item) in doc["paths"].as_object().unwrap() {
        for (method, _) in item.as_object().unwrap() {
            let method = match method.as_str() {
                "post" => Method::POST,
                "put" => Method::PUT,
                "patch" => Method::PATCH,
                "delete" => Method::DELETE,
                _ => continue,
            };
            ops.push((method, path.clone()));
        }
    }
    ops
}

/// `template` with every `{param}` replaced: `type` by `post` (a registered type, so a
/// handler that loaded it would get further), every other parameter by `value`.
fn fill(template: &str, value: &str) -> String {
    template
        .split('/')
        .map(
            |seg| match seg.strip_prefix('{').and_then(|s| s.strip_suffix('}')) {
                Some("type") => "post",
                Some(_) => value,
                None => seg,
            },
        )
        .collect::<Vec<_>>()
        .join("/")
}

/// The paths one template is tried at: a well-formed uuid for every parameter, and a
/// malformed one (a uuid parameter would be `404` if validated first).
fn paths(template: &str) -> [String; 2] {
    [
        fill(template, "0192b7a0-0000-7000-8000-000000000000"),
        fill(template, "not-a-uuid"),
    ]
}

async fn send_raw(api: &Api, method: &Method, path: &str, cookie: Option<&str>) -> Resp {
    let mut b = builder(method.clone(), path)
        .header(header::ORIGIN, crate::harness::SITE_URL)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(c) = cookie {
        b = b.header(header::COOKIE, format!("ferrite_session={c}"));
    }
    // Not JSON: a body extractor that ran first would answer 400.
    api.send(b.body(Body::from("{not json")).unwrap()).await
}

async fn unsafe_operations_need_auth_and_csrf(api: Api) -> Api {
    let ops = unsafe_operations();
    let skipped: Vec<String> = ops
        .iter()
        .map(|(m, p)| format!("{m} {p}"))
        .filter(|op| UNAUTHENTICATED.contains(&op.as_str()))
        .collect();
    // The exemptions exist, so a renamed login route cannot silently drop out.
    assert_eq!(skipped, UNAUTHENTICATED, "{ops:?}");
    let checked: Vec<&(Method, String)> = ops
        .iter()
        .filter(|(m, p)| !UNAUTHENTICATED.contains(&format!("{m} {p}").as_str()))
        .collect();
    assert!(checked.len() >= 30, "{} unsafe operations", checked.len());

    api.user("sweeper", Role::Admin, false).await;
    let session = api.logged_in("sweeper").await;
    let cookie = session.cookie.clone().unwrap();

    for (method, template) in &checked {
        for path in paths(template) {
            let r = send_raw(&api, method, &path, None).await;
            assert_eq!(r.status.as_u16(), 401, "{method} {path}: {r:?}");
            let p: Value = r.assert_problem(401, "unauthenticated");
            assert!(
                r.header("www-authenticate").is_some(),
                "{method} {path}: {p}"
            );
            let r = send_raw(&api, method, &path, Some(&cookie)).await;
            assert_eq!(r.status.as_u16(), 403, "{method} {path}: {r:?}");
            let p = r.assert_problem(403, "csrf");
            assert!(p["detail"].is_string(), "{method} {path}: {p}");
        }
    }
    // The session is still valid: nothing above reached a handler that could end it.
    let r = api
        .send(
            builder(Method::GET, "/api/v1/auth/me")
                .header(header::COOKIE, format!("ferrite_session={cookie}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(r.status, axum::http::StatusCode::OK, "{r:?}");
    api
}
