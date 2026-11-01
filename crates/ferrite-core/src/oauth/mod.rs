//! The built-in OAuth 2.1 authorization server (design 12 §3.2–§3.3, §7) for public
//! clients only: the authorization code grant with PKCE `S256`, refresh tokens with
//! rotation, RFC 7009 revocation, dynamic client registration (RFC 7591) and Client ID
//! Metadata Documents.
//!
//! * **Clients** are known by a metadata document (`client_id` an `https` URL, fetched by
//!   a [`ClientMetadataFetcher`], cached 24 hours, a cached copy kept up to 7 days while
//!   fetches fail) or by registration (`ferc_` + 22 base64url characters). Only
//!   [`begin`](OAuthService::begin) fetches; the token endpoint uses the stored copy.
//! * **Requests** (`/oauth/authorize`) are validated, then stored for 10 minutes until
//!   the user approves or denies them; an unknown client or an unregistered redirect URI
//!   is an error page ([`AuthorizeError::ShowPage`]), never a redirect (open redirect,
//!   §7). Approving takes the request (single use).
//! * **Codes** live 60 seconds; only their SHA-256 is stored. Claiming the code
//!   ([`OAuthRepo::take_code`](ferrite_store::OAuthRepo::take_code)), inserting the grant
//!   and inserting the access token happen in one `Tx`, so a concurrent replay sees either
//!   an unclaimed code or a grant that exists. A reused code revokes the grant made from
//!   it (OAuth 2.1 §4.1.3). A code presented with a wrong client, redirect URI or PKCE
//!   verifier is burnt (claimed, no grant).
//! * **Grants** hold the refresh token (`ferr_` + 32 random bytes, SHA-256 stored), last
//!   30 days from their last use and are replaced when the same user connects the same
//!   client again, so a user holds at most one grant per client.
//! * **Refresh** rotates in one `Tx`: a compare-and-set on the current hash, the grant's
//!   older access tokens deleted, a new one inserted. A refresh token used after its
//!   rotation revokes the grant (replay detection). Detection is **one rotation deep**:
//!   only the previous hash is kept, so a token two rotations old is merely unknown
//!   (`invalid_grant`) and does not revoke. `scope` may narrow the new access token, never
//!   widen it; the grant keeps what the user approved.
//! * **Access tokens** are `api_tokens` rows (`fer_…`, 1 hour) tied to the grant, named
//!   after the client, with the grant's (or the narrowed) scopes. They do not count
//!   towards the 50 personal tokens; [`AuthService::resolve_token`] marks them
//!   ([`Authenticated::oauth`](crate::Authenticated::oauth)) so the REST API can refuse
//!   them (audience binding, §3.1).
//! * **Scopes** are the 22 area scopes. A user can grant what their role can use
//!   ([`role_scopes`]) within their credential's scopes; a request asking for `<area>:write`
//!   that the role can only read offers `<area>:read`.
//! * Audit: `oauth.connect` when the user approves (`{client, scopes}`), and
//!   `oauth.disconnect` whenever a grant ends early (`{client, user, reason}`:
//!   `disconnected`, `revoked`, `code_reuse` or `refresh_replay`). Never codes, tokens or
//!   hashes.
//! * Limits: 10 registrations per IP and hour, 30 token requests per IP and minute
//!   (counted before anything else, outside any `Tx`).

mod metadata;
mod pkce;
mod redirect;

use std::collections::HashMap;
use std::fmt;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use ferrite_domain::{
    ApiToken, ApiTokenId, ClientKind, NewAudit, OAuthClient, OAuthClientId, OAuthCode, OAuthGrant,
    OAuthGrantId, OAuthRequest, OAuthRequestId, Secret, Timestamp, UserId, UserStatus,
};
use ferrite_store::{Repos, Store};
use serde_json::json;
use url::Url;

pub use metadata::{ClientMetadataFetcher, FetchError, MAX_METADATA_DOCUMENT, metadata_url};

use crate::auth::{
    AuthService, MAX_TOKEN_NAME, mint_bearer, older_than, plus, random_bytes, sha256_hex,
};
use crate::{
    Actor, CoreError, Scope, Scopes,
    actor::{Area, Capability, can, require, require_scope, role_scopes},
    tx::finish,
};

/// How long an access token lasts (`expires_in`).
pub const ACCESS_TOKEN_TTL: Duration = Duration::from_secs(3600);
/// How long an authorization code lasts.
pub const CODE_TTL: Duration = Duration::from_secs(60);
/// How long a pending authorization request waits for consent.
pub const REQUEST_TTL: Duration = Duration::from_secs(10 * 60);
/// How long a grant (its refresh token) lasts after its last use.
pub const GRANT_TTL: Duration = Duration::from_secs(30 * 24 * 3600);
/// A metadata document younger than this is not fetched again.
const METADATA_FRESH: Duration = Duration::from_secs(24 * 3600);
/// While fetches fail, a cached document is used up to this long after its last fetch.
const METADATA_STALE: Duration = Duration::from_secs(7 * 24 * 3600);
/// Longest `state` kept, in bytes.
const MAX_STATE: usize = 1024;
/// Longest code or token looked up, in bytes.
const MAX_CREDENTIAL: usize = 256;
/// Prefix of a registered client's id.
const CLIENT_ID_PREFIX: &str = "ferc_";
/// Prefix of a refresh token.
const REFRESH_PREFIX: &str = "ferr_";
/// The grant types a client may use.
const GRANT_TYPES: [&str; 2] = ["authorization_code", "refresh_token"];

