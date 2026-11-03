//! Content quality (M6-4, design 16 §10): the editor's check and suppressions.
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
use ferrite_core::quality::CheckDraft;
use ferrite_domain::ContentId;
use serde::Deserialize;
use serde_json::json;
use utoipa::IntoParams;
use uuid::Uuid;

use super::{
    ApiState, Authed,
    content::{load, served},
    dto::quality::{
        CheckRequestDto, CheckResponseDto, SuppressInput, SuppressionDto, SuppressionList,
    },
    extract::{ApiJson, ApiPath, ApiQuery},
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
