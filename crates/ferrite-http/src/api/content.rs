//! `/content/{type}`: content items, their lifecycle and revisions (07 §1; P11, P13).
//!
//! Handlers only translate: every permission is checked by `ContentService` (role,
//! then token scope), and every write emits the same events as any other core caller,
//! so the public page cache is invalidated the same way.
//!
//! * `{type}` must be a registered content type, else `404`; `media` is `404` too
//!   (uploads are `/media`, M2a-2; F21). An item of another type is `404`, as is a
//!   malformed id, and an item the caller may not read (others' items that are not
//!   publicly visible now, future-dated published ones included, without
//!   `edit_others`; R17, R23; core's `can_read`), for reads and writes alike. Reading
//!   revisions also needs a role that may edit the item (R26; core's `role_can_edit`),
//!   else `404`.
//! * Writes to an item (`PATCH`, `DELETE`, lifecycle `POST`s, revision restore) need
//!   `If-Match: "<version>"` (`428` without it). A stale version is `412
//!   precondition-failed` with the item as stored now in `current`; nothing is written.
//! * `DELETE` trashes; `DELETE ?force=true` deletes a trashed item for good (`204`).
//! * Every item response carries `ETag: "<version>"`.
//! * Items (and list entries) carry `author` (id, login, display name) and `allowed`,
//!   what the caller may do with the item now, computed in core
//!   (`ferrite_core::content::allowed_actions`, M2b P13); revisions carry `author`.
//! * `GET /content/{type}/counts` counts items per status as the list's visibility rule
//!   allows (P14). The static `counts` segment wins over `{id}` in the router.
//! * `GET /content/refs?ids=…` names items by id across types (relation fields, menu
//!   targets, filter chips): the ids the caller may read come back with type, title,
//!   status and permalink; unknown and unreadable ids are absent, never an error.
//!   1..=200 ids (`422`, field `ids`); a malformed id is `400`. The static `refs`
//!   segment wins over `{type}` in the router, and core reserves `refs` as a type name.
//! * `…/{id}/autosave` is the caller's own unsaved working copy of the item (P15): no
//!   `If-Match`, last write wins, never a revision. Reading needs a role that may edit
//!   the item (as revisions), writing `can_edit` (F12). The caller's next real save of
//!   the item deletes it.
//! * Items carry `seo` (`content_meta` `seo.title`, `seo.description`; M2b P28); list
//!   entries do not. `seo` in a create or `PATCH` changes them member by member, and a
//!   change to `seo` alone is a full save under `If-Match`. A `body` that is sent (create,
//!   `PATCH`, autosave `PUT`) or restored from a revision must be a document (`type`
//!   `doc`; `422`, field `body`); a `PATCH` without `body` keeps the stored one as is.

use std::{collections::HashMap, sync::Arc};

use axum::{
    Json,
    extract::State,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};

use ferrite_core::{
    Actor, AdminQuery, CoreError, MAX_REVISIONS_LISTED, Relocation, TranslateNote,
    content::{AuthorRef, allowed_actions},
};
use ferrite_domain::{
    ContentId, ContentItem, ContentOrder, ContentStatus, MAX_PAGE, MEDIA_KIND, ParentFilter,
    RevisionId, TermId, Timestamp, UserId,
};
use serde::Deserialize;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use super::{
    ApiState, Authed, BASE,
    dto::content::{
        AutosaveDto, AutosaveInput, ContentCountsDto, ContentDto, ContentInput, ContentList,
        ContentPatch, ContentRefList, ContentSummaryDto, RevisionDto, RevisionList,
        RevisionSummaryDto, ScheduleRequest, TranslateInput, TranslationCreated, TranslationRefDto,
        TranslationStatusDto,
    },
    extract::{
        ApiJson, ApiPath, ApiQuery, DEFAULT_LIMIT, IfMatch, check_limit, created, etag,
        invalid_cursor, invalid_param,
    },
    list,
    problem::{Problem, ProblemDocument},
};

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
    #[param(inline)]
    pub order: Option<ListOrder>,
    /// 1..=200; default 50.
    pub limit: Option<u32>,
    /// `next_cursor` of the previous page of the same query.
    pub cursor: Option<String>,
    /// Only items in this locale (one of `GET /meta`'s; another is `400 invalid-locale`).
    pub locale: Option<String>,
}

