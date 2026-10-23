//! `/content/{type}`: content items, their lifecycle and revisions (07 §1; P11, P13).
//!
//! Handlers only translate: every permission is checked by `ContentService` (role,
//! then token scope), and every write emits the same events as any other core caller,
//! so the public page cache is invalidated the same way.
//!
//! * `{type}` must be a registered content type, else `404`; `media` is `404` too
//!   (uploads are `/media`, M2a-2; F21). An item of another type is `404`, as is a
//!   malformed id.
//! * Writes to an item (`PATCH`, `DELETE`, lifecycle `POST`s, revision restore) need
//!   `If-Match: "<version>"` (`428` without it). A stale version is `412
//!   precondition-failed` with the item as stored now in `current`; nothing is written.
//! * `DELETE` trashes; `DELETE ?force=true` deletes a trashed item for good (`204`).
//! * Every item response carries `ETag: "<version>"`.

use std::sync::Arc;

use axum::{
    Json,
    extract::State,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use ferrite_core::{Actor, AdminQuery, CoreError, MAX_REVISIONS_LISTED};
use ferrite_domain::{
    ContentId, ContentItem, ContentOrder, ContentStatus, MAX_PAGE, ParentFilter, RevisionId,
    TermId, Timestamp, UserId,
};
use serde::Deserialize;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use super::{
    ApiState, Authed, BASE,
    dto::content::{
        ContentDto, ContentInput, ContentList, ContentPatch, ContentSummaryDto, RevisionDto,
        RevisionList, RevisionSummaryDto, ScheduleRequest,
    },
    extract::{
        ApiJson, ApiPath, ApiQuery, DEFAULT_LIMIT, IfMatch, check_limit, created, etag,
        invalid_cursor, invalid_param,
    },
    list,
    problem::{Problem, ProblemDocument},
};

/// The type whose items are not served here in M2a-1 (F21).
const MEDIA_TYPE: &str = "media";

/// `ORDER BY` of a list without `q`.
#[derive(Debug, Clone, Copy, Default, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ListOrder {
    /// Newest publish time first (never published last).
    Published,
    /// Newest change first (the default).
    #[default]
    Updated,
    Title,
    /// `menu_order`, ascending.
    Menu,
}

impl From<ListOrder> for ContentOrder {
    fn from(o: ListOrder) -> Self {
        match o {
            ListOrder::Published => ContentOrder::PublishedDesc,
            ListOrder::Updated => ContentOrder::UpdatedDesc,
            ListOrder::Title => ContentOrder::TitleAsc,
            ListOrder::Menu => ContentOrder::MenuOrderAsc,
        }
    }
}

/// The query of `GET /content/{type}`.
#[derive(Debug, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ContentListParams {
    /// Comma-separated statuses (`draft,pending,scheduled,published,private,trash`);
    /// default: all but `trash`.
    pub status: Option<String>,
    /// Only items by this user.
    pub author: Option<Uuid>,
    /// Only items with this term.
    pub term: Option<Uuid>,
    /// `root` (top-level items) or the parent's id.
    pub parent: Option<String>,
    /// Full-text search over title and body. Not combinable with `author`, `term` or
    /// `parent` (`422`); results come by relevance and `order` is ignored.
    pub q: Option<String>,
    /// Default `updated`.
    pub order: Option<ListOrder>,
    /// 1..=200; default 50.
    pub limit: Option<u32>,
    /// `next_cursor` of the previous page of the same query.
    pub cursor: Option<String>,
}

impl ContentListParams {
    fn into_query(self, kind: &str) -> Result<AdminQuery, Problem> {
        let mut query = AdminQuery::new(kind);
        if let Some(list) = &self.status {
            query.statuses = list
                .split(',')
                .map(|s| s.trim().parse::<ContentStatus>())
                .collect::<Result<_, _>>()
                .map_err(|_| {
                    invalid_param(
                        "status",
                        "unknown status",
                        "comma-separated draft, pending, scheduled, published, private, trash",
                    )
                })?;
        }
        query.author = self.author.map(UserId::from_uuid);
        query.term = self.term.map(TermId::from_uuid);
        query.parent = match self.parent.as_deref() {
            None => ParentFilter::Any,
            Some("root") => ParentFilter::Root,
            Some(p) => ParentFilter::Of(
                p.parse::<ContentId>()
                    .map_err(|_| invalid_param("parent", "invalid parent", "root or an id"))?,
            ),
        };
        query.q = self.q;
        query.order = self.order.unwrap_or_default().into();
        query.limit = check_limit(self.limit, DEFAULT_LIMIT, MAX_PAGE)?;
        if let Some(c) = &self.cursor {
            // The store decodes it again; decoding here makes a bad one a 400 naming it.
            let valid = if query.search_text().is_some() {
                ferrite_store::rank_cursor(c).is_ok()
            } else {
                ferrite_store::content_cursor(query.order, c).is_ok()
            };
            if !valid {
                return Err(invalid_cursor());
            }
        }
        query.cursor = self.cursor;
        Ok(query)
    }
}

