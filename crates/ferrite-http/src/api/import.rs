//! `/import`: WordPress imports from an upload (M3-2 P23, P25, P26-1).
//!
//! Handlers translate; `ImportService` checks `import` with `import:write` to start and
//! `import:read` to read, and keeps runs' fencing tokens to itself.
//!
//! * `POST /import/wordpress` is `multipart/form-data` with the parts `wxr` (required),
//!   `uploads` (optional, a zip of `wp-content/uploads`) and `options` (optional JSON,
//!   at most [`MAX_OPTIONS_PART`]), in any order. The caller is authorized and an
//!   optional `Idempotency-Key` claimed (as for `POST /media`, see [`idempotency`])
//!   before any of the body is read; a declared `Content-Length` over
//!   [`IMPORT_ROUTE_LIMIT`] is `413` at once. `wxr` and `uploads` are streamed into the
//!   media store as they arrive, cut at [`IMPORT_MAX_WXR`] and [`IMPORT_MAX_ARCHIVE`]
//!   (`413`), and never sniffed: a WXR sent as any type is accepted. Core then records
//!   them as `import` artifacts and queues the `import.wordpress` job in one
//!   transaction: `202` with the job and `Location: /api/v1/import/jobs/{job_id}`. While
//!   another import is queued or running that is `409 conflict`. When the request fails
//!   after a file was stored, the files this request created (and no row records) are
//!   deleted again; a file that was already stored stays.
//! * `GET /import/jobs/{job_id}` is what the admin polls; `GET /import/runs` lists runs
//!   newest first; `GET /import/runs/{id}` reads one, and `…/report.json` and
//!   `…/report.html` its report. The HTML is sent sandboxed (no script, no requests)
//!   and every report as `Cache-Control: private, no-store`.

use std::sync::Arc;

