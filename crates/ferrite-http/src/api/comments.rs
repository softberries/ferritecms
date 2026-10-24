//! Comments (07 §1; M2a-2b P11–P14, R2, F18, F22).
//!
//! Handlers translate; `CommentService` checks the rules.
//!
//! * `POST /content/{type}/{id}/comments` is the public submit (the theme form,
//!   `POST /_comment`, is the other path to the same service, P12). It takes
//!   [`MaybeAuthed`]: a bearer token (needs `comments:write`), else the site's origin
//!   (`403 csrf`) with an optional session (which then needs `X-CSRF-Token`). The
//!   request body is at most [`MAX_COMMENT_REQUEST`]. Without a client address the
//!   per-IP limit cannot apply: `503 busy`. `201` when the comment is approved, `202`
//!   when it is held, and also `202`, in the same shape under an id that is never
//!   stored, when the honeypot member `website` is filled (F18). No `Location`: a
//!   comment has no single-resource endpoint.
//! * `GET /comments`, `PATCH /comments/{id}` and `DELETE /comments/{id}` are the
//!   moderation endpoints (`moderate_comments`; reads `comments:read`, writes
//!   `comments:write`). `DELETE` takes only spam or trashed comments. No `If-Match`
//!   (P11): last write wins. Moderation is not audited (P14).

use std::sync::Arc;

use axum::{
    Json,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use ferrite_core::comments::{CommentInput, Commenter, Submitted};
use ferrite_domain::{CommentId, CommentQuery, CommentStatus, ContentId, MAX_PAGE};
use serde::Deserialize;
use utoipa::IntoParams;
use uuid::Uuid;

use super::{
    ApiState, Authed, ClientInfo, MaybeAuthed,
    dto::comments::{CommentDto, CommentList, CommentModeration, CommentPublicDto, CommentSubmit},
    extract::{
        ApiJson, ApiPath, ApiQuery, DEFAULT_LIMIT, check_limit, invalid_cursor, invalid_param,
    },
    problem::{Problem, ProblemDocument},
};

/// The body limit of the public submit (as the theme form's, P13).
pub const MAX_COMMENT_REQUEST: usize = 64 * 1024;

/// The query of `GET /comments`.
#[derive(Debug, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct CommentListParams {
    /// `pending`, `approved`, `spam` or `trash`; all when absent.
    pub status: Option<String>,
    /// Only comments on this content item.
    pub item: Option<Uuid>,
    /// 1..=200; default 50.
    pub limit: Option<u32>,
    /// `next_cursor` of the previous page.
    pub cursor: Option<String>,
}