/// `?force=` of `DELETE /content/{type}/{id}`.
#[derive(Debug, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct DeleteParams {
    /// `true`: delete a trashed item for good instead of trashing.
    #[serde(default)]
    pub force: bool,
}

/// `?limit=` of `GET …/revisions`.
#[derive(Debug, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct RevisionParams {
    /// 1..=50; default 50.
    pub limit: Option<u32>,
}

/// `404` for the types not served here.
fn served(kind: &str) -> Result<(), Problem> {
    if kind == MEDIA_TYPE {
        Err(Problem::not_found())
    } else {
        Ok(())
    }
}

/// Item `id`, if it is of type `kind` (else `404`).
async fn load(api: &ApiState, actor: &Actor, kind: &str, id: Uuid) -> Result<ContentItem, Problem> {
    served(kind)?;
    let item = api.content.get_for(actor, ContentId::from_uuid(id)).await?;
    if item.kind != kind {
        return Err(Problem::not_found());
    }
    Ok(item)
}

/// `item` with its terms and permalink.
async fn dto(api: &ApiState, actor: &Actor, item: ContentItem) -> Result<ContentDto, Problem> {
    let terms = api.content.terms_of(actor, item.id).await?;
    let permalink = api.content.permalink(item.id).await?;
    Ok(ContentDto::new(item, terms, permalink))
}

/// `200` (or `status`) with `ETag` and the item.
async fn respond(
    api: &ApiState,
    actor: &Actor,
    item: ContentItem,
    status: StatusCode,
) -> Result<Response, Problem> {
    let tag = etag(item.version);
    let body = dto(api, actor, item).await?;
    Ok((status, [(header::ETAG, tag)], Json(body)).into_response())
}

/// The problem of a failed write to item `id`: a version conflict is `412` with the
/// item as stored now in `current` (when it can still be read).
async fn write_problem(api: &ApiState, actor: &Actor, id: ContentId, e: CoreError) -> Problem {
    let conflict = matches!(e, CoreError::Conflict { .. });
    let problem = Problem::from(e);
    if !conflict {
        return problem;
    }
    let current = match api.content.get_for(actor, id).await {
        Ok(item) => dto(api, actor, item).await,
        Err(e) => Err(Problem::from(e)),
    };
    match current.map(serde_json::to_value) {
        Ok(Ok(value)) => problem.with_current(value),
        Ok(Err(e)) => {
            tracing::error!(error = %e, "cannot serialize the current item");
            problem
        }
        // Gone or unreadable since: the 412 stays without `current`.
        Err(_) => problem,
    }
}

/// The result of a write to item `id` as a response (see [`write_problem`]).
async fn written(
    api: &ApiState,
    actor: &Actor,
    id: ContentId,
    result: Result<ContentItem, CoreError>,
) -> Result<Response, Problem> {
    match result {
        Ok(item) => respond(api, actor, item, StatusCode::OK).await,
        Err(e) => Err(write_problem(api, actor, id, e).await),
    }
}