/// A dynamic client registration (RFC 7591), as the client sent it.
#[derive(Debug, Clone, Default)]
pub struct ClientRegistration {
    pub client_name: Option<String>,
    pub redirect_uris: Vec<String>,
    pub client_uri: Option<String>,
    pub logo_uri: Option<String>,
    /// Absent or `none` (public clients only).
    pub token_endpoint_auth_method: Option<String>,
    /// A subset of `authorization_code` and `refresh_token`.
    pub grant_types: Option<Vec<String>>,
    /// Only `code`.
    pub response_types: Option<Vec<String>>,
}

/// The query of `GET /oauth/authorize`, as sent.
#[derive(Debug, Clone, Default)]
pub struct AuthorizeParams {
    pub response_type: Option<String>,
    pub client_id: Option<String>,
    pub redirect_uri: Option<String>,
    pub code_challenge: Option<String>,
    pub code_challenge_method: Option<String>,
    pub state: Option<String>,
    /// Space-separated area scopes; absent or empty asks for all of them.
    pub scope: Option<String>,
    pub resource: Option<String>,
}

/// The form of `POST /oauth/token`, as sent. `Debug` redacts the code, verifier and
/// refresh token.
#[derive(Clone, Default)]
pub struct TokenRequest {
    pub grant_type: Option<String>,
    pub code: Option<String>,
    pub redirect_uri: Option<String>,
    pub client_id: Option<String>,
    pub code_verifier: Option<String>,
    pub refresh_token: Option<String>,
    pub scope: Option<String>,
    pub resource: Option<String>,
}

impl fmt::Debug for TokenRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let redacted = |v: &Option<String>| v.as_ref().map(|_| "[redacted]");
        f.debug_struct("TokenRequest")
            .field("grant_type", &self.grant_type)
            .field("code", &redacted(&self.code))
            .field("redirect_uri", &self.redirect_uri)
            .field("client_id", &self.client_id)
            .field("code_verifier", &redacted(&self.code_verifier))
            .field("refresh_token", &redacted(&self.refresh_token))
            .field("scope", &self.scope)
            .field("resource", &self.resource)
            .finish()
    }
}

/// A successful token response (RFC 6749 §5.1).
#[derive(Debug, Clone)]
pub struct TokenResponse {
    pub access_token: Secret,
    /// Always `Bearer`.
    pub token_type: &'static str,
    /// Seconds: always 3600.
    pub expires_in: u64,
    pub refresh_token: Secret,
    /// The access token's scopes, space-separated.
    pub scope: String,
}

/// What the consent screen shows for a pending request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsentView {
    pub client: OAuthClient,
    /// Known by a metadata document (not self-registered).
    pub verified: bool,
    /// Where the browser goes back to: the host of an `http(s)` redirect URI,
    /// `scheme://host` for another scheme with a host, the whole URI without one (so
    /// `cursor://example.com/…` cannot pass for `example.com`).
    pub redirect_host: String,
    /// The requested scopes (all 22 when the client sent no `scope`).
    pub requested: Vec<String>,
    /// The requested scopes the user can grant (preselected).
    pub grantable: Vec<String>,
}

/// A grant with its client (Account → Connected apps, Users → user).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrantView {
    pub grant: OAuthGrant,
    pub client: OAuthClient,
}

/// An OAuth error (RFC 6749 §5.2, RFC 7591 §3.2.2, RFC 8707). The text never holds a
/// credential.
#[derive(Debug, thiserror::Error)]
pub enum OAuthError {
    #[error("{0}")]
    InvalidRequest(String),
    #[error("{0}")]
    InvalidClient(String),
    #[error("{0}")]
    InvalidGrant(String),
    #[error("{0}")]
    UnsupportedGrantType(String),
    #[error("{0}")]
    InvalidScope(String),
    /// `resource` is not this site's MCP endpoint.
    #[error("{0}")]
    InvalidTarget(String),
    #[error("{0}")]
    InvalidRedirectUri(String),
    #[error("{0}")]
    InvalidClientMetadata(String),
    /// A per-IP limit was hit (`429`).
    #[error("too many requests (retry after {retry_after_secs} s)")]
    RateLimited { retry_after_secs: u64 },
    /// Storage or another internal failure (`500`).
    #[error("server error: {0}")]
    Server(CoreError),
}

impl OAuthError {
    /// The `error` code.
    pub fn as_code(&self) -> &'static str {
        match self {
            Self::InvalidRequest(_) => "invalid_request",
            Self::InvalidClient(_) => "invalid_client",
            Self::InvalidGrant(_) => "invalid_grant",
            Self::UnsupportedGrantType(_) => "unsupported_grant_type",
            Self::InvalidScope(_) => "invalid_scope",
            Self::InvalidTarget(_) => "invalid_target",
            Self::InvalidRedirectUri(_) => "invalid_redirect_uri",
            Self::InvalidClientMetadata(_) => "invalid_client_metadata",
            Self::RateLimited { .. } => "temporarily_unavailable",
            Self::Server(_) => "server_error",
        }
    }

    /// The `error_description`: says what is wrong, never internal details.
    pub fn description(&self) -> String {
        match self {
            Self::Server(_) => "the request could not be completed".to_owned(),
            other => other.to_string(),
        }
    }

    /// The HTTP status: `429` when limited, `500` for a server error, else `400`.
    pub fn status(&self) -> u16 {
        match self {
            Self::RateLimited { .. } => 429,
            Self::Server(_) => 500,
            _ => 400,
        }
    }
}

