//! `/users` and `/users/{id}/tokens`: accounts and their API tokens (07 §1–2; P11,
//! P14, P16).
//!
//! Handlers only translate: `UserService` and `AuthService` check every right
//! (`manage_users` by role, `users:read`/`users:write` by scope; oneself needs only
//! the scope) and every rule. No `If-Match` (P11): last write wins. The current user is
//! also `GET /auth/me`.
//!
//! * A user can read and edit their own account (`email`, `display_name`); everything
//!   else, and any other account, needs `manage_users`. So only the user themselves and
//!   managers ever see an e-mail address. Changing one's own e-mail to a different
//!   address needs `current_password` (the reset channel, M2b-3 P44), rate limited like
//!   `POST /auth/password` (`AuthService::update_user`).
//! * `GET /users?q&role&status` filters the list (M2b-3 P46).
//! * `POST /users/{id}/totp/disable` turns off another user's TOTP (a lost
//!   authenticator, M2b-3 P43); their sessions are kept. One's own TOTP goes through
//!   `/auth/totp/disable` with the password.
//! * Logins never change (R3); role and status never change on oneself; the last active
//!   admin cannot be demoted, disabled or deleted (`422`). A role change, disable or
//!   delete ends the user's sessions on every node at once; disable and delete also
//!   revoke their tokens (Review Focus 3).
//! * `DELETE /users/{id}?reassign=<id>` moves the user's content and revisions to
//!   another active user in the same transaction; without it a user with content is
//!   `422`.
//! * `POST /users/{id}/password` is an admin setting **another** user's password: it
//!   sets `must_reset` and ends that user's sessions. For one's own password the API
//!   has `POST /auth/password` only (it checks and rate limits the current password,
//!   R11/R14), so this endpoint refuses oneself with `400 bad-request`.
//! * Tokens: the bearer value is in the `201` response of `POST /users/{id}/tokens`
//!   and nowhere else; lists show metadata. A token gets only scopes the request's own
//!   credential holds, and is narrowed by its owner's role whenever it is used. At most
//!   50 per user, counted after locking the owner's row (`AuthService::create_token`).

use std::sync::Arc;

