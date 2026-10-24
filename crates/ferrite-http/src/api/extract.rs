//! Request extractors and response header helpers for API handlers. Every rejection is
//! a [`Problem`].
//!
//! * [`ApiJson<T>`]: needs `Content-Type: application/json` (parameters such as
//!   `charset` allowed), else `415 unsupported-media-type`. The body is at most
//!   [`MAX_JSON_BODY`] (4 MiB, the router's [`body_limit`](super::body_limit)), else
//!   `413 too-large`. Malformed JSON, trailing data, or a document that does not fit
//!   `T` is `400 malformed-json` with the serde path in `errors[0].field` (`.` for the
//!   document itself). A string anywhere in the document (keys and ignored members
//!   included) holding a NUL character is `422 validation`: the store refuses NUL in
//!   text, so it is refused at the edge for every endpoint (M1a NUL-at-the-edge item).
//! * [`ApiQuery<T>`]: the query string as `T`; `400 malformed-query`.
//! * [`IfMatch`]: `If-Match: "<n>"` (one strong entity tag holding a version number);
//!   missing `428 precondition-required`; any other form (weak, unquoted, a list, `*`)
//!   `400 malformed-header`.
//! * [`ApiPath<T>`]: the path parameters as `T`; one that does not parse (a malformed
//!   id) is `404 not-found`, as no resource has that path.
//! * [`PageParams`]: `?limit=&cursor=`; [`PageParams::limit`] defaults to 50 and is
//!   `400 validation` outside 1..=200 ([`check_limit`]). A cursor the list cannot use is
//!   `400 validation` with `field = "cursor"` ([`invalid_cursor`]): list handlers decode
//!   it before calling core, which would report it as `422`.
//!
//! Query parameter values that parse but are not valid (`limit`, `cursor`, enumerated
//! lists) are `400 validation` naming the parameter ([`invalid_param`]); rules of the
//! resource itself, checked by core, stay `422 validation`.

use axum::{
    body::Bytes,
    extract::{FromRequest, FromRequestParts, Path, Query, Request},
    http::{HeaderMap, HeaderValue, StatusCode, header, request::Parts},
};
use ferrite_domain::MAX_PAGE;
use serde::{Deserialize, de::DeserializeOwned};
use utoipa::IntoParams;

use super::problem::Problem;

/// Largest JSON request body (4 MiB).
pub const MAX_JSON_BODY: usize = 4 * 1024 * 1024;
/// `limit` when a list request gives none.
pub const DEFAULT_LIMIT: u32 = 50;

/// A JSON request body; see the module docs.
#[derive(Debug, Clone)]
pub struct ApiJson<T>(pub T);

impl<T, S> FromRequest<S> for ApiJson<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = Problem;

    async fn from_request(req: Request, state: &S) -> Result<Self, Problem> {
        if !is_json(req.headers()) {
            return Err(Problem::new(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "unsupported-media-type",
                "Unsupported media type",
                "the body must be application/json",
            ));
        }
        let bytes = Bytes::from_request(req, state).await.map_err(|r| {
            if r.status() == StatusCode::PAYLOAD_TOO_LARGE {
                Problem::new(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "too-large",
                    "Request too large",
                    format!("the body is larger than {MAX_JSON_BODY} bytes"),
                )
            } else {
                Problem::new(
                    StatusCode::BAD_REQUEST,
                    "malformed-body",
                    "Malformed request body",
                    r.body_text(),
                )
            }
        })?;
        parse_json(&bytes).map(Self)
    }
}

/// `bytes` as `T` with the rules of [`ApiJson`] (after the media type and size checks).
pub fn parse_json<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, Problem> {
    let malformed = |field: String, detail: String| {
        Problem::new(
            StatusCode::BAD_REQUEST,
            "malformed-json",
            "Malformed JSON",
            "the body is not valid JSON for this request",
        )
        .with_error(Some(&field), detail)
    };
    let mut de = serde_json::Deserializer::from_slice(bytes);
    let value: T = serde_path_to_error::deserialize(&mut de)
        .map_err(|e| malformed(e.path().to_string(), e.inner().to_string()))?;
    de.end()
        .map_err(|e| malformed(".".to_owned(), e.to_string()))?;
    if has_nul_escape(bytes) {
        return Err(Problem::validation("text must not contain NUL characters")
            .with_error(None, "a string contains \\u0000"));
    }
    Ok(value)
}