impl From<CoreError> for OAuthError {
    fn from(e: CoreError) -> Self {
        match e {
            CoreError::RateLimited { retry_after_secs } => Self::RateLimited { retry_after_secs },
            other => Self::Server(other),
        }
    }
}

impl From<ferrite_store::StoreError> for OAuthError {
    fn from(e: ferrite_store::StoreError) -> Self {
        Self::Server(e.into())
    }
}

/// Why an authorization request was refused.
#[derive(Debug)]
pub enum AuthorizeError {
    /// The client or redirect URI cannot be validated: show an error page with this text
    /// (plain text, to be escaped), never redirect.
    ShowPage(String),
    /// Send the browser to `uri`: the request's redirect URI with `error`,
    /// `error_description`, `state` (when the client sent one) and `iss` added (RFC 6749
    /// §4.1.2.1, RFC 9207). The other fields repeat what `uri` carries.
    Redirect {
        uri: Box<Url>,
        error: &'static str,
        description: String,
        state: Option<String>,
    },
    /// Storage failed (`500`).
    Server(CoreError),
}

/// The client lookup's failure.
enum Lookup {
    /// Not a client we can serve; the text says why.
    Unknown(String),
    Store(CoreError),
}

impl From<ferrite_store::StoreError> for Lookup {
    fn from(e: ferrite_store::StoreError) -> Self {
        Self::Store(e.into())
    }
}

/// The OAuth authorization server.
pub struct OAuthService {
    store: Arc<dyn Store>,
    auth: Arc<AuthService>,
    fetcher: Arc<dyn ClientMetadataFetcher>,
    /// `site.url` without a trailing slash.
    issuer: String,
    /// `<issuer>/mcp`.
    resource: String,
}

impl OAuthService {
    /// `issuer` is `site.url`; a trailing slash is dropped. The clock is `auth`'s.
    pub fn new(
        store: Arc<dyn Store>,
        auth: Arc<AuthService>,
        fetcher: Arc<dyn ClientMetadataFetcher>,
        issuer: Url,
    ) -> Self {
        let issuer = issuer.as_str().trim_end_matches('/').to_owned();
        let resource = format!("{issuer}/mcp");
        Self {
            store,
            auth,
            fetcher,
            issuer,
            resource,
        }
    }

    /// The issuer: `site.url` without a trailing slash (RFC 8414, RFC 9207 `iss`).
    pub fn issuer(&self) -> &str {
        &self.issuer
    }

    /// The protected resource: `<issuer>/mcp` (RFC 8707, RFC 9728).
    pub fn resource(&self) -> &str {
        &self.resource
    }

    fn now(&self) -> Timestamp {
        self.auth.now()
    }

    // ---- clients --------------------------------------------------------------------

    /// Registers a public client (RFC 7591). Counts on the per-IP limit first (10 an
    /// hour, refused registrations included).
    pub async fn register_client(
        &self,
        input: ClientRegistration,
        ip: Option<IpAddr>,
    ) -> Result<OAuthClient, OAuthError> {
        self.auth.hit_oauth_register(ip).await?;
        let meta = OAuthError::InvalidClientMetadata;
        if input
            .token_endpoint_auth_method
            .as_deref()
            .is_some_and(|m| m != "none")
        {
            return Err(meta(
                "only public clients are supported (token_endpoint_auth_method none)".into(),
            ));
        }
        if let Some(t) = input
            .grant_types
            .iter()
            .flatten()
            .find(|t| !GRANT_TYPES.contains(&t.as_str()))
        {
            return Err(meta(format!("grant type {t:?} is not supported")));
        }
        if input.response_types.iter().flatten().any(|t| t != "code") {
            return Err(meta("only the response type code is supported".into()));
        }
        let fields = metadata::client_fields(
            input.client_name.as_deref(),
            &input.redirect_uris,
            input.client_uri.as_deref(),
            input.logo_uri.as_deref(),
        )
        .map_err(|e| match e {
            metadata::FieldError::RedirectUri(m) => OAuthError::InvalidRedirectUri(m),
            metadata::FieldError::Metadata(m) => OAuthError::InvalidClientMetadata(m),
        })?;
        let id = format!(
            "{CLIENT_ID_PREFIX}{}",
            URL_SAFE_NO_PAD.encode(random_bytes::<16>()?)
        );
        let client = OAuthClient {
            id: OAuthClientId::new(id),
            kind: ClientKind::Registered,
            name: fields.name,
            redirect_uris: fields.redirect_uris,
            client_uri: fields.client_uri,
            logo_uri: fields.logo_uri,
            created_at: self.now(),
            fetched_at: None,
            last_used_at: None,
        };
        self.store.read().oauth().upsert_client(&client).await?;
        Ok(client)
    }

