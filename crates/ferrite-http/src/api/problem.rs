//! RFC 9457 problem documents (`application/problem+json`), ruling P10.
//!
//! Every API error is a [`Problem`]. Its `type` is `/api/v1/problems/<slug>`; clients
//! branch on the slug, never on `title` or `detail`. [`From<CoreError>`] is the one
//! mapping from core errors to HTTP:
//!
//! | Core error | Status | Slug |
//! |---|---|---|
//! | `NotFound` | 404 | `not-found` |
//! | `Forbidden(what)` | 403 | `forbidden` (detail names the capability or scope) |
//! | `Unauthenticated` | 401 | `unauthenticated` |
//! | `RateLimited { retry_after_secs }` | 429 | `rate-limited` + `Retry-After` |
//! | `Invalid(msg)` | 422 | `validation` (detail = msg) |
//! | `Conflict { .. }` | 412 | `precondition-failed` (+ `current`, set by the handler) |
//! | `Store(UniqueViolation)` | 409 | `conflict` (detail names the constraint) |
//! | `Store(ForeignKey)` | 409 | `conflict` ("a related record changed; retry", F15) |
//! | `Store(Busy)` | 503 | `busy` + `Retry-After: 5` |
//! | `Random`, `Hashing`, any other `Store` | 500 | `internal` |
//!
//! Store, randomness and hashing failures are logged with their text; the body says only
//! "internal error" or "busy, retry". Request-level problems (malformed JSON or query,
//! `If-Match`, media type, size, unknown path, method) are built by
//! [`extract`](super::extract) and [`api::router`](super::router).

use axum::{
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use ferrite_core::CoreError;
use ferrite_store::StoreError;
use serde::Serialize;
use utoipa::ToSchema;

/// The `type` prefix of every problem.
pub const PROBLEM_TYPE_PREFIX: &str = "/api/v1/problems/";
/// `Content-Type` of every problem response.
pub const PROBLEM_JSON: &str = "application/problem+json";
/// `Retry-After` of a `busy` problem.
pub const BUSY_RETRY_AFTER: u64 = 5;

/// One entry of `errors`: what is wrong with one input (`field` = the JSON path or
/// parameter name, when known).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct FieldError {
    pub field: Option<String>,
    pub detail: String,
}

/// An API error; see the module docs.
#[derive(Debug, Clone)]
pub struct Problem {
    pub status: StatusCode,
    pub slug: &'static str,
    pub title: &'static str,
    pub detail: String,
    pub errors: Vec<FieldError>,
    /// The current resource (412 only). Boxed: keeps `Result<_, Problem>` small (clippy
    /// `result_large_err`).
    pub current: Option<Box<serde_json::Value>>,
    /// Seconds for the `Retry-After` header.
    pub retry_after: Option<u64>,
}

/// The body of a problem response (the OpenAPI schema `ProblemDocument`).
#[derive(Debug, Serialize, ToSchema)]
pub struct ProblemDocument {
    /// `/api/v1/problems/<slug>`.
    #[serde(rename = "type")]
    pub kind: String,
    pub title: String,
    pub status: u16,
    pub detail: String,
    /// Omitted when empty.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<FieldError>,
    /// The current resource; only on `precondition-failed` (412).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current: Option<serde_json::Value>,
}

