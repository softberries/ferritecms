//! Authentication of API requests and the `/auth` endpoints (07 §2; P7, P8, P15).
//!
//! **Credentials.** [`Authed`] and [`AuthedAllowReset`] resolve a request:
//!
//! 1. An `Authorization` header must be exactly one `Bearer <token>`; anything else
//!    (another scheme, no token, two headers) is `401` with `Bearer
//!    error="invalid_request"`. The token goes to `AuthService::resolve_token` with the
//!    client address (unknown tokens count on the per-IP limit, `429` when exceeded);
//!    unknown, expired or of a disabled user is `401` with `Bearer
//!    error="invalid_token"`. **A request with `Authorization` never falls back to the
//!    cookie**, whether or not its token is valid.
//! 2. Else the session cookie (`__Host-ferrite_session` when `site.url` is `https`,
//!    else `ferrite_session`; only that name, P23) goes to
//!    `AuthService::resolve_session`.
//! 3. Neither: `401 unauthenticated` with `WWW-Authenticate: Bearer` (every 401 has a
//!    challenge, see [`problem`](super::problem)).
//!
//! **Cookie requests with a method other than GET and HEAD** (P8, Review Focus 1) must
//! come from the site's origin and carry the session's CSRF token, else `403 csrf`
//! before the handler runs (and before the body is read):
//!
//! * `Origin` (exactly one) must equal [`ApiState::origin`] (ASCII case-insensitive);
//!   `Origin: null` never does. Without `Origin`, the origin of the one `Referer`
//!   decides. Neither header: refused.
//! * `X-CSRF-Token` (exactly one) must pass `AuthService::check_csrf`.
//!
//! Bearer requests skip both: a browser never attaches a bearer token by itself.
//!
//! [`Authed`] then refuses session requests of accounts with `must_reset` (`403
//! password-reset-required`); only `GET /auth/me`, `POST /auth/logout` and `POST
//! /auth/password` use [`AuthedAllowReset`]. Bearer requests are not refused: a reset
//! keeps the account's API tokens working (R22, R26).
//!
//! **Login** (`POST /auth/login`, `POST /auth/login/totp`) has no credentials yet, so it
//! checks the origin alone ([`SameOrigin`], the same rule) before the body is read and
//! before any credential or rate-limit work: a cross-site form cannot log a browser
//! into an attacker's account (login CSRF). Every login failure is `401
//! invalid-credentials`. A successful login replaces the browser's previous session: a
//! session cookie sent with the login is deleted server-side.
//!
//! **Password reset** (`POST /auth/password-reset`, `POST /auth/password-reset/confirm`,
//! M2a-2 P19) has no credentials either and checks the origin the same way. The request
//! answers `202` with no body whether or not the input names an account (the mail goes
//! out from the node's queue in the background); without `[mail]` it is `503
//! mail-unavailable`. The mailed link points at `/admin/reset-password#token=<token>`,
//! which the admin SPA serves from M2b (F33: until then it is `404` and the token is
//! posted to the confirm endpoint directly). A confirm issues no session: the user
//! logs in with the new password (and TOTP, which a reset keeps).
//!
//! Nothing here logs a password, code, token, cookie or challenge.

use std::{net::IpAddr, sync::Arc};

use axum::{
    Json,
    extract::{FromRequestParts, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, header, request::Parts},
    response::{IntoResponse, Response},
};
use ferrite_core::{Authenticated, CoreError, IssuedSession, LoginAttempt, LoginOutcome, Via};
use ferrite_domain::Session;

use super::{
    ApiState,
    cookie::{clear_cookie, read_session_cookie, session_cookie},
    dto::auth::{
        LoginRequest, LoginResponse, MeDto, PasswordChange, PasswordChanged, PasswordResetConfirm,
        PasswordResetRequest, SessionDto, SessionList, SessionsRevoked, TotpConfirm, TotpDisable,
        TotpLoginRequest, TotpSetupDto,
    },
    extract::{ApiJson, ApiPath},
    problem::{Challenge, Problem, ProblemDocument},
    state::origin_of,
};
use crate::ClientIp;

