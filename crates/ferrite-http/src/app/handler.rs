//! The one handler behind every public URL.

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use axum::{
    body::Body,
    extract::{Request, State},
    http::{HeaderMap, HeaderValue, Method, Response, StatusCode, header},
};
use ferrite_cache::CacheKey;
use ferrite_core::Authenticated;
use ferrite_domain::{ContentId, Redirect};

use super::{
    AppState, LoggedIn,
    error_page::{self, Missing},
    media, pipeline,
    response::{method_not_allowed, noindex, page_response, private_no_store, redirect, text},
    theme_asset,
};
use crate::api::cookie::read_session_cookie;
use crate::plugins::{CacheOutcome, ServedEvent, is_preview_path};
use crate::route::{MAX_PATH_BYTES, NormalQuery, Route, resolve};

/// Longest wait for the `/readyz` database ping.
const READY_TIMEOUT: Duration = Duration::from_secs(2);

/// GET/HEAD only. Order: health checks and assets → previews → redirects (before the
/// cache) → canonical 301 → session cookie → page cache (or none) → render. The
/// cache-key host is always the site's host (R25).
///
/// A request with a session cookie that resolves to a session is logged in
/// (P21): under [`LoggedIn::Bypass`] it is rendered without reading or filling the
/// cache, under [`LoggedIn::PerUser`] cached under the user's own variant; either way
/// the response is `private, no-store`. A cookie that resolves to nothing (unknown,
/// expired, disabled user) or a failed lookup is ignored: the request is anonymous.
/// Without a cookie no session lookup happens.
///
/// Every response after the health, asset and preview branches is a page response
/// (pages, archives, feeds, robots and sitemaps, redirects, 404/410 pages): it is
/// recorded as a `request.served` event when the state has a
/// [`ServedBuffer`](crate::plugins::ServedBuffer) (P21; a non-blocking push). Not
/// recorded: non-GET/HEAD requests, health checks, `/_media` and `/_theme` assets,
/// previews (any path whose first segment decodes to `_preview`, also the 404s), and
/// plugin routes and the API (separate axum routes, never this handler).
pub(crate) async fn handle(State(state): State<Arc<AppState>>, req: Request) -> Response<Body> {
    if req.method() != Method::GET && req.method() != Method::HEAD {
        return method_not_allowed();
    }
    let path = req.uri().path().to_owned();
    let raw_query = req.uri().query().map(str::to_owned);
    let route = resolve(&path, raw_query.as_deref(), &state.routes.load());
    match &route {
        Route::Health => return text(StatusCode::OK, "ok\n"),
        Route::Ready => return ready(&state).await,
        // Exact asset URLs: never redirected, not in the page cache.
        Route::Media { hash, name } => {
            return media::serve(&state, req.method(), req.headers(), hash, name).await;
        }
        Route::Theme { hash, path } => {
            return theme_asset::serve(&state, req.headers(), hash, path);
        }
        // Before the redirect rows, so none can shadow it (F27); never cached.
        Route::Preview { id } => {
            let id = *id;
            return preview(&state, req.headers(), route, path, id).await;
        }
        _ => {}
    }
    let (res, cache) = page(&state, req.headers(), route, path, raw_query).await;
    if let Some(served) = &state.served
        && !is_preview_path(req.uri().path())
    {
        served.record(ServedEvent::new(
            req.uri().path(),
            req.headers(),
            res.status().as_u16(),
            cache,
        ));
    }
    res
}

