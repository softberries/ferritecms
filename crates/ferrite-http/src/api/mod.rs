//! The REST API under `/api/v1` (07 §1; rulings P10, P12, P24).
//!
//! [`router`] builds it with utoipa-axum: handlers carry `#[utoipa::path]`s relative to
//! `/api/v1` and are registered with `routes!` on an [`OpenApiRouter`] nested at
//! [`BASE`], so the spec and the routes come from the same list. Around them:
//!
//! * `/api/v1`, `/api/v1/` and `/api/v1/{*rest}` that match no route: `404 not-found`
//!   (any method). Paths outside `/api/v1` (`/api`, `/api/v2/…`) stay the public site's.
//! * A known path with another method: `405 method-not-allowed` with `Allow`.
//! * A panicking handler: `500 internal` (the public site's HTML 500 never shows for
//!   the API).
//! * `Cache-Control: no-store` on every API response, and request bodies of at most
//!   [`MAX_JSON_BODY`] ([`body_limit`]).
//!
//! The API never touches the page cache. [`build_site`](crate::build_site) merges this
//! router in front of the public fallback, inside the shared layers (security headers,
//! compression, request span, client address, the outer catch-panic).
//!
//! Authentication (session cookie with CSRF and `Origin` checks, bearer tokens) is the
//! [`auth`] extractors; `GET /meta` and `GET /openapi.json` need none.

pub mod auth;
pub mod content;
pub mod cookie;
pub mod csv;
pub mod dto;
pub mod extract;
pub mod list;
pub mod menus;
pub mod meta;
pub mod openapi;
pub mod problem;
pub mod redirects;
pub mod settings;
mod state;
pub mod taxonomies;
pub mod types;
pub mod users;

use std::sync::Arc;

use axum::{
    Extension, Router,
    extract::DefaultBodyLimit,
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::any,
};
use tower_http::{catch_panic::CatchPanicLayer, set_header::SetResponseHeaderLayer};
use utoipa::OpenApi as _;
use utoipa_axum::{router::OpenApiRouter, routes};

pub use auth::{Authed, AuthedAllowReset, CSRF_HEADER, ClientInfo, SameOrigin};
pub use extract::{
    ApiJson, ApiPath, ApiQuery, DEFAULT_LIMIT, IfMatch, MAX_JSON_BODY, PageParams, check_limit,
    created, created_at, etag, invalid_cursor, invalid_param,
};
pub use problem::{FieldError, Problem};
pub use state::{ApiParts, ApiState, InvalidSiteUrl};

use self::{meta::SpecJson, openapi::ApiDoc};

/// Where the API is mounted.
pub const BASE: &str = "/api/v1";

/// Every endpoint, relative to [`BASE`]. Later tasks add their `routes!` here.
fn v1() -> OpenApiRouter<Arc<ApiState>> {
    OpenApiRouter::new()
        .routes(routes!(meta::meta))
        .routes(routes!(meta::openapi_json))
        .routes(routes!(auth::login))
        .routes(routes!(auth::login_totp))
        .routes(routes!(auth::logout))
        .routes(routes!(auth::me))
        .routes(routes!(auth::change_password))
        .routes(routes!(auth::totp_setup))
        .routes(routes!(auth::totp_confirm))
        .routes(routes!(auth::totp_disable))
        .routes(routes!(content::list_content, content::create_content))
        .routes(routes!(
            content::get_content,
            content::update_content,
            content::delete_content
        ))
        .routes(routes!(content::publish_content))
        .routes(routes!(content::unpublish_content))
        .routes(routes!(content::submit_content))
        .routes(routes!(content::restore_content))
        .routes(routes!(content::schedule_content))
        .routes(routes!(content::list_revisions))
        .routes(routes!(content::get_revision))
        .routes(routes!(content::restore_revision))
        .routes(routes!(types::list_types, types::create_type))
        .routes(routes!(
            types::get_type,
            types::update_type,
            types::delete_type
        ))
        .routes(routes!(
            taxonomies::list_taxonomies,
            taxonomies::create_taxonomy
        ))
        .routes(routes!(
            taxonomies::get_taxonomy,
            taxonomies::update_taxonomy,
            taxonomies::delete_taxonomy
        ))
        .routes(routes!(taxonomies::list_terms, taxonomies::create_term))
        .routes(routes!(
            taxonomies::get_term,
            taxonomies::update_term,
            taxonomies::delete_term
        ))
        .routes(routes!(menus::list_menus))
        .routes(routes!(
            menus::get_menu,
            menus::put_menu,
            menus::delete_menu
        ))
        .routes(routes!(
            redirects::list_redirects,
            redirects::create_redirect
        ))
        .routes(routes!(redirects::import_redirects))
        .routes(routes!(redirects::export_redirects))
        .routes(routes!(
            redirects::get_redirect,
            redirects::update_redirect,
            redirects::delete_redirect
        ))
        .routes(routes!(settings::get_settings, settings::update_settings))
        .routes(routes!(users::list_users, users::create_user))
        .routes(routes!(
            users::get_user,
            users::update_user,
            users::delete_user
        ))
        .routes(routes!(users::set_user_password))
        .routes(routes!(users::list_tokens, users::create_token))
        .routes(routes!(users::revoke_token))
}

