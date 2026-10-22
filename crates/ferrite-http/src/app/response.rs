//! Turning pages into responses, and the middleware stack.

use std::time::Duration;

use axum::{
    body::Body,
    extract::Request,
    http::{HeaderName, HeaderValue, Response, StatusCode, header},
    middleware::Next,
};
use ferrite_cache::{CachedPage, if_none_match_matches};
use tracing::Span;

/// `Content-Type` of every HTML page.
pub(crate) const HTML: &str = "text/html; charset=utf-8";
const TEXT: &str = "text/plain; charset=utf-8";

/// Headers added to every response that does not set them itself (HTML pages, error
/// pages, health checks).
pub(crate) const SECURITY_HEADERS: [(HeaderName, &str); 3] = [
    (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
    (header::REFERRER_POLICY, "strict-origin-when-cross-origin"),
    (header::X_FRAME_OPTIONS, "SAMEORIGIN"),
];

/// `page` as a response; `304 Not Modified` (no body) when it is a `200` and
/// `if_none_match` matches its ETag. Every non-error page carries its `ETag`.
pub(crate) fn page_response(page: &CachedPage, if_none_match: Option<&str>) -> Response<Body> {
    let ok = page.status < 400;
    if page.status == 200 && if_none_match.is_some_and(|v| if_none_match_matches(v, &page.etag)) {
        let mut res = Response::new(Body::empty());
        *res.status_mut() = StatusCode::NOT_MODIFIED;
        insert(&mut res, header::ETAG, &page.etag);
        return res;
    }
    let mut res = Response::new(Body::from(page.body.clone()));
    *res.status_mut() =
        StatusCode::from_u16(page.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    for (name, value) in &page.headers {
        if let (Ok(name), Ok(value)) = (
            HeaderName::from_bytes(name.as_bytes()),
            HeaderValue::from_str(value),
        ) {
            res.headers_mut().insert(name, value);
        }
    }
    if ok {
        insert(&mut res, header::ETAG, &page.etag);
    }
    res
}

/// A plain-text response.
pub(crate) fn text(status: StatusCode, body: &'static str) -> Response<Body> {
    let mut res = Response::new(Body::from(body));
    *res.status_mut() = status;
    insert(&mut res, header::CONTENT_TYPE, TEXT);
    insert(&mut res, header::CACHE_CONTROL, "no-store");
    res
}

/// `405` naming the allowed methods.
pub(crate) fn method_not_allowed() -> Response<Body> {
    let mut res = text(StatusCode::METHOD_NOT_ALLOWED, "method not allowed\n");
    insert(&mut res, header::ALLOW, "GET, HEAD");
    res
}

/// A redirect to `location` (already a valid header value: visible ASCII).
pub(crate) fn redirect(status: StatusCode, location: HeaderValue) -> Response<Body> {
    let mut res = Response::new(Body::empty());
    *res.status_mut() = status;
    res.headers_mut().insert(header::LOCATION, location);
    res
}

fn insert(res: &mut Response<Body>, name: HeaderName, value: &str) {
    if let Ok(v) = HeaderValue::from_str(value) {
        res.headers_mut().insert(name, v);
    }
}

/// Runs outside compression: a compressed body gets a weak ETag (`W/"…"`). The
/// strong tag names the uncompressed bytes, and a strong validator must differ per
/// content-coding (RFC 9110 §8.8.1); `If-None-Match` uses the weak comparison, so
/// revalidation still matches.
pub(crate) async fn weaken_etag(req: Request, next: Next) -> Response<Body> {
    let mut res = next.run(req).await;
    if res.headers().contains_key(header::CONTENT_ENCODING)
        && let Some(tag) = res
            .headers()
            .get(header::ETAG)
            .and_then(|v| v.to_str().ok())
        && !tag.starts_with("W/")
        && let Ok(weak) = HeaderValue::from_str(&format!("W/{tag}"))
    {
        res.headers_mut().insert(header::ETAG, weak);
    }
    res
}

/// The request span: method and path only. The query is never logged (`s=` may hold
/// personal data); status and latency are recorded when the response is ready.
pub(crate) fn request_span(req: &Request) -> Span {
    tracing::info_span!(
        "request",
        method = %req.method(),
        path = %req.uri().path(),
        status = tracing::field::Empty,
        latency_ms = tracing::field::Empty,
    )
}

pub(crate) fn record_response(res: &Response<Body>, latency: Duration, span: &Span) {
    span.record("status", res.status().as_u16());
    span.record(
        "latency_ms",
        u64::try_from(latency.as_millis()).unwrap_or(u64::MAX),
    );
    tracing::debug!("response");
}
