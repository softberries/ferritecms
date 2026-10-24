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
//! | `Unauthenticated` | 401 | `unauthenticated` (login endpoints: `invalid-credentials`) |
//! | `RateLimited { retry_after_secs }` | 429 | `rate-limited` + `Retry-After` |
//! | `Invalid(msg)` | 422 | `validation` (detail = msg) |
//! | `InvalidField { field, message }` | 422 | `validation` (detail = message, `errors[0] = { field, detail: message }`) |
//! | `InvalidKey { key, message }` | 422 | `validation` (as `InvalidField`, `field` = the setting key) |
//! | `InvalidRows(rows)` | 422 | `validation` (one `errors` entry per row, `field` = `row <index>`; the CSV import names lines instead) |
//! | `Conflict { .. }` | 412 | `precondition-failed` (+ `current`, set by the handler) |
//! | `Changed(what)` | 409 | `conflict` (detail = what changed; retry) |
//! | `Store(UniqueViolation)` | 409 | `conflict` (a known constraint adds an `errors` entry for its field, see [`UNIQUE_FIELDS`]; the constraint name is never sent) |
//! | `Store(ForeignKey)` | 409 | `conflict` ("a related record changed; retry", F15) |
//! | `Store(Busy)` | 503 | `busy` + `Retry-After: 5` |
//! | `Random`, `Hashing`, any other `Store` | 500 | `internal` |
//!
//! Problems built by the API layer itself, not mapped from a core error:
//!
//! | Status | Slug | When |
//! |---|---|---|
//! | 400 | `bad-request` | the request is well formed but not valid for the endpoint (e.g. logout with a bearer token) |
//! | 401 | `invalid-credentials` | a login or TOTP login step failed |
//! | 403 | `csrf` | a cookie request without a valid `X-CSRF-Token`, or from another origin |
//! | 403 | `password-reset-required` | the account must change its password first |
//! | 503 | `mail-unavailable` | the request needs outgoing mail and the site has no `[mail]` |
//!
//! **Every 401 carries `WWW-Authenticate`** (RFC 9110 §11.6.1): `Bearer` unless the
//! problem names another challenge ([`Problem::with_challenge`], e.g. `Bearer
//! error="invalid_token"`). Bearer is the API's only HTTP authentication scheme; the
//! session cookie has no standard challenge, so a cookie request that fails is answered
//! with the same `Bearer` challenge, and browsers (the admin SPA) branch on the status
//! and the slug instead.
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

/// Unique constraints a request can run into, with the input member each one is about
/// and what to tell the client: (constraint, field, detail). Later resources add theirs.
pub const UNIQUE_FIELDS: &[(&str, &str, &str)] = &[
    (
        "content_items_slug_uq",
        "slug",
        "the slug was taken by concurrent saves; retry",
    ),
    (
        "terms_taxonomy_slug_uq",
        "slug",
        "the slug was taken by concurrent saves; retry",
    ),
    (
        "content_types_pkey",
        "name",
        "a content type with this name exists",
    ),
    (
        "taxonomies_pkey",
        "name",
        "a taxonomy with this name exists",
    ),
    // Core maps these two itself (`UserService`); listed for any path that does not.
    ("users_login_uq", "login", "login taken"),
    ("users_email_uq", "email", "e-mail taken"),
    (
        "menu_items_pkey",
        "items",
        "an entry id is used by another menu (or twice in this one)",
    ),
];

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
    /// The permission report of the upload (an approval mismatch of `POST /plugins`
    /// only). Boxed like `current`.
    pub report: Option<Box<serde_json::Value>>,
    /// Seconds for the `Retry-After` header.
    pub retry_after: Option<u64>,
    /// The `WWW-Authenticate` value of a 401 ([`Challenge::Bearer`] when `None`).
    pub challenge: Option<Challenge>,
}

/// The `WWW-Authenticate` challenge of a 401 (RFC 6750 §3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Challenge {
    /// `Bearer`: no credentials were sent.
    Bearer,
    /// `Bearer error="invalid_request"`: a malformed `Authorization` header.
    InvalidRequest,
    /// `Bearer error="invalid_token"`: an unknown, expired or revoked token.
    InvalidToken,
}

