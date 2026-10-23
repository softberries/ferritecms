//! The one handler behind every public URL.

use std::{sync::Arc, time::Duration};

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
use crate::route::{MAX_PATH_BYTES, NormalQuery, Route, resolve};

/// Longest wait for the `/readyz` database ping.
const READY_TIMEOUT: Duration = Duration::from_secs(2);

/// GET/HEAD only. Order: health checks and assets → previews → redirects (before the
/// cache) → canonical 301 → session cookie → page cache (or none) → render. The
/// cache-key host is always the site's host (R25).
///
/// A request with a `ferrite_session` cookie that resolves to a session is logged in
/// (P21): under [`LoggedIn::Bypass`] it is rendered without reading or filling the
/// cache, under [`LoggedIn::PerUser`] cached under the user's own variant; either way
/// the response is `private, no-store`. A cookie that resolves to nothing (unknown,
/// expired, disabled user) or a failed lookup is ignored: the request is anonymous.
/// Without a cookie no session lookup happens.
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
    // Redirect rows, before the cache. Longer paths cannot match a row: sources are at
    // most 2048 characters (`RedirectService`), a raw path matches only an ASCII one,
    // and the lookup also tries the path with its trailing slash toggled (+1 byte).
    if path.len() <= MAX_PATH_BYTES + 1 {
        match state.redirects.resolve(&path).await {
            Ok(Some(r)) => return follow(&state, r).await,
            Ok(None) => {}
            Err(e) => {
                tracing::error!(error = %e, "redirect lookup failed");
                return page_response(&error_page::server_error(&state).await, None);
            }
        }
    }
    if let Route::Canonical(to) = &route {
        return match HeaderValue::from_str(to) {
            Ok(location) => redirect(StatusCode::MOVED_PERMANENTLY, location),
            // Unreachable: the router writes visible ASCII only.
            Err(_) => missing(&state, Missing::NotFound).await,
        };
    }
    let query = NormalQuery::parse(raw_query.as_deref());
    let user = session(&state, req.headers()).await.map(|who| who.user.id);
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
    let if_none_match = req
        .headers()
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    // The route is resolved again when the render starts, which is inside the cache's
    // flush guard: a route-table swap and its flush (a node reload) that land between
    // the lookup above and the render would otherwise let a page built from the old
    // table be stored after the flush, where no later event drops it.
    let render = {
        let state = state.clone();
        async move {
            let route = resolve(&path, raw_query.as_deref(), &state.routes.load());
            pipeline::render(state, route, path, query, logged_in).await
        }
    };
    let page = match key {
        Some(key) => state.cache.get_or_render(key, render).await.map_err(drop),
        None => render.await.map(Arc::new).map_err(drop),
    };
    let mut res = match page {
        Ok(page) => page_response(&page, if_none_match.as_deref()),
        // Never cached under the path: the shared 404 body (R26).
        Err(()) => missing(&state, Missing::NotFound).await,
    };
    if logged_in {
        private_no_store(&mut res);
    }
    res
}

/// The session behind the request's `ferrite_session` cookie, if any. No cookie, no
/// auth service: `None` without touching the store. A lookup error is logged and
/// treated as no session (the request is served anonymously).
async fn session(state: &AppState, headers: &HeaderMap) -> Option<Authenticated> {
    let auth = state.auth.as_ref()?;
    let token = read_session_cookie(headers)?;
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
    // Fire and forget: the hit count must not delay the redirect (`hit` logs failures).
    let redirects = state.redirects.clone();
    let id = r.id;
    tokio::spawn(async move { redirects.hit(id).await });
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