    /// The client behind a `client_id` at the authorization endpoint: a registered one
    /// from the store, a metadata-document one from the cache or a fresh fetch.
    async fn known_client(&self, client_id: &str) -> Result<OAuthClient, Lookup> {
        let oauth = self.store.read().oauth();
        let Some(url) = metadata_url(client_id) else {
            if !client_id.starts_with(CLIENT_ID_PREFIX) {
                return Err(Lookup::Unknown(
                    "the client_id is neither a registered client nor an https URL with a path"
                        .into(),
                ));
            }
            return oauth
                .get_client(&OAuthClientId::new(client_id))
                .await?
                .filter(|c| c.kind == ClientKind::Registered)
                .ok_or_else(|| Lookup::Unknown("the client is not registered here".into()));
        };
        let now = self.now();
        let cached = oauth
            .get_client(&OAuthClientId::new(client_id))
            .await?
            .filter(|c| c.kind == ClientKind::Metadata);
        let fetched_within = |c: &OAuthClient, d: Duration| {
            c.fetched_at
                .is_some_and(|at| !older_than(at, now, d) && at <= now)
        };
        if let Some(c) = cached
            .as_ref()
            .filter(|c| fetched_within(c, METADATA_FRESH))
        {
            return Ok(c.clone());
        }
        match self.fetcher.fetch(&url).await {
            Ok(bytes) => {
                let fields = metadata::parse_document(&url, &bytes).map_err(|why| {
                    Lookup::Unknown(format!(
                        "the client's metadata document is not valid: {why}"
                    ))
                })?;
                let client = OAuthClient {
                    id: OAuthClientId::new(client_id),
                    kind: ClientKind::Metadata,
                    name: fields.name,
                    redirect_uris: fields.redirect_uris,
                    client_uri: fields.client_uri,
                    logo_uri: fields.logo_uri,
                    created_at: cached.as_ref().map_or(now, |c| c.created_at),
                    fetched_at: Some(now),
                    last_used_at: cached.as_ref().and_then(|c| c.last_used_at),
                };
                oauth.upsert_client(&client).await?;
                Ok(client)
            }
            Err(e) => {
                tracing::info!(client = client_id, error = %e, "client metadata fetch failed");
                match cached.filter(|c| fetched_within(c, METADATA_STALE)) {
                    Some(c) => Ok(c),
                    None => Err(Lookup::Unknown(
                        "the client's metadata document could not be fetched".into(),
                    )),
                }
            }
        }
    }

    // ---- authorization requests ---------------------------------------------------------