impl Challenge {
    pub fn header_value(self) -> &'static str {
        match self {
            Self::Bearer => "Bearer",
            Self::InvalidRequest => "Bearer error=\"invalid_request\"",
            Self::InvalidToken => "Bearer error=\"invalid_token\"",
        }
    }
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
    /// The permission report (`PermissionReportDto`) of the refused upload; only on a
    /// `validation` (422) of `POST /plugins` whose `approve` does not match, so the
    /// admin can review what this upload asks for and approve it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub report: Option<serde_json::Value>,
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
            report: None,
            retry_after: None,
            challenge: None,
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

    /// Sets `report` (a plugin upload's permission report).
    pub fn with_report(mut self, report: serde_json::Value) -> Self {
        self.report = Some(Box::new(report));
        self
    }

    pub fn with_retry_after(mut self, secs: u64) -> Self {
        self.retry_after = Some(secs);
        self
    }

    /// Sets the `WWW-Authenticate` value sent with a 401.
    pub fn with_challenge(mut self, challenge: Challenge) -> Self {
        self.challenge = Some(challenge);
        self
    }

    /// `400 bad-request`: a well-formed request this endpoint does not accept.
    pub fn bad_request(detail: impl Into<String>) -> Self {
        Self::new(
            StatusCode::BAD_REQUEST,
            "bad-request",
            "Bad request",
            detail,
        )
    }

    /// `401 invalid-credentials`: a login or its TOTP step failed (whatever failed).
    pub fn invalid_credentials() -> Self {
        Self::new(
            StatusCode::UNAUTHORIZED,
            "invalid-credentials",
            "Invalid credentials",
            "login, password or code is not valid",
        )
    }

    /// `403 csrf`: a cookie request without a valid CSRF token, or from another origin.
    pub fn csrf(detail: &'static str) -> Self {
        Self::new(StatusCode::FORBIDDEN, "csrf", "CSRF check failed", detail)
    }

    /// `403 password-reset-required`: the account must change its password first.
    pub fn password_reset_required() -> Self {
        Self::new(
            StatusCode::FORBIDDEN,
            "password-reset-required",
            "Password change required",
            "change your password first (POST /api/v1/auth/password)",
        )
    }

    /// `503 mail-unavailable`: the site has no `[mail]` section.
    pub fn mail_unavailable() -> Self {
        Self::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "mail-unavailable",
            "Mail unavailable",
            "this site cannot send mail (no [mail] section); ask an administrator",
        )
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
            report: self.report.as_deref().cloned(),
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
        if self.status == StatusCode::UNAUTHORIZED {
            headers.insert(
                header::WWW_AUTHENTICATE,
                HeaderValue::from_static(
                    self.challenge.unwrap_or(Challenge::Bearer).header_value(),
                ),
            );
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
            CoreError::InvalidField { field, message } => {
                Self::validation(message.clone()).with_error(Some(field), message)
            }
            CoreError::InvalidKey { key, message } => {
                Self::validation(message.clone()).with_error(Some(&key), message)
            }
            CoreError::InvalidRows(rows) => {
                let mut p = Self::validation("some rows are invalid; nothing was written");
                for row in rows {
                    p = p.with_error(Some(&format!("row {}", row.row)), row.message);
                }
                p
            }
            CoreError::Conflict { current_version } => Self::new(
                StatusCode::PRECONDITION_FAILED,
                "precondition-failed",
                "Version mismatch",
                format!("the resource was changed (current version {current_version})"),
            ),
            CoreError::Changed(what) => Self::new(
                StatusCode::CONFLICT,
                "conflict",
                "Conflict",
                format!("{what}; retry"),
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
            StoreError::UniqueViolation { constraint } => {
                let p = Self::new(
                    StatusCode::CONFLICT,
                    "conflict",
                    "Conflict",
                    "conflicts with an existing record",
                );
                match UNIQUE_FIELDS.iter().find(|(c, ..)| *c == constraint) {
                    Some((_, field, detail)) => p.with_error(Some(field), *detail),
                    None => {
                        tracing::warn!(%constraint, "unique violation without a field mapping");
                        p
                    }
                }
            }
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
                CoreError::Changed("the theme changed"),
                409,
                "conflict",
                Some("the theme changed; retry"),
                None,
            ),
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
                CoreError::invalid_field("slug", "reserved slug"),
                422,
                "validation",
                Some("reserved slug"),
                None,
            ),
            (
                CoreError::invalid_key("accent_color", "must be a colour"),
                422,
                "validation",
                Some("must be a colour"),
                None,
            ),
            (
                CoreError::InvalidRows(vec![ferrite_core::RowError {
                    row: 2,
                    message: "redirect loop".into(),
                }]),
                422,
                "validation",
                Some("some rows are invalid; nothing was written"),
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
                Some("conflicts with an existing record"),
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

    #[test]
    fn invalid_field_names_the_member() {
        let p = Problem::from(CoreError::invalid_field("terms", "unknown term"));
        assert_eq!(p.status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(
            p.errors,
            [FieldError {
                field: Some("terms".into()),
                detail: "unknown term".into()
            }]
        );
    }

    #[test]
    fn invalid_key_names_the_setting() {
        let p = Problem::from(CoreError::invalid_key(
            "api_key",
            "must be 1-4096 characters",
        ));
        assert_eq!(p.status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(
            p.errors,
            [FieldError {
                field: Some("api_key".into()),
                detail: "must be 1-4096 characters".into()
            }]
        );
    }

    #[test]
    fn known_unique_constraints_name_a_field_not_the_constraint() {
        let p = Problem::from(StoreError::UniqueViolation {
            constraint: "content_items_slug_uq".into(),
        });
        assert_eq!(p.status, StatusCode::CONFLICT);
        assert_eq!(p.errors.len(), 1);
        assert_eq!(p.errors[0].field.as_deref(), Some("slug"));
        let body = serde_json::to_string(&p.document()).unwrap();
        assert!(!body.contains("_uq"), "{body}");
        let p = Problem::from(StoreError::UniqueViolation {
            constraint: "some_table_x_uq".into(),
        });
        assert!(p.errors.is_empty());
        assert!(
            !serde_json::to_string(&p.document())
                .unwrap()
                .contains("_uq")
        );
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

        let res = Problem::mail_unavailable().into_response();
        assert_eq!(res.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert!(res.headers().get(header::RETRY_AFTER).is_none());
        let body = res.into_body().collect().await.unwrap().to_bytes();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["type"], "/api/v1/problems/mail-unavailable");

        let res = Problem::busy().into_response();
        assert_eq!(res.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(res.headers()[header::RETRY_AFTER], "5");
        assert!(res.headers().get(header::WWW_AUTHENTICATE).is_none());

        // Every 401 has a challenge.
        let res = Problem::from(CoreError::Unauthenticated).into_response();
        assert_eq!(res.headers()[header::WWW_AUTHENTICATE], "Bearer");
        let res = Problem::invalid_credentials().into_response();
        assert_eq!(res.headers()[header::WWW_AUTHENTICATE], "Bearer");
        let res = Problem::from(CoreError::Unauthenticated)
            .with_challenge(Challenge::InvalidToken)
            .into_response();
        assert_eq!(
            res.headers()[header::WWW_AUTHENTICATE],
            "Bearer error=\"invalid_token\""
        );
    }
}
