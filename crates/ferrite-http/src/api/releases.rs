//! `/releases` and preview links (M6-3, design 15 §10).
//!
//! Handlers only translate: `ReleaseService` and `PreviewLinkService` check visibility
//! (the creator or `edit_others`) and every entry's own rights with `content:read` /
//! `content:write`. Writes of a release take `If-Match` (its version) and answer the
//! release with its new `ETag`; a stale version is `412` with `current`. Apply is
//! version-checked, so a retry is answered `412` instead of applying twice.

use std::sync::Arc;

use axum::{
    Json,
    extract::State,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use ferrite_core::{
    Actor, CoreError,
    preview_link::PreviewLinkService,
    release::{EntryInput as CoreEntry, NewRelease, ReleaseService},
    workflow::{ReviewService, WorkflowService},
};
use ferrite_domain::{
    ContentId, MAX_PAGE, PreviewLinkId, PreviewTarget, ReleaseId, ReleaseQuery, ReleaseStatus,
    Timestamp,
};
use serde::Deserialize;
use utoipa::IntoParams;
use uuid::Uuid;

use super::{
    ApiState, Authed, BASE,
    dto::editorial::{
        EntryInput, NewReleaseInput, OverrideInput, PreviewLinkDto, PreviewLinkInput,
        PreviewLinkList, ReleaseCheckDto, ReleaseDetailDto, ReleaseDto, ReleaseList, ReleasePatch,
        ReviewDto, ScheduleReleaseInput, SubmitInput,
    },
    extract::{
        ApiJson, ApiPath, ApiQuery, DEFAULT_LIMIT, IfMatch, OptionalJson, check_limit, created,
        etag, invalid_param,
    },
    problem::{Problem, ProblemDocument},
};

/// The editorial services of the API (M6-3).
#[derive(Clone)]
pub struct EditorialApi {
    pub releases: Arc<ReleaseService>,
    pub preview_links: Arc<PreviewLinkService>,
    pub workflows: Arc<WorkflowService>,
    pub reviews: Arc<ReviewService>,
}

impl EditorialApi {
    /// The four services over `store`; `releases` the node's one release service.
    pub fn new(
        store: Arc<dyn ferrite_store::Store>,
        releases: Arc<ReleaseService>,
        preview_links: Arc<PreviewLinkService>,
    ) -> Self {
        Self {
            releases,
            preview_links,
            workflows: Arc::new(WorkflowService::new(store.clone())),
            reviews: Arc::new(ReviewService::new(store)),
        }
    }
}

/// The release as stored now, as the response body with its `ETag`.
async fn respond(
    api: &ApiState,
    actor: &Actor,
    id: ReleaseId,
    status: StatusCode,
) -> Result<Response, Problem> {
    let view = api.editorial.releases.get_for(actor, id).await?;
    let version = view.release.version;
    let body = ReleaseDetailDto::from(view);
    Ok((status, [(header::ETAG, etag(version))], Json(body)).into_response())
}

/// A write's error; a version conflict carries the release as `current`.
async fn problem(api: &ApiState, actor: &Actor, id: ReleaseId, e: CoreError) -> Problem {
    let conflict = matches!(e, CoreError::Conflict { .. });
    let p = Problem::from(e);
    if conflict && let Ok(view) = api.editorial.releases.get_for(actor, id).await {
        return p
            .with_current(serde_json::to_value(ReleaseDetailDto::from(view)).unwrap_or_default());
    }
    p
}

async fn written(
    api: &ApiState,
    actor: &Actor,
    id: ReleaseId,
    result: Result<ferrite_domain::Release, CoreError>,
) -> Result<Response, Problem> {
    match result {
        Ok(_) => respond(api, actor, id, StatusCode::OK).await,
        Err(e) => Err(problem(api, actor, id, e).await),
    }
}

/// `GET /releases` query.
#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ReleaseListParams {
    /// `open`, `scheduled`, `applied` or `failed`.
    pub status: Option<String>,
    /// `true`: only the caller's own releases.
    pub mine: Option<bool>,
    pub cursor: Option<String>,
    /// 1..=100, default 50.
    pub limit: Option<u32>,
}

