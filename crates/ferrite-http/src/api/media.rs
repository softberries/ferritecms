//! `/media`: uploads and media items (07 §1; M2a-2b P3–P5, P7, P9, R1).
//!
//! Handlers translate; `MediaService` checks `upload_media` with `media:write` for
//! uploads, the content role rules with `media:write` for edits, trash and delete, and
//! `media:read` with per-item visibility for reads.
//!
//! * `POST /media` is `multipart/form-data` with the parts `file` (required, streamed
//!   into the media store by the node's [`Uploader`](crate::media::Uploader); its
//!   part's file name becomes `original_name`), `title` and `alt` (any order; an
//!   unknown or repeated part is `400`). The caller is authorized before any of the
//!   body is read; then the declared `Content-Length` is checked against
//!   `media.max_upload` + 1 MiB, and the file is cut at `media.max_upload` while it
//!   streams (F12; the route's own limit is the config ceiling,
//!   [`MEDIA_ROUTE_LIMIT`]). The type comes from the content (see
//!   [`crate::media`]). The item is published at once (P5): `201` with `Location`
//!   and `ETag`. An optional `Idempotency-Key` makes a retry replay the first `201`
//!   (see [`idempotency`](super::idempotency)).
//! * Writes to an item (`PATCH`, `DELETE`) need `If-Match: "<version>"` (`428`
//!   without it; a stale one is `412` with `current`). `DELETE` trashes;
//!   `DELETE ?force=true` deletes a trashed item for good. Both answer `204`.
//!   `POST /media/{id}/restore` (also `If-Match`) restores a trashed item as published
//!   (M2b P33) and answers `200` with the item.
//! * `GET /media` takes `q` (search over titles), `kind`, `author` and `trash`.
//! * A malformed id, an item of another type and one the caller may not read are
//!   `404`.

use std::sync::Arc;

use axum::{
    Json,
    extract::{OriginalUri, Request, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use ferrite_core::{
    Actor, CoreError, MediaItem, MediaPatch, MediaQuery, NewUpload, check_file_name,
};
use ferrite_domain::{ContentId, ContentOrder, MAX_PAGE, MediaKind, UserId};
use serde::Deserialize;
use serde_json::Value;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use super::{
    ApiState, Authed, BASE,
    content::DeleteParams,
    dto::media::{MediaDto, MediaList, MediaPatchInput, MediaUpload},
    extract::{
        ApiJson, ApiPath, ApiQuery, DEFAULT_LIMIT, IfMatch, check_limit, created, etag,
        invalid_cursor,
    },
    idempotency,
    multipart::{body_error, malformed, open, read_field},
    problem::{Problem, ProblemDocument},
};

/// The route-level body limit of `POST /media`: the largest `media.max_upload` (1 GiB)
/// plus 1 MiB for the small parts and the multipart framing (F12). The configured
/// limit is enforced by the handler.
pub const MEDIA_ROUTE_LIMIT: usize = ferrite_config::MAX_UPLOAD_CEILING as usize + FRAMING;

/// What a body may carry beyond the file: the small parts and the multipart framing.
const FRAMING: usize = 1024 * 1024;

/// Largest `title` part in bytes: 200 characters of up to 4 bytes.
const TITLE_PART: usize = ferrite_core::MAX_MEDIA_TITLE * 4;
/// Largest `alt` part in bytes: 1 000 characters of up to 4 bytes.
const ALT_PART: usize = ferrite_core::MAX_MEDIA_ALT * 4;

/// The kind of a media item, from its type.
#[derive(Debug, Clone, Copy, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum MediaKindParam {
    /// `image/…` (SVG included).
    Image,
    /// `video/…`.
    Video,
    /// `audio/…`.
    Audio,
    /// Every other type (PDF, office documents, archives, …).
    Document,
}

impl From<MediaKindParam> for MediaKind {
    fn from(k: MediaKindParam) -> Self {
        match k {
            MediaKindParam::Image => MediaKind::Image,
            MediaKindParam::Video => MediaKind::Video,
            MediaKindParam::Audio => MediaKind::Audio,
            MediaKindParam::Document => MediaKind::Document,
        }
    }
}

/// The query of `GET /media`.
#[derive(Debug, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct MediaListParams {
    /// Full-text search over the title (the original file name is not searched).
    /// Blank is no search. Not combinable with `author` or `trash=true` (`422`, field
    /// `q`); results come by relevance.
    pub q: Option<String>,
    /// Only items of this kind; combines with `q` and `trash`.
    #[param(inline)]
    pub kind: Option<MediaKindParam>,
    /// Only items uploaded by this user.
    pub author: Option<Uuid>,
    /// `true`: only trashed items; default `false` (every status but trash).
    #[serde(default)]
    pub trash: bool,
    /// 1..=200; default 50.
    pub limit: Option<u32>,
    /// `next_cursor` of the previous page of the same query (a search page's cursor
    /// only continues a search).
    pub cursor: Option<String>,
}

