//! The OAuth 2.1 authorization server's HTTP endpoints (design 12 §3.2, §6, §7), outside
//! `/api/v1`, form-encoded and JSON as OAuth needs. [`router`] is merged by
//! [`build_site`](crate::build_site) in front of the public site; the logic is core's
//! [`OAuthService`](ferrite_core::oauth::OAuthService).
//!
//! | Route | Answer |
//! |---|---|
//! | `GET /.well-known/oauth-protected-resource[/mcp]` | RFC 9728 metadata of `<site>/mcp` (also at `…/<path>/mcp` when `site.url` has a path) |
//! | `GET /.well-known/oauth-authorization-server` | RFC 8414 metadata (also at `…/<path>` when `site.url` has a path) |
//! | `GET /oauth/authorize` | `303` to `<site>/admin/connect?request=<id>`; `303` back to the client with `error=`; or an HTML error page |
//! | `POST /oauth/token` | RFC 6749 §5 JSON |
//! | `POST /oauth/register` | RFC 7591 `201` JSON |
//! | `POST /oauth/revoke` | RFC 7009 `200` |
//!
//! * **Off switch.** While `mcp.enabled` is off every route here answers `404` (any
//!   method), checked before anything else (design 12 §2).
//! * **Bodies** of at most [`MAX_OAUTH_BODY`] (`413` beyond).
//! * **CORS.** Browser-based MCP clients read the metadata documents and call the
//!   token, registration and revocation endpoints from their own origin: those answer
//!   `Access-Control-Allow-Origin: *` and an `OPTIONS` preflight (`204`). No cookie is
//!   read by any of them, so a wildcard origin gives nothing away. The authorization
//!   endpoint is a browser navigation and has no CORS headers. (Set by hand: tower-http's
//!   `cors` feature is not compiled in for five routes.)
//! * **Caching.** The metadata documents are `Cache-Control: public, max-age=3600`;
//!   everything else is `no-store` (the token endpoint adds `Pragma: no-cache`).
//! * **The authorization endpoint** counts each request on a per-IP limit
//!   ([`AUTHORIZE_PER_IP`] a minute, bucket `oauth:authorize`) before core looks at it:
//!   an unknown `https` client id makes core fetch a document. A request repeating a
//!   parameter is refused with the error page (RFC 6749 §3.1). A client or redirect URI
//!   core cannot validate gets the error page ([`error_page`]: status 400, standalone,
//!   `no-store`, a CSP without scripts, `X-Frame-Options: DENY`, every text escaped,
//!   since core's message can quote what the request carried); never a redirect.
//! * **The token endpoint** takes `application/x-www-form-urlencoded` only (else
//!   `invalid_request`); a repeated parameter is `invalid_request`. Errors are RFC 6749
//!   §5.2 JSON with the status of [`OAuthError::status`]; a rate limit is `429` with
//!   `Retry-After`.
//! * **Registration** takes `application/json` (RFC 7591 §3.1); unknown members are
//!   ignored, members of the wrong type are `invalid_client_metadata`.
//! * **Revocation** takes the form's `token` (a missing one is `invalid_request`) and
//!   answers `200` whether or not the token was known (RFC 7009 §2.2); it is limited to
//!   [`REVOKE_PER_IP`] a minute per IP (bucket `oauth:revoke`).

use std::sync::Arc;
use std::time::Duration;