const BUSY: &str =
    "`forbidden` (capability or scope named in `detail`), `csrf` or `password-reset-required`";

#[utoipa::path(
    get,
    path = "/releases",
    tag = "releases",
    operation_id = "list_releases",
    security(("session" = []), ("bearer" = [])),
    params(ReleaseListParams),
    responses(
        (status = 200, description = "Releases the caller may see, newest first", body = ReleaseList),
        (status = 400, description = "`validation` (`status`, `limit`, `cursor`)", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = BUSY, body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn list_releases(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiQuery(params): ApiQuery<ReleaseListParams>,
) -> Result<Json<ReleaseList>, Problem> {
    let status = params
        .status
        .as_deref()
        .map(|s| {
            s.parse::<ReleaseStatus>().map_err(|_| {
                invalid_param(
                    "status",
                    "unknown status",
                    "open, scheduled, applied or failed",
                )
            })
        })
        .transpose()?;
    let limit = check_limit(params.limit, DEFAULT_LIMIT, 100.min(MAX_PAGE))?;
    let (page, counts) = api
        .editorial
        .releases
        .list(
            &who.actor,
            ReleaseQuery {
                status,
                created_by: None,
                cursor: params.cursor,
                limit,
            },
            params.mine.unwrap_or(false),
        )
        .await
        .map_err(|e| match e {
            CoreError::Invalid(_) => super::extract::invalid_cursor(),
            other => other.into(),
        })?;
    Ok(Json(ReleaseList {
        items: page
            .items
            .into_iter()
            .map(|r| {
                let n = counts
                    .iter()
                    .find(|(id, _)| *id == r.id)
                    .map_or(0, |(_, n)| *n);
                ReleaseDto::new(r, n)
            })
            .collect(),
        next_cursor: page.next_cursor,
    }))
}

#[utoipa::path(
    post,
    path = "/releases",
    tag = "releases",
    operation_id = "create_release",
    security(("session" = []), ("bearer" = [])),
    request_body = NewReleaseInput,
    responses(
        (status = 201, description = "Created (`Location`, `ETag`)", body = ReleaseDetailDto),
        (status = 400, description = "`malformed-json`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = BUSY, body = ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "`validation` (`title`, `description`)", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn create_release(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiJson(input): ApiJson<NewReleaseInput>,
) -> Result<Response, Problem> {
    let r = api
        .editorial
        .releases
        .create(
            &who.actor,
            NewRelease {
                title: input.title,
                description: input.description,
            },
        )
        .await?;
    let view = api.editorial.releases.get_for(&who.actor, r.id).await?;
    let (status, headers) = created(&format!("{BASE}/releases/{}", r.id), etag(r.version));
    Ok((status, headers, Json(ReleaseDetailDto::from(view))).into_response())
}

#[utoipa::path(
    get,
    path = "/releases/{id}",
    tag = "releases",
    operation_id = "get_release",
    security(("session" = []), ("bearer" = [])),
    params(("id" = Uuid, Path)),
    responses(
        (status = 200, description = "The release with its entries (`ETag`)", body = ReleaseDetailDto),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = BUSY, body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn get_release(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Response, Problem> {
    respond(&api, &who.actor, ReleaseId::from_uuid(id), StatusCode::OK).await
}

#[utoipa::path(
    patch,
    path = "/releases/{id}",
    tag = "releases",
    operation_id = "update_release",
    security(("session" = []), ("bearer" = [])),
    params(("id" = Uuid, Path), ("If-Match" = String, Header, description = "`\"<version>\"`")),
    request_body = ReleasePatch,
    responses(
        (status = 200, description = "Updated (`ETag`)", body = ReleaseDetailDto),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = BUSY, body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 412, description = "`precondition-failed` (`current`)", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "`validation`; a scheduled or applied release", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 428, description = "`precondition-required`", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn update_release(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath(id): ApiPath<Uuid>,
    IfMatch(version): IfMatch,
    ApiJson(patch): ApiJson<ReleasePatch>,
) -> Result<Response, Problem> {
    let id = ReleaseId::from_uuid(id);
    let r = api
        .editorial
        .releases
        .update(&who.actor, id, version, patch.title, patch.description)
        .await;
    written(&api, &who.actor, id, r).await
}

#[utoipa::path(
    delete,
    path = "/releases/{id}",
    tag = "releases",
    operation_id = "delete_release",
    security(("session" = []), ("bearer" = [])),
    params(("id" = Uuid, Path), ("If-Match" = String, Header, description = "`\"<version>\"`")),
    responses(
        (status = 204, description = "Deleted with its entries, reviews and preview links"),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = BUSY, body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 412, description = "`precondition-failed` (`current`)", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "`validation`: scheduled or applied", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 428, description = "`precondition-required`", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn delete_release(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath(id): ApiPath<Uuid>,
    IfMatch(version): IfMatch,
) -> Result<StatusCode, Problem> {
    let id = ReleaseId::from_uuid(id);
    match api.editorial.releases.delete(&who.actor, id, version).await {
        Ok(()) => Ok(StatusCode::NO_CONTENT),
        Err(e) => Err(problem(&api, &who.actor, id, e).await),
    }
}

fn entry(item: Uuid, input: EntryInput) -> CoreEntry {
    CoreEntry {
        item: ContentId::from_uuid(item),
        transition: input.transition.into(),
        changes: input.changes.map(Into::into),
    }
}

#[utoipa::path(
    post,
    path = "/releases/{id}/items",
    tag = "releases",
    operation_id = "add_release_item",
    security(("session" = []), ("bearer" = [])),
    params(("id" = Uuid, Path), ("If-Match" = String, Header, description = "`\"<version>\"` of the release")),
    request_body = EntryInput,
    responses(
        (status = 200, description = "The release with the entry (`ETag`)", body = ReleaseDetailDto),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden`: the item's edit or publish right, or a scope", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 412, description = "`precondition-failed` (`current`)", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "`validation`: `item_id`, `transition`, the staged change's fields, at most 200 entries", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 428, description = "`precondition-required`", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn add_release_item(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath(id): ApiPath<Uuid>,
    IfMatch(version): IfMatch,
    ApiJson(input): ApiJson<EntryInput>,
) -> Result<Response, Problem> {
    let Some(item) = input.item_id else {
        return Err(
            Problem::validation("item_id is required").with_error(Some("item_id"), "required")
        );
    };
    let id = ReleaseId::from_uuid(id);
    let r = api
        .editorial
        .releases
        .stage(&who.actor, id, version, entry(item, input))
        .await;
    written(&api, &who.actor, id, r).await
}

#[utoipa::path(
    put,
    path = "/releases/{id}/items/{item}",
    tag = "releases",
    operation_id = "put_release_item",
    security(("session" = []), ("bearer" = [])),
    params(("id" = Uuid, Path), ("item" = Uuid, Path), ("If-Match" = String, Header, description = "`\"<version>\"` of the release")),
    request_body = EntryInput,
    responses(
        (status = 200, description = "The release with the replaced entry (`ETag`)", body = ReleaseDetailDto),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 412, description = "`precondition-failed` (`current`)", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "`validation`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 428, description = "`precondition-required`", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn put_release_item(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath((id, item)): ApiPath<(Uuid, Uuid)>,
    IfMatch(version): IfMatch,
    ApiJson(input): ApiJson<EntryInput>,
) -> Result<Response, Problem> {
    let id = ReleaseId::from_uuid(id);
    let r = api
        .editorial
        .releases
        .stage(&who.actor, id, version, entry(item, input))
        .await;
    written(&api, &who.actor, id, r).await
}

#[utoipa::path(
    delete,
    path = "/releases/{id}/items/{item}",
    tag = "releases",
    operation_id = "delete_release_item",
    security(("session" = []), ("bearer" = [])),
    params(("id" = Uuid, Path), ("item" = Uuid, Path), ("If-Match" = String, Header, description = "`\"<version>\"` of the release")),
    responses(
        (status = 200, description = "The release without the entry (`ETag`)", body = ReleaseDetailDto),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = BUSY, body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 412, description = "`precondition-failed` (`current`)", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 428, description = "`precondition-required`", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn delete_release_item(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath((id, item)): ApiPath<(Uuid, Uuid)>,
    IfMatch(version): IfMatch,
) -> Result<Response, Problem> {
    let id = ReleaseId::from_uuid(id);
    let r = api
        .editorial
        .releases
        .unstage(&who.actor, id, version, ContentId::from_uuid(item))
        .await;
    written(&api, &who.actor, id, r).await
}

#[utoipa::path(
    post,
    path = "/releases/{id}/items/{item}/rebase",
    tag = "releases",
    operation_id = "rebase_release_item",
    security(("session" = []), ("bearer" = [])),
    params(("id" = Uuid, Path), ("item" = Uuid, Path), ("If-Match" = String, Header, description = "`\"<version>\"` of the release")),
    responses(
        (status = 200, description = "The entry is based on the item's current version (`ETag`); its approval goes stale", body = ReleaseDetailDto),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = BUSY, body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 412, description = "`precondition-failed` (`current`)", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 428, description = "`precondition-required`", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn rebase_release_item(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath((id, item)): ApiPath<(Uuid, Uuid)>,
    IfMatch(version): IfMatch,
) -> Result<Response, Problem> {
    let id = ReleaseId::from_uuid(id);
    let r = api
        .editorial
        .releases
        .rebase(&who.actor, id, version, ContentId::from_uuid(item))
        .await;
    written(&api, &who.actor, id, r).await
}

#[utoipa::path(
    post,
    path = "/releases/{id}/items/{item}/submit",
    tag = "releases",
    operation_id = "submit_release_item",
    security(("session" = []), ("bearer" = [])),
    params(("id" = Uuid, Path), ("item" = Uuid, Path)),
    request_body(content = Option<SubmitInput>, description = "Optional"),
    responses(
        (status = 201, description = "The entry review", body = ReviewDto),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = BUSY, body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "`validation`: no workflow, already in review, `reviewers`, `note`", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn submit_release_item(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath((id, item)): ApiPath<(Uuid, Uuid)>,
    OptionalJson(input): OptionalJson<SubmitInput>,
) -> Result<Response, Problem> {
    let review = api
        .editorial
        .reviews
        .submit_entry(
            &who.actor,
            ReleaseId::from_uuid(id),
            ContentId::from_uuid(item),
            input.into(),
        )
        .await?;
    Ok((StatusCode::CREATED, Json(ReviewDto::from(review))).into_response())
}

#[utoipa::path(
    get,
    path = "/releases/{id}/check",
    tag = "releases",
    operation_id = "check_release",
    security(("session" = []), ("bearer" = [])),
    params(("id" = Uuid, Path)),
    responses(
        (status = 200, description = "What applying it now would do, per entry (nothing is written)", body = ReleaseCheckDto),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = BUSY, body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn check_release(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<ReleaseCheckDto>, Problem> {
    let report = api
        .editorial
        .releases
        .check(&who.actor, ReleaseId::from_uuid(id))
        .await?;
    Ok(Json(report.into()))
}

#[utoipa::path(
    post,
    path = "/releases/{id}/schedule",
    tag = "releases",
    operation_id = "schedule_release",
    security(("session" = []), ("bearer" = [])),
    params(("id" = Uuid, Path), ("If-Match" = String, Header, description = "`\"<version>\"`")),
    request_body = ScheduleReleaseInput,
    responses(
        (status = 200, description = "Scheduled; its items are locked (`ETag`)", body = ReleaseDetailDto),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = BUSY, body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 409, description = "`release-not-ready` (`entries`), or `release-locked`: an item is in another scheduled release", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 412, description = "`precondition-failed` (`current`)", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "`validation`: `at` not in the future or more than a year ahead, `override_reason`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 428, description = "`precondition-required`", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn schedule_release(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath(id): ApiPath<Uuid>,
    IfMatch(version): IfMatch,
    ApiJson(input): ApiJson<ScheduleReleaseInput>,
) -> Result<Response, Problem> {
    let id = ReleaseId::from_uuid(id);
    let r = api
        .editorial
        .releases
        .schedule(
            &who.actor,
            id,
            version,
            Timestamp::from(input.at),
            input.override_reason,
        )
        .await;
    written(&api, &who.actor, id, r).await
}

#[utoipa::path(
    post,
    path = "/releases/{id}/unschedule",
    tag = "releases",
    operation_id = "unschedule_release",
    security(("session" = []), ("bearer" = [])),
    params(("id" = Uuid, Path), ("If-Match" = String, Header, description = "`\"<version>\"`")),
    responses(
        (status = 200, description = "Open again; locks removed (`ETag`)", body = ReleaseDetailDto),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = BUSY, body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 412, description = "`precondition-failed` (`current`)", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "`validation`: not scheduled", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 428, description = "`precondition-required`", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn unschedule_release(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath(id): ApiPath<Uuid>,
    IfMatch(version): IfMatch,
) -> Result<Response, Problem> {
    let id = ReleaseId::from_uuid(id);
    let r = api
        .editorial
        .releases
        .unschedule(&who.actor, id, version)
        .await;
    written(&api, &who.actor, id, r).await
}

#[utoipa::path(
    post,
    path = "/releases/{id}/apply",
    tag = "releases",
    operation_id = "apply_release",
    security(("session" = []), ("bearer" = [])),
    params(("id" = Uuid, Path), ("If-Match" = String, Header, description = "`\"<version>\"`")),
    request_body(content = Option<OverrideInput>, description = "Optional; `override_reason` (admins) for entries without approval"),
    responses(
        (status = 200, description = "Applied in one transaction (`ETag`)", body = ReleaseDetailDto),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden`: `admin` for an override, or a scope", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 409, description = "`release-conflict` (`entries`): nothing was written", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 412, description = "`precondition-failed` (`current`)", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "`validation`: no entries, scheduled or applied, `override_reason`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 428, description = "`precondition-required`", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn apply_release(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath(id): ApiPath<Uuid>,
    IfMatch(version): IfMatch,
    OptionalJson(input): OptionalJson<OverrideInput>,
) -> Result<Response, Problem> {
    let id = ReleaseId::from_uuid(id);
    let r = api
        .editorial
        .releases
        .apply(&who.actor, id, version, input.override_reason)
        .await;
    written(&api, &who.actor, id, r).await
}

fn link_dto(
    api: &ApiState,
    link: ferrite_domain::PreviewLink,
    token: Option<String>,
) -> PreviewLinkDto {
    let mut dto = PreviewLinkDto::from(link);
    if let Some(token) = token {
        dto.url = Some(format!("{}/_preview/link/{token}", api.site_url));
        dto.token = Some(token);
    }
    dto
}

async fn create_link(
    api: &ApiState,
    actor: &Actor,
    target: PreviewTarget,
    input: PreviewLinkInput,
) -> Result<Response, Problem> {
    let (link, token) = api
        .editorial
        .preview_links
        .create(actor, target, input.expires_in_hours)
        .await?;
    let dto = link_dto(api, link, Some(token));
    Ok((StatusCode::CREATED, Json(dto)).into_response())
}

async fn list_links(
    api: &ApiState,
    actor: &Actor,
    target: PreviewTarget,
) -> Result<Json<PreviewLinkList>, Problem> {
    let links = api.editorial.preview_links.list(actor, target).await?;
    Ok(Json(PreviewLinkList {
        items: links.into_iter().map(|l| link_dto(api, l, None)).collect(),
    }))
}

#[utoipa::path(
    get,
    path = "/releases/{id}/preview-links",
    tag = "releases",
    operation_id = "list_release_preview_links",
    security(("session" = []), ("bearer" = [])),
    params(("id" = Uuid, Path)),
    responses(
        (status = 200, description = "The release's links (without tokens)", body = PreviewLinkList),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = BUSY, body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn list_release_preview_links(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<PreviewLinkList>, Problem> {
    list_links(
        &api,
        &who.actor,
        PreviewTarget::Release(ReleaseId::from_uuid(id)),
    )
    .await
}

#[utoipa::path(
    post,
    path = "/releases/{id}/preview-links",
    tag = "releases",
    operation_id = "create_release_preview_link",
    security(("session" = []), ("bearer" = [])),
    params(("id" = Uuid, Path)),
    request_body(content = Option<PreviewLinkInput>, description = "Optional"),
    responses(
        (status = 201, description = "The link with its `token` and `url` (shown once)", body = PreviewLinkDto),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = BUSY, body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "`validation` (`expires_in_hours` 1 to 168)", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 429, description = "`rate-limited`: 30 links per user and hour", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn create_release_preview_link(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath(id): ApiPath<Uuid>,
    OptionalJson(input): OptionalJson<PreviewLinkInput>,
) -> Result<Response, Problem> {
    create_link(
        &api,
        &who.actor,
        PreviewTarget::Release(ReleaseId::from_uuid(id)),
        input,
    )
    .await
}

#[utoipa::path(
    get,
    path = "/content/{type}/{id}/preview-links",
    tag = "content",
    operation_id = "list_content_preview_links",
    security(("session" = []), ("bearer" = [])),
    params(("type" = String, Path), ("id" = Uuid, Path)),
    responses(
        (status = 200, description = "The item's links (without tokens)", body = PreviewLinkList),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = BUSY, body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn list_content_preview_links(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath((kind, id)): ApiPath<(String, Uuid)>,
) -> Result<Json<PreviewLinkList>, Problem> {
    let item = super::content::load(&api, &who.actor, &kind, id).await?;
    list_links(&api, &who.actor, PreviewTarget::Item(item.id)).await
}

#[utoipa::path(
    post,
    path = "/content/{type}/{id}/preview-links",
    tag = "content",
    operation_id = "create_content_preview_link",
    security(("session" = []), ("bearer" = [])),
    params(("type" = String, Path), ("id" = Uuid, Path)),
    request_body(content = Option<PreviewLinkInput>, description = "Optional"),
    responses(
        (status = 201, description = "The link with its `token` and `url` (shown once)", body = PreviewLinkDto),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = BUSY, body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "`validation` (`expires_in_hours` 1 to 168)", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 429, description = "`rate-limited`: 30 links per user and hour", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn create_content_preview_link(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath((kind, id)): ApiPath<(String, Uuid)>,
    OptionalJson(input): OptionalJson<PreviewLinkInput>,
) -> Result<Response, Problem> {
    let item = super::content::load(&api, &who.actor, &kind, id).await?;
    create_link(&api, &who.actor, PreviewTarget::Item(item.id), input).await
}

#[utoipa::path(
    delete,
    path = "/preview-links/{id}",
    tag = "releases",
    operation_id = "revoke_preview_link",
    security(("session" = []), ("bearer" = [])),
    params(("id" = Uuid, Path)),
    responses(
        (status = 204, description = "Revoked"),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = BUSY, body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn revoke_preview_link(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<StatusCode, Problem> {
    api.editorial
        .preview_links
        .revoke(&who.actor, PreviewLinkId::from_uuid(id))
        .await?;
    Ok(StatusCode::NO_CONTENT)
}