/// The items with their uploaders (one author lookup for all of them). Items reference
/// existing users (foreign keys), so a missing uploader is a `500`.
async fn dtos(
    api: &ApiState,
    actor: &Actor,
    items: Vec<MediaItem>,
) -> Result<Vec<MediaDto>, Problem> {
    let ids: Vec<UserId> = items.iter().map(|m| m.item.author_id).collect();
    let authors = api.media_service.authors(actor, &ids).await?;
    items
        .into_iter()
        .map(|m| {
            let author = authors.get(&m.item.author_id).cloned().ok_or_else(|| {
                tracing::error!(user = %m.item.author_id, "uploader not found");
                Problem::internal()
            })?;
            Ok(MediaDto::new(m, &api.media, author))
        })
        .collect()
}

async fn dto(api: &ApiState, actor: &Actor, m: MediaItem) -> Result<MediaDto, Problem> {
    dtos(api, actor, vec![m])
        .await?
        .pop()
        .ok_or_else(Problem::internal)
}

/// `status` with `ETag` and the item.
async fn respond(
    api: &ApiState,
    actor: &Actor,
    m: MediaItem,
    status: StatusCode,
) -> Result<Response, Problem> {
    let tag = etag(m.item.version);
    let body = dto(api, actor, m).await?;
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
    let current = match api.media_service.get(actor, id).await {
        Ok(m) => dto(api, actor, m).await,
        Err(e) => Err(Problem::from(e)),
    };
    match current {
        Ok(m) => match serde_json::to_value(m) {
            Ok(value) => problem.with_current(value),
            Err(e) => {
                tracing::error!(error = %e, "cannot serialize the current media item");
                problem
            }
        },
        // Gone or unreadable since: the 412 stays without `current`.
        Err(_) => problem,
    }
}

