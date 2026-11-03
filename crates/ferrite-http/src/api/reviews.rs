//! `/workflows` and `/reviews` (M6-3, design 15 §10).
//!
//! Workflow configuration needs `manage_settings` (`settings:write`); reads need
//! `content:read`. Reviews: decisions need an eligible approver (`edit_others`, named by
//! the current step, not the submitter or the item's author), checked in core.

use std::sync::Arc;

use axum::{
    Json,
    extract::State,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use ferrite_domain::{Decision, MAX_PAGE, ReviewId, ReviewQuery, ReviewStatus, UserId};
use serde::Deserialize;
use utoipa::IntoParams;
use uuid::Uuid;

use super::{
    ApiState, Authed,
    dto::editorial::{
        ApproverList, AssigneeDto, AssigneeList, AssigneesInput, DecisionDto, DecisionInput,
        ReviewDto, ReviewList, WorkflowDto, WorkflowInput, WorkflowList,
    },
    extract::{
        ApiJson, ApiPath, ApiQuery, DEFAULT_LIMIT, IfMatch, check_limit, etag, invalid_param,
    },
    problem::{Problem, ProblemDocument},
};

const DENIED: &str =
    "`forbidden` (capability or scope named in `detail`), `csrf` or `password-reset-required`";

fn workflow_response(w: ferrite_domain::Workflow) -> Response {
    let v = w.version;
    ([(header::ETAG, etag(v))], Json(WorkflowDto::from(w))).into_response()
}

#[utoipa::path(
    get,
    path = "/workflows",
    tag = "workflows",
    operation_id = "list_workflows",
    security(("session" = []), ("bearer" = [])),
    responses(
        (status = 200, description = "Every workflow, by type", body = WorkflowList),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = DENIED, body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn list_workflows(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
) -> Result<Json<WorkflowList>, Problem> {
    let items = api.editorial.workflows.list(&who.actor).await?;
    Ok(Json(WorkflowList {
        items: items.into_iter().map(Into::into).collect(),
    }))
}

#[utoipa::path(
    get,
    path = "/workflows/{type}",
    tag = "workflows",
    operation_id = "get_workflow",
    security(("session" = []), ("bearer" = [])),
    params(("type" = String, Path)),
    responses(
        (status = 200, description = "The type's workflow (`ETag`)", body = WorkflowDto),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = DENIED, body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`: the type has no workflow", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn get_workflow(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath(kind): ApiPath<String>,
) -> Result<Response, Problem> {
    Ok(workflow_response(
        api.editorial.workflows.get(&who.actor, &kind).await?,
    ))
}

/// `GET /workflows/{type}/approvers` query.
#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ApproverParams {
    /// The step (0-based), default 0: the first step, whose reviewers a submit picks.
    pub step: Option<u32>,
}

#[utoipa::path(
    get,
    path = "/workflows/{type}/approvers",
    tag = "workflows",
    operation_id = "list_workflow_approvers",
    security(("session" = []), ("bearer" = [])),
    params(("type" = String, Path), ApproverParams),
    responses(
        (status = 200, description = "The active users who may approve the step (`edit_others`, named by its roles or users), by display name; for anyone who may submit for review (`edit_own`, `content:read`)", body = ApproverList),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = DENIED, body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`: the type has no workflow", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "`validation` (`errors[0].field` = `step`)", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn list_workflow_approvers(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath(kind): ApiPath<String>,
    ApiQuery(params): ApiQuery<ApproverParams>,
) -> Result<Json<ApproverList>, Problem> {
    let items = api
        .editorial
        .workflows
        .approvers(&who.actor, &kind, params.step.unwrap_or(0))
        .await?;
    Ok(Json(ApproverList {
        items: items.into_iter().map(Into::into).collect(),
    }))
}

#[utoipa::path(
    put,
    path = "/workflows/{type}",
    tag = "workflows",
    operation_id = "put_workflow",
    security(("session" = []), ("bearer" = [])),
    params(("type" = String, Path), ("If-Match" = Option<String>, Header, description = "`\"<version>\"` to replace one; absent to create")),
    request_body = WorkflowInput,
    responses(
        (status = 200, description = "Saved (`ETag`); open reviews keep their steps", body = WorkflowDto),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (`manage_settings`, `settings:write`) or `csrf`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`: no such type", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 412, description = "`precondition-failed`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "`validation` (`errors[0].field` = `steps`)", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn put_workflow(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath(kind): ApiPath<String>,
    headers: axum::http::HeaderMap,
    ApiJson(input): ApiJson<WorkflowInput>,
) -> Result<Response, Problem> {
    let version = if headers.contains_key(header::IF_MATCH) {
        Some(super::extract::parse_if_match(&headers)?)
    } else {
        None
    };
    let steps = input
        .steps
        .into_iter()
        .map(|s| s.into_step())
        .collect::<Result<Vec<_>, _>>()?;
    let w = api
        .editorial
        .workflows
        .put(&who.actor, &kind, steps, version)
        .await?;
    Ok(workflow_response(w))
}

#[utoipa::path(
    delete,
    path = "/workflows/{type}",
    tag = "workflows",
    operation_id = "delete_workflow",
    security(("session" = []), ("bearer" = [])),
    params(("type" = String, Path), ("If-Match" = String, Header, description = "`\"<version>\"`")),
    responses(
        (status = 204, description = "Removed; its open reviews are withdrawn"),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (`manage_settings`, `settings:write`) or `csrf`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 412, description = "`precondition-failed`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 428, description = "`precondition-required`", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn delete_workflow(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath(kind): ApiPath<String>,
    IfMatch(version): IfMatch,
) -> Result<StatusCode, Problem> {
    api.editorial
        .workflows
        .delete(&who.actor, &kind, version)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /reviews` query.
#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ReviewListParams {
    /// Default `in_review`; `all` for every status.
    pub status: Option<String>,
    /// `me`: assigned to the caller at the current step.
    pub assigned: Option<String>,
    /// `me`: submitted by the caller.
    pub submitted: Option<String>,
    /// A content type.
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub cursor: Option<String>,
    /// 1..=100, default 50.
    pub limit: Option<u32>,
}

async fn with_item(api: &ApiState, mut dto: ReviewDto) -> ReviewDto {
    if let Ok(Some(item)) = api
        .store
        .read()
        .content()
        .get(ferrite_domain::ContentId::from_uuid(dto.item_id))
        .await
    {
        dto.item_title = Some(item.title);
        dto.item_type = Some(item.kind);
    }
    dto
}

#[utoipa::path(
    get,
    path = "/reviews",
    tag = "reviews",
    operation_id = "list_reviews",
    security(("session" = []), ("bearer" = [])),
    params(ReviewListParams),
    responses(
        (status = 200, description = "The review queue, oldest first (without `edit_others`: the caller's own submissions)", body = ReviewList),
        (status = 400, description = "`validation` (`status`, `assigned`, `submitted`, `limit`, `cursor`)", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = DENIED, body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn list_reviews(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiQuery(params): ApiQuery<ReviewListParams>,
) -> Result<Json<ReviewList>, Problem> {
    let status = match params.status.as_deref() {
        None => Some(ReviewStatus::InReview),
        Some("all") => None,
        Some(s) => Some(s.parse::<ReviewStatus>().map_err(|_| {
            invalid_param("status", "unknown status", "in_review, approved, … or all")
        })?),
    };
    let me = |v: &Option<String>, field: &str| -> Result<Option<UserId>, Problem> {
        match v.as_deref() {
            None => Ok(None),
            Some("me") => Ok(Some(who.actor.user)),
            Some(_) => Err(invalid_param(field, "only `me` is supported", "me")),
        }
    };
    let assignee = me(&params.assigned, "assigned")?;
    let submitted_by = me(&params.submitted, "submitted")?;
    let limit = check_limit(params.limit, DEFAULT_LIMIT, 100.min(MAX_PAGE))?;
    let page = api
        .editorial
        .reviews
        .list(
            &who.actor,
            ReviewQuery {
                status,
                assignee,
                submitted_by,
                kind: params.kind,
                cursor: params.cursor,
                limit,
            },
        )
        .await
        .map_err(|e| match e {
            ferrite_core::CoreError::Invalid(_) => super::extract::invalid_cursor(),
            other => other.into(),
        })?;
    let mut items = Vec::with_capacity(page.items.len());
    for r in page.items {
        items.push(with_item(&api, ReviewDto::from(r)).await);
    }
    Ok(Json(ReviewList {
        items,
        next_cursor: page.next_cursor,
        assigned_to_me: api.editorial.reviews.assigned_count(&who.actor).await?,
    }))
}

async fn detail(
    api: &ApiState,
    who: &ferrite_core::Authenticated,
    id: ReviewId,
) -> Result<Response, Problem> {
    let d = api.editorial.reviews.get_for(&who.actor, id).await?;
    let version = d.review.version;
    let mut dto = ReviewDto::from(d.review);
    dto.item_title = Some(d.item.title);
    dto.item_type = Some(d.item.kind);
    dto.decisions = Some(d.decisions.into_iter().map(DecisionDto::from).collect());
    dto.assignees = Some(
        d.assignees
            .into_iter()
            .map(|(step, u)| AssigneeDto {
                step,
                user_id: u.as_uuid(),
            })
            .collect(),
    );
    Ok(([(header::ETAG, etag(version))], Json(dto)).into_response())
}

#[utoipa::path(
    get,
    path = "/reviews/{id}",
    tag = "reviews",
    operation_id = "get_review",
    security(("session" = []), ("bearer" = [])),
    params(("id" = Uuid, Path)),
    responses(
        (status = 200, description = "The review with its steps, decisions and assignees (`ETag`)", body = ReviewDto),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = DENIED, body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn get_review(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Response, Problem> {
    detail(&api, &who, ReviewId::from_uuid(id)).await
}

#[utoipa::path(
    post,
    path = "/reviews/{id}/decisions",
    tag = "reviews",
    operation_id = "decide_review",
    security(("session" = []), ("bearer" = [])),
    params(("id" = Uuid, Path), ("If-Match" = String, Header, description = "`\"<version>\"` of the review")),
    request_body = DecisionInput,
    responses(
        (status = 200, description = "Recorded; the review as it is now (`ETag`)", body = ReviewDto),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden`: not an approver of this step, the submitter or the item's author", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 412, description = "`precondition-failed`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "`validation`: closed, already decided, `decision`, `comment`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 428, description = "`precondition-required`", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn decide_review(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath(id): ApiPath<Uuid>,
    IfMatch(version): IfMatch,
    ApiJson(input): ApiJson<DecisionInput>,
) -> Result<Response, Problem> {
    let decision = input.decision.parse::<Decision>().map_err(|_| {
        Problem::validation("decision is approve or request_changes")
            .with_error(Some("decision"), "approve or request_changes")
    })?;
    let id = ReviewId::from_uuid(id);
    api.editorial
        .reviews
        .decide(&who.actor, id, version, decision, input.comment)
        .await?;
    detail(&api, &who, id).await
}

#[utoipa::path(
    put,
    path = "/reviews/{id}/assignees",
    tag = "reviews",
    operation_id = "assign_review",
    security(("session" = []), ("bearer" = [])),
    params(("id" = Uuid, Path)),
    request_body = AssigneesInput,
    responses(
        (status = 200, description = "Every assignee of the review", body = AssigneeList),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = DENIED, body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "`validation` (`step`, `reviewers`: at most 10 eligible approvers)", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn assign_review(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(input): ApiJson<AssigneesInput>,
) -> Result<Json<AssigneeList>, Problem> {
    let got = api
        .editorial
        .reviews
        .assign(
            &who.actor,
            ReviewId::from_uuid(id),
            input.step,
            input.users.into_iter().map(UserId::from_uuid).collect(),
        )
        .await?;
    Ok(Json(AssigneeList {
        items: got
            .into_iter()
            .map(|(step, u)| AssigneeDto {
                step,
                user_id: u.as_uuid(),
            })
            .collect(),
    }))
}

#[utoipa::path(
    post,
    path = "/reviews/{id}/withdraw",
    tag = "reviews",
    operation_id = "withdraw_review",
    security(("session" = []), ("bearer" = [])),
    params(("id" = Uuid, Path), ("If-Match" = String, Header, description = "`\"<version>\"` of the review")),
    responses(
        (status = 200, description = "Withdrawn; a pending item is back to draft (`ETag`)", body = ReviewDto),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = DENIED, body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 412, description = "`precondition-failed`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "`validation`: closed", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 428, description = "`precondition-required`", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn withdraw_review(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath(id): ApiPath<Uuid>,
    IfMatch(version): IfMatch,
) -> Result<Response, Problem> {
    let id = ReviewId::from_uuid(id);
    api.editorial
        .reviews
        .withdraw(&who.actor, id, version)
        .await?;
    detail(&api, &who, id).await
}