/// A page response for `route` and how the cache answered it: redirects and logged-in
/// bypass requests skip the cache; otherwise a hit is a request whose render closure
/// did not run (another request's render, or the stored page, served it).
async fn page(
    state: &Arc<AppState>,
    headers: &HeaderMap,
    route: Route,
    path: String,
    raw_query: Option<String>,
) -> (Response<Body>, CacheOutcome) {
    // Redirect rows, before the cache. Longer paths cannot match a row: sources are at
    // most 2048 characters (`RedirectService`), a raw path matches only an ASCII one,
    // and the lookup also tries the path with its trailing slash toggled (+1 byte).
    if path.len() <= MAX_PATH_BYTES + 1 {
        match state.redirects.resolve(&path).await {
            Ok(Some(r)) => return (follow(state, r).await, CacheOutcome::Bypass),
            Ok(None) => {}
            Err(e) => {
                tracing::error!(error = %e, "redirect lookup failed");
                let res = page_response(&error_page::server_error(state).await, None);
                return (res, CacheOutcome::Bypass);
            }
        }
    }
    if let Route::Canonical(to) = &route {
        let res = match HeaderValue::from_str(to) {
            Ok(location) => redirect(StatusCode::MOVED_PERMANENTLY, location),
            // Unreachable: the router writes visible ASCII only.
            Err(_) => missing(state, Missing::NotFound).await,
        };
        return (res, CacheOutcome::Bypass);
    }
    let mut query = NormalQuery::parse(raw_query.as_deref());
    // A comment page exists on single items only: elsewhere `?cpage=N` is ignored, so
    // it adds no cache entries.
    if !route.may_show_item() {
        query.cpage = None;
    }
    let user = session(state, headers).await.map(|who| who.user.id);
    let host = state.config.default_host.clone();
    // Per-user variants are keyed by the user id alone, which is safe only because
    // nothing user-specific reaches the render: plugins get `is_logged_in` and nothing
    // else, templates get no user, role or capability. Any future per-user output (an
    // admin bar, the current user in templates) must add role/capabilities to the key
    // (or bypass) and invalidate `user:<id>` on role change and logout.
    let key = match (user, state.config.logged_in) {
        (None, _) => Some(CacheKey::anon(host, path.clone(), query.cache_query())),
        (Some(user), LoggedIn::PerUser) => Some(CacheKey::user(
            host,
            path.clone(),
            query.cache_query(),
            user,
        )),
        (Some(_), LoggedIn::Bypass) => None,
    };
    let logged_in = user.is_some();
    let if_none_match = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    // The route is resolved again when the render starts, which is inside the cache's
    // flush guard: a route-table swap and its flush (a node reload) that land between
    // the lookup above and the render would otherwise let a page built from the old
    // table be stored after the flush, where no later event drops it.
    let rendered = Arc::new(AtomicBool::new(false));
    let render = {
        let state = state.clone();
        let rendered = rendered.clone();
        async move {
            rendered.store(true, Ordering::Relaxed);
            let route = resolve(&path, raw_query.as_deref(), &state.routes.load());
            pipeline::render(state, route, path, query, logged_in).await
        }
    };
    let (page, cache) = match key {
        Some(key) => {
            let page = state.cache.get_or_render(key, render).await.map_err(drop);
            let cache = if rendered.load(Ordering::Relaxed) {
                CacheOutcome::Miss
            } else {
                CacheOutcome::Hit
            };
            (page, cache)
        }
        None => (
            render.await.map(Arc::new).map_err(drop),
            CacheOutcome::Bypass,
        ),
    };
    let mut res = match page {
        Ok(page) => page_response(&page, if_none_match.as_deref()),
        // Never cached under the path: the shared 404 body (R26).
        Err(()) => missing(state, Missing::NotFound).await,
    };
    if logged_in {
        private_no_store(&mut res);
    }
    (res, cache)
}

/// The session behind the request's session cookie (`__Host-ferrite_session` on an
/// `https` site, `ferrite_session` on `http`; P23), if any. No cookie, no
/// auth service: `None` without touching the store. A lookup error is logged and
/// treated as no session (the request is served anonymously).
pub(crate) async fn session(state: &AppState, headers: &HeaderMap) -> Option<Authenticated> {
    let auth = state.auth.as_ref()?;
    let token = read_session_cookie(headers, state.cookie_secure)?;
    match auth.resolve_session(token).await {
        Ok(who) => who,
        Err(e) => {
            tracing::warn!(error = %e, "session lookup failed; request served anonymously");
            None
        }
    }
}

/// `/_preview/<id>` (P20): the session cookie only (no bearer tokens), an account that
/// need not reset its password, and `SiteReader::preview`'s rights (R17); anything
/// else is the shared 404 page. Never cached; `private, no-store` and `noindex` on
/// every answer, no `ETag`, and `If-None-Match` is ignored (always the full page).
async fn preview(
    state: &Arc<AppState>,
    headers: &HeaderMap,
    route: Route,
    path: String,
    id: ContentId,
) -> Response<Body> {
    let page = match session(state, headers).await {
        Some(who) if !who.user.must_reset => {
            pipeline::preview(state.clone(), route, path, who.actor, id).await
        }
        _ => Err(pipeline::NotFound),
    };
    let mut res = match page {
        Ok(page) => page_response(&page, None),
        Err(pipeline::NotFound) => missing(state, Missing::NotFound).await,
    };
    // `no-store`: nothing to revalidate, so no validator (and `If-None-Match` is
    // ignored above).
    res.headers_mut().remove(header::ETAG);
    private_no_store(&mut res);
    noindex(&mut res);
    res
}

async fn missing(state: &Arc<AppState>, kind: Missing) -> Response<Body> {
    page_response(&*error_page::missing(state, kind).await, None)
}

/// Answers with redirect row `r`: its code and target, `410` as the gone page.
async fn follow(state: &Arc<AppState>, r: Redirect) -> Response<Body> {
    // Buffered, never I/O: the hit count must not delay the redirect.
    if let Some(hits) = &state.hits {
        hits.record(r.id);
    }
    let status = StatusCode::from_u16(r.code)
        .ok()
        .filter(StatusCode::is_redirection);
    match (status, HeaderValue::from_str(&r.target)) {
        (Some(status), Ok(location)) => redirect(status, location),
        _ if r.code == 410 => missing(state, Missing::Gone).await,
        _ => {
            tracing::warn!(redirect = %r.id, code = r.code, "unusable redirect row ignored");
            missing(state, Missing::NotFound).await
        }
    }
}

/// 200 when a cheap store read succeeds and migrations are compatible; else 503. The
/// theme is part of the state, so a running app always has one loaded.
async fn ready(state: &AppState) -> Response<Body> {
    let ping = tokio::time::timeout(READY_TIMEOUT, state.store.read().events().max_seq()).await;
    let db = matches!(ping, Ok(Ok(_)));
    if !db {
        tracing::warn!("readiness: database ping failed");
    }
    if db && state.migrations_ok {
        text(StatusCode::OK, "ready\n")
    } else {
        text(StatusCode::SERVICE_UNAVAILABLE, "not ready\n")
    }
}