#[utoipa::path(
    get,
    path = "/media",
    tag = "media",
    operation_id = "list_media",
    security(("session" = []), ("bearer" = [])),
    params(MediaListParams),
    responses(
        (status = 200, description = "One page of media items (every status but trash, or with `trash=true` only trashed items), newest change first; with `q` by relevance (a search page can be shorter than `limit` while `next_cursor` is set)", body = MediaList),
        (status = 400, description = "`validation` (`limit`, `cursor`) or `malformed-query` (an unknown `kind`, a `trash` other than `true`/`false`)", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (needs `media:read`) or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "`validation`: `q` with `author` or `trash=true` (`errors[0].field` = `q`)", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn list_media(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiQuery(params): ApiQuery<MediaListParams>,
) -> Result<Json<MediaList>, Problem> {
    let limit = check_limit(params.limit, DEFAULT_LIMIT, MAX_PAGE)?;
    let query = MediaQuery {
        limit,
        cursor: params.cursor,
        author: params.author.map(UserId::from_uuid),
        q: params.q,
        kind: params.kind.map(MediaKind::from),
        trash: params.trash,
    };
    if let Some(c) = &query.cursor {
        // The store decodes it again; decoding here makes a bad one a 400 naming it.
        let valid = if query.search_text().is_some() {
            ferrite_store::rank_cursor(c).is_ok()
        } else {
            ferrite_store::content_cursor(ContentOrder::UpdatedDesc, c).is_ok()
        };
        if !valid {
            return Err(invalid_cursor());
        }
    }
    let page = api.media_service.list(&who.actor, query).await?;
    Ok(Json(MediaList {
        items: dtos(&api, &who.actor, page.items).await?,
        next_cursor: page.next_cursor,
    }))
}

#[utoipa::path(
    post,
    path = "/media",
    tag = "media",
    operation_id = "upload_media",
    security(("session" = []), ("bearer" = [])),
    params(("Idempotency-Key" = Option<String>, Header, nullable = false, description = "Optional; 1 to 255 visible ASCII characters (`0x21`-`0x7E`), scoped to the caller and kept 24 h. A retry with a finished key replays the first `201` (`Idempotent-Replayed: true`) without reading the body; the key must be reused with the same method and path (query included)")),
    request_body(content = MediaUpload, content_type = "multipart/form-data", description = "Parts `file` (required, at most `media.max_upload`), `title` (at most 200 characters) and `alt` (at most 1 000 characters), in any order; the body at most `media.max_upload` + 1 MiB"),
    responses(
        (status = 201, description = "Uploaded and published (`Location`, `ETag`); with a finished `Idempotency-Key`, the first response replayed (`Idempotent-Replayed: true`)", body = MediaDto),
        (status = 400, description = "`malformed-body`: an unknown, unnamed or repeated part, a malformed body, or no bytes for `media.upload_idle_timeout` (`errors[0].field` = `file`; the upload is aborted); `malformed-header`: `Idempotency-Key` sent twice, empty, over 255 bytes or not visible ASCII", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (`upload_media`, `media:write`), `csrf` or `password-reset-required`; checked before the body is read", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 409, description = "`conflict`: a request with this `Idempotency-Key` is in progress (`Retry-After: 5`)", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 413, description = "`too-large`: the declared body, the file (`errors[0].field` = `file`; an SVG at most 2 MiB) or a small part over its limit", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 415, description = "`unsupported-media-type`: not `multipart/form-data`, or a file of no accepted type (`errors[0].field` = `file`)", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "`validation`: no `file` part, a file name that is empty, over 255 bytes or holds control or format characters (checked before the file is read), an SVG that cannot be made safe (`file`), `title`/`alt` too long or not UTF-8; the `Idempotency-Key` was used for another method or path (`errors[0].field` = `Idempotency-Key`)", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 503, description = "`busy`: every upload slot of this node is taken (`Retry-After`)", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn upload_media(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    req: Request,
) -> Response {
    let actor = &who.actor;
    if let Err(p) = api.media_service.authorize_upload(actor) {
        return Problem::from(p).into_response();
    }
    // The key is claimed after the caller is authorized and before a byte of the body
    // is read (P10): a replay, `409` and `422` never read it.
    let key = match idempotency::parse_key(req.headers()) {
        Ok(key) => key,
        Err(p) => return p.into_response(),
    };
    let guard = match key {
        Some(key) => {
            let path = request_path(&req);
            match idempotency::begin(&api, actor.user, &key, req.method(), &path).await {
                Ok(guard) => Some(guard),
                Err(res) => return *res,
            }
        }
        None => None,
    };
    let result = upload(&api, actor, req).await;
    match (guard, result) {
        (Some(guard), Ok((status, headers, body))) => {
            guard.finish(status, &headers, &body).await;
            (status, headers, Json(body)).into_response()
        }
        (Some(guard), Err(p)) => {
            guard.release().await;
            p.into_response()
        }
        (None, Ok((status, headers, body))) => (status, headers, Json(body)).into_response(),
        (None, Err(p)) => p.into_response(),
    }
}

/// The request's path with its query, as the client sent it: axum's [`OriginalUri`]
/// (the nested router hands the handler only the part after [`BASE`]).
fn request_path(req: &Request) -> String {
    let uri = req
        .extensions()
        .get::<OriginalUri>()
        .map_or(req.uri(), |o| &o.0);
    uri.path_and_query()
        .map_or_else(|| uri.path().to_owned(), |pq| pq.as_str().to_owned())
}

/// The upload itself: reads the parts, stages the file and records the item; the
/// `201`'s status, headers and body.
async fn upload(
    api: &ApiState,
    actor: &Actor,
    req: Request,
) -> Result<(StatusCode, HeaderMap, Value), Problem> {
    let max_body = usize::try_from(api.max_upload)
        .unwrap_or(usize::MAX)
        .saturating_add(FRAMING);
    let mut mp = open(req, max_body).await?;
    let mut staged = None;
    let mut title = None;
    let mut alt = None;
    while let Some(mut field) = mp.next_field().await.map_err(body_error)? {
        // Echoed in a problem: cut to 64 characters.
        let name: String = field.name().unwrap_or_default().chars().take(64).collect();
        let repeated = || malformed(format!("part {name:?} is sent more than once"));
        match name.as_str() {
            "file" => {
                if staged.is_some() {
                    return Err(repeated());
                }
                // Core's own rule (length, `.`/`..`, control and format characters),
                // before a byte of the file is stored; `record_upload` checks it again.
                check_file_name(field.file_name().unwrap_or_default())?;
                staged = Some(api.uploader.stage(field).await?);
            }
            "title" | "alt" => {
                let (slot, max) = if name == "title" {
                    (&mut title, TITLE_PART)
                } else {
                    (&mut alt, ALT_PART)
                };
                if slot.is_some() {
                    return Err(repeated());
                }
                let bytes = read_field(&mut field, &name, max).await?;
                let text = String::from_utf8(bytes.to_vec()).map_err(|_| {
                    Problem::validation(format!("part {name:?} is not UTF-8"))
                        .with_error(Some(&name), "must be UTF-8")
                })?;
                *slot = Some(text);
            }
            "" => return Err(malformed("a part has no name".to_owned())),
            _ => return Err(malformed(format!("unknown part {name:?}"))),
        }
    }
    let Some(staged) = staged else {
        return Err(
            Problem::validation("part \"file\" is required").with_error(Some("file"), "required")
        );
    };
    let item = api
        .media_service
        .record_upload(
            actor,
            NewUpload {
                artifact: staged.artifact.clone(),
                mime: staged.mime.clone(),
                bytes: staged.bytes,
                original_name: staged.original_name.clone(),
                title,
                alt: alt.unwrap_or_default(),
                sanitized: staged.sanitized,
                raster: staged.raster,
            },
        )
        .await?;
    // The slot is released once the upload is recorded.
    drop(staged);
    let location = format!("{BASE}/media/{}", item.item.id);
    let (status, headers) = created(&location, etag(item.item.version));
    let body = serde_json::to_value(dto(api, actor, item).await?).map_err(|e| {
        tracing::error!(error = %e, "cannot serialize an uploaded media item");
        Problem::internal()
    })?;
    Ok((status, headers, body))
}

#[utoipa::path(
    get,
    path = "/media/{id}",
    tag = "media",
    operation_id = "get_media",
    security(("session" = []), ("bearer" = [])),
    params(("id" = Uuid, Path)),
    responses(
        (status = 200, description = "The media item (`ETag`)", body = MediaDto),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (needs `media:read`) or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn get_media(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Response, Problem> {
    let m = api
        .media_service
        .get(&who.actor, ContentId::from_uuid(id))
        .await?;
    respond(&api, &who.actor, m, StatusCode::OK).await
}

#[utoipa::path(
    patch,
    path = "/media/{id}",
    tag = "media",
    operation_id = "update_media",
    security(("session" = []), ("bearer" = [])),
    params(("id" = Uuid, Path), ("If-Match" = String, Header, description = "`\"<version>\"`")),
    request_body = MediaPatchInput,
    responses(
        (status = 200, description = "Saved (`ETag`)", body = MediaDto),
        (status = 400, description = "`malformed-json` or `malformed-header` (`If-Match`)", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (capability or `media:write`), `csrf` or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 412, description = "`precondition-failed`: stale version (`current`)", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "`validation`: `title` or `alt` too long or with control characters, or the item is trashed", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 428, description = "`precondition-required`: no `If-Match`", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn update_media(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath(id): ApiPath<Uuid>,
    IfMatch(version): IfMatch,
    ApiJson(patch): ApiJson<MediaPatchInput>,
) -> Result<Response, Problem> {
    let actor = &who.actor;
    let id = ContentId::from_uuid(id);
    let patch = MediaPatch {
        title: patch.title,
        alt: patch.alt,
    };
    match api.media_service.update(actor, id, version, patch).await {
        Ok(m) => respond(&api, actor, m, StatusCode::OK).await,
        Err(e) => Err(write_problem(&api, actor, id, e).await),
    }
}

#[utoipa::path(
    delete,
    path = "/media/{id}",
    tag = "media",
    operation_id = "delete_media",
    security(("session" = []), ("bearer" = [])),
    params(("id" = Uuid, Path), ("If-Match" = String, Header, description = "`\"<version>\"`"), DeleteParams),
    responses(
        (status = 204, description = "Trashed, or (`force=true`, trashed items only) deleted for good; the stored file stays until garbage collection"),
        (status = 400, description = "`malformed-header` (`If-Match`) or `malformed-query`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (capability or `media:write`), `csrf` or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 412, description = "`precondition-failed`: stale version (`current`)", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "`validation`: not trashed (`force`) or already trashed", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 428, description = "`precondition-required`: no `If-Match`", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn delete_media(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath(id): ApiPath<Uuid>,
    ApiQuery(params): ApiQuery<DeleteParams>,
    IfMatch(version): IfMatch,
) -> Result<Response, Problem> {
    let actor = &who.actor;
    let id = ContentId::from_uuid(id);
    let result = if params.force {
        api.media_service.delete(actor, id, version).await
    } else {
        api.media_service.trash(actor, id, version).await.map(drop)
    };
    match result {
        Ok(()) => Ok(StatusCode::NO_CONTENT.into_response()),
        Err(e) => Err(write_problem(&api, actor, id, e).await),
    }
}

#[utoipa::path(
    post,
    path = "/media/{id}/restore",
    tag = "media",
    operation_id = "restore_media",
    security(("session" = []), ("bearer" = [])),
    params(("id" = Uuid, Path), ("If-Match" = String, Header, description = "`\"<version>\"`")),
    responses(
        (status = 200, description = "Restored from trash as published, its `published_at` kept, so its files are served again (`ETag`)", body = MediaDto),
        (status = 400, description = "`malformed-header` (`If-Match`)", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (capability or `media:write`; contributors never restore), `csrf` or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 412, description = "`precondition-failed`: stale version (`current`)", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "`validation`: not trashed", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 428, description = "`precondition-required`: no `If-Match`", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn restore_media(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath(id): ApiPath<Uuid>,
    IfMatch(version): IfMatch,
) -> Result<Response, Problem> {
    let actor = &who.actor;
    let id = ContentId::from_uuid(id);
    match api.media_service.restore(actor, id, version).await {
        Ok(m) => respond(&api, actor, m, StatusCode::OK).await,
        Err(e) => Err(write_problem(&api, actor, id, e).await),
    }
}