/// The spec and routes, before the state and layers.
fn parts() -> (Router<Arc<ApiState>>, utoipa::openapi::OpenApi) {
    OpenApiRouter::with_openapi(ApiDoc::openapi())
        .nest(BASE, v1())
        .split_for_parts()
}

/// The OpenAPI document of the API.
pub fn openapi() -> utoipa::openapi::OpenApi {
    parts().1
}

/// The committed snapshot's text: [`openapi`] pretty-printed, with a final newline.
pub fn openapi_snapshot() -> String {
    match openapi().to_pretty_json() {
        Ok(mut s) => {
            s.push('\n');
            s
        }
        Err(e) => format!("cannot serialize the OpenAPI document: {e}\n"),
    }
}

/// The API's routes (see the module docs) and its OpenAPI document.
pub fn router(state: Arc<ApiState>) -> (Router, utoipa::openapi::OpenApi) {
    let (routes, spec) = parts();
    let json = spec.to_json().unwrap_or_else(|e| {
        tracing::error!(error = %e, "cannot serialize the OpenAPI document");
        String::new()
    });
    let router = routes
        .route(BASE, any(not_found))
        .route(&format!("{BASE}/"), any(not_found))
        .route(&format!("{BASE}/{{*rest}}"), any(not_found))
        .method_not_allowed_fallback(method_not_allowed)
        .with_state(state)
        .layer(Extension(SpecJson(json.into())));
    (layers(router), spec)
}

/// The request-body limit of every API route ([`MAX_JSON_BODY`]).
pub fn body_limit() -> DefaultBodyLimit {
    DefaultBodyLimit::max(MAX_JSON_BODY)
}

/// The API-only layers: body limit, problem 500 on panic, `Cache-Control: no-store`.
fn layers(router: Router) -> Router {
    router
        .layer(body_limit())
        .layer(CatchPanicLayer::custom(panic_problem))
        .layer(SetResponseHeaderLayer::overriding(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-store"),
        ))
}

async fn not_found() -> Problem {
    Problem::new(
        StatusCode::NOT_FOUND,
        "not-found",
        "Not found",
        "no such API endpoint",
    )
}

async fn method_not_allowed() -> Problem {
    Problem::new(
        StatusCode::METHOD_NOT_ALLOWED,
        "method-not-allowed",
        "Method not allowed",
        "this endpoint does not accept that method (see Allow)",
    )
}

/// A panicking API handler: `500 internal`. The payload is not logged (it may echo
/// request data).
fn panic_problem(_panic: Box<dyn std::any::Any + Send + 'static>) -> Response {
    tracing::error!("API handler panicked; 500 problem served");
    Problem::internal().into_response()
}

#[cfg(test)]
mod tests {
    use axum::{body::Body, http::Request, routing::get};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use super::*;

    #[tokio::test]
    async fn a_panicking_api_handler_is_a_problem_500() {
        async fn boom() -> &'static str {
            panic!("handler bug with secret")
        }
        let app = layers(Router::new().route("/boom", get(boom)));
        let req = Request::builder().uri("/boom").body(Body::empty()).unwrap();
        let res = app.oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(res.headers()[header::CONTENT_TYPE], problem::PROBLEM_JSON);
        assert_eq!(res.headers()[header::CACHE_CONTROL], "no-store");
        let body = res.into_body().collect().await.unwrap().to_bytes();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["type"], "/api/v1/problems/internal");
        assert!(!body.to_string().contains("secret"));
    }

    #[test]
    fn every_path_is_under_the_base() {
        let spec = openapi();
        assert!(!spec.paths.paths.is_empty());
        for path in spec.paths.paths.keys() {
            assert!(path.starts_with("/api/v1/"), "{path}");
        }
    }
}
