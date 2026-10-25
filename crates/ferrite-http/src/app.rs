//! The axum application for the public site.
//!
//! [`build_app`] returns a [`Router`] with a single fallback handler; every public URL
//! goes through it, except plugin routes under `/_p/<plugin>/` (see `app/plugin_route.rs`:
//! own checks, never cached) and the theme comment form `POST /_comment` (see
//! `app/comment_form.rs`: same-origin, anonymous, never cached), mounted in front of it. [`build_site`] (what `serve` runs) puts the API's `/api/v1` routes
//! ([`crate::api`]) in front of the same fallback, inside the same layers:
//!
//! 1. Methods other than GET and HEAD: `405` with `Allow: GET, HEAD`. HEAD is answered
//!    like GET (same status and headers, compression included); axum's `Route` drops
//!    the body.
//! 2. [`resolve`](crate::route::resolve) gives the route. `/healthz` is `200 ok`;
//!    `/readyz` is `200` when a store read succeeds (2 s timeout) and the startup
//!    migration check passed, else `503`. `/_preview/<id>` (P20) is answered here,
//!    before redirect rows and never through the cache: the session cookie
//!    (bearer tokens are not accepted), an account without `must_reset`, and
//!    `SiteReader::preview` (R17 `can_read`); the item is rendered like a single page
//!    with the plugin preview scope and `request.preview = true`. Every failure (no or
//!    bad session, no right, trashed, missing, malformed id) is the shared 404 page.
//!    Every preview answer has `Cache-Control: private, no-store` and
//!    `X-Robots-Tag: noindex`.
//! 3. Redirect rows (exact path, then the trailing slash toggled), **before the cache**:
//!    301/302/307/308 to the row's target, `410` as the shared gone page. The hit is
//!    counted in a spawned task (fire and forget).
//! 4. A non-canonical path: `301` to the canonical one (query kept).
//! 5. The page cache ([`PageCache::get_or_render`](ferrite_cache::PageCache), single
//!    flight). Key: the site's host from the config (ruling R25: never the `Host` or
//!    `X-Forwarded-Host` header, so clients cannot multiply entries), the canonical
//!    path, and [`NormalQuery::cache_query`](crate::route::NormalQuery::cache_query).
//!    Never cached under the request path: error statuses (404/410/500/503). TTL 60 s
//!    ([`HttpConfig`]): search results, and pages whose plugin calls were volatile or
//!    failed or whose theme helper reads failed. **Logged-in requests (P21)**: a
//!    session cookie (`__Host-ferrite_session` on https, P23) that resolves to a session (looked up only when the
//!    cookie is present; an unusable one is ignored, the request stays anonymous) makes
//!    the request logged in: with [`LoggedIn::Bypass`] (default) it renders without
//!    reading or filling the cache, with [`LoggedIn::PerUser`] it uses the user's own
//!    cache variant; both answer `Cache-Control: private, no-store`, and plugins see
//!    `is_logged_in = true`.
//! 6. The render pipeline (06 §5): the candidates' items through `SiteReader`
//!    (published-only; a hit counts only if its URL equals the request path), plugin
//!    filters `content.body`/`content.excerpt` on every item, the head (core SEO +
//!    `render.head`), the template hierarchy, and `Renderer::render` on a blocking
//!    thread with a store-backed `HelperBridge` (`Handle::block_on`). The page's
//!    dependency keys are the reads', the plugin trace's, the renderer's,
//!    `option:theme.<name>.settings`, `theme` and `plugins`.
//! 7. A weak `ETag` (`W/"…"`) on every page and on its 304; `If-None-Match` → `304`
//!    (weak comparison). Every HTML response, 304 included, has
//!    `Vary: Accept-Encoding`.
//!
//! **Render limits (ruling R28).** At most [`HttpConfig::max_concurrent_renders`]
//! template renders run at once (default 2 × available parallelism); a render waits up
//! to [`HttpConfig::render_wait`] (5 s) for a slot, then the request gets a built-in
//! `503` with `Retry-After: 5` (never cached).
//!
//! Error pages: 404 and 410 are **shared bodies** (ruling R26): rendered without the
//! request path through the theme hierarchy (`404.html`, else `index.html`) and cached
//! under internal keys with their dependency keys, so random missing URLs render once.
//! 500 uses the theme's `500.html` with minimal context, else a built-in page. A failing
//! error-page render falls back to the built-in page. Template errors are logged with
//! `templates/<file>:<line>`. A panicking handler gets the built-in 500 (tower-http
//! `CatchPanic`).
//!
//! Every response gets `X-Content-Type-Options: nosniff`,
//! `Referrer-Policy: strict-origin-when-cross-origin` and `X-Frame-Options: SAMEORIGIN`
//! unless it sets them itself; bodies are compressed (br, gzip) per `Accept-Encoding`
//! with tower-http's default predicate (no images, nothing under 32 bytes). Each request
//! runs in a `request` span with method, path, status, latency (never the query) and
//! the client address (`client_ip`; `X-Forwarded-For` only from
//! `server.trusted_proxies`, see [`client`]).
//!
//! **406 (tower-http 0.7 compression).** A request whose `Accept-Encoding` rules out
//! every encoding we offer, identity included (`identity;q=0` alone, or `*;q=0`), gets
//! its status replaced by `406 Not Acceptable` (body and headers kept, `Vary` added),
//! per RFC 9110 §12.5.3. We keep that behaviour; no override.
//!
//! Assets: `/_media/{hash}/{name}` (see [`media`]) and `/_theme/{hash}/{path}` (see
//! [`theme_asset`]) are answered before redirects and the page cache, with a plain 404
//! when unknown; media and binary theme assets are not compressed. Feeds, sitemaps and
//! `robots.txt` (see [`feeds`]) go through the page cache like pages.