    /// Validates an authorization request and stores it for 10 minutes; the browser then
    /// goes to the consent page with the returned id. The client and redirect URI are
    /// checked first (failures are [`AuthorizeError::ShowPage`]), then the rest (failures
    /// redirect back with an error).
    pub async fn begin(&self, req: AuthorizeParams) -> Result<OAuthRequestId, AuthorizeError> {
        let page = |m: &str| AuthorizeError::ShowPage(m.to_owned());
        let client_id = req
            .client_id
            .as_deref()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| page("The request has no client_id."))?;
        let client = match self.known_client(client_id).await {
            Ok(c) => c,
            Err(Lookup::Unknown(why)) => return Err(AuthorizeError::ShowPage(why)),
            Err(Lookup::Store(e)) => return Err(AuthorizeError::Server(e)),
        };
        let redirect_uri = req
            .redirect_uri
            .as_deref()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| page("The request has no redirect_uri."))?;
        if !redirect::matches(&client.redirect_uris, redirect_uri) {
            return Err(page("The redirect_uri is not one the client registered."));
        }
        let base = Url::parse(redirect_uri).map_err(|_| page("The redirect_uri is not a URI."))?;

        // From here on, errors go back to the client.
        let state_ok = req
            .state
            .as_deref()
            .is_none_or(|s| s.len() <= MAX_STATE && !s.chars().any(char::is_control));
        let state = req.state.clone().filter(|_| state_ok);
        let fail = |error: &'static str, description: &str| {
            let mut params = vec![("error", error), ("error_description", description)];
            if let Some(s) = state.as_deref() {
                params.push(("state", s));
            }
            AuthorizeError::Redirect {
                uri: Box::new(self.location(&base, &params)),
                error,
                description: description.to_owned(),
                state: state.clone(),
            }
        };
        if !state_ok {
            return Err(fail(
                "invalid_request",
                "state must be at most 1024 bytes without control characters",
            ));
        }
        match req.response_type.as_deref() {
            Some("code") => {}
            None => return Err(fail("invalid_request", "response_type is missing")),
            Some(_) => {
                return Err(fail(
                    "unsupported_response_type",
                    "only the response type code is supported",
                ));
            }
        }
        if req.code_challenge_method.as_deref() != Some("S256") {
            return Err(fail(
                "invalid_request",
                "PKCE is required with code_challenge_method S256",
            ));
        }
        let challenge = match req.code_challenge.as_deref() {
            Some(c) if pkce::is_challenge(c) => c.to_owned(),
            _ => {
                return Err(fail(
                    "invalid_request",
                    "code_challenge must be 43 base64url characters",
                ));
            }
        };
        let scopes = match req.scope.as_deref().map(parse_scope) {
            None | Some(Ok(None)) => Scopes::ALL,
            Some(Ok(Some(s))) => s,
            Some(Err(why)) => return Err(fail("invalid_scope", &why)),
        };
        let resource = match req.resource.as_deref() {
            None => None,
            Some(r) if self.is_resource(r) => Some(self.resource.clone()),
            Some(_) => {
                return Err(fail(
                    "invalid_target",
                    "resource must be this site's MCP endpoint",
                ));
            }
        };
        let now = self.now();
        let request = OAuthRequest {
            id: OAuthRequestId::new(),
            client_id: client.id,
            redirect_uri: redirect_uri.to_owned(),
            scopes: scopes.to_strings(),
            state,
            code_challenge: challenge,
            resource,
            created_at: now,
            expires_at: plus(now, REQUEST_TTL).map_err(AuthorizeError::Server)?,
        };
        self.store
            .read()
            .oauth()
            .insert_request(&request)
            .await
            .map_err(|e| AuthorizeError::Server(e.into()))?;
        Ok(request.id)
    }

    /// The pending request as the consent screen shows it (`users:read`). Unknown or
    /// expired: `NotFound`.
    pub async fn describe(
        &self,
        actor: &Actor,
        id: OAuthRequestId,
    ) -> Result<ConsentView, CoreError> {
        require_scope(actor, Some(Scope::read(Area::Users)))?;
        let oauth = self.store.read().oauth();
        let request = oauth
            .get_request(id, self.now())
            .await?
            .ok_or(CoreError::NotFound)?;
        let client = oauth
            .get_client(&request.client_id)
            .await?
            .ok_or(CoreError::NotFound)?;
        let requested = Scopes::from_strs(&request.scopes)?;
        let redirect_host = redirect_host(&parse_stored(&request.redirect_uri)?);
        Ok(ConsentView {
            verified: client.kind == ClientKind::Metadata,
            client,
            redirect_host,
            requested: requested.to_strings(),
            grantable: requested.within(grantable_by(actor)).to_strings(),
        })
    }

    /// Approves a pending request with the scopes the user chose (`users:write`): they
    /// must be non-empty and within what [`describe`](Self::describe) calls grantable
    /// (else `Forbidden(<scope>)`, and the request stays). Takes the request, stores a
    /// 60-second code and audits `oauth.connect`; returns where the browser goes:
    /// `redirect_uri` with `code`, `state` and `iss`.
    pub async fn approve(
        &self,
        actor: &Actor,
        id: OAuthRequestId,
        scopes: Vec<String>,
    ) -> Result<Url, CoreError> {
        require_scope(actor, Some(Scope::write(Area::Users)))?;
        let chosen = Scopes::from_strs(&scopes)?;
        if chosen == Scopes::NONE {
            return Err(CoreError::invalid_field(
                "scopes",
                "choose at least one permission",
            ));
        }
        let now = self.now();
        let code = URL_SAFE_NO_PAD.encode(random_bytes::<32>()?);
        let code_hash = sha256_hex(code.as_bytes());
        let expires_at = plus(now, CODE_TTL)?;
        let tx = self.store.begin().await?;
        let done = async {
            let oauth = tx.oauth();
            let request = oauth
                .take_request(id, now)
                .await?
                .ok_or(CoreError::NotFound)?;
            let client = oauth
                .get_client(&request.client_id)
                .await?
                .ok_or(CoreError::NotFound)?;
            let grantable = Scopes::from_strs(&request.scopes)?.within(grantable_by(actor));
            if let Some(s) = chosen.iter().find(|&s| !grantable.allows(s)) {
                return Err(CoreError::Forbidden(s.as_str()));
            }
            oauth
                .insert_code(&OAuthCode {
                    code_hash,
                    client_id: client.id.clone(),
                    user_id: actor.user,
                    redirect_uri: request.redirect_uri.clone(),
                    scopes: chosen.to_strings(),
                    code_challenge: request.code_challenge.clone(),
                    resource: request.resource.clone(),
                    created_at: now,
                    expires_at,
                    grant_id: None,
                })
                .await?;
            tx.audit()
                .append(&NewAudit {
                    actor_id: Some(actor.user),
                    action: "oauth.connect".to_owned(),
                    target_kind: "oauth_client".to_owned(),
                    target_id: Some(client.id.to_string()),
                    details: json!({ "client": client.name, "scopes": chosen.to_strings() }),
                })
                .await?;
            Ok(request)
        }
        .await;
        let request = finish(tx, done).await?;
        let base = parse_stored(&request.redirect_uri)?;
        let mut params = vec![("code", code.as_str())];
        if let Some(s) = request.state.as_deref() {
            params.push(("state", s));
        }
        Ok(self.location(&base, &params))
    }

    /// Denies a pending request (takes it) with the same authorization as
    /// [`approve`](Self::approve) (`users:write`); returns `redirect_uri` with
    /// `error=access_denied`, `state` and `iss`. Unknown or expired: `NotFound`.
    pub async fn deny(&self, actor: &Actor, id: OAuthRequestId) -> Result<Url, CoreError> {
        require_scope(actor, Some(Scope::write(Area::Users)))?;
        let request = self
            .store
            .read()
            .oauth()
            .take_request(id, self.now())
            .await?
            .ok_or(CoreError::NotFound)?;
        let base = parse_stored(&request.redirect_uri)?;
        let mut params = vec![
            ("error", "access_denied"),
            ("error_description", "the user denied access"),
        ];
        if let Some(s) = request.state.as_deref() {
            params.push(("state", s));
        }
        Ok(self.location(&base, &params))
    }

    /// `base` with `params` and `iss` appended to its query (its own parameters stay).
    fn location(&self, base: &Url, params: &[(&str, &str)]) -> Url {
        let mut url = base.clone();
        {
            let mut q = url.query_pairs_mut();
            for (k, v) in params {
                q.append_pair(k, v);
            }
            q.append_pair("iss", &self.issuer);
        }
        url
    }

    /// `<issuer>/mcp`, a trailing slash tolerated.
    fn is_resource(&self, r: &str) -> bool {
        r.strip_suffix('/').unwrap_or(r) == self.resource
    }

    // ---- token endpoint ------------------------------------------------------------------

    /// `POST /oauth/token`: the `authorization_code` and `refresh_token` grants. Counts on
    /// the per-IP limit first (30 a minute).
    pub async fn exchange(
        &self,
        form: TokenRequest,
        ip: Option<IpAddr>,
    ) -> Result<TokenResponse, OAuthError> {
        self.auth.hit_oauth_token(ip).await?;
        if let Some(r) = form.resource.as_deref()
            && !self.is_resource(r)
        {
            return Err(OAuthError::InvalidTarget(
                "resource must be this site's MCP endpoint".into(),
            ));
        }
        match form.grant_type.as_deref() {
            Some("authorization_code") => self.exchange_code(&form).await,
            Some("refresh_token") => self.exchange_refresh(&form).await,
            Some(other) => Err(OAuthError::UnsupportedGrantType(format!(
                "grant type {other:?} is not supported"
            ))),
            None => Err(OAuthError::InvalidRequest("grant_type is missing".into())),
        }
    }

    async fn exchange_code(&self, form: &TokenRequest) -> Result<TokenResponse, OAuthError> {
        let code = required(&form.code, "code")?;
        let client_id = required(&form.client_id, "client_id")?;
        let redirect_uri = required(&form.redirect_uri, "redirect_uri")?;
        let verifier = required(&form.code_verifier, "code_verifier")?;
        if !pkce::is_verifier(verifier) {
            return Err(OAuthError::InvalidRequest(
                "code_verifier must be 43 to 128 unreserved characters".into(),
            ));
        }
        if code.len() > MAX_CREDENTIAL {
            return Err(OAuthError::InvalidGrant(
                "the code is unknown or expired".into(),
            ));
        }
        let code_hash = sha256_hex(code.as_bytes());
        let now = self.now();
        let grant_id = OAuthGrantId::new();
        let (access, access_hash) = mint_bearer()?;
        let (refresh, refresh_hash) = mint_refresh()?;
        let grant_expires = plus(now, GRANT_TTL)?;
        let access_expires = plus(now, ACCESS_TOKEN_TTL)?;

        /// What the claiming `Tx` decided.
        enum Claim {
            Issued(String),
            /// Refused after the code was claimed: commit, so it stays burnt.
            Burnt(OAuthError),
            Unclaimed,
        }
        let tx = self.store.begin().await?;
        let done = async {
            let oauth = tx.oauth();
            let Some(claimed) = oauth.take_code(&code_hash, now, grant_id).await? else {
                return Ok(Claim::Unclaimed);
            };
            let burnt = |m: &str| -> Result<Claim, CoreError> {
                Ok(Claim::Burnt(OAuthError::InvalidGrant(m.to_owned())))
            };
            if claimed.client_id.as_str() != client_id {
                return burnt("the code was issued to another client");
            }
            if claimed.redirect_uri != redirect_uri {
                return burnt("redirect_uri does not match the authorization request");
            }
            if !pkce::verify(verifier, &claimed.code_challenge) {
                return burnt("code_verifier does not match the code_challenge");
            }
            if !active(tx.as_ref(), claimed.user_id).await? {
                return burnt("the user can no longer sign in");
            }
            let Some(client) = oauth.get_client(&claimed.client_id).await? else {
                return Ok(Claim::Burnt(OAuthError::InvalidClient(
                    "the client is no longer known".into(),
                )));
            };
            // One grant per user and client: connecting again replaces the earlier one.
            for old in oauth.grants_for_user(claimed.user_id).await? {
                if old.client_id == client.id {
                    oauth.delete_grant(old.id).await?;
                }
            }
            let grant = OAuthGrant {
                id: grant_id,
                client_id: client.id.clone(),
                user_id: claimed.user_id,
                scopes: claimed.scopes.clone(),
                created_at: now,
                last_used_at: now,
                expires_at: grant_expires,
            };
            oauth.insert_grant(&grant, &refresh_hash).await?;
            tx.api_tokens()
                .insert(&access_token(
                    &client,
                    &grant,
                    &claimed.scopes,
                    access_hash,
                    now,
                    access_expires,
                ))
                .await?;
            oauth.touch_client(&client.id, now).await?;
            Ok(Claim::Issued(claimed.scopes.join(" ")))
        }
        .await;
        match finish(tx, done).await? {
            Claim::Issued(scope) => Ok(TokenResponse {
                access_token: access,
                token_type: "Bearer",
                expires_in: ACCESS_TOKEN_TTL.as_secs(),
                refresh_token: refresh,
                scope,
            }),
            Claim::Burnt(e) => Err(e),
            Claim::Unclaimed => {
                // Reuse detection: a used code names the grant made from it.
                let used = self
                    .store
                    .read()
                    .oauth()
                    .code_by_hash_any(&code_hash)
                    .await?;
                match used.and_then(|c| c.grant_id) {
                    Some(grant) => {
                        if self.end_grant(grant, None, "code_reuse").await? {
                            tracing::warn!(%grant, "authorization code reused: grant revoked");
                        }
                        Err(OAuthError::InvalidGrant(
                            "the code was already used; the connection was revoked".into(),
                        ))
                    }
                    None => Err(OAuthError::InvalidGrant(
                        "the code is unknown or expired".into(),
                    )),
                }
            }
        }
    }

    async fn exchange_refresh(&self, form: &TokenRequest) -> Result<TokenResponse, OAuthError> {
        let presented = required(&form.refresh_token, "refresh_token")?;
        let client_id = required(&form.client_id, "client_id")?;
        let unknown = || OAuthError::InvalidGrant("the refresh token is unknown or expired".into());
        if presented.len() > MAX_CREDENTIAL {
            return Err(unknown());
        }
        let hash = sha256_hex(presented.as_bytes());
        let now = self.now();
        let oauth = self.store.read().oauth();
        let Some(grant) = oauth.grant_by_refresh(&hash).await? else {
            if let Some(old) = oauth.grant_by_previous(&hash).await? {
                if self.end_grant(old.id, None, "refresh_replay").await? {
                    tracing::warn!(grant = %old.id, "refresh token replayed: grant revoked");
                }
                return Err(OAuthError::InvalidGrant(
                    "the refresh token was already used; the connection was revoked".into(),
                ));
            }
            return Err(unknown());
        };
        if grant.client_id.as_str() != client_id {
            return Err(OAuthError::InvalidGrant(
                "the refresh token was issued to another client".into(),
            ));
        }
        if grant.expires_at <= now {
            return Err(unknown());
        }
        let granted = Scopes::from_strs(&grant.scopes)?;
        let scopes = match form.scope.as_deref().map(parse_scope) {
            None | Some(Ok(None)) => granted,
            Some(Ok(Some(asked))) if granted.covers(asked) => asked,
            Some(Ok(Some(_))) => {
                return Err(OAuthError::InvalidScope(
                    "scope may narrow the grant, never widen it".into(),
                ));
            }
            Some(Err(why)) => return Err(OAuthError::InvalidScope(why)),
        };
        let scope_names = scopes.to_strings();
        let (access, access_hash) = mint_bearer()?;
        let (refresh, refresh_hash) = mint_refresh()?;
        let grant_expires = plus(now, GRANT_TTL)?;
        let access_expires = plus(now, ACCESS_TOKEN_TTL)?;
        let tx = self.store.begin().await?;
        let done = async {
            let oauth = tx.oauth();
            if !active(tx.as_ref(), grant.user_id).await? {
                return Ok(Err(OAuthError::InvalidGrant(
                    "the user can no longer sign in".into(),
                )));
            }
            let Some(client) = oauth.get_client(&grant.client_id).await? else {
                return Ok(Err(OAuthError::InvalidClient(
                    "the client is no longer known".into(),
                )));
            };
            if !oauth
                .rotate_grant(grant.id, &refresh_hash, &hash, grant_expires, now)
                .await?
            {
                // A concurrent refresh rotated it first.
                return Ok(Err(unknown()));
            }
            tx.api_tokens().delete_for_grant(grant.id).await?;
            tx.api_tokens()
                .insert(&access_token(
                    &client,
                    &grant,
                    &scope_names,
                    access_hash,
                    now,
                    access_expires,
                ))
                .await?;
            oauth.touch_client(&client.id, now).await?;
            Ok(Ok(()))
        }
        .await;
        finish(tx, done).await??;
        Ok(TokenResponse {
            access_token: access,
            token_type: "Bearer",
            expires_in: ACCESS_TOKEN_TTL.as_secs(),
            refresh_token: refresh,
            scope: scope_names.join(" "),
        })
    }

    /// RFC 7009: revokes an OAuth access token (that token only) or a refresh token (the
    /// whole grant, audited `oauth.disconnect` with reason `revoked`). Unknown tokens and
    /// personal API tokens are ignored (`Ok`).
    pub async fn revoke(&self, token: &str) -> Result<(), CoreError> {
        if token.is_empty() || token.len() > MAX_CREDENTIAL {
            return Ok(());
        }
        let hash = sha256_hex(token.as_bytes());
        let repos = self.store.read();
        if let Some(t) = repos.api_tokens().get_by_hash(&hash).await? {
            if t.oauth_grant_id.is_some() {
                repos.api_tokens().delete(t.user_id, t.id).await?;
            }
            return Ok(());
        }
        if let Some(g) = repos.oauth().grant_by_refresh(&hash).await? {
            self.end_grant(g.id, Some(g.user_id), "revoked").await?;
        }
        Ok(())
    }

    // ---- grants ----------------------------------------------------------------------------

    /// The unexpired grants of `user` (default: the actor), newest first: one's own with
    /// `users:read`, others' with `manage_users` and `users:read`.
    pub async fn grants(
        &self,
        actor: &Actor,
        user: Option<UserId>,
    ) -> Result<Vec<GrantView>, CoreError> {
        let user = user.unwrap_or(actor.user);
        if user != actor.user && !can(actor.role, Capability::ManageUsers) {
            return Err(CoreError::Forbidden(Capability::ManageUsers.as_str()));
        }
        require_scope(actor, Some(Scope::read(Area::Users)))?;
        let oauth = self.store.read().oauth();
        let now = self.now();
        let mut clients: HashMap<OAuthClientId, Option<OAuthClient>> = HashMap::new();
        let mut views = Vec::new();
        for grant in oauth.grants_for_user(user).await? {
            if grant.expires_at <= now {
                continue;
            }
            let client = match clients.get(&grant.client_id) {
                Some(c) => c.clone(),
                None => {
                    let c = oauth.get_client(&grant.client_id).await?;
                    clients.insert(grant.client_id.clone(), c.clone());
                    c
                }
            };
            if let Some(client) = client {
                views.push(GrantView { grant, client });
            }
        }
        Ok(views)
    }

    /// Disconnects a grant, deleting it and its access tokens, and audits
    /// `oauth.disconnect`: one's own with `users:write`, others' with `manage_users`. A
    /// grant that does not exist, or someone else's without `manage_users`, is `NotFound`.
    pub async fn disconnect(&self, actor: &Actor, id: OAuthGrantId) -> Result<(), CoreError> {
        let grant = self
            .store
            .read()
            .oauth()
            .get_grant(id)
            .await?
            .ok_or(CoreError::NotFound)?;
        if grant.user_id == actor.user {
            require_scope(actor, Some(Scope::write(Area::Users)))?;
        } else {
            if !can(actor.role, Capability::ManageUsers) {
                return Err(CoreError::NotFound);
            }
            require(actor, Capability::ManageUsers)?;
        }
        if self.end_grant(id, Some(actor.user), "disconnected").await? {
            Ok(())
        } else {
            Err(CoreError::NotFound)
        }
    }

    /// Deletes a grant (and so its access tokens) and audits `oauth.disconnect` in one
    /// `Tx`; `false` when it was already gone.
    async fn end_grant(
        &self,
        id: OAuthGrantId,
        actor: Option<UserId>,
        reason: &str,
    ) -> Result<bool, CoreError> {
        let tx = self.store.begin().await?;
        let done = async {
            let oauth = tx.oauth();
            let Some(grant) = oauth.get_grant(id).await? else {
                return Ok(false);
            };
            let client = oauth.get_client(&grant.client_id).await?;
            if !oauth.delete_grant(id).await? {
                return Ok(false);
            }
            let name = client.map_or_else(|| grant.client_id.to_string(), |c| c.name);
            tx.audit()
                .append(&NewAudit {
                    actor_id: actor,
                    action: "oauth.disconnect".to_owned(),
                    target_kind: "oauth_grant".to_owned(),
                    target_id: Some(id.to_string()),
                    details: json!({
                        "client": name,
                        "user": grant.user_id.to_string(),
                        "reason": reason,
                    }),
                })
                .await?;
            Ok(true)
        }
        .await;
        finish(tx, done).await
    }
}