use axum::{
    Json,
    extract::{OriginalUri, Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use bytes::Bytes;
use ferrite_core::{
    Actor,
    import::ReportFormat,
    jobs::{ImportJobOptions, ImportJobRequest},
};
use ferrite_domain::{Artifact, ArtifactKind, Cursor, ImportRunId, JobId, MAX_PAGE, Timestamp};
use ferrite_media::{ArtifactRef, MediaError, sanitize_filename};
use serde::Deserialize;
use serde_json::Value;
use utoipa::IntoParams;
use uuid::Uuid;

use super::{
    ApiState, Authed, BASE,
    dto::import::{
        ImportJobDto, ImportOptionsDto, ImportRunDto, ImportRunPageDto, ImportStartUpload,
    },
    extract::{ApiPath, ApiQuery, DEFAULT_LIMIT, check_limit, invalid_cursor},
    idempotency,
    multipart::{body_error, malformed, open, read_field},
    problem::{Problem, ProblemDocument},
};

/// The largest `wxr` part (P23): 512 MiB.
pub const IMPORT_MAX_WXR: u64 = 512 * 1024 * 1024;
/// The largest `uploads` part (P23): 4 GiB.
pub const IMPORT_MAX_ARCHIVE: u64 = 4 * 1024 * 1024 * 1024;
/// The largest `options` part: 4 KiB.
pub const MAX_OPTIONS_PART: usize = 4 * 1024;
/// What a body may carry beyond the two files: `options` and the multipart framing.
const FRAMING: u64 = 1024 * 1024;
/// The route-level body limit of `POST /import/wordpress`: both files plus 1 MiB.
pub const IMPORT_ROUTE_LIMIT: usize = (IMPORT_MAX_WXR + IMPORT_MAX_ARCHIVE + FRAMING) as usize;
/// The run name used when the `wxr` part has no usable file name.
const DEFAULT_SOURCE_NAME: &str = "export.xml";
/// The cursor tag of `GET /import/runs`.
const RUNS_CURSOR: &str = "import:runs";

/// The CSP of `report.html`: inline styles only, no script, no requests, sandboxed.
pub const REPORT_CSP: &str = "default-src 'none'; style-src 'unsafe-inline'; sandbox";
/// `Cache-Control` of both report files.
const REPORT_CACHE: &str = "private, no-store";

#[utoipa::path(
    post,
    path = "/import/wordpress",
    tag = "import",
    operation_id = "start_wordpress_import",
    security(("session" = []), ("bearer" = [])),
    params(("Idempotency-Key" = Option<String>, Header, nullable = false, description = "Optional; 1 to 255 visible ASCII characters (`0x21`-`0x7E`), scoped to the caller and kept 24 h. A retry with a finished key replays the first `202` (`Idempotent-Replayed: true`) without reading the body; the key must be reused with the same method and path")),
    request_body(content = ImportStartUpload, content_type = "multipart/form-data", description = "Parts `wxr` (required, at most 512 MiB), `uploads` (a zip, at most 4 GiB) and `options` (JSON, at most 4 KiB), in any order"),
    responses(
        (status = 202, description = "Queued (`Location`: the job); with a finished `Idempotency-Key`, the first response replayed (`Idempotent-Replayed: true`)", body = ImportJobDto),
        (status = 400, description = "`malformed-body`: an unknown, unnamed or repeated part, a malformed body or `options` part; `malformed-header`: a malformed `Idempotency-Key`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (`import`, `import:write`), `csrf` or `password-reset-required`; checked before the body is read", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 409, description = "`conflict`: another import is queued or running, or a request with this `Idempotency-Key` is in progress (`Retry-After: 5`)", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 413, description = "`too-large`: the declared body, a file (`errors[0].field` = `wxr` or `uploads`) or `options` over its limit", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 415, description = "`unsupported-media-type`: not `multipart/form-data`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "`validation`: no `wxr` part; the `Idempotency-Key` was used for another method or path (`errors[0].field` = `Idempotency-Key`)", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn start_wordpress_import(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    req: Request,
) -> Response {
    let actor = &who.actor;
    if let Err(e) = api.import.authorize_import(actor) {
        return Problem::from(e).into_response();
    }
    // As for media (P10): the key is claimed before a byte of the body is read.
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
    let result = start(&api, actor, req).await;
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

/// The request's path with its query, as the client sent it.
fn request_path(req: &Request) -> String {
    let uri = req
        .extensions()
        .get::<OriginalUri>()
        .map_or(req.uri(), |o| &o.0);
    uri.path_and_query()
        .map_or_else(|| uri.path().to_owned(), |pq| pq.as_str().to_owned())
}

/// The files a start stored, so a failure can remove the ones it created.
#[derive(Default)]
struct Stored {
    wxr: Option<(ArtifactRef, String)>,
    uploads: Option<ArtifactRef>,
}

impl Stored {
    /// Deletes the files this request created that no artifact row records (a row means
    /// another upload or a job holds the same bytes).
    async fn remove_created(&self, api: &ApiState) {
        let refs = self
            .wxr
            .iter()
            .map(|(r, _)| r)
            .chain(self.uploads.as_ref())
            .filter(|r| r.created);
        for r in refs {
            match api.store.read().artifacts().get(&r.hash).await {
                Ok(None) => {
                    if let Err(e) = api.media.delete(&r.hash).await {
                        tracing::warn!(error = %e, "cannot delete an unused import upload");
                    }
                }
                Ok(Some(_)) => {}
                Err(e) => tracing::warn!(error = %e, "cannot check an import upload's row"),
            }
        }
    }
}

/// The start itself: reads the parts, stores the files and queues the job; the `202`'s
/// status, headers and body. On a failure after a file was stored, the files it
/// created are removed.
async fn start(
    api: &ApiState,
    actor: &Actor,
    req: Request,
) -> Result<(StatusCode, HeaderMap, Value), Problem> {
    let mut stored = Stored::default();
    let result = read_and_queue(api, actor, req, &mut stored).await;
    if result.is_err() {
        stored.remove_created(api).await;
    }
    result
}

async fn read_and_queue(
    api: &ApiState,
    actor: &Actor,
    req: Request,
    stored: &mut Stored,
) -> Result<(StatusCode, HeaderMap, Value), Problem> {
    let mut mp = open(req, IMPORT_ROUTE_LIMIT).await?;
    let mut options: Option<ImportJobOptions> = None;
    while let Some(mut field) = mp.next_field().await.map_err(body_error)? {
        // Echoed in a problem: cut to 64 characters.
        let name: String = field.name().unwrap_or_default().chars().take(64).collect();
        let repeated = || malformed(format!("part {name:?} is sent more than once"));
        match name.as_str() {
            "wxr" => {
                if stored.wxr.is_some() {
                    return Err(repeated());
                }
                let file_name = field
                    .file_name()
                    .map(sanitize_filename)
                    .filter(|n| !n.is_empty())
                    .unwrap_or_else(|| DEFAULT_SOURCE_NAME.to_owned());
                let r = store_part(api, field, "wxr", IMPORT_MAX_WXR).await?;
                stored.wxr = Some((r, file_name));
            }
            "uploads" => {
                if stored.uploads.is_some() {
                    return Err(repeated());
                }
                stored.uploads = Some(store_part(api, field, "uploads", IMPORT_MAX_ARCHIVE).await?);
            }
            "options" => {
                if options.is_some() {
                    return Err(repeated());
                }
                let bytes = read_field(&mut field, "options", MAX_OPTIONS_PART).await?;
                let dto: ImportOptionsDto = serde_json::from_slice(&bytes)
                    .map_err(|e| malformed(format!("part \"options\" is not valid: {e}")))?;
                options = Some(dto.into());
            }
            "" => return Err(malformed("a part has no name".to_owned())),
            _ => return Err(malformed(format!("unknown part {name:?}"))),
        }
    }
    let Some((wxr, source_name)) = &stored.wxr else {
        return Err(
            Problem::validation("part \"wxr\" is required").with_error(Some("wxr"), "required")
        );
    };
    let request = ImportJobRequest {
        wxr: artifact(wxr, "application/xml"),
        uploads: stored
            .uploads
            .as_ref()
            .map(|u| artifact(u, "application/zip")),
        options: options.unwrap_or_default(),
        source_name: source_name.clone(),
    };
    let job = api.import.enqueue(actor, request).await?;
    let view = api.import.job_status(actor, job).await?;
    let location = format!("{BASE}/import/jobs/{job}");
    let mut headers = HeaderMap::new();
    match HeaderValue::from_str(&location) {
        Ok(v) => {
            headers.insert(header::LOCATION, v);
        }
        Err(_) => tracing::warn!(location, "Location is not a valid header value"),
    }
    let body = serde_json::to_value(ImportJobDto::from(view)).map_err(|e| {
        tracing::error!(error = %e, "cannot serialize an import job");
        Problem::internal()
    })?;
    Ok((StatusCode::ACCEPTED, headers, body))
}

/// The artifact row of a stored input (kind `import`; the type is what the part should
/// be, since inputs are never sniffed).
fn artifact(r: &ArtifactRef, mime: &str) -> Artifact {
    Artifact {
        hash: r.hash.clone(),
        kind: ArtifactKind::Import,
        bytes: i64::try_from(r.bytes).unwrap_or(i64::MAX),
        mime: mime.to_owned(),
        created_at: Timestamp::now(),
    }
}

/// Streams part `field` (named `name`) into the media store, cut at `max` bytes.
async fn store_part(
    api: &ApiState,
    field: axum::extract::multipart::Field<'_>,
    name: &str,
    max: u64,
) -> Result<ArtifactRef, Problem> {
    // The store sees an I/O error and aborts; the body's own error is kept for the
    // answer.
    let mut failure = None;
    let stream =
        futures::stream::unfold((field, &mut failure), |(mut field, failure)| async move {
            match field.chunk().await {
                Ok(Some(chunk)) => Some((Ok::<Bytes, std::io::Error>(chunk), (field, failure))),
                Ok(None) => None,
                Err(e) => {
                    *failure = Some(e);
                    let io = std::io::Error::other("the request body failed");
                    Some((Err(io), (field, failure)))
                }
            }
        });
    let result = api.media.put_stream(Box::pin(stream), max).await;
    match (result, failure) {
        (Ok(r), _) => Ok(r),
        (Err(_), Some(body)) => Err(body_error(body)),
        (Err(MediaError::TooLarge { limit }), None) => Err(Problem::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "too-large",
            "Request too large",
            format!("part {name:?} is larger than {limit} bytes"),
        )
        .with_error(Some(name), format!("at most {limit} bytes"))),
        (Err(e), None) => {
            tracing::error!(error = %e, part = name, "storing an import upload failed");
            Err(Problem::internal())
        }
    }
}

#[utoipa::path(
    get,
    path = "/import/jobs/{job_id}",
    tag = "import",
    operation_id = "get_import_job",
    security(("session" = []), ("bearer" = [])),
    params(("job_id" = Uuid, Path)),
    responses(
        (status = 200, description = "The import job and, once it started one, its run", body = ImportJobDto),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (`import`, `import:read`) or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`: no such import job (jobs of other kinds included) or a malformed id", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn get_import_job(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<ImportJobDto>, Problem> {
    let view = api
        .import
        .job_status(&who.actor, JobId::from_uuid(id))
        .await?;
    Ok(Json(view.into()))
}

/// The query of `GET /import/runs`.
#[derive(Debug, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ImportRunParams {
    /// 1..=200; default 50.
    pub limit: Option<u32>,
    /// `next_cursor` of the previous page.
    pub cursor: Option<String>,
}

#[utoipa::path(
    get,
    path = "/import/runs",
    tag = "import",
    operation_id = "list_import_runs",
    security(("session" = []), ("bearer" = [])),
    params(ImportRunParams),
    responses(
        (status = 200, description = "One page of runs, newest first (CLI runs included)", body = ImportRunPageDto),
        (status = 400, description = "`validation` (`limit`, `cursor`) or `malformed-query`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (`import`, `import:read`) or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn list_import_runs(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiQuery(params): ApiQuery<ImportRunParams>,
) -> Result<Json<ImportRunPageDto>, Problem> {
    let limit = check_limit(params.limit, DEFAULT_LIMIT, MAX_PAGE)?;
    let cursor = params
        .cursor
        .as_deref()
        .map(|c| {
            let (key, id) = Cursor::decode(c, RUNS_CURSOR).map_err(|_| invalid_cursor())?;
            let at = key
                .as_i64()
                .and_then(|m| Timestamp::from_micros(m).ok())
                .ok_or_else(invalid_cursor)?;
            Ok::<_, Problem>((at, id))
        })
        .transpose()?;
    let runs = api.import.list_runs(&who.actor, limit, cursor).await?;
    let next_cursor = (runs.len() == limit as usize)
        .then(|| runs.last())
        .flatten()
        .map(|r| {
            Cursor::encode(
                RUNS_CURSOR,
                &Value::from(r.started_at.as_micros()),
                r.id.as_uuid(),
            )
        });
    Ok(Json(ImportRunPageDto {
        items: runs.into_iter().map(ImportRunDto::from).collect(),
        next_cursor,
    }))
}

#[utoipa::path(
    get,
    path = "/import/runs/{id}",
    tag = "import",
    operation_id = "get_import_run",
    security(("session" = []), ("bearer" = [])),
    params(("id" = Uuid, Path)),
    responses(
        (status = 200, description = "The run", body = ImportRunDto),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (`import`, `import:read`) or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found` (also a malformed id)", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn get_import_run(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<ImportRunDto>, Problem> {
    let run = api
        .import
        .get_run(&who.actor, ImportRunId::from_uuid(id))
        .await?;
    Ok(Json(run.into()))
}

#[utoipa::path(
    get,
    path = "/import/runs/{id}/report.json",
    tag = "import",
    operation_id = "get_import_report_json",
    security(("session" = []), ("bearer" = [])),
    params(("id" = Uuid, Path)),
    responses(
        (status = 200, description = "The run's report (`Cache-Control: private, no-store`)", body = Object, content_type = "application/json"),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (`import`, `import:read`) or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`: no such run (or a malformed id), or it has no report yet", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn get_import_report_json(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Response, Problem> {
    let body = api
        .import
        .report(&who.actor, ImportRunId::from_uuid(id), ReportFormat::Json)
        .await?;
    Ok((
        [
            (header::CONTENT_TYPE, "application/json"),
            (header::CACHE_CONTROL, REPORT_CACHE),
        ],
        body,
    )
        .into_response())
}

#[utoipa::path(
    get,
    path = "/import/runs/{id}/report.html",
    tag = "import",
    operation_id = "get_import_report_html",
    security(("session" = []), ("bearer" = [])),
    params(("id" = Uuid, Path)),
    responses(
        (status = 200, description = "The run's report as a page, sandboxed (`Content-Security-Policy: default-src 'none'; style-src 'unsafe-inline'; sandbox`, `X-Content-Type-Options: nosniff`, `Cache-Control: private, no-store`)", body = String, content_type = "text/html"),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (`import`, `import:read`) or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`: no such run (or a malformed id), or it has no report yet", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn get_import_report_html(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Response, Problem> {
    let body = api
        .import
        .report(&who.actor, ImportRunId::from_uuid(id), ReportFormat::Html)
        .await?;
    Ok((
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::CONTENT_SECURITY_POLICY, REPORT_CSP),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            (header::CACHE_CONTROL, REPORT_CACHE),
        ],
        body,
    )
        .into_response())
}