use axum::{
    Json, Router,
    body::Bytes,
    extract::{DefaultBodyLimit, RawQuery, Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, Uri, header},
    middleware::Next,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use ferrite_core::{
    CoreError, Scopes,
    oauth::{AuthorizeError, AuthorizeParams, ClientRegistration, OAuthError, TokenRequest},
    settings::site_title_or_configured,
};
use serde::Deserialize;
use serde_json::{Value, json};
use tower_http::set_header::SetResponseHeaderLayer;

use crate::api::{ApiState, ClientInfo, oauth::MAX_OAUTH_BODY};

/// The consent screen of the admin SPA, under `site.url`.
pub const CONSENT_PAGE: &str = "/admin/connect";
/// The docs page the protected-resource metadata names (`resource_documentation`).
pub const RESOURCE_DOCUMENTATION: &str =
    "https://softberries.github.io/ferritecms/guides/ai-assistants/";
/// Authorization requests per IP and [`LIMIT_WINDOW`].
pub const AUTHORIZE_PER_IP: u32 = 30;
/// Revocation requests per IP and [`LIMIT_WINDOW`].
pub const REVOKE_PER_IP: u32 = 30;
/// The window of the limits set here.
pub const LIMIT_WINDOW: Duration = Duration::from_secs(60);

const PROTECTED_RESOURCE: &str = "/.well-known/oauth-protected-resource";
const AUTHORIZATION_SERVER: &str = "/.well-known/oauth-authorization-server";
const METADATA_CACHE: &str = "public, max-age=3600";
const NO_STORE: &str = "no-store";
const FORM: &str = "application/x-www-form-urlencoded";
const PAGE_CSP: &str = "default-src 'none'; style-src 'unsafe-inline'; frame-ancestors 'none'";
const GRANT_TYPES: [&str; 2] = ["authorization_code", "refresh_token"];

/// The routes of the module docs, with the off switch, the body limit and CORS.
pub fn router(api: Arc<ApiState>) -> Router {
    let cors = Router::new()
        .route(
            PROTECTED_RESOURCE,
            get(protected_resource).options(preflight_get),
        )
        .route(
            &format!("{PROTECTED_RESOURCE}/{{*rest}}"),
            get(protected_resource).options(preflight_get),
        )
        .route(
            AUTHORIZATION_SERVER,
            get(authorization_server).options(preflight_get),
        )
        .route(
            &format!("{AUTHORIZATION_SERVER}/{{*rest}}"),
            get(authorization_server).options(preflight_get),
        )
        .route("/oauth/token", post(token).options(preflight_post))
        .route("/oauth/register", post(register).options(preflight_post))
        .route("/oauth/revoke", post(revoke).options(preflight_post))
        .layer(SetResponseHeaderLayer::overriding(
            header::ACCESS_CONTROL_ALLOW_ORIGIN,
            HeaderValue::from_static("*"),
        ));
    Router::new()
        .route("/oauth/authorize", get(authorize))
        .merge(cors)
        .layer(DefaultBodyLimit::max(MAX_OAUTH_BODY))
        .layer(axum::middleware::from_fn_with_state(api.clone(), gate))
        .with_state(api)
}

/// The off switch: `404` for every route while `mcp.enabled` is off; `503` when the
/// option cannot be read.
async fn gate(State(api): State<Arc<ApiState>>, req: Request, next: Next) -> Response {
    match api.mcp_enabled().await {
        Ok(true) => next.run(req).await,
        Ok(false) => plain(StatusCode::NOT_FOUND, "not found\n"),
        Err(e) => {
            tracing::error!(error = %e, "mcp.enabled unreadable; OAuth endpoint refused");
            let mut res = plain(StatusCode::SERVICE_UNAVAILABLE, "unavailable, retry\n");
            res.headers_mut()
                .insert(header::RETRY_AFTER, HeaderValue::from_static("5"));
            res
        }
    }
}

fn plain(status: StatusCode, body: &'static str) -> Response {
    (
        status,
        [
            (header::CONTENT_TYPE, "text/plain; charset=utf-8"),
            (header::CACHE_CONTROL, NO_STORE),
        ],
        body,
    )
        .into_response()
}

// ---- metadata documents --------------------------------------------------------------

/// The path of `site.url` (percent-encoded, no trailing slash; empty without one).
fn site_path(api: &ApiState) -> &str {
    let issuer = api.oauth.issuer();
    // `scheme://host[:port]` is the origin; what follows is the path.
    issuer
        .find("://")
        .and_then(|i| issuer[i + 3..].find('/').map(|j| &issuer[i + 3 + j..]))
        .unwrap_or("")
}

fn metadata(doc: Value) -> Response {
    ([(header::CACHE_CONTROL, METADATA_CACHE)], Json(doc)).into_response()
}

/// RFC 9728 metadata of `<site>/mcp`, at `PROTECTED_RESOURCE` with nothing, `/mcp` or
/// `<site path>/mcp` after it.
async fn protected_resource(State(api): State<Arc<ApiState>>, uri: Uri) -> Response {
    let rest = uri.path().strip_prefix(PROTECTED_RESOURCE).unwrap_or("-");
    let path = site_path(&api);
    let known = rest.is_empty()
        || rest == "/mcp"
        || (!path.is_empty() && rest.strip_suffix("/mcp") == Some(path));
    if !known {
        return plain(StatusCode::NOT_FOUND, "not found\n");
    }
    let title = site_title_or_configured(api.store.as_ref(), &api.site_title).await;
    metadata(json!({
        "resource": api.oauth.resource(),
        "authorization_servers": [api.oauth.issuer()],
        "scopes_supported": Scopes::ALL.to_strings(),
        "bearer_methods_supported": ["header"],
        "resource_name": title,
        "resource_documentation": RESOURCE_DOCUMENTATION,
    }))
}

/// RFC 8414 metadata, at `AUTHORIZATION_SERVER` and, when `site.url` has a path, at
/// `AUTHORIZATION_SERVER<site path>` (the path-inserted form).
async fn authorization_server(State(api): State<Arc<ApiState>>, uri: Uri) -> Response {
    let rest = uri.path().strip_prefix(AUTHORIZATION_SERVER).unwrap_or("-");
    let path = site_path(&api);
    if !(rest.is_empty() || (!path.is_empty() && rest == path)) {
        return plain(StatusCode::NOT_FOUND, "not found\n");
    }
    let issuer = api.oauth.issuer();
    metadata(json!({
        "issuer": issuer,
        "authorization_endpoint": format!("{issuer}/oauth/authorize"),
        "token_endpoint": format!("{issuer}/oauth/token"),
        "registration_endpoint": format!("{issuer}/oauth/register"),
        "revocation_endpoint": format!("{issuer}/oauth/revoke"),
        "response_types_supported": ["code"],
        "response_modes_supported": ["query"],
        "grant_types_supported": GRANT_TYPES,
        "code_challenge_methods_supported": ["S256"],
        "token_endpoint_auth_methods_supported": ["none"],
        "revocation_endpoint_auth_methods_supported": ["none"],
        "scopes_supported": Scopes::ALL.to_strings(),
        "client_id_metadata_document_supported": true,
        "authorization_response_iss_parameter_supported": true,
        "service_documentation": RESOURCE_DOCUMENTATION,
    }))
}

async fn preflight_get() -> Response {
    preflight("GET, OPTIONS")
}

async fn preflight_post() -> Response {
    preflight("POST, OPTIONS")
}

/// A CORS preflight answer (`Access-Control-Allow-Origin` comes from the router's layer).
fn preflight(methods: &'static str) -> Response {
    (
        StatusCode::NO_CONTENT,
        [
            (header::ACCESS_CONTROL_ALLOW_METHODS, methods),
            (
                header::ACCESS_CONTROL_ALLOW_HEADERS,
                "Authorization, Content-Type, MCP-Protocol-Version",
            ),
            (header::ACCESS_CONTROL_MAX_AGE, "86400"),
            (header::CACHE_CONTROL, NO_STORE),
        ],
    )
        .into_response()
}

// ---- authorization endpoint -----------------------------------------------------------

async fn authorize(
    State(api): State<Arc<ApiState>>,
    client: ClientInfo,
    RawQuery(query): RawQuery,
) -> Response {
    match api
        .auth
        .hit_limit("oauth:authorize", client.ip, AUTHORIZE_PER_IP, LIMIT_WINDOW)
        .await
    {
        Ok(()) => {}
        Err(CoreError::RateLimited { retry_after_secs }) => {
            let mut res = error_page(
                StatusCode::TOO_MANY_REQUESTS,
                "Too many requests",
                &format!(
                    "Too many sign-in requests came from this address. Try again in {retry_after_secs} seconds."
                ),
            );
            res.headers_mut()
                .insert(header::RETRY_AFTER, HeaderValue::from(retry_after_secs));
            return res;
        }
        Err(e) => {
            tracing::error!(error = %e, "oauth authorize rate limit failed");
            return server_error_page();
        }
    }
    let params = match authorize_params(query.as_deref().unwrap_or("")) {
        Ok(p) => p,
        Err(message) => return error_page(StatusCode::BAD_REQUEST, "Cannot connect", &message),
    };
    match api.oauth.begin(params).await {
        Ok(id) => see_other(&format!("{}{CONSENT_PAGE}?request={id}", api.site_url)),
        Err(AuthorizeError::Redirect { uri, .. }) => see_other(uri.as_str()),
        Err(AuthorizeError::ShowPage(message)) => {
            error_page(StatusCode::BAD_REQUEST, "Cannot connect", &message)
        }
        Err(AuthorizeError::Server(e)) => {
            tracing::error!(error = %e, "oauth authorization request failed");
            server_error_page()
        }
    }
}

/// The authorization request's parameters; a known one sent twice is the error text.
fn authorize_params(query: &str) -> Result<AuthorizeParams, String> {
    let mut p = AuthorizeParams::default();
    for (key, value) in url::form_urlencoded::parse(query.as_bytes()) {
        let slot = match key.as_ref() {
            "response_type" => &mut p.response_type,
            "client_id" => &mut p.client_id,
            "redirect_uri" => &mut p.redirect_uri,
            "code_challenge" => &mut p.code_challenge,
            "code_challenge_method" => &mut p.code_challenge_method,
            "state" => &mut p.state,
            "scope" => &mut p.scope,
            "resource" => &mut p.resource,
            _ => continue,
        };
        if slot.is_some() {
            return Err(format!("The request repeats the parameter {key}."));
        }
        *slot = Some(value.into_owned());
    }
    Ok(p)
}

/// `303 See Other` to `location`, `no-store`, no `Referer` sent on.
fn see_other(location: &str) -> Response {
    match HeaderValue::from_str(location) {
        Ok(to) => (
            StatusCode::SEE_OTHER,
            [
                (header::LOCATION, to),
                (header::CACHE_CONTROL, HeaderValue::from_static(NO_STORE)),
                (
                    header::REFERRER_POLICY,
                    HeaderValue::from_static("no-referrer"),
                ),
            ],
        )
            .into_response(),
        Err(_) => {
            tracing::error!("oauth redirect location is not a header value");
            server_error_page()
        }
    }
}

fn server_error_page() -> Response {
    error_page(
        StatusCode::INTERNAL_SERVER_ERROR,
        "Something went wrong",
        "The request could not be completed. Try again in a moment.",
    )
}

/// A small standalone HTML page for an authorization request that cannot go back to the
/// client: `status`, `no-store`, no scripts, not framed, `title` and `message` escaped.
pub fn error_page(status: StatusCode, title: &str, message: &str) -> Response {
    let title = html_escape::encode_text(title);
    let message = html_escape::encode_text(message);
    let body = format!(
        "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
<meta name=\"robots\" content=\"noindex\">\n<title>{title}</title>\n<style>\n\
body {{ font: 16px/1.5 system-ui, sans-serif; max-width: 40rem; margin: 4rem auto; padding: 0 1rem; color: #1f2328; background: #fff; }}\n\
p.note {{ color: #59636e; }}\n\
@media (prefers-color-scheme: dark) {{ body {{ color: #e6e7e9; background: #16181c; }} p.note {{ color: #9198a1; }} }}\n\
</style>\n</head>\n<body>\n<h1>{title}</h1>\n<p>{message}</p>\n\
<p class=\"note\">The application that sent you here could not be connected to this site. \
You can close this page and try again from the application.</p>\n</body>\n</html>\n"
    );
    (
        status,
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::CACHE_CONTROL, NO_STORE),
            (header::CONTENT_SECURITY_POLICY, PAGE_CSP),
            (header::X_FRAME_OPTIONS, "DENY"),
            (header::REFERRER_POLICY, "no-referrer"),
        ],
        body,
    )
        .into_response()
}

