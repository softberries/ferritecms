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
//!   and `ETag`.
//! * Writes to an item (`PATCH`, `DELETE`) need `If-Match: "<version>"` (`428`
//!   without it; a stale one is `412` with `current`). `DELETE` trashes;
//!   `DELETE ?force=true` deletes a trashed item for good. Both answer `204`.
//! * A malformed id, an item of another type and one the caller may not read are
//!   `404`.

use std::sync::Arc;

use axum::{
    Json,
    extract::{Request, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use ferrite_core::{
    Actor, CoreError, MediaItem, MediaPatch, MediaQuery, NewUpload, check_file_name,
};
use ferrite_domain::{ContentId, ContentOrder, MAX_PAGE, UserId};
use serde::Deserialize;
use utoipa::IntoParams;
use uuid::Uuid;

use super::{
    ApiState, Authed, BASE,
    content::DeleteParams,
    dto::media::{MediaDto, MediaList, MediaPatchInput, MediaUpload},
    extract::{
        ApiJson, ApiPath, ApiQuery, DEFAULT_LIMIT, IfMatch, check_limit, created, etag,
        invalid_cursor,
    },
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

/// The query of `GET /media`.
#[derive(Debug, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct MediaListParams {
    /// 1..=200; default 50.
    pub limit: Option<u32>,
    /// `next_cursor` of the previous page.
    pub cursor: Option<String>,
    /// Only items uploaded by this user.
    pub author: Option<Uuid>,
}

fn dto(api: &ApiState, m: MediaItem) -> MediaDto {
    MediaDto::new(m, &api.media)
}

/// `status` with `ETag` and the item.
fn respond(api: &ApiState, m: MediaItem, status: StatusCode) -> Response {
    let tag = etag(m.item.version);
    (status, [(header::ETAG, tag)], Json(dto(api, m))).into_response()
}

/// The problem of a failed write to item `id`: a version conflict is `412` with the
/// item as stored now in `current` (when it can still be read).
async fn write_problem(api: &ApiState, actor: &Actor, id: ContentId, e: CoreError) -> Problem {
    let conflict = matches!(e, CoreError::Conflict { .. });
    let problem = Problem::from(e);
    if !conflict {
        return problem;
    }
    match api.media_service.get(actor, id).await {
        Ok(m) => match serde_json::to_value(dto(api, m)) {
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
        (status = 200, description = "One page of media items (every status but trash), newest change first", body = MediaList),
        (status = 400, description = "`validation` (`limit`, `cursor`) or `malformed-query`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (needs `media:read`) or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn list_media(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiQuery(params): ApiQuery<MediaListParams>,
) -> Result<Json<MediaList>, Problem> {
    let limit = check_limit(params.limit, DEFAULT_LIMIT, MAX_PAGE)?;
    if let Some(c) = &params.cursor
        && ferrite_store::content_cursor(ContentOrder::UpdatedDesc, c).is_err()
    {
        return Err(invalid_cursor());
    }
    let page = api
        .media_service
        .list(
            &who.actor,
            MediaQuery {
                limit,
                cursor: params.cursor,
                author: params.author.map(UserId::from_uuid),
            },
        )
        .await?;
    Ok(Json(MediaList {
        items: page.items.into_iter().map(|m| dto(&api, m)).collect(),
        next_cursor: page.next_cursor,
    }))
}

#[utoipa::path(
    post,
    path = "/media",
    tag = "media",
    operation_id = "upload_media",
    security(("session" = []), ("bearer" = [])),
    request_body(content = MediaUpload, content_type = "multipart/form-data", description = "Parts `file` (required, at most `media.max_upload`), `title` (at most 200 characters) and `alt` (at most 1 000 characters), in any order; the body at most `media.max_upload` + 1 MiB"),
    responses(
        (status = 201, description = "Uploaded and published (`Location`, `ETag`)", body = MediaDto),
        (status = 400, description = "`malformed-body`: an unknown, unnamed or repeated part, a malformed body, or no bytes for `media.upload_idle_timeout` (`errors[0].field` = `file`; the upload is aborted)", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (`upload_media`, `media:write`), `csrf` or `password-reset-required`; checked before the body is read", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 413, description = "`too-large`: the declared body, the file (`errors[0].field` = `file`; an SVG at most 2 MiB) or a small part over its limit", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 415, description = "`unsupported-media-type`: not `multipart/form-data`, or a file of no accepted type (`errors[0].field` = `file`)", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "`validation`: no `file` part, a file name that is empty, over 255 bytes or holds control or format characters (checked before the file is read), an SVG that cannot be made safe (`file`), `title`/`alt` too long or not UTF-8", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 503, description = "`busy`: every upload slot of this node is taken (`Retry-After`)", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn upload_media(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    req: Request,
) -> Result<Response, Problem> {
    let actor = &who.actor;
    api.media_service.authorize_upload(actor)?;
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
    Ok((status, headers, Json(dto(&api, item))).into_response())
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
    Ok(respond(&api, m, StatusCode::OK))
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
        Ok(m) => Ok(respond(&api, m, StatusCode::OK)),
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