#[utoipa::path(
    get,
    path = "/comments",
    tag = "comments",
    operation_id = "list_comments",
    security(("session" = []), ("bearer" = [])),
    params(CommentListParams),
    responses(
        (status = 200, description = "One page of comments, newest first", body = CommentList),
        (status = 400, description = "`validation` (`status`, `limit`, `cursor`) or `malformed-query`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (`moderate_comments`, `comments:read`) or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn list_comments(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiQuery(params): ApiQuery<CommentListParams>,
) -> Result<Json<CommentList>, Problem> {
    let limit = check_limit(params.limit, DEFAULT_LIMIT, MAX_PAGE)?;
    let status = params
        .status
        .as_deref()
        .map(|s| {
            s.parse::<CommentStatus>().map_err(|_| {
                invalid_param(
                    "status",
                    format!("unknown comment status {s:?}"),
                    "pending, approved, spam or trash",
                )
            })
        })
        .transpose()?;
    if let Some(c) = &params.cursor
        && ferrite_store::comment_cursor(c).is_err()
    {
        return Err(invalid_cursor());
    }
    let page = api
        .comments
        .list(
            &who.actor,
            CommentQuery {
                status,
                item: params.item.map(ContentId::from_uuid),
                limit,
                cursor: params.cursor,
            },
        )
        .await?;
    Ok(Json(CommentList {
        items: page.items.into_iter().map(CommentDto::from).collect(),
        next_cursor: page.next_cursor,
    }))
}

#[utoipa::path(
    patch,
    path = "/comments/{id}",
    tag = "comments",
    operation_id = "moderate_comment",
    security(("session" = []), ("bearer" = [])),
    params(("id" = Uuid, Path)),
    request_body = CommentModeration,
    responses(
        (status = 200, description = "The comment with its new status; its post's page is refreshed", body = CommentDto),
        (status = 400, description = "`malformed-json`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (`moderate_comments`, `comments:write`), `csrf` or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "`validation`: an unknown `status`", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn moderate_comment(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(req): ApiJson<CommentModeration>,
) -> Result<Json<CommentDto>, Problem> {
    let status = req.status.parse::<CommentStatus>().map_err(|_| {
        Problem::validation(format!("unknown comment status {:?}", req.status))
            .with_error(Some("status"), "must be pending, approved, spam or trash")
    })?;
    let comment = api
        .comments
        .moderate(&who.actor, CommentId::from_uuid(id), status)
        .await?;
    Ok(Json(CommentDto::from(comment)))
}

#[utoipa::path(
    delete,
    path = "/comments/{id}",
    tag = "comments",
    operation_id = "delete_comment",
    security(("session" = []), ("bearer" = [])),
    params(("id" = Uuid, Path)),
    responses(
        (status = 204, description = "Deleted with its replies; its post's page is refreshed"),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (`moderate_comments`, `comments:write`), `csrf` or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "`validation` (`status`): only spam or trashed comments can be deleted", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn delete_comment(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<StatusCode, Problem> {
    api.comments
        .delete(&who.actor, CommentId::from_uuid(id))
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/content/{type}/{id}/comments",
    tag = "comments",
    operation_id = "submit_comment",
    security((), ("session" = []), ("bearer" = [])),
    params(("type" = String, Path), ("id" = Uuid, Path)),
    request_body = CommentSubmit,
    responses(
        (status = 201, description = "Approved and shown on the post", body = CommentPublicDto),
        (status = 202, description = "Held for moderation (also the answer when the honeypot `website` is filled; nothing is stored then)", body = CommentPublicDto),
        (status = 400, description = "`malformed-json`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 401, description = "`unauthenticated`: a bad bearer token", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`csrf` (not from the site's origin without a token, or a session without `X-CSRF-Token`), `forbidden` (a token without `comments:write`) or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`: no such public post, or comments are off", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 413, description = "`too-large`: the body is over 64 KiB", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "`validation` (`errors[0].field`: `body`, `author_name`, `author_email`, `author_url` or `parent_id`)", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 429, description = "`rate-limited`: 3 a minute or 20 an hour per client address (`Retry-After`)", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 503, description = "`busy`: the client address is unknown", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn submit_comment(
    State(api): State<Arc<ApiState>>,
    who: MaybeAuthed,
    client: ClientInfo,
    ApiPath((kind, id)): ApiPath<(String, Uuid)>,
    ApiJson(req): ApiJson<CommentSubmit>,
) -> Result<Response, Problem> {
    let Some(ip) = client.ip else {
        return Err(Problem::busy());
    };
    let (commenter, account_name) = match &who {
        MaybeAuthed::Anonymous => (Commenter::Anonymous, None),
        MaybeAuthed::Authed(a) => (Commenter::User(a.actor), Some(a.user.display_name.clone())),
    };
    let parent = req.parent_id.map(CommentId::from_uuid);
    let input = CommentInput {
        item: ContentId::from_uuid(id),
        kind,
        parent,
        author_name: req.author_name.unwrap_or_default(),
        author_email: req.author_email.unwrap_or_default(),
        author_url: req.author_url.unwrap_or_default(),
        body: req.body,
        honeypot: req.website.unwrap_or_default(),
    };
    let echo_name = account_name.unwrap_or_else(|| input.author_name.trim().to_owned());
    let echo_url = Some(input.author_url.trim().to_owned()).filter(|u| !u.is_empty());
    let echo_body = input.body.clone();
    match api.comments.submit(input, commenter, ip).await? {
        Submitted::Accepted(c) => {
            let status = if c.status == CommentStatus::Approved {
                StatusCode::CREATED
            } else {
                StatusCode::ACCEPTED
            };
            Ok((status, Json(CommentPublicDto::of(&c))).into_response())
        }
        Submitted::Discarded => Ok((
            StatusCode::ACCEPTED,
            Json(CommentPublicDto::discarded(
                parent, echo_name, echo_url, &echo_body,
            )),
        )
            .into_response()),
    }
}
