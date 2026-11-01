//! The guard in front of the MCP transport (design 12 §2, §3.1); see the
//! [module docs](super) for the order of its checks.
//!
//! * `401` answers carry `WWW-Authenticate: Bearer
//!   resource_metadata="<site>/.well-known/oauth-protected-resource/mcp"` (RFC 9728 §5.1),
//!   plus `error="invalid_token"` when a token was sent and not accepted, or
//!   `error="invalid_request"` for a malformed `Authorization` (another scheme, two
//!   headers): that is how an MCP client finds the sign-in flow.
//! * The session cookie is never looked at: an assistant authenticates with a bearer
//!   token only, so a browser's ambient cookie can do nothing here.
//! * A bearer token of an account that must change its password works, as on the REST
//!   API (R22, R26).
//! * Too many bad tokens from one address: core's `429` (with `Retry-After`).

use std::sync::Arc;

use axum::{
    extract::{Request, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use ferrite_core::Authenticated;

use super::server::Caller;
use crate::{
    ClientIp,
    api::{ApiState, auth::bearer_token, problem::Problem},
};

/// The protected-resource metadata document of `/mcp`, under `site.url`.
pub const RESOURCE_METADATA_PATH: &str = "/.well-known/oauth-protected-resource/mcp";

/// Why a request has no usable credential (the `error` of the challenge).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Refusal {
    /// No `Authorization` header.
    Missing,
    /// Not exactly one `Bearer <token>`.
    Malformed,
    /// A token that does not resolve (unknown, expired, of a disabled user).
    Invalid,
}

/// The checks of the module docs; inserts the [`Caller`].
pub async fn guard(State(api): State<Arc<ApiState>>, mut req: Request, next: Next) -> Response {
    if req.method() != Method::POST {
        let mut res = plain(StatusCode::METHOD_NOT_ALLOWED, "method not allowed\n");
        res.headers_mut()
            .insert(header::ALLOW, HeaderValue::from_static("POST"));
        return res;
    }
    if !origin_allowed(req.headers(), &api.origin) {
        return plain(
            StatusCode::FORBIDDEN,
            "requests from another origin are refused\n",
        );
    }
    let ip = req.extensions().get::<ClientIp>().map(|c| c.0);
    let token = match one_bearer(req.headers()) {
        Ok(token) => token,
        Err(refusal) => return unauthorized(&api, refusal),
    };
    let who = match api.auth.resolve_token(token, ip).await {
        Ok(Some(who)) => who,
        Ok(None) => return unauthorized(&api, Refusal::Invalid),
        Err(e) => return Problem::from(e).into_response(),
    };
    let client = client_name(&who);
    req.extensions_mut().insert(Caller { who, client });
    next.run(req).await
}

/// The label the audit and `site_overview` show for the caller's connection: the OAuth
/// client's name (the access token is named after it), else `API token <name>`.
fn client_name(who: &Authenticated) -> String {
    let name = who.token_name.as_deref().unwrap_or("");
    if who.oauth.is_some() {
        name.to_owned()
    } else {
        format!("API token {name}")
    }
}

/// The token of the one `Authorization: Bearer` header.
fn one_bearer(headers: &HeaderMap) -> Result<&str, Refusal> {
    let mut values = headers.get_all(header::AUTHORIZATION).iter();
    let value = values.next().ok_or(Refusal::Missing)?;
    if values.next().is_some() {
        return Err(Refusal::Malformed);
    }
    bearer_token(value).ok_or(Refusal::Malformed)
}

/// No `Origin`, or exactly one equal to the site's origin (ASCII case-insensitive).
/// `Origin: null` never matches.
fn origin_allowed(headers: &HeaderMap, origin: &str) -> bool {
    let mut values = headers.get_all(header::ORIGIN).iter();
    match (values.next(), values.next()) {
        (None, _) => true,
        (Some(v), None) => v
            .to_str()
            .is_ok_and(|v| v.trim().eq_ignore_ascii_case(origin)),
        (Some(_), Some(_)) => false,
    }
}

/// `401` with the challenge of the module docs.
fn unauthorized(api: &ApiState, refusal: Refusal) -> Response {
    let mut challenge = format!(
        "Bearer resource_metadata=\"{}{RESOURCE_METADATA_PATH}\"",
        api.site_url
    );
    match refusal {
        Refusal::Missing => {}
        Refusal::Malformed => challenge.push_str(", error=\"invalid_request\""),
        Refusal::Invalid => challenge.push_str(", error=\"invalid_token\""),
    }
    let mut res = plain(
        StatusCode::UNAUTHORIZED,
        "a bearer token is required: sign in through OAuth or use a personal API token\n",
    );
    match HeaderValue::from_str(&challenge) {
        Ok(v) => {
            res.headers_mut().insert(header::WWW_AUTHENTICATE, v);
        }
        // `site.url` is a parsed URL: it never holds a byte a header refuses.
        Err(e) => tracing::error!(error = %e, "MCP challenge not a header value"),
    }
    res
}

fn plain(status: StatusCode, body: &'static str) -> Response {
    (
        status,
        [
            (header::CONTENT_TYPE, "text/plain; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        body,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(header::HeaderName, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.append(k.clone(), v.parse().unwrap());
        }
        h
    }

    #[test]
    fn origins() {
        let site = "https://example.com";
        assert!(origin_allowed(&HeaderMap::new(), site));
        assert!(origin_allowed(
            &headers(&[(header::ORIGIN, "https://Example.com")]),
            site
        ));
        for bad in [
            "https://evil.example",
            "null",
            "https://example.com.evil",
            "",
        ] {
            assert!(
                !origin_allowed(&headers(&[(header::ORIGIN, bad)]), site),
                "{bad}"
            );
        }
        let two = headers(&[(header::ORIGIN, site), (header::ORIGIN, site)]);
        assert!(!origin_allowed(&two, site));
        // A Referer alone is not an Origin: server-side clients send neither.
        assert!(origin_allowed(
            &headers(&[(header::REFERER, "https://evil.example/")]),
            site
        ));
    }

    #[test]
    fn bearer_headers() {
        assert_eq!(one_bearer(&HeaderMap::new()), Err(Refusal::Missing));
        assert_eq!(
            one_bearer(&headers(&[(header::AUTHORIZATION, "Bearer fer_x")])),
            Ok("fer_x")
        );
        for bad in ["Basic abc", "Bearer", "Bearer a b"] {
            assert_eq!(
                one_bearer(&headers(&[(header::AUTHORIZATION, bad)])),
                Err(Refusal::Malformed),
                "{bad}"
            );
        }
        let two = headers(&[
            (header::AUTHORIZATION, "Bearer a"),
            (header::AUTHORIZATION, "Bearer a"),
        ]);
        assert_eq!(one_bearer(&two), Err(Refusal::Malformed));
    }
}
