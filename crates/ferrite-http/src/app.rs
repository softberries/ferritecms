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
//!    301/302/307/308 to the row's target, `410` as the gone page.
//! 4. A non-canonical path: `301` to the canonical one (query kept).
//! 5. The page cache ([`PageCache::get_or_render`](ferrite_cache::PageCache), single
//!    flight). Key: host from the `Host` header (lowercased; the site host when absent
//!    or implausible; `X-Forwarded-Host` ignored), the canonical path, and
//!    [`NormalQuery::cache_query`](crate::route::NormalQuery::cache_query). Not cached:
//!    error statuses (404/410/500). TTL 60 s ([`HttpConfig`]): search results, and
//!    pages whose plugin calls were volatile or failed or whose theme helper reads
//!    failed. Previews (never cached) need authentication, which does not exist yet.
//! 6. The render pipeline (06 §5): the candidates' items through `SiteReader`
//!    (published-only; a hit counts only if its URL equals the request path), plugin
//!    filters `content.body`/`content.excerpt` on every item, the head (core SEO +
//!    `render.head`), the template hierarchy, and `Renderer::render` on a blocking
//!    thread with a store-backed `HelperBridge` (`Handle::block_on`). The page's
//!    dependency keys are the reads', the plugin trace's, the renderer's,
//!    `option:theme.<name>.settings`, `theme` and `plugins`.
//! 7. `ETag` on every page (weak, `W/…`, when the body is compressed);
//!    `If-None-Match` → `304` (weak comparison).
//!
//! Error pages: 404 through the theme hierarchy (`404.html`, else `index.html`), 500
//! through the theme's `500.html` with minimal context, else a built-in page; a failing
//! error-page render falls back to the built-in page. Template errors are logged with
//! `templates/<file>:<line>`.
//!
//! Every response gets `X-Content-Type-Options: nosniff`,
//! `Referrer-Policy: strict-origin-when-cross-origin` and `X-Frame-Options: SAMEORIGIN`
//! unless it sets them itself; bodies are compressed (br, gzip) per `Accept-Encoding`
//! with tower-http's default predicate (no images, nothing under 32 bytes). Each request
//! runs in a `request` span with method, path, status and latency (never the query).
//!
//! **Not yet (Task 8b):** `/_media`, `/_theme`, feeds, sitemaps and `robots.txt` answer
//! with the 404 page.

mod bridge;
mod error_page;
mod handler;
mod pipeline;
mod response;
mod state;

use std::sync::Arc;

use axum::{Router, http::HeaderValue, middleware};
use tower_http::{
    compression::CompressionLayer, set_header::SetResponseHeaderLayer, trace::TraceLayer,
};

pub use state::{
    AppParts, AppState, DESCRIPTION_OPTION, HttpConfig, LOCALE_OPTION, SHORT_TTL, TITLE_OPTION,
    load_route_table, theme_settings_option,
};

/// The public site as an axum [`Router`]; see the module docs.
pub fn build_app(state: Arc<AppState>) -> Router {
    let mut app = Router::new()
        .fallback(handler::handle)
        .with_state(state)
        .layer(CompressionLayer::new().br(true).gzip(true));
    for (name, value) in response::SECURITY_HEADERS {
        app = app.layer(SetResponseHeaderLayer::if_not_present(
            name,
            HeaderValue::from_static(value),
        ));
    }
    app.layer(middleware::from_fn(response::weaken_etag)).layer(
        TraceLayer::new_for_http()
            .make_span_with(response::request_span)
            .on_response(response::record_response),
    )
}