/// What `actor` can grant: the role's scopes ([`role_scopes`]) within the credential's.
fn grantable_by(actor: &Actor) -> Scopes {
    role_scopes(actor.role).within(actor.scopes)
}

/// A space-separated scope list: `None` when empty, else the set; an unknown name is
/// the error text.
fn parse_scope(s: &str) -> Result<Option<Scopes>, String> {
    let names: Vec<String> = s
        .split(' ')
        .filter(|n| !n.is_empty())
        .map(String::from)
        .collect();
    if names.is_empty() {
        return Ok(None);
    }
    Scopes::from_strs(&names)
        .map(Some)
        .map_err(|_| "scope holds an unknown scope".to_owned())
}

/// A required, non-empty form field.
fn required<'a>(value: &'a Option<String>, name: &str) -> Result<&'a str, OAuthError> {
    value
        .as_deref()
        .filter(|v| !v.is_empty())
        .ok_or_else(|| OAuthError::InvalidRequest(format!("{name} is missing")))
}

/// A new refresh token (`ferr_` + 32 random bytes as base64url) and its SHA-256 hex.
fn mint_refresh() -> Result<(Secret, String), CoreError> {
    let token = format!(
        "{REFRESH_PREFIX}{}",
        URL_SAFE_NO_PAD.encode(random_bytes::<32>()?)
    );
    let hash = sha256_hex(token.as_bytes());
    Ok((Secret::new(token), hash))
}