// ---- token, registration, revocation ---------------------------------------------------

/// An OAuth error as JSON (RFC 6749 §5.2): its status, `no-store`, `Retry-After` when
/// limited.
fn oauth_error(e: &OAuthError) -> Response {
    if let OAuthError::Server(inner) = e {
        tracing::error!(error = %inner, "oauth request failed");
    }
    let status = StatusCode::from_u16(e.status()).unwrap_or(StatusCode::BAD_REQUEST);
    let mut res = (
        status,
        [
            (header::CACHE_CONTROL, NO_STORE),
            (header::PRAGMA, "no-cache"),
        ],
        Json(json!({ "error": e.as_code(), "error_description": e.description() })),
    )
        .into_response();
    if let OAuthError::RateLimited { retry_after_secs } = e {
        res.headers_mut()
            .insert(header::RETRY_AFTER, HeaderValue::from(*retry_after_secs));
    }
    res
}

/// Whether the request's `Content-Type` is `essence` (parameters such as `charset`
/// allowed, ASCII case-insensitive).
fn content_type_is(headers: &HeaderMap, essence: &str) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .is_some_and(|v| v.trim().eq_ignore_ascii_case(essence))
}

/// A form body as `(name, value)` pairs; a name sent twice is `invalid_request`.
fn form_fields(body: &[u8]) -> Result<Vec<(String, String)>, OAuthError> {
    let mut fields: Vec<(String, String)> = Vec::new();
    for (key, value) in url::form_urlencoded::parse(body) {
        if fields.iter().any(|(k, _)| *k == key) {
            return Err(OAuthError::InvalidRequest(format!(
                "the parameter {key} is repeated"
            )));
        }
        fields.push((key.into_owned(), value.into_owned()));
    }
    Ok(fields)
}