use axum::{
    Json,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use ferrite_core::{NewAccount, Scopes, UserPatch};
use ferrite_domain::{ApiTokenId, MAX_PAGE, Timestamp, UserId, UserQuery};
use ferrite_store::USER_CURSOR_TAG;
use uuid::Uuid;

use super::{
    ApiState, Authed, BASE,
    dto::Password,
    dto::users::{
        CreatedTokenDto, DeleteUserParams, PasswordSet, TokenDto, TokenInput, TokenList, UserDto,
        UserInput, UserList, UserListParams, UserPatchInput,
    },
    extract::{ApiJson, ApiPath, ApiQuery, DEFAULT_LIMIT, check_limit, created_at, invalid_cursor},
    list,
    problem::{Problem, ProblemDocument},
};

#[utoipa::path(
    get,
    path = "/users",
    tag = "users",
    operation_id = "list_users",
    security(("session" = []), ("bearer" = [])),
    params(UserListParams),
    responses(
        (status = 200, description = "One page of users, by login", body = UserList),
        (status = 400, description = "`validation` (`limit`, `cursor`) or `malformed-query` (also an unknown `role` or `status`)", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (`manage_users`, `users:read`) or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "`validation`: `q` longer than 100 characters (`errors[0].field` = `q`)", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn list_users(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiQuery(params): ApiQuery<UserListParams>,
) -> Result<Json<UserList>, Problem> {
    let limit = check_limit(params.limit, DEFAULT_LIMIT, MAX_PAGE)?;
    if let Some(c) = &params.cursor
        && ferrite_store::text_cursor(USER_CURSOR_TAG, c).is_err()
    {
        return Err(invalid_cursor());
    }
    let found = api
        .users
        .list(
            &who.actor,
            UserQuery {
                q: params.q,
                role: params.role.map(Into::into),
                status: params.status.map(Into::into),
                limit,
                cursor: params.cursor,
            },
        )
        .await?;
    let (items, next_cursor) = list::items(found, UserDto::from);
    Ok(Json(UserList { items, next_cursor }))
}

#[utoipa::path(
    post,
    path = "/users",
    tag = "users",
    operation_id = "create_user",
    security(("session" = []), ("bearer" = [])),
    request_body = UserInput,
    responses(
        (status = 201, description = "Created (`Location`)", body = UserDto),
        (status = 400, description = "`malformed-json` (also an unknown `role`)", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (`manage_users`, `users:write`), `csrf` or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "`validation` (`errors[0].field`: `login` (also taken), `email` (also taken), `display_name` or `password`)", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn create_user(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiJson(input): ApiJson<UserInput>,
) -> Result<Response, Problem> {
    let user = api
        .users
        .create(
            &who.actor,
            NewAccount {
                login: input.login,
                email: input.email,
                display_name: input.display_name,
                role: input.role.into(),
                password: input.password.map(Password::into_secret),
                must_reset: input.must_reset,
            },
        )
        .await?;
    let (status, headers) = created_at(&format!("{BASE}/users/{}", user.id));
    Ok((status, headers, Json(UserDto::from(user))).into_response())
}

#[utoipa::path(
    get,
    path = "/users/{id}",
    tag = "users",
    operation_id = "get_user",
    security(("session" = []), ("bearer" = [])),
    params(("id" = Uuid, Path)),
    responses(
        (status = 200, description = "The user", body = UserDto),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (another user: `manage_users`; `users:read`) or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn get_user(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<UserDto>, Problem> {
    let user = api.users.get(&who.actor, UserId::from_uuid(id)).await?;
    Ok(Json(user.into()))
}

#[utoipa::path(
    patch,
    path = "/users/{id}",
    tag = "users",
    operation_id = "update_user",
    security(("session" = []), ("bearer" = [])),
    params(("id" = Uuid, Path)),
    request_body = UserPatchInput,
    responses(
        (status = 200, description = "Saved (a role change or disable also ended the user's sessions; a disable revoked their tokens)", body = UserDto),
        (status = 400, description = "`malformed-json` (also `null` members)", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (another user, role or status: `manage_users`; `users:write`; a wrong `current_password`), `csrf` or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "`validation`: `errors[0].field` `login` (cannot change), `email` (also taken), `display_name` or `current_password` (missing for an own e-mail change); one's own role or status; the last active admin", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 429, description = "`rate-limited`: the own-e-mail password check (see `Retry-After`)", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn update_user(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(patch): ApiJson<UserPatchInput>,
) -> Result<Json<UserDto>, Problem> {
    let user = api
        .auth
        .update_user(
            &api.users,
            &who.actor,
            UserId::from_uuid(id),
            UserPatch {
                login: patch.login,
                email: patch.email,
                display_name: patch.display_name,
                role: patch.role.map(Into::into),
                status: patch.status.map(Into::into),
                current_password: patch.current_password.map(Password::into_secret),
            },
        )
        .await?;
    Ok(Json(user.into()))
}

#[utoipa::path(
    delete,
    path = "/users/{id}",
    tag = "users",
    operation_id = "delete_user",
    security(("session" = []), ("bearer" = [])),
    params(("id" = Uuid, Path), DeleteUserParams),
    responses(
        (status = 204, description = "Deleted, with its sessions and tokens; its content moved to `reassign`"),
        (status = 400, description = "`malformed-query` (`reassign` is not a UUID)", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (`manage_users`, `users:write`), `csrf` or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 409, description = "`conflict`: content was added concurrently; retry", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "`validation`: oneself, the last active admin, or (field `reassign`) content without `reassign`, or `reassign` the user itself or not an active user", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn delete_user(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath(id): ApiPath<Uuid>,
    ApiQuery(params): ApiQuery<DeleteUserParams>,
) -> Result<StatusCode, Problem> {
    api.users
        .delete(
            &who.actor,
            UserId::from_uuid(id),
            params.reassign.map(UserId::from_uuid),
        )
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/users/{id}/password",
    tag = "users",
    operation_id = "set_user_password",
    security(("session" = []), ("bearer" = [])),
    params(("id" = Uuid, Path)),
    request_body = PasswordSet,
    responses(
        (status = 204, description = "Set; the user must change it at the next login and every session of theirs ended"),
        (status = 400, description = "`malformed-json`, or `bad-request`: the request's own account (use `POST /auth/password`)", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (`manage_users`, `users:write`), `csrf` or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "`validation`: the password breaks the policy", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn set_user_password(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(req): ApiJson<PasswordSet>,
) -> Result<StatusCode, Problem> {
    let id = UserId::from_uuid(id);
    if id == who.actor.user {
        // A self change must check the current password under the rate limit (R11).
        return Err(Problem::bad_request(
            "change your own password with POST /api/v1/auth/password",
        ));
    }
    api.users
        .set_password(&who.actor, id, None, req.password.expose(), None)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/users/{id}/totp/disable",
    tag = "users",
    operation_id = "disable_user_totp",
    security(("session" = []), ("bearer" = [])),
    params(("id" = Uuid, Path)),
    responses(
        (status = 204, description = "TOTP is off for the user (a lost authenticator); their sessions and tokens are kept"),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (`manage_users`, `users:write`), `csrf` or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "`validation`: the request's own account (`errors[0].field` = `id`; use `POST /auth/totp/disable` with the password), or TOTP is not on", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn disable_user_totp(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<StatusCode, Problem> {
    api.users
        .disable_totp_for(&who.actor, UserId::from_uuid(id))
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    get,
    path = "/users/{id}/tokens",
    tag = "users",
    operation_id = "list_tokens",
    security(("session" = []), ("bearer" = [])),
    params(("id" = Uuid, Path)),
    responses(
        (status = 200, description = "The user's tokens, oldest first (metadata only)", body = TokenList),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (another user: `manage_users`; `users:read`) or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn list_tokens(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<TokenList>, Problem> {
    let id = UserId::from_uuid(id);
    // 404 for an unknown user (same rights as the token list).
    api.users.get(&who.actor, id).await?;
    let tokens = api.auth.list_tokens(&who.actor, id).await?;
    Ok(Json(TokenList {
        items: tokens.into_iter().map(TokenDto::from).collect(),
        next_cursor: None,
    }))
}

#[utoipa::path(
    post,
    path = "/users/{id}/tokens",
    tag = "users",
    operation_id = "create_token",
    security(("session" = []), ("bearer" = [])),
    params(("id" = Uuid, Path)),
    request_body = TokenInput,
    responses(
        (status = 201, description = "Created (`Location`); `token` is shown only here", body = CreatedTokenDto),
        (status = 400, description = "`malformed-json`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (another user: `manage_users`; `users:write`; a scope the request's credential lacks), `csrf` or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "`validation`: name, no or unknown scopes, `expires_at` not in the future, or 50 tokens already", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn create_token(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(input): ApiJson<TokenInput>,
) -> Result<Response, Problem> {
    let user = UserId::from_uuid(id);
    let scopes = Scopes::from_strs(&input.scopes)?;
    let (secret, token) = api
        .auth
        .create_token(
            &who.actor,
            user,
            &input.name,
            scopes,
            input.expires_at.map(Timestamp::from),
        )
        .await?;
    let (status, headers) = created_at(&format!("{BASE}/users/{user}/tokens/{}", token.id));
    let body = CreatedTokenDto::new(secret.expose().to_owned(), token.into());
    Ok((status, headers, Json(body)).into_response())
}

#[utoipa::path(
    delete,
    path = "/users/{id}/tokens/{tid}",
    tag = "users",
    operation_id = "revoke_token",
    security(("session" = []), ("bearer" = [])),
    params(("id" = Uuid, Path), ("tid" = Uuid, Path, description = "Token id")),
    responses(
        (status = 204, description = "Revoked; the token is refused from the next request on"),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (another user: `manage_users`; `users:write`), `csrf` or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`: the user holds no such token", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn revoke_token(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath((id, tid)): ApiPath<(Uuid, Uuid)>,
) -> Result<StatusCode, Problem> {
    api.auth
        .revoke_token(
            &who.actor,
            UserId::from_uuid(id),
            ApiTokenId::from_uuid(tid),
        )
        .await?;
    Ok(StatusCode::NO_CONTENT)
}