/// The `api_tokens` row of an access token: named after the client (cut to the token
/// name limit), bound to the grant.
fn access_token(
    client: &OAuthClient,
    grant: &OAuthGrant,
    scopes: &[String],
    token_hash: String,
    now: Timestamp,
    expires_at: Timestamp,
) -> ApiToken {
    ApiToken {
        id: ApiTokenId::new(),
        user_id: grant.user_id,
        name: client.name.chars().take(MAX_TOKEN_NAME).collect(),
        token_hash,
        scopes: scopes.to_vec(),
        created_at: now,
        last_used_at: None,
        expires_at: Some(expires_at),
        oauth_grant_id: Some(grant.id),
    }
}

/// Whether the user exists and is active.
async fn active(repos: &dyn Repos, user: UserId) -> Result<bool, CoreError> {
    Ok(repos
        .users()
        .get(user)
        .await?
        .is_some_and(|u| u.status == UserStatus::Active))
}

/// The consent screen's redirect host ([`ConsentView::redirect_host`]).
fn redirect_host(uri: &Url) -> String {
    match (uri.scheme(), uri.host_str()) {
        ("http" | "https", Some(host)) => host.to_owned(),
        (scheme, Some(host)) => format!("{scheme}://{host}"),
        (_, None) => uri.as_str().to_owned(),
    }
}