/// `fields` of a form-encoded request (`invalid_request` for another body type).
fn form_of(headers: &HeaderMap, body: &[u8]) -> Result<Vec<(String, String)>, OAuthError> {
    if !content_type_is(headers, FORM) {
        return Err(OAuthError::InvalidRequest(format!(
            "the body must be {FORM}"
        )));
    }
    form_fields(body)
}

fn take(fields: &mut Vec<(String, String)>, name: &str) -> Option<String> {
    let i = fields.iter().position(|(k, _)| k == name)?;
    Some(fields.swap_remove(i).1)
}

async fn token(
    State(api): State<Arc<ApiState>>,
    client: ClientInfo,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let mut fields = match form_of(&headers, &body) {
        Ok(f) => f,
        Err(e) => return oauth_error(&e),
    };
    let form = TokenRequest {
        grant_type: take(&mut fields, "grant_type"),
        code: take(&mut fields, "code"),
        redirect_uri: take(&mut fields, "redirect_uri"),
        client_id: take(&mut fields, "client_id"),
        code_verifier: take(&mut fields, "code_verifier"),
        refresh_token: take(&mut fields, "refresh_token"),
        scope: take(&mut fields, "scope"),
        resource: take(&mut fields, "resource"),
    };
    match api.oauth.exchange(form, client.ip).await {
        Ok(t) => (
            [
                (header::CACHE_CONTROL, NO_STORE),
                (header::PRAGMA, "no-cache"),
            ],
            Json(json!({
                "access_token": t.access_token.expose(),
                "token_type": t.token_type,
                "expires_in": t.expires_in,
                "refresh_token": t.refresh_token.expose(),
                "scope": t.scope,
            })),
        )
            .into_response(),
        Err(e) => oauth_error(&e),
    }
}