/// Whether a JSON text (already parsed, so valid) has a string with a NUL character.
/// Valid JSON holds control characters only as escapes, and backslashes only inside
/// strings, so a NUL is exactly a `\u0000` escape at an escape position.
fn has_nul_escape(json: &[u8]) -> bool {
    let mut i = 0;
    while i < json.len() {
        if json[i] == b'\\' {
            if json.get(i + 1) == Some(&b'u') && json.get(i + 2..i + 6) == Some(b"0000") {
                return true;
            }
            // Skip the escaped character (`\\` must not start another escape).
            i += 2;
        } else {
            i += 1;
        }
    }
    false
}

/// `Content-Type` is `application/json` (any parameters, any case).
fn is_json(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .is_some_and(|essence| essence.trim().eq_ignore_ascii_case("application/json"))
}

/// The query string as `T`; see the module docs.
#[derive(Debug, Clone)]
pub struct ApiQuery<T>(pub T);

impl<T, S> FromRequestParts<S> for ApiQuery<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = Problem;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Problem> {
        parse_query(&parts.uri).map(Self)
    }
}

/// The query string of `uri` as `T` (`400 malformed-query`), as [`ApiQuery`] reads it;
/// for handlers that authorize before they look at the query.
pub fn parse_query<T: DeserializeOwned>(uri: &axum::http::Uri) -> Result<T, Problem> {
    Query::<T>::try_from_uri(uri)
        .map(|Query(q)| q)
        .map_err(|r| {
            Problem::new(
                StatusCode::BAD_REQUEST,
                "malformed-query",
                "Malformed query string",
                r.body_text(),
            )
        })
}

/// The path parameters as `T`; see the module docs.
#[derive(Debug, Clone)]
pub struct ApiPath<T>(pub T);

impl<T, S> FromRequestParts<S> for ApiPath<T>
where
    T: DeserializeOwned + Send,
    S: Send + Sync,
{
    type Rejection = Problem;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Problem> {
        Path::<T>::from_request_parts(parts, state)
            .await
            .map(|Path(p)| Self(p))
            .map_err(|_| Problem::not_found())
    }
}

/// The version from `If-Match: "<n>"`; see the module docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IfMatch(pub i64);

impl<S: Send + Sync> FromRequestParts<S> for IfMatch {
    type Rejection = Problem;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Problem> {
        parse_if_match(&parts.headers).map(Self)
    }
}

/// The version in the request's `If-Match` header.
pub fn parse_if_match(headers: &HeaderMap) -> Result<i64, Problem> {
    let mut values = headers.get_all(header::IF_MATCH).iter();
    let Some(value) = values.next() else {
        return Err(Problem::new(
            StatusCode::PRECONDITION_REQUIRED,
            "precondition-required",
            "Precondition required",
            "send If-Match: \"<version>\" with the version you last read",
        ));
    };
    let malformed = || {
        Problem::new(
            StatusCode::BAD_REQUEST,
            "malformed-header",
            "Malformed header",
            "If-Match must be one strong entity tag: \"<version>\"",
        )
        .with_error(Some("If-Match"), "expected \"<version>\"")
    };
    if values.next().is_some() {
        return Err(malformed());
    }
    let digits = value
        .to_str()
        .ok()
        .map(str::trim)
        .and_then(|v| v.strip_prefix('"'))
        .and_then(|v| v.strip_suffix('"'))
        .filter(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()))
        .ok_or_else(malformed)?;
    digits.parse::<i64>().map_err(|_| malformed())
}

/// `?limit=&cursor=` of a list endpoint.
#[derive(Debug, Clone, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct PageParams {
    /// 1..=200; default 50.
    pub limit: Option<u32>,
    /// `next_cursor` of the previous page.
    pub cursor: Option<String>,
}

impl PageParams {
    /// The page size: [`DEFAULT_LIMIT`] when absent, `400 validation` outside 1..=200.
    pub fn limit(&self) -> Result<u32, Problem> {
        check_limit(self.limit, DEFAULT_LIMIT, MAX_PAGE)
    }
}

/// `limit`, or `default` when absent; `400 validation` (field `limit`) outside 1..=`max`.
pub fn check_limit(limit: Option<u32>, default: u32, max: u32) -> Result<u32, Problem> {
    match limit {
        None => Ok(default),
        Some(n) if (1..=max).contains(&n) => Ok(n),
        Some(_) => Err(invalid_param(
            "limit",
            format!("limit must be between 1 and {max}"),
            format!("1..={max}"),
        )),
    }
}