mod bridge;
pub mod client;
mod comment_form;
mod error_page;
pub mod feeds;
mod handler;
pub mod media;
mod pipeline;
mod plugin_route;
mod response;
mod state;
pub mod theme_asset;

use std::sync::Arc;

use axum::{
    Router,
    http::HeaderValue,
    routing::{any, post},
};
use tower_http::{
    catch_panic::CatchPanicLayer,
    compression::{CompressionLayer, DefaultPredicate, Predicate},
    set_header::SetResponseHeaderLayer,
    trace::TraceLayer,
};

use crate::api::ApiState;

pub use state::{
    AppParts, AppState, DESCRIPTION_OPTION, HttpConfig, LOCALE_OPTION, LoggedIn, RENDER_WAIT,
    SHORT_TTL, TITLE_OPTION, load_route_table, theme_settings_option,
};

/// The public site as an axum [`Router`]; see the module docs. Without the API (tests);
/// `serve` uses [`build_site`].
pub fn build_app(state: Arc<AppState>) -> Router {
    shared_layers(&state, public(&state))
}

/// The whole site: `/api/v1` ([`api::router`](crate::api::router)) in front of the
/// public fallback, both inside the shared layers (catch-panic, compression, security
/// headers, request span, client address). API responses also get `no-store`, and a
/// panic in an API handler is a problem document (the API's own layers).
pub fn build_site(state: Arc<AppState>, api: Arc<ApiState>) -> Router {
    let (api, _) = crate::api::router(api);
    shared_layers(&state, api.merge(public(&state)))
}

/// Plugin routes ([`plugin_route`]; `/_p/{plugin}/` too, since the wildcard does not
/// match an empty rest, F25) and the theme comment form ([`comment_form`], POST only;
/// other methods get a `no-store` `405` with `Allow: POST`) in front of the public
/// fallback.
fn public(state: &Arc<AppState>) -> Router {
    Router::new()
        .route(
            ferrite_core::comments::COMMENT_FORM_PATH,
            post(comment_form::handle).fallback(comment_form::method_not_allowed),
        )
        .route("/_p/{plugin}/", any(plugin_route::handle))
        .route("/_p/{plugin}/{*path}", any(plugin_route::handle))
        .fallback(handler::handle)
        .with_state(state.clone())
}

/// The layers every response passes, inner to outer (see the module docs).
fn shared_layers(state: &AppState, router: Router) -> Router {
    let mut app = router
        .layer(CatchPanicLayer::custom(response::panic_response))
        .layer(
            CompressionLayer::new()
                .br(true)
                .gzip(true)
                .compress_when(DefaultPredicate::new().and(response::compressible)),
        );
    for (name, value) in response::SECURITY_HEADERS {
        app = app.layer(SetResponseHeaderLayer::if_not_present(
            name,
            HeaderValue::from_static(value),
        ));
    }
    let resolver = Arc::new(client::ClientResolver::new(
        state.config.trusted_proxies.clone().into(),
    ));
    app.layer(
        TraceLayer::new_for_http()
            .make_span_with(response::request_span)
            .on_response(response::record_response),
    )
    // Outermost: the client address is known before the request span opens.
    .layer(axum::middleware::from_fn_with_state(
        resolver,
        client::record_client,
    ))
}