impl Problem {
    pub fn new(
        status: StatusCode,
        slug: &'static str,
        title: &'static str,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            status,
            slug,
            title,
            detail: detail.into(),
            errors: Vec::new(),
            current: None,
            retry_after: None,
        }
    }

    /// Adds one `errors` entry.
    pub fn with_error(mut self, field: Option<&str>, detail: impl Into<String>) -> Self {
        self.errors.push(FieldError {
            field: field.map(str::to_owned),
            detail: detail.into(),
        });
        self
    }

    /// Sets `current` (the resource as stored, for a 412).
    pub fn with_current(mut self, current: serde_json::Value) -> Self {
        self.current = Some(Box::new(current));
        self
    }

    pub fn with_retry_after(mut self, secs: u64) -> Self {
        self.retry_after = Some(secs);
        self
    }

    pub fn not_found() -> Self {
        Self::new(
            StatusCode::NOT_FOUND,
            "not-found",
            "Not found",
            "no such resource",
        )
    }

    /// `422 validation`: invalid input.
    pub fn validation(detail: impl Into<String>) -> Self {
        Self::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation",
            "Invalid input",
            detail,
        )
    }

    /// `500 internal`; the cause is logged by the caller, never sent.
    pub fn internal() -> Self {
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "Internal error",
            "internal error",
        )
    }

    pub fn busy() -> Self {
        Self::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "busy",
            "Service busy",
            "busy, retry",
        )
        .with_retry_after(BUSY_RETRY_AFTER)
    }

    /// The response body.
    pub fn document(&self) -> ProblemDocument {
        ProblemDocument {
            kind: format!("{PROBLEM_TYPE_PREFIX}{}", self.slug),
            title: self.title.to_owned(),
            status: self.status.as_u16(),
            detail: self.detail.clone(),
            errors: self.errors.clone(),
            current: self.current.as_deref().cloned(),
        }
    }
}

impl IntoResponse for Problem {
    fn into_response(self) -> Response {
        let body = serde_json::to_vec(&self.document()).unwrap_or_else(|e| {
            tracing::error!(error = %e, "cannot serialize a problem document");
            Vec::new()
        });
        let mut res = (self.status, body).into_response();
        let headers = res.headers_mut();
        headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(PROBLEM_JSON));
        if let Some(secs) = self.retry_after {
            headers.insert(header::RETRY_AFTER, HeaderValue::from(secs));
        }
        res
    }
}

impl From<CoreError> for Problem {
    fn from(e: CoreError) -> Self {
        match e {
            CoreError::NotFound => Self::not_found(),
            CoreError::Forbidden(what) => Self::new(
                StatusCode::FORBIDDEN,
                "forbidden",
                "Forbidden",
                format!("missing {what}"),
            ),
            CoreError::Unauthenticated => Self::new(
                StatusCode::UNAUTHORIZED,
                "unauthenticated",
                "Not authenticated",
                "no valid session or token",
            ),
            CoreError::RateLimited { retry_after_secs } => Self::new(
                StatusCode::TOO_MANY_REQUESTS,
                "rate-limited",
                "Too many requests",
                format!("rate limited; retry after {retry_after_secs} s"),
            )
            .with_retry_after(retry_after_secs),
            CoreError::Invalid(msg) => Self::validation(msg),
            CoreError::Conflict { current_version } => Self::new(
                StatusCode::PRECONDITION_FAILED,
                "precondition-failed",
                "Version mismatch",
                format!("the resource was changed (current version {current_version})"),
            ),
            CoreError::Random | CoreError::Hashing => {
                tracing::error!(error = %e, "request failed");
                Self::internal()
            }
            CoreError::Store(s) => Self::from(s),
        }
    }
}