#[utoipa::path(
    get,
    path = "/content/{type}",
    tag = "content",
    operation_id = "list_content",
    security(("session" = []), ("bearer" = [])),
    params(("type" = String, Path, description = "Content type"), ContentListParams),
    responses(
        (status = 200, description = "One page of items", body = ContentList),
        (status = 400, description = "`validation` (`limit`, `cursor`, `status`, `parent`) or `malformed-query`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (needs `content:read`)", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`: no such type", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "`validation`: `q` with `author`, `term` or `parent`", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn list_content(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath(kind): ApiPath<String>,
    ApiQuery(params): ApiQuery<ContentListParams>,
) -> Result<Json<ContentList>, Problem> {
    served(&kind)?;
    api.content.check_type(&who.actor, &kind).await?;
    let query = params.into_query(&kind)?;
    let page = api.content.list(&who.actor, &query).await?;
    let (items, next_cursor) = list::items(page, ContentSummaryDto::from);
    Ok(Json(ContentList { items, next_cursor }))
}

#[utoipa::path(
    post,
    path = "/content/{type}",
    tag = "content",
    operation_id = "create_content",
    security(("session" = []), ("bearer" = [])),
    params(("type" = String, Path, description = "Content type")),
    request_body = ContentInput,
    responses(
        (status = 201, description = "Created as a draft (`Location`, `ETag`)", body = ContentDto),
        (status = 403, description = "`forbidden` or `csrf`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`: no such type", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 409, description = "`conflict`: the slug was taken concurrently (`errors[0].field` = `slug`)", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "`validation`", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn create_content(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath(kind): ApiPath<String>,
    ApiJson(input): ApiJson<ContentInput>,
) -> Result<Response, Problem> {
    served(&kind)?;
    api.content.check_type(&who.actor, &kind).await?;
    let item = api
        .content
        .create(&who.actor, input.into_draft(&kind))
        .await?;
    let location = format!("{BASE}/content/{kind}/{}", item.id);
    let tag = etag(item.version);
    let body = dto(&api, &who.actor, item).await?;
    let (status, headers) = created(&location, tag);
    Ok((status, headers, Json(body)).into_response())
}

#[utoipa::path(
    get,
    path = "/content/{type}/{id}",
    tag = "content",
    operation_id = "get_content",
    security(("session" = []), ("bearer" = [])),
    params(("type" = String, Path), ("id" = Uuid, Path)),
    responses(
        (status = 200, description = "The item (`ETag`)", body = ContentDto),
        (status = 404, description = "`not-found`", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn get_content(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath((kind, id)): ApiPath<(String, Uuid)>,
) -> Result<Response, Problem> {
    let item = load(&api, &who.actor, &kind, id).await?;
    respond(&api, &who.actor, item, StatusCode::OK).await
}

#[utoipa::path(
    patch,
    path = "/content/{type}/{id}",
    tag = "content",
    operation_id = "update_content",
    security(("session" = []), ("bearer" = [])),
    params(("type" = String, Path), ("id" = Uuid, Path), ("If-Match" = String, Header, description = "`\"<version>\"`")),
    request_body = ContentPatch,
    responses(
        (status = 200, description = "Saved (`ETag`)", body = ContentDto),
        (status = 404, description = "`not-found`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 409, description = "`conflict`: the slug was taken concurrently", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 412, description = "`precondition-failed`: stale version (`current`)", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "`validation`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 428, description = "`precondition-required`: no `If-Match`", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn update_content(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath((kind, id)): ApiPath<(String, Uuid)>,
    IfMatch(version): IfMatch,
    ApiJson(patch): ApiJson<ContentPatch>,
) -> Result<Response, Problem> {
    let actor = &who.actor;
    let current = load(&api, actor, &kind, id).await?;
    let id = current.id;
    // The patch is merged onto the version the client named: any other is a 412 before
    // the merge (and `update` checks the version again in its transaction).
    if current.version != version {
        let current_version = current.version;
        return Err(write_problem(&api, actor, id, CoreError::Conflict { current_version }).await);
    }
    let terms = api.content.terms_of(actor, id).await?;
    let draft = patch.apply(&current, terms);
    let result = api.content.update(actor, id, version, draft).await;
    written(&api, actor, id, result).await
}

#[utoipa::path(
    delete,
    path = "/content/{type}/{id}",
    tag = "content",
    operation_id = "delete_content",
    security(("session" = []), ("bearer" = [])),
    params(("type" = String, Path), ("id" = Uuid, Path), ("If-Match" = String, Header, description = "`\"<version>\"`"), DeleteParams),
    responses(
        (status = 200, description = "Trashed (`ETag`)", body = ContentDto),
        (status = 204, description = "Deleted for good (`force=true`, trashed items only)"),
        (status = 404, description = "`not-found`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 412, description = "`precondition-failed`: stale version (`current`)", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "`validation`: not trashed (`force`), already trashed, or has children", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 428, description = "`precondition-required`: no `If-Match`", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn delete_content(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath((kind, id)): ApiPath<(String, Uuid)>,
    ApiQuery(params): ApiQuery<DeleteParams>,
    IfMatch(version): IfMatch,
) -> Result<Response, Problem> {
    let actor = &who.actor;
    let id = load(&api, actor, &kind, id).await?.id;
    if !params.force {
        let result = api.content.trash(actor, id, version).await;
        return written(&api, actor, id, result).await;
    }
    match api.content.delete(actor, id, version).await {
        Ok(()) => Ok(StatusCode::NO_CONTENT.into_response()),
        Err(e) => Err(write_problem(&api, actor, id, e).await),
    }
}

/// A status transition without a body.
#[derive(Debug, Clone, Copy)]
enum Transition {
    Publish,
    Unpublish,
    Submit,
    Restore,
}

async fn transition(
    api: &ApiState,
    who: &Authed,
    kind: &str,
    id: Uuid,
    version: i64,
    t: Transition,
) -> Result<Response, Problem> {
    let actor = &who.0.actor;
    let id = load(api, actor, kind, id).await?.id;
    let svc = &api.content;
    let result = match t {
        Transition::Publish => svc.publish(actor, id, version).await,
        Transition::Unpublish => svc.unpublish(actor, id, version).await,
        Transition::Submit => svc.submit(actor, id, version).await,
        Transition::Restore => svc.restore(actor, id, version).await,
    };
    written(api, actor, id, result).await
}

#[utoipa::path(
    post,
    path = "/content/{type}/{id}/publish",
    tag = "content",
    operation_id = "publish_content",
    security(("session" = []), ("bearer" = [])),
    params(("type" = String, Path), ("id" = Uuid, Path), ("If-Match" = String, Header, description = "`\"<version>\"`")),
    responses(
        (status = 200, description = "Published (from draft, pending or scheduled; `ETag`)", body = ContentDto),
        (status = 403, description = "`forbidden` (`publish_own`, `publish_others`, `content:write`) or `csrf`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 412, description = "`precondition-failed` (`current`)", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "`validation`: illegal transition", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 428, description = "`precondition-required`", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn publish_content(
    State(api): State<Arc<ApiState>>,
    who: Authed,
    ApiPath((kind, id)): ApiPath<(String, Uuid)>,
    IfMatch(version): IfMatch,
) -> Result<Response, Problem> {
    transition(&api, &who, &kind, id, version, Transition::Publish).await
}

#[utoipa::path(
    post,
    path = "/content/{type}/{id}/unpublish",
    tag = "content",
    operation_id = "unpublish_content",
    security(("session" = []), ("bearer" = [])),
    params(("type" = String, Path), ("id" = Uuid, Path), ("If-Match" = String, Header, description = "`\"<version>\"`")),
    responses(
        (status = 200, description = "Back to draft (from published, scheduled or private; `ETag`)", body = ContentDto),
        (status = 412, description = "`precondition-failed` (`current`)", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "`validation`: illegal transition", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 428, description = "`precondition-required`", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn unpublish_content(
    State(api): State<Arc<ApiState>>,
    who: Authed,
    ApiPath((kind, id)): ApiPath<(String, Uuid)>,
    IfMatch(version): IfMatch,
) -> Result<Response, Problem> {
    transition(&api, &who, &kind, id, version, Transition::Unpublish).await
}

#[utoipa::path(
    post,
    path = "/content/{type}/{id}/submit",
    tag = "content",
    operation_id = "submit_content",
    security(("session" = []), ("bearer" = [])),
    params(("type" = String, Path), ("id" = Uuid, Path), ("If-Match" = String, Header, description = "`\"<version>\"`")),
    responses(
        (status = 200, description = "Submitted for review (draft to pending; `ETag`)", body = ContentDto),
        (status = 412, description = "`precondition-failed` (`current`)", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "`validation`: illegal transition", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 428, description = "`precondition-required`", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn submit_content(
    State(api): State<Arc<ApiState>>,
    who: Authed,
    ApiPath((kind, id)): ApiPath<(String, Uuid)>,
    IfMatch(version): IfMatch,
) -> Result<Response, Problem> {
    transition(&api, &who, &kind, id, version, Transition::Submit).await
}

#[utoipa::path(
    post,
    path = "/content/{type}/{id}/restore",
    tag = "content",
    operation_id = "restore_content",
    security(("session" = []), ("bearer" = [])),
    params(("type" = String, Path), ("id" = Uuid, Path), ("If-Match" = String, Header, description = "`\"<version>\"`")),
    responses(
        (status = 200, description = "Restored from trash as a draft (`ETag`)", body = ContentDto),
        (status = 412, description = "`precondition-failed` (`current`)", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "`validation`: not trashed", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 428, description = "`precondition-required`", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn restore_content(
    State(api): State<Arc<ApiState>>,
    who: Authed,
    ApiPath((kind, id)): ApiPath<(String, Uuid)>,
    IfMatch(version): IfMatch,
) -> Result<Response, Problem> {
    transition(&api, &who, &kind, id, version, Transition::Restore).await
}

#[utoipa::path(
    post,
    path = "/content/{type}/{id}/schedule",
    tag = "content",
    operation_id = "schedule_content",
    security(("session" = []), ("bearer" = [])),
    params(("type" = String, Path), ("id" = Uuid, Path), ("If-Match" = String, Header, description = "`\"<version>\"`")),
    request_body = ScheduleRequest,
    responses(
        (status = 200, description = "Scheduled (from draft or pending; `ETag`)", body = ContentDto),
        (status = 412, description = "`precondition-failed` (`current`)", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "`validation`: `at` not in the future, or illegal transition", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 428, description = "`precondition-required`", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn schedule_content(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath((kind, id)): ApiPath<(String, Uuid)>,
    IfMatch(version): IfMatch,
    ApiJson(req): ApiJson<ScheduleRequest>,
) -> Result<Response, Problem> {
    let actor = &who.actor;
    let id = load(&api, actor, &kind, id).await?.id;
    let at = Timestamp::from(req.at);
    let result = api.content.schedule(actor, id, version, at).await;
    written(&api, actor, id, result).await
}

#[utoipa::path(
    get,
    path = "/content/{type}/{id}/revisions",
    tag = "content",
    operation_id = "list_revisions",
    security(("session" = []), ("bearer" = [])),
    params(("type" = String, Path), ("id" = Uuid, Path), RevisionParams),
    responses(
        (status = 200, description = "Newest first", body = RevisionList),
        (status = 400, description = "`validation` (`limit`)", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn list_revisions(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath((kind, id)): ApiPath<(String, Uuid)>,
    ApiQuery(params): ApiQuery<RevisionParams>,
) -> Result<Json<RevisionList>, Problem> {
    let limit = check_limit(params.limit, MAX_REVISIONS_LISTED, MAX_REVISIONS_LISTED)?;
    let id = load(&api, &who.actor, &kind, id).await?.id;
    let revisions = api.content.revisions(&who.actor, id, limit).await?;
    Ok(Json(RevisionList {
        items: revisions
            .into_iter()
            .map(RevisionSummaryDto::from)
            .collect(),
        next_cursor: None,
    }))
}

#[utoipa::path(
    get,
    path = "/content/{type}/{id}/revisions/{rid}",
    tag = "content",
    operation_id = "get_revision",
    security(("session" = []), ("bearer" = [])),
    params(("type" = String, Path), ("id" = Uuid, Path), ("rid" = Uuid, Path)),
    responses(
        (status = 200, description = "The revision", body = RevisionDto),
        (status = 404, description = "`not-found` (also a revision of another item)", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn get_revision(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath((kind, id, rid)): ApiPath<(String, Uuid, Uuid)>,
) -> Result<Json<RevisionDto>, Problem> {
    let id = load(&api, &who.actor, &kind, id).await?.id;
    let rev = api
        .content
        .revision(&who.actor, id, RevisionId::from_uuid(rid))
        .await?;
    Ok(Json(RevisionDto::from(rev)))
}

#[utoipa::path(
    post,
    path = "/content/{type}/{id}/revisions/{rid}/restore",
    tag = "content",
    operation_id = "restore_revision",
    security(("session" = []), ("bearer" = [])),
    params(("type" = String, Path), ("id" = Uuid, Path), ("rid" = Uuid, Path), ("If-Match" = String, Header, description = "`\"<version>\"`")),
    responses(
        (status = 200, description = "Title, excerpt, body and fields restored (meta is not; `ETag`)", body = ContentDto),
        (status = 404, description = "`not-found` (also a revision of another item)", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 412, description = "`precondition-failed` (`current`)", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "`validation`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 428, description = "`precondition-required`", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn restore_revision(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath((kind, id, rid)): ApiPath<(String, Uuid, Uuid)>,
    IfMatch(version): IfMatch,
) -> Result<Response, Problem> {
    let actor = &who.actor;
    let id = load(&api, actor, &kind, id).await?.id;
    let result = api
        .content
        .restore_revision(actor, id, RevisionId::from_uuid(rid), version)
        .await;
    written(&api, actor, id, result).await
}
