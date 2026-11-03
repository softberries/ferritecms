//! Content quality (M6-4, design 16 §10): the editor's check, suppressions and the
//! content health report (`/health/*`: `edit_others`).
//!
//! Handlers only translate: `QualityService` checks `content:read` and read access to
//! the item (check, suppressions) or the right to edit it, plus `edit_others` for an
//! error on a gated type (suppress, unsuppress). The check probes internal links
//! through the node's `SiteProbe` (at most 200 paths) and never contacts another host.

use std::sync::Arc;

use axum::{
    Json,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use ferrite_core::{CoreError, quality::CheckDraft};
use ferrite_domain::{ContentId, FindingQuery, MAX_PAGE};
use serde::Deserialize;
use serde_json::json;
use utoipa::IntoParams;
use uuid::Uuid;

use super::{
    ApiState, Authed,
    content::{load, served},
    dto::quality::{
        CheckRequestDto, CheckResponseDto, HealthFindingList, HealthRunQueuedDto, HealthSummaryDto,
        SuppressInput, SuppressionDto, SuppressionList,
    },
    extract::{ApiJson, ApiPath, ApiQuery, DEFAULT_LIMIT, check_limit, invalid_cursor},
    problem::{Problem, ProblemDocument},
};

#[utoipa::path(
    post,
    path = "/content/{type}/check",
    tag = "content",
    operation_id = "check_content",
    security(("session" = []), ("bearer" = [])),
    params(("type" = String, Path)),
    request_body = CheckRequestDto,
    responses(
        (status = 200, description = "The findings of the draft, their counts and the type's gate", body = CheckResponseDto),
        (status = 400, description = "`malformed-json`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (`content:read`) or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`: no such type, or `id` names no item of this type you may read", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn check_content(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath(kind): ApiPath<String>,
    ApiJson(input): ApiJson<CheckRequestDto>,
) -> Result<Json<CheckResponseDto>, Problem> {
    served(&kind)?;
    let draft = CheckDraft {
        id: input.id.map(ContentId::from_uuid),
        title: input.title,
        body: input.body,
        fields: input.fields.unwrap_or_else(|| json!({})),
        locale: input.locale,
    };
    let report = api
        .quality
        .check(&who.actor, &kind, draft, api.probe.as_deref())
        .await?;
    Ok(Json(report.into()))
}

#[utoipa::path(
    get,
    path = "/content/{type}/{id}/suppressions",
    tag = "content",
    operation_id = "list_suppressions",
    security(("session" = []), ("bearer" = [])),
    params(("type" = String, Path), ("id" = Uuid, Path)),
    responses(
        (status = 200, description = "The item's suppressions", body = SuppressionList),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (`content:read`) or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn list_suppressions(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath((kind, id)): ApiPath<(String, Uuid)>,
) -> Result<Json<SuppressionList>, Problem> {
    let item = load(&api, &who.actor, &kind, id).await?;
    let items = api.quality.suppressions(&who.actor, item.id).await?;
    Ok(Json(SuppressionList {
        items: items.iter().map(Into::into).collect(),
    }))
}

#[utoipa::path(
    post,
    path = "/content/{type}/{id}/suppressions",
    tag = "content",
    operation_id = "create_suppression",
    security(("session" = []), ("bearer" = [])),
    params(("type" = String, Path), ("id" = Uuid, Path)),
    request_body = SuppressInput,
    responses(
        (status = 201, description = "Suppressed (an existing one for the same rule and key is replaced)", body = SuppressionDto),
        (status = 400, description = "`malformed-json`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (the right to edit the item; `edit_others` for an error on a gated type; `content:write`), `csrf` or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "`validation` (`errors[0].field`: `rule`, `key` or `reason`)", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn create_suppression(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath((kind, id)): ApiPath<(String, Uuid)>,
    ApiJson(input): ApiJson<SuppressInput>,
) -> Result<Response, Problem> {
    let item = load(&api, &who.actor, &kind, id).await?;
    let s = api
        .quality
        .suppress(&who.actor, item.id, &input.rule, &input.key, &input.reason)
        .await?;
    Ok((StatusCode::CREATED, Json(SuppressionDto::from(&s))).into_response())
}

/// `DELETE …/suppressions/{rule}`: which finding.
#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct SuppressionKey {
    /// The finding's key; absent or empty: the suppression of the whole rule.
    #[serde(default)]
    pub key: String,
}

#[utoipa::path(
    delete,
    path = "/content/{type}/{id}/suppressions/{rule}",
    tag = "content",
    operation_id = "delete_suppression",
    security(("session" = []), ("bearer" = [])),
    params(("type" = String, Path), ("id" = Uuid, Path), ("rule" = String, Path), SuppressionKey),
    responses(
        (status = 204, description = "Removed"),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden`, `csrf` or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`: no such item or suppression", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "`validation` (`errors[0].field`: `rule` or `key`)", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn delete_suppression(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath((kind, id, rule)): ApiPath<(String, Uuid, String)>,
    ApiQuery(q): ApiQuery<SuppressionKey>,
) -> Result<StatusCode, Problem> {
    let item = load(&api, &who.actor, &kind, id).await?;
    api.quality
        .unsuppress(&who.actor, item.id, &rule, &q.key)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    get,
    path = "/health/summary",
    tag = "health",
    operation_id = "health_summary",
    security(("session" = []), ("bearer" = [])),
    responses(
        (status = 200, description = "The newest run, the running one, the next run and whether external links are checked", body = HealthSummaryDto),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (`edit_others`, `content:read`) or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn health_summary(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
) -> Result<Json<HealthSummaryDto>, Problem> {
    let s = api.quality.summary(&who.actor).await?;
    Ok(Json(HealthSummaryDto {
        last: s.last.as_ref().map(Into::into),
        running: s.running.as_ref().map(Into::into),
        next_at: s.next_at.as_datetime(),
        external_links: s.external_links,
    }))
}

/// `GET /health/findings` filters.
#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct HealthFindingParams {
    /// `accessibility`, `links`, `media`, `site` or `freshness`.
    pub category: Option<String>,
    /// A rule id.
    pub rule: Option<String>,
    /// `error`, `warning` or `notice`.
    pub severity: Option<String>,
    /// A content type.
    #[serde(rename = "type")]
    #[param(rename = "type")]
    pub kind: Option<String>,
    pub locale: Option<String>,
    /// 1 to 200 (default 50).
    pub limit: Option<u32>,
    pub cursor: Option<String>,
}

#[utoipa::path(
    get,
    path = "/health/findings",
    tag = "health",
    operation_id = "health_findings",
    security(("session" = []), ("bearer" = [])),
    params(HealthFindingParams),
    responses(
        (status = 200, description = "A page of the newest run's findings, each with its item or redirect", body = HealthFindingList),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (`edit_others`, `content:read`) or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 400, description = "`validation` (`errors[0].field`: `limit` or `cursor`)", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "`validation` (`errors[0].field`: `category`, `rule` or `severity`)", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn health_findings(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiQuery(p): ApiQuery<HealthFindingParams>,
) -> Result<Json<HealthFindingList>, Problem> {
    let limit = check_limit(p.limit, DEFAULT_LIMIT, MAX_PAGE)?;
    let blank = |v: Option<String>| v.filter(|s| !s.trim().is_empty());
    let query = FindingQuery {
        run: None,
        category: blank(p.category),
        rule: blank(p.rule),
        severity: blank(p.severity),
        item_type: blank(p.kind),
        locale: blank(p.locale),
        limit,
        cursor: blank(p.cursor),
    };
    let page = match api.quality.findings(&who.actor, query).await {
        Err(CoreError::Invalid(_)) => return Err(invalid_cursor()),
        other => other?,
    };
    Ok(Json(HealthFindingList {
        items: page.items.iter().map(Into::into).collect(),
        next_cursor: page.next_cursor,
    }))
}

#[utoipa::path(
    post,
    path = "/health/runs",
    tag = "health",
    operation_id = "start_health_run",
    security(("session" = []), ("bearer" = [])),
    responses(
        (status = 202, description = "Queued", body = HealthRunQueuedDto),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (`edit_others`, `content:write`), `csrf` or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 409, description = "`conflict`: a run is queued or running", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn start_health_run(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
) -> Result<Response, Problem> {
    let id = api.quality.request_run(&who.actor).await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(HealthRunQueuedDto {
            job_id: id.as_uuid(),
        }),
    )
        .into_response())
}
