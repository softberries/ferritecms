//! `/types`: content types and their field schemas (07 §1; P18).
//!
//! Handlers only translate: `ContentTypeService` checks `manage_settings` (writes) and
//! `content:read` (reads), the name and permalink rules, and emits `types.changed` +
//! `invalidate [all]`, which every node turns into new routes and permalink rules. No
//! `If-Match` (P11): type writes are last-write-wins.

use std::sync::Arc;

use axum::{
    Json,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
};

use super::{
    ApiState, Authed, BASE,
    dto::types::{ContentTypeDto, ContentTypeInput, ContentTypeList, ContentTypePatch},
    extract::{ApiJson, ApiPath, created_at},
    problem::{Problem, ProblemDocument},
};

#[utoipa::path(
    get,
    path = "/types",
    tag = "types",
    operation_id = "list_types",
    security(("session" = []), ("bearer" = [])),
    responses(
        (status = 200, description = "Every content type, by name", body = ContentTypeList),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (needs `content:read`) or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn list_types(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
) -> Result<Json<ContentTypeList>, Problem> {
    let items = api.types.list(&who.actor).await?;
    Ok(Json(ContentTypeList {
        items: items.into_iter().map(ContentTypeDto::from).collect(),
        next_cursor: None,
    }))
}

#[utoipa::path(
    post,
    path = "/types",
    tag = "types",
    operation_id = "create_type",
    security(("session" = []), ("bearer" = [])),
    request_body = ContentTypeInput,
    responses(
        (status = 201, description = "Registered (`Location`); every node serves its routes within a poll interval", body = ContentTypeDto),
        (status = 400, description = "`malformed-json`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (`manage_settings`, `settings:write`), `csrf` or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "`validation` (`errors[0].field`: `name`, `label` or `fields`)", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn create_type(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiJson(input): ApiJson<ContentTypeInput>,
) -> Result<Response, Problem> {
    let ty = api.types.create(&who.actor, input.into_new()).await?;
    let (status, headers) = created_at(&format!("{BASE}/types/{}", ty.name));
    Ok((status, headers, Json(ContentTypeDto::from(ty))).into_response())
}

#[utoipa::path(
    get,
    path = "/types/{name}",
    tag = "types",
    operation_id = "get_type",
    security(("session" = []), ("bearer" = [])),
    params(("name" = String, Path, description = "Content type")),
    responses(
        (status = 200, description = "The type", body = ContentTypeDto),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (needs `content:read`) or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn get_type(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath(name): ApiPath<String>,
) -> Result<Json<ContentTypeDto>, Problem> {
    let ty = api.types.get(&who.actor, &name).await?;
    Ok(Json(ty.into()))
}

#[utoipa::path(
    patch,
    path = "/types/{name}",
    tag = "types",
    operation_id = "update_type",
    security(("session" = []), ("bearer" = [])),
    params(("name" = String, Path, description = "Content type")),
    request_body = ContentTypePatch,
    responses(
        (status = 200, description = "Saved", body = ContentTypeDto),
        (status = 400, description = "`malformed-json`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (`manage_settings`, `settings:write`), `csrf` or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "`validation` (`errors[0].field`: `label` or `fields`)", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn update_type(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath(name): ApiPath<String>,
    ApiJson(patch): ApiJson<ContentTypePatch>,
) -> Result<Json<ContentTypeDto>, Problem> {
    let ty = api
        .types
        .update(&who.actor, &name, patch.into_patch())
        .await?;
    Ok(Json(ty.into()))
}

#[utoipa::path(
    delete,
    path = "/types/{name}",
    tag = "types",
    operation_id = "delete_type",
    security(("session" = []), ("bearer" = [])),
    params(("name" = String, Path, description = "Content type")),
    responses(
        (status = 204, description = "Removed"),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (`manage_settings`, `settings:write`), `csrf` or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 409, description = "`conflict`: an item of the type was created concurrently; retry", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "`validation`: built in, has content, or a taxonomy uses it", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn delete_type(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath(name): ApiPath<String>,
) -> Result<StatusCode, Problem> {
    api.types.delete(&who.actor, &name).await?;
    Ok(StatusCode::NO_CONTENT)
}
