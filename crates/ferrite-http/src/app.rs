//! The axum application for the public site (Task 8a).
//!
//! [`build_app`] returns a [`Router`] with a single fallback handler; every public URL
//! goes through it:
//!
//! 1. Methods other than GET and HEAD: `405` with `Allow: GET, HEAD`. HEAD is answered
//!    like GET (same status and headers, compression included); axum's `Route` drops
//!    the body.
//! 2. [`resolve`](crate::route::resolve) gives the route. `/healthz` is `200 ok`;
//!    `/readyz` is `200` when a store read succeeds (2 s timeout) and the startup
//!    migration check passed, else `503`.
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
//!    failed or whose theme helper reads failed. Previews (never cached) need
//!    authentication, which does not exist yet.
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
mod error_page;
pub mod feeds;
mod handler;
pub mod media;
mod pipeline;
mod response;
mod state;
pub mod theme_asset;

use std::sync::Arc;

use axum::{Router, http::HeaderValue};
use tower_http::{
    catch_panic::CatchPanicLayer,
    compression::{CompressionLayer, DefaultPredicate, Predicate},
    set_header::SetResponseHeaderLayer,
    trace::TraceLayer,
};

pub use state::{
    AppParts, AppState, DESCRIPTION_OPTION, HttpConfig, LOCALE_OPTION, RENDER_WAIT, SHORT_TTL,
    TITLE_OPTION, load_route_table, theme_settings_option,
};

/// The public site as an axum [`Router`]; see the module docs.
pub fn build_app(state: Arc<AppState>) -> Router {
    let state_proxies = state.config.trusted_proxies.clone();
    let mut app = Router::new()
        .fallback(handler::handle)
        .with_state(state)
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
    let trusted: Arc<[ipnet::IpNet]> = state_proxies.into();
    app.layer(
        TraceLayer::new_for_http()
            .make_span_with(response::request_span)
            .on_response(response::record_response),
    )
    // Outermost: the client address is known before the request span opens.
    .layer(axum::middleware::from_fn_with_state(
        trusted,
        client::record_client,
    ))
}