/// `?locale=` of a list: one of the site's locales, else `400` `invalid-locale`.
pub(crate) async fn checked_locale(
    api: &ApiState,
    locale: Option<String>,
) -> Result<Option<String>, Problem> {
    let Some(l) = locale else {
        return Ok(None);
    };
    let locales = ferrite_core::locales::read(api.store.read())
        .await
        .map_err(Problem::from)?;
    if locales.contains(&l) {
        Ok(Some(l))
    } else {
        Err(Problem::new(
            StatusCode::BAD_REQUEST,
            "invalid-locale",
            "Unknown locale",
            format!("{l:?} is not one of the site's locales"),
        ))
    }
}

/// The query of `GET /content/{type}/counts`.
#[derive(Debug, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ContentCountParams {
    /// Only items in this locale (`400 invalid-locale` for another).
    pub locale: Option<String>,
}

impl ContentListParams {
    pub(crate) fn into_query(self, kind: &str) -> Result<AdminQuery, Problem> {
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
pub(crate) fn served(kind: &str) -> Result<(), Problem> {
    if kind == MEDIA_KIND {
        Err(Problem::not_found())
    } else {
        Ok(())
    }
}

/// Item `id`, if it is of type `kind` (else `404`).
pub(crate) async fn load(
    api: &ApiState,
    actor: &Actor,
    kind: &str,
    id: Uuid,
) -> Result<ContentItem, Problem> {
    served(kind)?;
    let item = api.content.get_for(actor, ContentId::from_uuid(id)).await?;
    if item.kind != kind {
        return Err(Problem::not_found());
    }
    Ok(item)
}

/// `item` with its terms, permalink, author, allowed actions and SEO meta.
pub(crate) async fn dto(
    api: &ApiState,
    actor: &Actor,
    item: ContentItem,
) -> Result<ContentDto, Problem> {
    let (terms, seo) = api.content.extras_for(actor, &item).await?;
    let permalink = api.content.permalink(item.id).await?;
    let author = author_of(api, actor, item.author_id).await?;
    let allowed = allowed_actions(actor, &item);
    let refs = api
        .content
        .translations(actor, std::slice::from_ref(&item))
        .await?
        .remove(&item.id)
        .unwrap_or_default();
    let mut translations = Vec::with_capacity(refs.len());
    for r in refs {
        translations.push(TranslationRefDto {
            permalink: api.content.permalink(r.id).await?,
            id: r.id.as_uuid(),
            locale: r.locale,
            status: r.status.as_str().to_owned(),
            title: r.title,
        });
    }
    let mut dto = ContentDto::new(item, terms, permalink, author, allowed, seo);
    dto.translations = translations;
    Ok(dto)
}

/// The authors of `ids` (one lookup), keyed by id.
pub(crate) async fn authors(
    api: &ApiState,
    actor: &Actor,
    ids: impl IntoIterator<Item = UserId>,
) -> Result<HashMap<UserId, AuthorRef>, Problem> {
    let ids: Vec<UserId> = ids.into_iter().collect();
    Ok(api.content.authors(actor, &ids).await?)
}

/// Author `id` from `map`. Items and revisions reference existing users (foreign
/// keys), so a miss is a `500`.
pub(crate) fn pick(map: &HashMap<UserId, AuthorRef>, id: UserId) -> Result<AuthorRef, Problem> {
    map.get(&id).cloned().ok_or_else(|| {
        tracing::error!(user = %id, "author not found");
        Problem::internal()
    })
}

/// The author of one item or revision.
pub(crate) async fn author_of(
    api: &ApiState,
    actor: &Actor,
    id: UserId,
) -> Result<AuthorRef, Problem> {
    pick(&authors(api, actor, [id]).await?, id)
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
    api.types.get(&who.actor, &kind).await?;
    let locale = checked_locale(&api, params.locale.clone()).await?;
    let mut query = params.into_query(&kind)?;
    query.locale = locale;
    let page = api.content.list(&who.actor, &query).await?;
    let map = authors(&api, &who.actor, page.items.iter().map(|i| i.author_id)).await?;
    let mut translations = api.content.translations(&who.actor, &page.items).await?;
    let (items, next_cursor) = list::items(page, |item| {
        let author = pick(&map, item.author_id)?;
        let allowed = allowed_actions(&who.actor, &item);
        let refs = translations.remove(&item.id).unwrap_or_default();
        let mut dto = ContentSummaryDto::new(item, author, allowed);
        dto.translations = refs
            .into_iter()
            .map(|r| TranslationStatusDto {
                id: r.id.as_uuid(),
                locale: r.locale,
                status: r.status.as_str().to_owned(),
            })
            .collect();
        Ok::<_, Problem>(dto)
    });
    let items = items.into_iter().collect::<Result<_, _>>()?;
    Ok(Json(ContentList { items, next_cursor }))
}

/// The query of `GET /content/refs`.
#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ContentRefsParams {
    /// Comma-separated item ids, 1..=200.
    pub ids: String,
}

#[utoipa::path(
    get,
    path = "/content/refs",
    tag = "content",
    operation_id = "content_refs",
    security(("session" = []), ("bearer" = [])),
    params(ContentRefsParams),
    responses(
        (status = 200, description = "The requested items the caller may read (as the lists decide; media items only with `media:read`), in no particular order; unknown and unreadable ids are absent", body = ContentRefList),
        (status = 400, description = "`validation` (`ids`: not a UUID) or `malformed-query`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (needs `content:read`) or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "`validation`: not 1..=200 ids (`ids`)", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn content_refs(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiQuery(params): ApiQuery<ContentRefsParams>,
) -> Result<Json<ContentRefList>, Problem> {
    let ids = params
        .ids
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.parse::<Uuid>().map(ContentId::from_uuid))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| invalid_param("ids", "ids must be UUIDs", "comma-separated UUIDs"))?;
    let refs = api.content.refs(&who.actor, &ids).await?;
    Ok(Json(ContentRefList {
        items: refs.into_iter().map(Into::into).collect(),
    }))
}

