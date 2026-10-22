//! The one handler behind every public URL.

use std::{convert::Infallible, sync::Arc, time::Duration};

use axum::{
    body::Body,
    extract::{Request, State},
    http::{HeaderValue, Method, Response, StatusCode, header},
};
use ferrite_cache::CacheKey;
use ferrite_domain::Redirect;

use super::{
    AppState,
    error_page::{self, ErrorKind},
    pipeline,
    response::{method_not_allowed, page_response, redirect, text},
};
use crate::route::{NormalQuery, Route, resolve};

/// Longest wait for the `/readyz` database ping.
const READY_TIMEOUT: Duration = Duration::from_secs(2);
/// Longest `Host` header used in a cache key.
const MAX_HOST: usize = 255;

/// GET/HEAD only. Order: health checks → redirects (before the cache) → canonical
/// 301 → page cache → render.
pub(crate) async fn handle(State(state): State<Arc<AppState>>, req: Request) -> Response<Body> {
    if req.method() != Method::GET && req.method() != Method::HEAD {
        return method_not_allowed();
    }
    let path = req.uri().path().to_owned();
    let raw_query = req.uri().query().map(str::to_owned);
    let route = resolve(&path, raw_query.as_deref(), &state.routes.load());
    match route {
        Route::Health => return text(StatusCode::OK, "ok\n"),
        Route::Ready => return ready(&state).await,
        _ => {}
    }
    // `/_media` and `/_theme` URLs are exact and never redirected.
    if !matches!(route, Route::Media { .. } | Route::Theme { .. }) {
        match state.redirects.resolve(&path).await {
            Ok(Some(r)) => return follow(&state, &path, r).await,
            Ok(None) => {}
            Err(e) => {
                tracing::error!(error = %e, "redirect lookup failed");
                let page = error_page::render(&state, &path, ErrorKind::Server).await;
                return page_response(&page, None);
            }
        }
    }
    if let Route::Canonical(to) = &route {
        return match HeaderValue::from_str(to) {
            Ok(location) => redirect(StatusCode::MOVED_PERMANENTLY, location),
            // Unreachable: the router writes visible ASCII only.
            Err(_) => not_found(&state, &path).await,
        };
    }
    let query = NormalQuery::parse(raw_query.as_deref());
    let key = CacheKey::anon(
        cache_host(&req, &state.config.default_host),
        path.clone(),
        query.cache_query(),
    );
    let if_none_match = req
        .headers()
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let render =
        async { Ok::<_, Infallible>(pipeline::render(state.clone(), route, path, query).await) };
    let page = match state.cache.get_or_render(key, render).await {
        Ok(page) => page,
        Err(never) => match *never {},
    };
    page_response(&page, if_none_match.as_deref())
}

async fn not_found(state: &Arc<AppState>, path: &str) -> Response<Body> {
    page_response(
        &error_page::render(state, path, ErrorKind::NotFound).await,
        None,
    )
}

/// Answers with redirect row `r`: its code and target, `410` as the gone page.
async fn follow(state: &Arc<AppState>, path: &str, r: Redirect) -> Response<Body> {
    state.redirects.hit(r.id).await;
    let status = StatusCode::from_u16(r.code)
        .ok()
        .filter(StatusCode::is_redirection);
    match (status, HeaderValue::from_str(&r.target)) {
        (Some(status), Ok(location)) => redirect(status, location),
        _ if r.code == 410 => page_response(
            &error_page::render(state, path, ErrorKind::Gone).await,
            None,
        ),
        _ => {
            tracing::warn!(redirect = %r.id, code = r.code, "unusable redirect row ignored");
            not_found(state, path).await
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

/// The cache-key host: the `Host` header (or the URI authority, HTTP/2), lowercased,
/// without a trailing dot; `default` when missing or not a plausible host name.
/// `X-Forwarded-Host` is ignored (trusted proxies are handled by `serve`).
fn cache_host(req: &Request, default: &str) -> String {
    let raw = req
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .or_else(|| req.uri().authority().map(|a| a.as_str()));
    raw.map(|h| h.trim().trim_end_matches('.').to_ascii_lowercase())
        .filter(|h| {
            !h.is_empty()
                && h.len() <= MAX_HOST
                && h.bytes().all(|b| {
                    b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b':' | b'[' | b']')
                })
        })
        .unwrap_or_else(|| default.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(h: Option<&str>) -> String {
        let mut b = Request::builder().uri("/");
        if let Some(h) = h {
            b = b.header(header::HOST, h);
        }
        cache_host(&b.body(Body::empty()).unwrap(), "example.com")
    }

    #[test]
    fn cache_host_normalises() {
        assert_eq!(host(Some("Example.COM")), "example.com");
        assert_eq!(host(Some("example.com.")), "example.com");
        assert_eq!(host(Some("localhost:8080")), "localhost:8080");
        assert_eq!(host(Some("[::1]:8080")), "[::1]:8080");
        assert_eq!(host(None), "example.com");
        assert_eq!(host(Some("")), "example.com");
        assert_eq!(host(Some("a b")), "example.com");
        assert_eq!(host(Some("evil/path")), "example.com");
        assert_eq!(host(Some(&"a".repeat(300))), "example.com");
    }
}