/// An RFC 7591 registration request (members this server does not use are ignored).
#[derive(Debug, Deserialize)]
struct RegistrationBody {
    client_name: Option<String>,
    #[serde(default)]
    redirect_uris: Vec<String>,
    client_uri: Option<String>,
    logo_uri: Option<String>,
    token_endpoint_auth_method: Option<String>,
    grant_types: Option<Vec<String>>,
    response_types: Option<Vec<String>>,
}

async fn register(
    State(api): State<Arc<ApiState>>,
    client: ClientInfo,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let meta = |m: String| oauth_error(&OAuthError::InvalidClientMetadata(m));
    if !content_type_is(&headers, "application/json") {
        return meta("the body must be application/json".into());
    }
    let mut de = serde_json::Deserializer::from_slice(&body);
    let input: RegistrationBody = match serde_path_to_error::deserialize(&mut de) {
        Ok(b) => b,
        Err(e) => {
            let path = e.path().to_string();
            return meta(if path == "." {
                "the body is not a JSON object of client metadata".into()
            } else {
                format!("{path} has the wrong type")
            });
        }
    };
    let registered = api
        .oauth
        .register_client(
            ClientRegistration {
                client_name: input.client_name,
                redirect_uris: input.redirect_uris,
                client_uri: input.client_uri,
                logo_uri: input.logo_uri,
                token_endpoint_auth_method: input.token_endpoint_auth_method,
                grant_types: input.grant_types,
                response_types: input.response_types,
            },
            client.ip,
        )
        .await;
    match registered {
        Ok(c) => {
            let mut doc = json!({
                "client_id": c.id.as_str(),
                "client_id_issued_at": c.created_at.as_datetime().timestamp(),
                "client_name": c.name,
                "redirect_uris": c.redirect_uris,
                "grant_types": GRANT_TYPES,
                "response_types": ["code"],
                "token_endpoint_auth_method": "none",
            });
            if let Some(uri) = c.client_uri {
                doc["client_uri"] = uri.into();
            }
            if let Some(uri) = c.logo_uri {
                doc["logo_uri"] = uri.into();
            }
            (
                StatusCode::CREATED,
                [(header::CACHE_CONTROL, NO_STORE)],
                Json(doc),
            )
                .into_response()
        }
        Err(e) => oauth_error(&e),
    }
}