/// The CSRF header (P8).
pub const CSRF_HEADER: &str = "x-csrf-token";
/// Where the mailed reset link points, under `site.url` (the admin SPA's page, M2b).
pub const RESET_PAGE: &str = "/admin/reset-password";

/// An authenticated request, refused when it uses a session of an account that must
/// change its password (`must_reset`); bearer requests pass.
#[derive(Debug, Clone)]
pub struct Authed(pub Authenticated);

/// An authenticated request, `must_reset` accounts included (only `me`, `logout`,
/// `password`).
#[derive(Debug, Clone)]
pub struct AuthedAllowReset(pub Authenticated);

/// The request comes from the site's origin (login endpoints).
#[derive(Debug, Clone, Copy)]
pub struct SameOrigin;

/// Optional credentials (the public comment submit, M2a-2b P12, F22):
///
/// * An `Authorization` header is resolved as for [`Authed`] (`401` when bad); no
///   origin check.
/// * Else the request must come from the site's origin (`403 csrf`, as [`SameOrigin`]).
///   A session cookie that resolves needs one valid `X-CSRF-Token` (`403 csrf`) and is
///   refused for an account that must change its password (`403
///   password-reset-required`); a cookie that does not resolve, and no cookie, are
///   [`MaybeAuthed::Anonymous`].
#[derive(Debug, Clone)]
pub enum MaybeAuthed {
    Anonymous,
    Authed(Box<Authenticated>),
}

/// The client address (`ClientIp`, absent in-process without connect info) and the
/// `User-Agent`.
#[derive(Debug, Clone, Default)]
pub struct ClientInfo {
    pub ip: Option<IpAddr>,
    pub user_agent: Option<String>,
}

impl FromRequestParts<Arc<ApiState>> for AuthedAllowReset {
    type Rejection = Problem;

    async fn from_request_parts(parts: &mut Parts, api: &Arc<ApiState>) -> Result<Self, Problem> {
        authenticate(parts, api).await.map(Self)
    }
}

impl FromRequestParts<Arc<ApiState>> for Authed {
    type Rejection = Problem;

    async fn from_request_parts(parts: &mut Parts, api: &Arc<ApiState>) -> Result<Self, Problem> {
        let who = authenticate(parts, api).await?;
        // Only sessions are held to the password change; API tokens are independent
        // credentials that a reset keeps (R22, R26).
        if who.user.must_reset && matches!(who.via, Via::Session(_)) {
            return Err(Problem::password_reset_required());
        }
        Ok(Self(who))
    }
}

impl FromRequestParts<Arc<ApiState>> for MaybeAuthed {
    type Rejection = Problem;

    async fn from_request_parts(parts: &mut Parts, api: &Arc<ApiState>) -> Result<Self, Problem> {
        if let Some(who) = bearer(parts, api).await {
            return who.map(|w| Self::Authed(Box::new(w)));
        }
        if !same_origin(&parts.headers, &api.origin) {
            return Err(Problem::csrf(
                "the request does not come from the site's origin",
            ));
        }
        let Some(token) = read_session_cookie(&parts.headers, api.cookie_secure) else {
            return Ok(Self::Anonymous);
        };
        let Some(who) = api.auth.resolve_session(token).await? else {
            return Ok(Self::Anonymous);
        };
        check_csrf(parts, api, &who)?;
        if who.user.must_reset {
            return Err(Problem::password_reset_required());
        }
        Ok(Self::Authed(Box::new(who)))
    }
}

impl FromRequestParts<Arc<ApiState>> for SameOrigin {
    type Rejection = Problem;