#[utoipa::path(
    get,
    path = "/content/{type}/counts",
    tag = "content",
    operation_id = "count_content",
    security(("session" = []), ("bearer" = [])),
    params(("type" = String, Path, description = "Content type"), ContentCountParams),
    responses(
        (status = 200, description = "Items per status that the caller may read (others' items that are not publicly visible only with `edit_others`)", body = ContentCountsDto),
        (status = 400, description = "`invalid-locale`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (needs `content:read`) or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`: no such type", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn count_content(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath(kind): ApiPath<String>,
    ApiQuery(params): ApiQuery<ContentCountParams>,
) -> Result<Json<ContentCountsDto>, Problem> {
    served(&kind)?;
    let locale = checked_locale(&api, params.locale).await?;
    let counts = api
        .content
        .counts(&who.actor, &kind, locale.as_deref())
        .await?;
    Ok(Json(counts.into()))
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
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 400, description = "`malformed-json`", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn create_content(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath(kind): ApiPath<String>,
    ApiJson(input): ApiJson<ContentInput>,
) -> Result<Response, Problem> {
    served(&kind)?;
    api.types.get(&who.actor, &kind).await?;
    let (locale, translation_of) = (input.locale.clone(), input.translation_of);
    let draft = input.into_draft(&kind);
    let item = if locale.is_some() || translation_of.is_some() {
        api.content
            .create_localized(
                &who.actor,
                draft,
                locale,
                translation_of.map(ContentId::from_uuid),
            )
            .await?
    } else {
        api.content.create(&who.actor, draft).await?
    };
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
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (capability or scope named in `detail`), `csrf` or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
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
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (capability or scope named in `detail`), `csrf` or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 400, description = "`malformed-json` or `malformed-header` (`If-Match`)", body = ProblemDocument, content_type = "application/problem+json"),
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
    // The patch is merged onto the item as read now. If that is not the version the
    // client named, `update` refuses it in its transaction (after its permission check)
    // with a `Conflict`, so a merge onto another version is never written.
    let terms = api.content.terms_of(actor, id).await?;
    let relocation = Relocation {
        locale: patch.locale.clone(),
        translation_of: patch.translation_of.map(|t| t.map(ContentId::from_uuid)),
    };
    let draft = patch.apply(&current, terms);
    // The locale and group move after the field update, in the same transaction (13 §7).
    let result = api
        .content
        .update_and_relocate(actor, id, version, draft, relocation)
        .await;
    written(&api, actor, id, result).await
}

#[utoipa::path(
    post,
    path = "/content/{type}/{id}/translations",
    tag = "content",
    operation_id = "translate_content",
    security(("session" = []), ("bearer" = [])),
    params(("type" = String, Path), ("id" = Uuid, Path)),
    request_body = TranslateInput,
    responses(
        (status = 201, description = "The new draft translation (`Location`) and its notes", body = TranslationCreated),
        (status = 404, description = "`not-found`: no such item the caller may read", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 409, description = "`conflict`: the locale has a translation already (`detail` names its id)", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "`validation` (`locale`)", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (needs `edit_own`, `content:write`) or `csrf`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 400, description = "`malformed-json`", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn translate_content(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath((kind, id)): ApiPath<(String, Uuid)>,
    ApiJson(input): ApiJson<TranslateInput>,
) -> Result<Response, Problem> {
    let source = load(&api, &who.actor, &kind, id).await?;
    let done = api
        .content
        .translate(&who.actor, source.id, &input.locale)
        .await?;
    let location = format!("{BASE}/content/{kind}/{}", done.item.id);
    let tag = etag(done.item.version);
    let item = dto(&api, &who.actor, done.item).await?;
    let notes = done.notes.iter().map(TranslateNote::code).collect();
    let (status, headers) = created(&location, tag);
    Ok((status, headers, Json(TranslationCreated { item, notes })).into_response())
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
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (capability or scope named in `detail`), `csrf` or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 400, description = "`malformed-header` (`If-Match`) or `malformed-query`", body = ProblemDocument, content_type = "application/problem+json"),
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
pub(crate) enum Transition {
    Publish,
    Unpublish,
    Submit,
    Restore,
}

/// Runs transition `t` on item `id` at `version` (the REST endpoints and the MCP tools
/// share it; core checks the permission and the version).
pub(crate) async fn apply_transition(
    api: &ApiState,
    actor: &Actor,
    id: ContentId,
    version: i64,
    t: Transition,
) -> Result<ContentItem, CoreError> {
    let svc = &api.content;
    match t {
        Transition::Publish => svc.publish(actor, id, version).await,
        Transition::Unpublish => svc.unpublish(actor, id, version).await,
        Transition::Submit => svc.submit(actor, id, version).await,
        Transition::Restore => svc.restore(actor, id, version).await,
    }
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
    let result = apply_transition(api, actor, id, version, t).await;
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
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`: no such type, or no such item of this type that you may read", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 400, description = "`malformed-header` (`If-Match`)", body = ProblemDocument, content_type = "application/problem+json"),
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
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (capability or scope named in `detail`), `csrf` or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`: no such type, or no such item of this type that you may read", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 400, description = "`malformed-header` (`If-Match`)", body = ProblemDocument, content_type = "application/problem+json"),
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
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (capability or scope named in `detail`), `csrf` or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`: no such type, or no such item of this type that you may read", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 400, description = "`malformed-header` (`If-Match`)", body = ProblemDocument, content_type = "application/problem+json"),
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
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (capability or scope named in `detail`), `csrf` or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`: no such type, or no such item of this type that you may read", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 400, description = "`malformed-header` (`If-Match`)", body = ProblemDocument, content_type = "application/problem+json"),
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
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (capability or scope named in `detail`), `csrf` or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`: no such type, or no such item of this type that you may read", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 400, description = "`malformed-json` or `malformed-header` (`If-Match`)", body = ProblemDocument, content_type = "application/problem+json"),
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
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (capability or scope named in `detail`), `csrf` or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
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
    let map = authors(&api, &who.actor, revisions.iter().map(|r| r.author_id)).await?;
    let items = revisions
        .into_iter()
        .map(|r| {
            let author = pick(&map, r.author_id)?;
            Ok(RevisionSummaryDto::new(r, author))
        })
        .collect::<Result<_, Problem>>()?;
    Ok(Json(RevisionList {
        items,
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
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (capability or scope named in `detail`), `csrf` or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
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
    let author = author_of(&api, &who.actor, rev.author_id).await?;
    Ok(Json(RevisionDto::new(rev, author)))
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
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (capability or scope named in `detail`), `csrf` or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 400, description = "`malformed-header` (`If-Match`)", body = ProblemDocument, content_type = "application/problem+json"),
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

#[utoipa::path(
    get,
    path = "/content/{type}/{id}/autosave",
    tag = "content",
    operation_id = "get_autosave",
    security(("session" = []), ("bearer" = [])),
    params(("type" = String, Path), ("id" = Uuid, Path)),
    responses(
        (status = 200, description = "The caller's autosave of the item", body = AutosaveDto),
        (status = 204, description = "You have no autosave of the item"),
        (status = 404, description = "`not-found`: no such item that you may read", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (your role may not edit the item, or no `content:read`) or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn get_autosave(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath((kind, id)): ApiPath<(String, Uuid)>,
) -> Result<Response, Problem> {
    let id = load(&api, &who.actor, &kind, id).await?.id;
    // None is an answer, not an error: `204` (the editor asks on every open).
    Ok(match api.content.get_autosave(&who.actor, id).await? {
        Some(a) => Json(AutosaveDto::from(a)).into_response(),
        None => StatusCode::NO_CONTENT.into_response(),
    })
}

#[utoipa::path(
    put,
    path = "/content/{type}/{id}/autosave",
    tag = "content",
    operation_id = "put_autosave",
    security(("session" = []), ("bearer" = [])),
    params(("type" = String, Path), ("id" = Uuid, Path)),
    request_body = AutosaveInput,
    responses(
        (status = 200, description = "Stored, replacing the caller's previous autosave", body = AutosaveDto),
        (status = 404, description = "`not-found`: no such item that you may read", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "`validation`: control characters or NUL (`errors[0].field`), or the item is trashed", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (capability or scope named in `detail`), `csrf` or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 400, description = "`malformed-json`", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn put_autosave(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath((kind, id)): ApiPath<(String, Uuid)>,
    ApiJson(input): ApiJson<AutosaveInput>,
) -> Result<Json<AutosaveDto>, Problem> {
    let id = load(&api, &who.actor, &kind, id).await?.id;
    let saved = api
        .content
        .put_autosave(&who.actor, id, input.into())
        .await?;
    Ok(Json(saved.into()))
}

#[utoipa::path(
    delete,
    path = "/content/{type}/{id}/autosave",
    tag = "content",
    operation_id = "delete_autosave",
    security(("session" = []), ("bearer" = [])),
    params(("type" = String, Path), ("id" = Uuid, Path)),
    responses(
        (status = 204, description = "The caller's autosave is gone (also when there was none)"),
        (status = 404, description = "`not-found`: no such item that you may read", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (capability or scope named in `detail`), `csrf` or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn delete_autosave(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath((kind, id)): ApiPath<(String, Uuid)>,
) -> Result<StatusCode, Problem> {
    let id = load(&api, &who.actor, &kind, id).await?.id;
    api.content.delete_autosave(&who.actor, id).await?;
    Ok(StatusCode::NO_CONTENT)
}