async fn revoke(
    State(api): State<Arc<ApiState>>,
    client: ClientInfo,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Err(e) = api
        .auth
        .hit_limit("oauth:revoke", client.ip, REVOKE_PER_IP, LIMIT_WINDOW)
        .await
    {
        return oauth_error(&OAuthError::from(e));
    }
    let token = match form_of(&headers, &body) {
        Ok(mut fields) => take(&mut fields, "token").filter(|t| !t.is_empty()),
        Err(e) => return oauth_error(&e),
    };
    let Some(token) = token else {
        return oauth_error(&OAuthError::InvalidRequest("token is missing".into()));
    };
    match api.oauth.revoke(&token).await {
        Ok(()) => (StatusCode::OK, [(header::CACHE_CONTROL, NO_STORE)]).into_response(),
        Err(e) => oauth_error(&OAuthError::Server(e)),
    }
}

#[cfg(test)]
mod tests {
    use http_body_util::BodyExt;

    use super::*;

    #[tokio::test]
    async fn error_page_escapes_and_locks_down() {
        let res = error_page(
            StatusCode::BAD_REQUEST,
            "<b>Title</b>",
            "client \"<script>alert(1)</script>\" & co",
        );
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
        assert_eq!(res.headers()[header::X_FRAME_OPTIONS], "DENY");
        assert_eq!(res.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(res.headers()[header::CONTENT_SECURITY_POLICY], PAGE_CSP);
        let body = res.into_body().collect().await.unwrap().to_bytes();
        let body = String::from_utf8(body.to_vec()).unwrap();
        assert!(!body.contains("<script>"), "{body}");
        assert!(!body.contains("<b>"), "{body}");
        assert!(
            body.contains("&lt;script&gt;alert(1)&lt;/script&gt;"),
            "{body}"
        );
        assert!(body.contains("&amp; co"), "{body}");
    }

    #[test]
    fn authorize_parameters_must_not_repeat() {
        let p = authorize_params("client_id=a&state=x%20y&unknown=1&unknown=2").unwrap();
        assert_eq!(p.client_id.as_deref(), Some("a"));
        assert_eq!(p.state.as_deref(), Some("x y"));
        assert!(authorize_params("state=1&state=2").is_err());
        assert!(authorize_params("").unwrap().client_id.is_none());
    }

    #[test]
    fn forms_and_content_types() {
        let mut h = HeaderMap::new();
        assert!(!content_type_is(&h, FORM));
        h.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("Application/X-WWW-Form-Urlencoded; charset=UTF-8"),
        );
        assert!(content_type_is(&h, FORM));
        let mut f = form_of(&h, b"a=1&b=%2B").unwrap();
        assert_eq!(take(&mut f, "b").as_deref(), Some("+"));
        assert!(take(&mut f, "c").is_none());
        assert_eq!(
            form_fields(b"a=1&a=2").unwrap_err().as_code(),
            "invalid_request"
        );
    }
}
