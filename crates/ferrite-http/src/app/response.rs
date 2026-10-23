//! Turning pages into responses, and the middleware stack.

use std::time::Duration;

use axum::{
    body::Body,
    extract::Request,
    http::{HeaderName, HeaderValue, Response, StatusCode, header},
};
use ferrite_cache::{CachedPage, if_none_match_matches};
use tracing::Span;

use super::client::ClientIp;

/// Response extension: the compression layer leaves this response alone (media files,
/// binary theme assets, ranged responses).
#[derive(Debug, Clone, Copy)]
pub(crate) struct NoCompress;

/// The compression predicate's extra rule: no [`NoCompress`] marker.
pub(crate) fn compressible(
    _: StatusCode,
    _: axum::http::Version,
    _: &axum::http::HeaderMap,
    extensions: &axum::http::Extensions,
) -> bool {
    extensions.get::<NoCompress>().is_none()
}

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

/// `page` (an HTML page) as a response; `304 Not Modified` (no body) when it is a `200`
/// and `if_none_match` matches its ETag.
///
/// The ETag is sent **weak** (`W/"…"`, Task 8a review): the tag hashes the
/// uncompressed body while compression may change the bytes on the wire, and a strong
/// validator must differ per content-coding (RFC 9110 §8.8.1). `If-None-Match` uses
/// the weak comparison. Every response from here, the 304 included, carries
/// `Vary: Accept-Encoding`, so shared caches keep the encodings apart. Error pages
/// carry no ETag.
pub(crate) fn page_response(page: &CachedPage, if_none_match: Option<&str>) -> Response<Body> {
    let etag = format!("W/{}", page.etag);
    if page.status == 200 && if_none_match.is_some_and(|v| if_none_match_matches(v, &page.etag)) {
        let mut res = Response::new(Body::empty());
        *res.status_mut() = StatusCode::NOT_MODIFIED;
        insert(&mut res, header::ETAG, &etag);
        vary(&mut res);
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
    if page.status < 400 {
        insert(&mut res, header::ETAG, &etag);
    }
    vary(&mut res);
    res
}

fn vary(res: &mut Response<Body>) {
    res.headers_mut()
        .insert(header::VARY, HeaderValue::from_static("accept-encoding"));
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

/// The built-in 500 page for a panicking handler (tower-http `CatchPanic`). The panic
/// payload is not logged: it may echo request data.
pub(crate) fn panic_response(_panic: Box<dyn std::any::Any + Send + 'static>) -> Response<Body> {
    tracing::error!("request handler panicked; built-in 500 page served");
    let mut res = Response::new(Body::from(super::error_page::builtin_server_error()));
    *res.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
    insert(&mut res, header::CONTENT_TYPE, HTML);
    insert(&mut res, header::CACHE_CONTROL, "no-store");
    res
}

/// The request span: method, path and the client address ([`ClientIp`], when known).
/// The query is never logged (`s=` may hold personal data); status and latency are
/// recorded when the response is ready.
pub(crate) fn request_span(req: &Request) -> Span {
    let span = tracing::info_span!(
        "request",
        method = %req.method(),
        path = %req.uri().path(),
        client_ip = tracing::field::Empty,
        status = tracing::field::Empty,
        latency_ms = tracing::field::Empty,
    );
    if let Some(ClientIp(ip)) = req.extensions().get::<ClientIp>() {
        span.record("client_ip", tracing::field::display(ip));
    }
    span
}

pub(crate) fn record_response(res: &Response<Body>, latency: Duration, span: &Span) {
    span.record("status", res.status().as_u16());
    span.record(
        "latency_ms",
        u64::try_from(latency.as_millis()).unwrap_or(u64::MAX),
    );
    tracing::debug!("response");
}

#[cfg(test)]
mod tests {
    use axum::{Router, routing::get};
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    use tower_http::catch_panic::CatchPanicLayer;

    use super::*;

    #[tokio::test]
    async fn a_panicking_handler_gets_the_builtin_500() {
        async fn boom() -> &'static str {
            panic!("handler bug")
        }
        let app = Router::new()
            .route("/", get(boom))
            .layer(CatchPanicLayer::custom(panic_response));
        let req = axum::http::Request::builder()
            .uri("/")
            .body(Body::empty())
            .unwrap();
        let res = app.oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(res.headers()[header::CONTENT_TYPE], HTML);
        let body = res.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8(body.to_vec()).unwrap();
        assert!(body.contains("<h1>Something went wrong</h1>"), "{body}");
        assert!(!body.contains("handler bug"));
    }
}