    async fn from_request_parts(parts: &mut Parts, api: &Arc<ApiState>) -> Result<Self, Problem> {
        if same_origin(&parts.headers, &api.origin) {
            Ok(Self)
        } else {
            Err(Problem::csrf(
                "the request does not come from the site's origin",
            ))
        }
    }
}

impl<S: Send + Sync> FromRequestParts<S> for ClientInfo {
    type Rejection = Problem;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Problem> {
        Ok(client_info(parts))
    }
}

fn client_info(parts: &Parts) -> ClientInfo {
    ClientInfo {
        ip: parts.extensions.get::<ClientIp>().map(|c| c.0),
        user_agent: parts
            .headers
            .get(header::USER_AGENT)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned),
    }
}

/// Resolves the request's credentials; see the module docs.
async fn authenticate(parts: &Parts, api: &ApiState) -> Result<Authenticated, Problem> {
    if let Some(who) = bearer(parts, api).await {
        return who;
    }
    let token =
        read_session_cookie(&parts.headers, api.cookie_secure).ok_or(CoreError::Unauthenticated)?;
    let who = api
        .auth
        .resolve_session(token)
        .await?
        .ok_or(CoreError::Unauthenticated)?;
    if !matches!(parts.method, Method::GET | Method::HEAD) {
        if !same_origin(&parts.headers, &api.origin) {
            return Err(Problem::csrf(
                "the request does not come from the site's origin",
            ));
        }
        check_csrf(parts, api, &who)?;
    }
    Ok(who)
}

/// The request's bearer credential, `None` without an `Authorization` header (see the
/// module docs: a request with one never falls back to the cookie).
async fn bearer(parts: &Parts, api: &ApiState) -> Option<Result<Authenticated, Problem>> {
    let mut authorization = parts.headers.get_all(header::AUTHORIZATION).iter();
    let value = authorization.next()?;
    let unauthenticated =
        |challenge| Problem::from(CoreError::Unauthenticated).with_challenge(challenge);
    if authorization.next().is_some() {
        return Some(Err(unauthenticated(Challenge::InvalidRequest)));
    }
    let Some(token) = bearer_token(value) else {
        return Some(Err(unauthenticated(Challenge::InvalidRequest)));
    };
    let ip = client_info(parts).ip;
    Some(match api.auth.resolve_token(token, ip).await {
        Ok(Some(who)) => Ok(who),
        Ok(None) => Err(unauthenticated(Challenge::InvalidToken)),
        Err(e) => Err(e.into()),
    })
}

/// `Ok` unless `who` is a session and the request lacks exactly one valid
/// `X-CSRF-Token`.
fn check_csrf(parts: &Parts, api: &ApiState, who: &Authenticated) -> Result<(), Problem> {
    let Via::Session(session) = &who.via else {
        return Ok(());
    };
    let mut sent = parts.headers.get_all(CSRF_HEADER).iter();
    let valid = match (sent.next(), sent.next()) {
        (Some(v), None) => v.to_str().is_ok_and(|v| api.auth.check_csrf(session, v)),
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(Problem::csrf("missing or invalid X-CSRF-Token"))
    }
}

/// The token of `Authorization: Bearer <token>` (scheme case-insensitive).
fn bearer_token(value: &HeaderValue) -> Option<&str> {
    let (scheme, token) = value.to_str().ok()?.trim().split_once(' ')?;
    let token = token.trim();
    (scheme.eq_ignore_ascii_case("bearer")
        && !token.is_empty()
        && !token.contains(char::is_whitespace))
    .then_some(token)
}

/// Whether the request's `Origin` (else its `Referer`'s origin) is `origin`.
pub(crate) fn same_origin(headers: &HeaderMap, origin: &str) -> bool {
    let one = |name| {
        let mut values = headers.get_all(name).iter();
        match (values.next(), values.next()) {
            (Some(v), None) => Some(v.to_str().ok()),
            (None, _) => None,
            // Two headers: never trusted.
            (Some(_), Some(_)) => Some(None),
        }
    };
    match one(header::ORIGIN) {
        Some(sent) => sent.is_some_and(|o| o.trim().eq_ignore_ascii_case(origin)),
        None => one(header::REFERER)
            .flatten()
            .and_then(|r| origin_of(r.trim()).ok())
            .is_some_and(|(o, _)| o == origin),
    }
}