fn parse_stored(uri: &str) -> Result<Url, CoreError> {
    Url::parse(uri).map_err(|_| CoreError::Internal("stored redirect URI does not parse"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_lists() {
        assert_eq!(parse_scope(""), Ok(None));
        assert_eq!(parse_scope("  "), Ok(None));
        assert_eq!(
            parse_scope("content:read  media:write").map(|s| s.map(Scopes::to_strings)),
            Ok(Some(vec![
                "content:read".to_owned(),
                "media:write".to_owned()
            ]))
        );
        assert!(parse_scope("content:read nope").is_err());
    }

    #[test]
    fn redirect_hosts_show_custom_schemes() {
        let host = |u: &str| redirect_host(&Url::parse(u).unwrap());
        assert_eq!(host("https://app.example/cb"), "app.example");
        assert_eq!(host("http://127.0.0.1:5000/cb"), "127.0.0.1");
        assert_eq!(host("cursor://app.example/cb"), "cursor://app.example");
        assert_eq!(host("com.example.app:/oauth"), "com.example.app:/oauth");
    }

    #[test]
    fn errors_have_rfc_codes() {
        assert_eq!(
            OAuthError::InvalidGrant("x".into()).as_code(),
            "invalid_grant"
        );
        assert_eq!(OAuthError::InvalidGrant("x".into()).status(), 400);
        let limited = OAuthError::from(CoreError::RateLimited {
            retry_after_secs: 3,
        });
        assert_eq!(limited.status(), 429);
        let server = OAuthError::from(CoreError::Internal("boom"));
        assert_eq!(server.as_code(), "server_error");
        assert!(!server.description().contains("boom"));
    }
}
