//! The one handler behind every public URL.

use std::{sync::Arc, time::Duration};

use axum::{
    body::Body,
    extract::{Request, State},
    http::{HeaderValue, Method, Response, StatusCode, header},
};
use ferrite_cache::CacheKey;
use ferrite_domain::Redirect;

use super::{
    AppState,
    error_page::{self, Missing},
    media, pipeline,
    response::{method_not_allowed, page_response, redirect, text},
    theme_asset,
};
use crate::route::{NormalQuery, Route, resolve};

/// Longest wait for the `/readyz` database ping.
const READY_TIMEOUT: Duration = Duration::from_secs(2);

/// GET/HEAD only. Order: health checks → redirects (before the cache) → canonical
/// 301 → page cache → render. The cache-key host is always the site's host (R25).
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
        _ => {}
    }
    {
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
    let key = CacheKey::anon(
        state.config.default_host.clone(),
        path.clone(),
        query.cache_query(),
    );
    let if_none_match = req
        .headers()
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let render = pipeline::render(state.clone(), route, path, query);
    match state.cache.get_or_render(key, render).await {
        Ok(page) => page_response(&page, if_none_match.as_deref()),
        // Never cached under the path: the shared 404 body (R26).
        Err(_not_found) => missing(&state, Missing::NotFound).await,
    }
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