/// A login failure as the login endpoints report it.
fn login_problem(e: CoreError) -> Problem {
    match e {
        CoreError::Unauthenticated => Problem::invalid_credentials(),
        other => Problem::from(other),
    }
}

/// `200` with the new session's cookie and [`LoginResponse::Session`]; the browser's
/// previous session (its cookie in `headers`) is deleted.
async fn logged_in(api: &ApiState, headers: &HeaderMap, issued: IssuedSession) -> Response {
    if let Some(old) = read_session_cookie(headers, api.cookie_secure)
        && let Err(e) = api.auth.logout(old).await
    {
        tracing::warn!(error = %e, "previous session not deleted at login");
    }
    let cookie = session_cookie(issued.token.expose(), api.cookie_secure);
    let who = Authenticated {
        actor: ferrite_core::Actor::new(issued.user.id, issued.user.role),
        user: issued.user,
        via: Via::Session(issued.session),
        oauth: None,
    };
    let user = MeDto::of(&api.auth, &who, api.mail.is_some());
    let csrf_token = user.csrf_token.clone().unwrap_or_default();
    (
        [(header::SET_COOKIE, cookie)],
        Json(LoginResponse::Session { user, csrf_token }),
    )
        .into_response()
}

#[utoipa::path(
    post,
    path = "/auth/login",
    tag = "auth",
    operation_id = "login",
    request_body = LoginRequest,
    responses(
        (status = 200, description = "Logged in (sets the session cookie), or a TOTP code is needed", body = LoginResponse),
        (status = 401, description = "`invalid-credentials`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`csrf`: not from the site's origin", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 429, description = "`rate-limited` (see `Retry-After`)", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn login(
    State(api): State<Arc<ApiState>>,
    _origin: SameOrigin,
    client: ClientInfo,
    headers: HeaderMap,
    ApiJson(req): ApiJson<LoginRequest>,
) -> Result<Response, Problem> {
    let outcome = api
        .auth
        .login(LoginAttempt {
            login: &req.login,
            password: req.password.expose(),
            ip: client.ip,
            user_agent: client.user_agent.as_deref(),
        })
        .await
        .map_err(login_problem)?;
    Ok(match outcome {
        LoginOutcome::Session(issued) => logged_in(&api, &headers, *issued).await,
        LoginOutcome::TotpRequired { challenge } => {
            Json(LoginResponse::TotpRequired { challenge }).into_response()
        }
    })
}

#[utoipa::path(
    post,
    path = "/auth/login/totp",
    tag = "auth",
    operation_id = "login_totp",
    request_body = TotpLoginRequest,
    responses(
        (status = 200, description = "Logged in (sets the session cookie)", body = LoginResponse),
        (status = 401, description = "`invalid-credentials`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`csrf`: not from the site's origin", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 429, description = "`rate-limited` (see `Retry-After`)", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn login_totp(
    State(api): State<Arc<ApiState>>,
    _origin: SameOrigin,
    client: ClientInfo,
    headers: HeaderMap,
    ApiJson(req): ApiJson<TotpLoginRequest>,
) -> Result<Response, Problem> {
    let issued = api
        .auth
        .login_totp(
            &req.challenge,
            &req.code,
            client.ip,
            client.user_agent.as_deref(),
        )
        .await
        .map_err(login_problem)?;
    Ok(logged_in(&api, &headers, issued).await)
}

#[utoipa::path(
    post,
    path = "/auth/logout",
    tag = "auth",
    operation_id = "logout",
    security(("session" = [])),
    responses(
        (status = 204, description = "Session deleted, cookie cleared"),
        (status = 400, description = "`bad-request`: the request used a bearer token", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`csrf`", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn logout(
    State(api): State<Arc<ApiState>>,
    AuthedAllowReset(who): AuthedAllowReset,
    headers: HeaderMap,
) -> Result<Response, Problem> {
    if !matches!(who.via, Via::Session(_)) {
        return Err(Problem::bad_request(
            "logout ends a cookie session; revoke an API token instead",
        ));
    }
    // A session request has no `Authorization` header, so this is the cookie it used.
    if let Some(token) = read_session_cookie(&headers, api.cookie_secure) {
        api.auth.logout(token).await?;
    }
    Ok((
        StatusCode::NO_CONTENT,
        [(header::SET_COOKIE, clear_cookie(api.cookie_secure))],
    )
        .into_response())
}

#[utoipa::path(
    get,
    path = "/auth/me",
    tag = "auth",
    operation_id = "get_me",
    security(("session" = []), ("bearer" = [])),
    responses(
        (status = 200, description = "The authenticated user", body = MeDto),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn me(
    State(api): State<Arc<ApiState>>,
    AuthedAllowReset(who): AuthedAllowReset,
) -> Json<MeDto> {
    Json(MeDto::of(&api.auth, &who, api.mail.is_some()))
}

#[utoipa::path(
    post,
    path = "/auth/password",
    tag = "auth",
    operation_id = "change_password",
    security(("session" = []), ("bearer" = [])),
    request_body = PasswordChange,
    responses(
        (status = 200, description = "Changed; every session of the user is deleted. A session request gets a new session cookie and its `X-CSRF-Token` in `csrf_token` (as the login response); a bearer request gets `csrf_token: null`", body = PasswordChanged),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (wrong current password) or `csrf`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "`validation`: the new password breaks the policy", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 429, description = "`rate-limited` (see `Retry-After`)", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn change_password(
    State(api): State<Arc<ApiState>>,
    AuthedAllowReset(who): AuthedAllowReset,
    client: ClientInfo,
    ApiJson(req): ApiJson<PasswordChange>,
) -> Result<Response, Problem> {
    let issued = api
        .auth
        .change_password(
            &api.users,
            &who,
            req.current.expose(),
            req.new.expose(),
            client.ip,
            client.user_agent.as_deref(),
        )
        .await?;
    Ok(match issued {
        Some(issued) => {
            let csrf_token = Some(api.auth.csrf_token(&issued.session));
            (
                [(
                    header::SET_COOKIE,
                    session_cookie(issued.token.expose(), api.cookie_secure),
                )],
                Json(PasswordChanged { csrf_token }),
            )
                .into_response()
        }
        None => Json(PasswordChanged { csrf_token: None }).into_response(),
    })
}

#[utoipa::path(
    post,
    path = "/auth/password-reset",
    tag = "auth",
    operation_id = "request_password_reset",
    request_body = PasswordResetRequest,
    responses(
        (status = 202, description = "Accepted, whether or not `login` names an account: an active account gets a mail with a link to `/admin/reset-password#token=<token>` (valid 1 hour, once). No body"),
        (status = 403, description = "`csrf`: not from the site's origin", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 429, description = "`rate-limited`: 5 requests a minute per client, 3 an hour per login or e-mail (see `Retry-After`)", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 503, description = "`mail-unavailable`: the site has no `[mail]` section", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn request_password_reset(
    State(api): State<Arc<ApiState>>,
    _origin: SameOrigin,
    client: ClientInfo,
    ApiJson(req): ApiJson<PasswordResetRequest>,
) -> Result<StatusCode, Problem> {
    let mail = api.mail.as_ref().ok_or_else(Problem::mail_unavailable)?;
    let reset_base = format!("{}{RESET_PAGE}", api.site_url);
    api.auth
        .request_password_reset(&mail.queue, &req.login, client.ip, &reset_base)
        .await?;
    Ok(StatusCode::ACCEPTED)
}

#[utoipa::path(
    post,
    path = "/auth/password-reset/confirm",
    tag = "auth",
    operation_id = "confirm_password_reset",
    request_body = PasswordResetConfirm,
    responses(
        (status = 204, description = "Password set: every session of the account ended and `must_reset` cleared; API tokens and TOTP are kept. No session is issued"),
        (status = 403, description = "`csrf`: not from the site's origin", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "`validation`: `errors[0].field` is `password` (breaks the policy) or `token` (invalid, expired or used link)", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn confirm_password_reset(
    State(api): State<Arc<ApiState>>,
    _origin: SameOrigin,
    ApiJson(req): ApiJson<PasswordResetConfirm>,
) -> Result<StatusCode, Problem> {
    api.auth
        .confirm_password_reset(&api.users, &req.token, req.password.expose())
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/auth/totp/setup",
    tag = "auth",
    operation_id = "totp_setup",
    security(("session" = []), ("bearer" = [])),
    responses(
        (status = 200, description = "Enrollment started (nothing stored yet)", body = TotpSetupDto),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`csrf`, `forbidden` or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "`validation`: TOTP is already on", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn totp_setup(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
) -> Result<Json<TotpSetupDto>, Problem> {
    Ok(Json(api.auth.totp_setup(&who.actor).await?.into()))
}

#[utoipa::path(
    post,
    path = "/auth/totp/confirm",
    tag = "auth",
    operation_id = "totp_confirm",
    security(("session" = []), ("bearer" = [])),
    request_body = TotpConfirm,
    responses(
        (status = 204, description = "TOTP is on"),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (wrong current password), `csrf` or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "`validation`: bad or expired enrollment (`errors[0].field` = `enrollment`), wrong or replayed code (`code`), TOTP already on (no field)", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 429, description = "`rate-limited` (see `Retry-After`)", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn totp_confirm(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiJson(req): ApiJson<TotpConfirm>,
) -> Result<StatusCode, Problem> {
    // The caller's session survives; the user's other sessions end (R26).
    let keep = match &who.via {
        Via::Session(s) => Some(s.id_hash.as_str()),
        Via::Token(_) => None,
    };
    api.auth
        .totp_confirm(
            &who.actor,
            keep,
            &req.enrollment,
            &req.code,
            req.password.expose(),
        )
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/auth/totp/disable",
    tag = "auth",
    operation_id = "totp_disable",
    security(("session" = []), ("bearer" = [])),
    request_body = TotpDisable,
    responses(
        (status = 204, description = "TOTP is off"),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (wrong password), `csrf` or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "`validation`: TOTP is not on", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 429, description = "`rate-limited` (see `Retry-After`)", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn totp_disable(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiJson(req): ApiJson<TotpDisable>,
) -> Result<StatusCode, Problem> {
    api.auth
        .totp_disable(&who.actor, req.password.expose())
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// The cookie session of a session-only route (`/auth/sessions`); a bearer request is
/// `400 bad-request`, as for `POST /auth/logout`.
fn session_of(who: &Authenticated) -> Result<&Session, Problem> {
    match &who.via {
        Via::Session(session) => Ok(session),
        Via::Token(_) => Err(Problem::bad_request(
            "sessions are managed with a cookie session; revoke an API token instead",
        )),
    }
}

#[utoipa::path(
    get,
    path = "/auth/sessions",
    tag = "auth",
    operation_id = "list_sessions",
    security(("session" = [])),
    responses(
        (status = 200, description = "The user's live sessions, newest first; `current` is this one", body = SessionList),
        (status = 400, description = "`bad-request`: the request used a bearer token", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn list_sessions(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
) -> Result<Json<SessionList>, Problem> {
    let current = session_of(&who)?;
    let items = api.auth.list_sessions(&who.actor, current).await?;
    Ok(Json(SessionList {
        items: items.into_iter().map(SessionDto::from).collect(),
    }))
}

#[utoipa::path(
    delete,
    path = "/auth/sessions/{id}",
    tag = "auth",
    operation_id = "revoke_session",
    security(("session" = [])),
    params(("id" = String, Path, description = "A session's `id` from `GET /auth/sessions`")),
    responses(
        (status = 204, description = "The session ended (on every node at its next request); audited `session.revoke`"),
        (status = 400, description = "`bad-request`: the request used a bearer token", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`csrf` or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`: no session of the user has this id", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "`validation`: the current session (use `POST /auth/logout`)", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn revoke_session(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath(id): ApiPath<String>,
) -> Result<StatusCode, Problem> {
    let current = session_of(&who)?;
    api.auth.revoke_session(&who.actor, current, &id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/auth/sessions/revoke-others",
    tag = "auth",
    operation_id = "revoke_other_sessions",
    security(("session" = [])),
    responses(
        (status = 200, description = "Every other session of the user ended; this one is kept. Audited `session.revoke` with `details.count` (none ended: no audit row)", body = SessionsRevoked),
        (status = 400, description = "`bad-request`: the request used a bearer token", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`csrf` or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn revoke_other_sessions(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
) -> Result<Json<SessionsRevoked>, Problem> {
    let current = session_of(&who)?;
    let revoked = api.auth.revoke_other_sessions(&who.actor, current).await?;
    Ok(Json(SessionsRevoked { revoked }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(header::HeaderName, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (name, value) in pairs {
            h.append(name, HeaderValue::from_str(value).unwrap());
        }
        h
    }

    #[test]
    fn origin_rules() {
        const SITE: &str = "https://example.com";
        let ok = |pairs: &[(header::HeaderName, &str)]| same_origin(&headers(pairs), SITE);
        assert!(ok(&[(header::ORIGIN, "https://example.com")]));
        assert!(ok(&[(header::ORIGIN, "HTTPS://Example.com")]));
        assert!(!ok(&[(header::ORIGIN, "https://example.com:8443")]));
        assert!(!ok(&[(header::ORIGIN, "http://example.com")]));
        assert!(!ok(&[(header::ORIGIN, "https://example.com.evil")]));
        assert!(!ok(&[(header::ORIGIN, "null")]));
        assert!(!ok(&[
            (header::ORIGIN, "https://example.com"),
            (header::ORIGIN, "https://example.com"),
        ]));
        // Origin wins over Referer.
        assert!(!ok(&[
            (header::ORIGIN, "https://evil.example"),
            (header::REFERER, "https://example.com/admin"),
        ]));
        assert!(ok(&[(header::REFERER, "https://example.com/admin/x?y=1")]));
        assert!(ok(&[(header::REFERER, "https://example.com:443/")]));
        assert!(!ok(&[(
            header::REFERER,
            "https://evil.example/https://example.com"
        )]));
        assert!(!ok(&[(header::REFERER, "/admin")]));
        assert!(!ok(&[
            (header::REFERER, "https://example.com/"),
            (header::REFERER, "https://example.com/"),
        ]));
        assert!(!ok(&[]));
    }

    #[test]
    fn bearer_parsing() {
        let token = |v: &str| bearer_token(&HeaderValue::from_str(v).unwrap()).map(str::to_owned);
        assert_eq!(token("Bearer fer_abc").as_deref(), Some("fer_abc"));
        assert_eq!(token("bearer  fer_abc ").as_deref(), Some("fer_abc"));
        assert_eq!(token("Bearer"), None);
        assert_eq!(token("Bearer "), None);
        assert_eq!(token("Basic YTpi"), None);
        assert_eq!(token("Bearer a b"), None);
        assert_eq!(token("Bearerfer_abc"), None);
    }
}