/// `400 validation`: query parameter `field` has a value the endpoint cannot use.
pub fn invalid_param(field: &str, detail: impl Into<String>, error: impl Into<String>) -> Problem {
    Problem::new(
        StatusCode::BAD_REQUEST,
        "validation",
        "Invalid input",
        detail,
    )
    .with_error(Some(field), error)
}

/// `400 validation` with `field = "cursor"`: the cursor is malformed or belongs to
/// another list (another order, or search versus no search).
pub fn invalid_cursor() -> Problem {
    invalid_param(
        "cursor",
        "the cursor does not belong to this list",
        "pass next_cursor of the same list unchanged",
    )
}

/// The `ETag` of a resource at `version`: `"<n>"` (strong; what [`IfMatch`] reads).
pub fn etag(version: i64) -> HeaderValue {
    // Digits and quotes are always a valid header value.
    HeaderValue::from_str(&format!("\"{version}\"")).unwrap_or(HeaderValue::from_static("\"\""))
}

/// The headers of a `201 Created`: `Location` and `ETag`. A `location` that is not a
/// valid header value (never the case for API paths built from ids) is left out and
/// logged.
pub fn created(location: &str, etag: HeaderValue) -> (StatusCode, HeaderMap) {
    let (status, mut headers) = created_at(location);
    headers.insert(header::ETAG, etag);
    (status, headers)
}

/// `201 Created` with `Location` only, for resources without a version (types,
/// taxonomies, terms; P11).
pub fn created_at(location: &str) -> (StatusCode, HeaderMap) {
    let mut headers = HeaderMap::new();
    match HeaderValue::from_str(location) {
        Ok(v) => {
            headers.insert(header::LOCATION, v);
        }
        Err(_) => tracing::warn!(location, "Location is not a valid header value"),
    }
    (StatusCode::CREATED, headers)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn if_match(values: &[&str]) -> Result<i64, Problem> {
        let mut h = HeaderMap::new();
        for v in values {
            h.append(header::IF_MATCH, HeaderValue::from_str(v).unwrap());
        }
        parse_if_match(&h)
    }

    #[test]
    fn if_match_parsing() {
        assert_eq!(if_match(&["\"7\""]).unwrap(), 7);
        assert_eq!(if_match(&[" \"0\" "]).unwrap(), 0);
        assert_eq!(if_match(&["\"9223372036854775807\""]).unwrap(), i64::MAX);
        for bad in [
            &["W/\"7\""][..],
            &["7"],
            &["\"x\""],
            &["\"\""],
            &["\"-1\""],
            &["*"],
            &["\"7\", \"8\""],
            &["\"7\"", "\"8\""],
            &["\"9223372036854775808\""],
        ] {
            let p = if_match(bad).unwrap_err();
            assert_eq!(p.status, StatusCode::BAD_REQUEST, "{bad:?}");
            assert_eq!(p.slug, "malformed-header", "{bad:?}");
        }
        let p = if_match(&[]).unwrap_err();
        assert_eq!(p.status, StatusCode::PRECONDITION_REQUIRED);
        assert_eq!(p.slug, "precondition-required");
    }

    #[test]
    fn nul_escapes_are_found_at_escape_positions_only() {
        assert!(has_nul_escape(br#""\u0000""#));
        assert!(has_nul_escape(br#"{"a":"x\\\u0000"}"#));
        assert!(!has_nul_escape(br#""\\u0000""#));
        assert!(has_nul_escape(br#""\u00001""#));
        assert!(!has_nul_escape(br#""\u000""#));
        assert!(!has_nul_escape(br#""\u0001""#));
        assert!(!has_nul_escape(b"\"u0000\""));
    }

    #[test]
    fn json_media_type() {
        let ct = |v: &str| {
            let mut h = HeaderMap::new();
            h.insert(header::CONTENT_TYPE, HeaderValue::from_str(v).unwrap());
            is_json(&h)
        };
        assert!(ct("application/json"));
        assert!(ct("Application/JSON; charset=utf-8"));
        assert!(!ct("application/jsonx"));
        assert!(!ct("application/problem+json"));
        assert!(!ct("text/json"));
        assert!(!is_json(&HeaderMap::new()));
    }

    #[test]
    fn header_helpers() {
        assert_eq!(etag(12), "\"12\"");
        let (status, h) = created("/api/v1/x/1", etag(1));
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(h[header::LOCATION], "/api/v1/x/1");
        assert_eq!(h[header::ETAG], "\"1\"");
        let (_, h) = created("/bad\nvalue", etag(1));
        assert!(h.get(header::LOCATION).is_none());
    }
}