impl From<StoreError> for Problem {
    fn from(e: StoreError) -> Self {
        match e {
            StoreError::NotFound | StoreError::Conflict { .. } | StoreError::Invalid(_) => {
                Self::from(CoreError::from(e))
            }
            StoreError::UniqueViolation { constraint } => Self::new(
                StatusCode::CONFLICT,
                "conflict",
                "Conflict",
                format!("conflicts with an existing record ({constraint})"),
            ),
            StoreError::ForeignKey { .. } => Self::new(
                StatusCode::CONFLICT,
                "conflict",
                "Conflict",
                "a related record changed; retry",
            ),
            StoreError::Busy => {
                tracing::warn!(error = %e, "request failed: database busy");
                Self::busy()
            }
            other => {
                tracing::error!(error = %other, "request failed: store error");
                Self::internal()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(e: CoreError) -> (u16, &'static str, String, Option<u64>) {
        let p = Problem::from(e);
        (p.status.as_u16(), p.slug, p.detail, p.retry_after)
    }

    /// Every row of P10 (plus F15 and R9).
    #[test]
    fn problem_mapping_table() {
        let s = |e: StoreError| CoreError::Store(e);
        /// Error, status, slug, detail (when checked), `Retry-After`.
        type Row = (
            CoreError,
            u16,
            &'static str,
            Option<&'static str>,
            Option<u64>,
        );
        let table: Vec<Row> = vec![
            (CoreError::NotFound, 404, "not-found", None, None),
            (
                CoreError::Forbidden("manage_users"),
                403,
                "forbidden",
                Some("missing manage_users"),
                None,
            ),
            (
                CoreError::Forbidden("content:write"),
                403,
                "forbidden",
                Some("missing content:write"),
                None,
            ),
            (
                CoreError::Unauthenticated,
                401,
                "unauthenticated",
                None,
                None,
            ),
            (
                CoreError::RateLimited {
                    retry_after_secs: 17,
                },
                429,
                "rate-limited",
                None,
                Some(17),
            ),
            (
                CoreError::Invalid("bad slug".into()),
                422,
                "validation",
                Some("bad slug"),
                None,
            ),
            (
                CoreError::Conflict { current_version: 4 },
                412,
                "precondition-failed",
                None,
                None,
            ),
            (
                CoreError::Random,
                500,
                "internal",
                Some("internal error"),
                None,
            ),
            (
                CoreError::Hashing,
                500,
                "internal",
                Some("internal error"),
                None,
            ),
            (
                s(StoreError::UniqueViolation {
                    constraint: "users_login_uq".into(),
                }),
                409,
                "conflict",
                Some("conflicts with an existing record (users_login_uq)"),
                None,
            ),
            (
                s(StoreError::ForeignKey {
                    constraint: String::new(),
                }),
                409,
                "conflict",
                Some("a related record changed; retry"),
                None,
            ),
            (
                s(StoreError::Busy),
                503,
                "busy",
                Some("busy, retry"),
                Some(5),
            ),
            (
                s(StoreError::Database("secret detail".into())),
                500,
                "internal",
                Some("internal error"),
                None,
            ),
            (
                s(StoreError::Aborted),
                500,
                "internal",
                Some("internal error"),
                None,
            ),
            (
                s(StoreError::Unsupported("x")),
                500,
                "internal",
                Some("internal error"),
                None,
            ),
            (s(StoreError::NotFound), 404, "not-found", None, None),
            (
                s(StoreError::Invalid("too long".into())),
                422,
                "validation",
                Some("too long"),
                None,
            ),
        ];
        for (err, status, slug, detail, retry) in table {
            let name = format!("{err:?}");
            let (got_status, got_slug, got_detail, got_retry) = row(err);
            assert_eq!(got_status, status, "{name}");
            assert_eq!(got_slug, slug, "{name}");
            if let Some(d) = detail {
                assert_eq!(got_detail, d, "{name}");
            }
            assert_eq!(got_retry, retry, "{name}");
        }
    }

    #[tokio::test]
    async fn problem_response_shape() {
        use http_body_util::BodyExt;

        let res = Problem::validation("bad")
            .with_error(Some("title"), "required")
            .into_response();
        assert_eq!(res.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(res.headers()[header::CONTENT_TYPE], PROBLEM_JSON);
        assert!(res.headers().get(header::RETRY_AFTER).is_none());
        let body = res.into_body().collect().await.unwrap().to_bytes();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            body,
            serde_json::json!({
                "type": "/api/v1/problems/validation",
                "title": "Invalid input",
                "status": 422,
                "detail": "bad",
                "errors": [{"field": "title", "detail": "required"}],
            })
        );

        let res = Problem::from(CoreError::Conflict { current_version: 2 })
            .with_current(serde_json::json!({"version": 2}))
            .into_response();
        let body = res.into_body().collect().await.unwrap().to_bytes();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["current"], serde_json::json!({"version": 2}));
        assert!(body.get("errors").is_none(), "empty errors are omitted");

        let res = Problem::busy().into_response();
        assert_eq!(res.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(res.headers()[header::RETRY_AFTER], "5");
    }
}
