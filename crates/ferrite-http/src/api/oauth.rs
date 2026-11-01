//! The consent API under `/api/v1/oauth` (design 12 §3.2, §6, §8): what the admin's
//! consent screen (`/admin/connect?request=<id>`) and Connected apps use.
//!
//! * `GET /oauth/requests/{id}`, `POST …/approve`, `POST …/deny`: a **signed-in
//!   session** only (an API token is `403 forbidden`: connecting an application is a
//!   person's decision), with the usual CSRF checks on the POSTs; `404` while
//!   `mcp.enabled` is off. Approve and deny take the request (single use) and answer
//!   where the browser goes next. The checks (`users:read` to see, `users:write` to
//!   decide, the role's scopes) are core's.
//! * `GET /oauth/grants[?user=]`, `DELETE /oauth/grants/{id}`: the user's connected apps
//!   (another user's with `manage_users`); sessions and API tokens alike, and while
//!   `mcp.enabled` is off too, so an app can always be disconnected.
//!
//! Every route takes a body of at most [`MAX_OAUTH_BODY`].

use std::sync::Arc;

use axum::{Json, extract::State, http::StatusCode};
use ferrite_core::{Authenticated, Via};
use ferrite_domain::{OAuthGrantId, OAuthRequestId, UserId};
use uuid::Uuid;

use super::{
    ApiState, Authed,
    dto::oauth::{ApproveInput, ConsentDto, GrantList, GrantParams, RedirectDto},
    extract::{ApiJson, ApiPath, ApiQuery},
    problem::{Problem, ProblemDocument},
};

/// The body limit of the consent API's routes (and the `/oauth/*` endpoints).
pub const MAX_OAUTH_BODY: usize = 64 * 1024;

/// `who` when it is a cookie session; an API token is `403 forbidden`.
fn session_only(who: &Authenticated) -> Result<(), Problem> {
    match who.via {
        Via::Session(_) => Ok(()),
        Via::Token(_) => Err(Problem::new(
            StatusCode::FORBIDDEN,
            "forbidden",
            "Forbidden",
            "connecting an application needs a signed-in session, not an API token",
        )),
    }
}

/// `404 not-found` while `mcp.enabled` is off.
async fn enabled(api: &ApiState) -> Result<(), Problem> {
    if api.mcp_enabled().await? {
        Ok(())
    } else {
        Err(Problem::not_found())
    }
}

#[utoipa::path(
    get,
    path = "/oauth/requests/{id}",
    tag = "oauth",
    operation_id = "get_oauth_request",
    security(("session" = [])),
    params(("id" = Uuid, Path, description = "The `request` query parameter of `/admin/connect`")),
    responses(
        (status = 200, description = "The pending authorization request", body = ConsentDto),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (an API token, or `users:read`) or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`: unknown, expired or already decided; or `mcp.enabled` is off", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn get_oauth_request(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<ConsentDto>, Problem> {
    session_only(&who)?;
    enabled(&api).await?;
    let view = api
        .oauth
        .describe(&who.actor, OAuthRequestId::from_uuid(id))
        .await?;
    Ok(Json(ConsentDto::new(id, view)))
}

#[utoipa::path(
    post,
    path = "/oauth/requests/{id}/approve",
    tag = "oauth",
    operation_id = "approve_oauth_request",
    security(("session" = [])),
    params(("id" = Uuid, Path)),
    request_body = ApproveInput,
    responses(
        (status = 200, description = "Approved: the request is used up and a 60-second code was made; audited `oauth.connect`. Send the browser to `redirect_to`", body = RedirectDto),
        (status = 400, description = "`malformed-json`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (an API token, `users:write`, or a scope that is not grantable; the request stays), `csrf` or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`: unknown, expired or already decided; or `mcp.enabled` is off", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "`validation`: no scope chosen, or an unknown one (`errors[0].field` = `scopes`)", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn approve_oauth_request(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(input): ApiJson<ApproveInput>,
) -> Result<Json<RedirectDto>, Problem> {
    session_only(&who)?;
    enabled(&api).await?;
    let to = api
        .oauth
        .approve(&who.actor, OAuthRequestId::from_uuid(id), input.scopes)
        .await?;
    Ok(Json(RedirectDto {
        redirect_to: to.into(),
    }))
}

#[utoipa::path(
    post,
    path = "/oauth/requests/{id}/deny",
    tag = "oauth",
    operation_id = "deny_oauth_request",
    security(("session" = [])),
    params(("id" = Uuid, Path)),
    responses(
        (status = 200, description = "Denied: the request is used up. Send the browser to `redirect_to` (`error=access_denied`)", body = RedirectDto),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (an API token, `users:write`), `csrf` or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`: unknown, expired or already decided; or `mcp.enabled` is off", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn deny_oauth_request(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<RedirectDto>, Problem> {
    session_only(&who)?;
    enabled(&api).await?;
    let to = api
        .oauth
        .deny(&who.actor, OAuthRequestId::from_uuid(id))
        .await?;
    Ok(Json(RedirectDto {
        redirect_to: to.into(),
    }))
}

#[utoipa::path(
    get,
    path = "/oauth/grants",
    tag = "oauth",
    operation_id = "list_oauth_grants",
    security(("session" = []), ("bearer" = [])),
    params(GrantParams),
    responses(
        (status = 200, description = "The user's connected apps (unexpired grants), newest first", body = GrantList),
        (status = 400, description = "`malformed-query`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (another user: `manage_users`; `users:read`) or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn list_oauth_grants(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiQuery(params): ApiQuery<GrantParams>,
) -> Result<Json<GrantList>, Problem> {
    let grants = api
        .oauth
        .grants(&who.actor, params.user.map(UserId::from_uuid))
        .await?;
    Ok(Json(GrantList {
        items: grants.into_iter().map(Into::into).collect(),
    }))
}

#[utoipa::path(
    delete,
    path = "/oauth/grants/{id}",
    tag = "oauth",
    operation_id = "delete_oauth_grant",
    security(("session" = []), ("bearer" = [])),
    params(("id" = Uuid, Path, description = "A grant's `id` from `GET /oauth/grants`")),
    responses(
        (status = 204, description = "Disconnected: the grant and its access tokens are gone; audited `oauth.disconnect`"),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (`users:write`; another user's: `manage_users`), `csrf` or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`: no such grant, or another user's without `manage_users`", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn delete_oauth_grant(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<StatusCode, Problem> {
    api.oauth
        .disconnect(&who.actor, OAuthGrantId::from_uuid(id))
        .await?;
    Ok(StatusCode::NO_CONTENT)
}
