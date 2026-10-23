//! `/redirects`, with CSV import and export (07 §1, P29).
//!
//! Handlers only translate: `RedirectService` checks `manage_redirects` with
//! `redirects:write` for writes and `manage_redirects` with `redirects:read` for reads
//! (list, get, export), the source/target/code rules and loops and chains, and emits
//! `redirect:<source>` keys. No `If-Match` (P11): last write wins.
//!
//! * `POST /redirects` creates the redirect for `source`, or replaces the target and
//!   code of the one that has it; `201` with `Location` either way. A `source` whose
//!   trailing-slash variant another redirect holds is `422` (`source`; R26).
//! * `PATCH /redirects/{id}` merges the body into the redirect as read now; a new
//!   `source` another redirect holds is `422` (`errors[0].field` = `source`).
//! * `POST /redirects/import` takes `text/csv` of at most [`MAX_CSV_BODY`] bytes and
//!   [`MAX_IMPORT_ROWS`] rows: columns `source,target,code`, an optional header row
//!   `source,target,code`. All rows are checked first and nothing is written if any
//!   fails: `422 validation` with one `errors` entry per bad line,
//!   `{ field: "line <n>", detail }` (at most [`MAX_LINE_ERRORS`] entries).
//! * `GET /redirects/export` is every redirect as `text/csv` (`source,target,code`
//!   with a header row, by source), the format the import reads (see
//!   [`csv`] for quoting and the formula guard).

use std::sync::Arc;

use axum::{
    Json,
    body::Body,
    extract::State,
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use ferrite_core::{CoreError, MAX_IMPORT_ROWS};
use ferrite_domain::{MAX_PAGE, RedirectId};
use ferrite_store::REDIRECT_CURSOR_TAG;
use uuid::Uuid;

use super::{
    ApiState, Authed, BASE,
    csv::{self, CsvRecord},
    dto::site::{ImportReportDto, RedirectDto, RedirectInput, RedirectList, RedirectPatch},
    extract::{ApiJson, ApiPath, ApiQuery, PageParams, created_at, invalid_cursor},
    list,
    problem::{Problem, ProblemDocument},
};

/// An import row: source, target, code.
type Row = (String, String, u16);
/// A problem with an input line: its number and what is wrong.
type LineError = (usize, String);

/// Largest CSV import body (1 MiB).
pub const MAX_CSV_BODY: usize = 1024 * 1024;
/// Most `errors` entries a failed import lists.
pub const MAX_LINE_ERRORS: usize = 100;
/// The CSV columns, and the optional header row.
const HEADER: [&str; 3] = ["source", "target", "code"];

#[utoipa::path(
    get,
    path = "/redirects",
    tag = "redirects",
    operation_id = "list_redirects",
    security(("session" = []), ("bearer" = [])),
    params(PageParams),
    responses(
        (status = 200, description = "One page of redirects, by source", body = RedirectList),
        (status = 400, description = "`validation`: `limit` or `cursor`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (`manage_redirects`, `redirects:read`) or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn list_redirects(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiQuery(page): ApiQuery<PageParams>,
) -> Result<Json<RedirectList>, Problem> {
    let limit = page.limit()?;
    if let Some(c) = &page.cursor
        && ferrite_store::text_cursor(REDIRECT_CURSOR_TAG, c).is_err()
    {
        return Err(invalid_cursor());
    }
    let found = api
        .redirects
        .list(&who.actor, limit, page.cursor.as_deref())
        .await?;
    let (items, next_cursor) = list::items(found, RedirectDto::from);
    Ok(Json(RedirectList { items, next_cursor }))
}

#[utoipa::path(
    post,
    path = "/redirects",
    tag = "redirects",
    operation_id = "create_redirect",
    security(("session" = []), ("bearer" = [])),
    request_body = RedirectInput,
    responses(
        (status = 201, description = "Created, or the redirect with this source changed (`Location`)", body = RedirectDto),
        (status = 400, description = "`malformed-json`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (`manage_redirects`, `redirects:write`), `csrf` or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "`validation` (`errors[0].field`: `source` (also a path the site serves itself), `target` (also loops and chains of more than 10) or `code`)", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn create_redirect(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiJson(input): ApiJson<RedirectInput>,
) -> Result<Response, Problem> {
    let saved = api
        .redirects
        .upsert(&who.actor, &input.source, &input.target, input.code)
        .await?;
    let (status, headers) = created_at(&format!("{BASE}/redirects/{}", saved.id));
    Ok((status, headers, Json(RedirectDto::from(saved))).into_response())
}

#[utoipa::path(
    get,
    path = "/redirects/{id}",
    tag = "redirects",
    operation_id = "get_redirect",
    security(("session" = []), ("bearer" = [])),
    params(("id" = Uuid, Path)),
    responses(
        (status = 200, description = "The redirect", body = RedirectDto),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (`manage_redirects`, `redirects:read`) or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn get_redirect(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<Json<RedirectDto>, Problem> {
    let r = api
        .redirects
        .get(&who.actor, RedirectId::from_uuid(id))
        .await?;
    Ok(Json(r.into()))
}

#[utoipa::path(
    patch,
    path = "/redirects/{id}",
    tag = "redirects",
    operation_id = "update_redirect",
    security(("session" = []), ("bearer" = [])),
    params(("id" = Uuid, Path)),
    request_body = RedirectPatch,
    responses(
        (status = 200, description = "Saved", body = RedirectDto),
        (status = 400, description = "`malformed-json`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (`manage_redirects`, `redirects:write`), `csrf` or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 404, description = "`not-found`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "`validation` (`errors[0].field`: `source` (also taken by another redirect), `target` or `code`)", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn update_redirect(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(patch): ApiJson<RedirectPatch>,
) -> Result<Json<RedirectDto>, Problem> {
    let actor = &who.actor;
    let current = api.redirects.get(actor, RedirectId::from_uuid(id)).await?;
    let saved = api
        .redirects
        .update(
            actor,
            current.id,
            patch.source.as_deref().unwrap_or(&current.source_path),
            patch.target.as_deref().unwrap_or(&current.target),
            patch.code.unwrap_or(current.code),
        )
        .await?;
    Ok(Json(saved.into()))
}

#[utoipa::path(
    delete,
    path = "/redirects/{id}",
    tag = "redirects",
    operation_id = "delete_redirect",
    security(("session" = []), ("bearer" = [])),
    params(("id" = Uuid, Path)),
    responses(
        (status = 204, description = "Deleted (also when there was none)"),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (`manage_redirects`, `redirects:write`), `csrf` or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn delete_redirect(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    ApiPath(id): ApiPath<Uuid>,
) -> Result<StatusCode, Problem> {
    api.redirects
        .delete(&who.actor, RedirectId::from_uuid(id))
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post,
    path = "/redirects/import",
    tag = "redirects",
    operation_id = "import_redirects",
    security(("session" = []), ("bearer" = [])),
    request_body(content = String, content_type = "text/csv", description = "`source,target,code` rows (RFC 4180), optional header row; at most 1 MiB and 10 000 rows"),
    responses(
        (status = 200, description = "Every row imported", body = ImportReportDto),
        (status = 400, description = "`malformed-body`: not UTF-8", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (`manage_redirects`, `redirects:write`), `csrf` or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 413, description = "`too-large`: more than 1 MiB", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 415, description = "`unsupported-media-type`: not `text/csv`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 422, description = "`validation`: nothing imported; `errors[]` = `{ field: \"line <n>\", detail }` per bad line, or more than 10 000 rows", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn import_redirects(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
    headers: HeaderMap,
    body: Body,
) -> Result<Json<ImportReportDto>, Problem> {
    if !is_csv(&headers) {
        return Err(Problem::new(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "unsupported-media-type",
            "Unsupported media type",
            "the body must be text/csv",
        ));
    }
    let bytes = axum::body::to_bytes(body, MAX_CSV_BODY)
        .await
        .map_err(|_| {
            Problem::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                "too-large",
                "Request too large",
                format!("the body is larger than {MAX_CSV_BODY} bytes"),
            )
        })?;
    let text = std::str::from_utf8(&bytes).map_err(|e| {
        Problem::new(
            StatusCode::BAD_REQUEST,
            "malformed-body",
            "Malformed request body",
            format!("the body is not UTF-8: {e}"),
        )
    })?;
    let mut records = csv::parse(text)
        .map_err(|e| line_errors("the CSV cannot be read", vec![(e.line, e.detail)]))?;
    if records.first().is_some_and(is_header) {
        records.remove(0);
    }
    if records.len() > MAX_IMPORT_ROWS {
        return Err(Problem::validation(format!(
            "an import takes at most {MAX_IMPORT_ROWS} rows"
        )));
    }
    let (rows, errors) = to_rows(&records);
    if !errors.is_empty() {
        return Err(line_errors(
            "rows are invalid; nothing was imported",
            errors,
        ));
    }
    match api.redirects.import(&who.actor, &rows).await {
        Ok(report) => Ok(Json(report.into())),
        Err(CoreError::InvalidRows(bad)) => Err(line_errors(
            "rows are invalid; nothing was imported",
            bad.into_iter()
                .map(|e| (records.get(e.row).map_or(0, |r| r.line), e.message))
                .collect(),
        )),
        Err(e) => Err(e.into()),
    }
}

#[utoipa::path(
    get,
    path = "/redirects/export",
    tag = "redirects",
    operation_id = "export_redirects",
    security(("session" = []), ("bearer" = [])),
    responses(
        (status = 200, description = "Every redirect as `source,target,code` with a header row, by source", body = String, content_type = "text/csv"),
        (status = 401, description = "`unauthenticated`", body = ProblemDocument, content_type = "application/problem+json"),
        (status = 403, description = "`forbidden` (`manage_redirects`, `redirects:read`) or `password-reset-required`", body = ProblemDocument, content_type = "application/problem+json"),
    ),
)]
pub async fn export_redirects(
    State(api): State<Arc<ApiState>>,
    Authed(who): Authed,
) -> Result<Response, Problem> {
    let mut out = csv::write_row(&HEADER);
    let mut cursor: Option<String> = None;
    loop {
        let page = api
            .redirects
            .list(&who.actor, MAX_PAGE, cursor.as_deref())
            .await?;
        for r in &page.items {
            out.push_str(&csv::write_row(&[
                &r.source_path,
                &r.target,
                &r.code.to_string(),
            ]));
        }
        match page.next_cursor {
            Some(c) => cursor = Some(c),
            None => break,
        }
    }
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/csv; charset=utf-8"),
    );
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_static("attachment; filename=\"redirects.csv\""),
    );
    Ok((StatusCode::OK, headers, out).into_response())
}

/// `Content-Type` is `text/csv` (any parameters, any case).
fn is_csv(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .is_some_and(|essence| essence.trim().eq_ignore_ascii_case("text/csv"))
}

/// The record is the header row `source,target,code` (any case, spaces trimmed).
fn is_header(r: &CsvRecord) -> bool {
    r.fields.len() == HEADER.len()
        && r.fields
            .iter()
            .zip(HEADER)
            .all(|(f, h)| f.trim().eq_ignore_ascii_case(h))
}

/// The records as `(source, target, code)`, and `(line, detail)` for each record that
/// does not have three fields or whose code is not a number.
fn to_rows(records: &[CsvRecord]) -> (Vec<Row>, Vec<LineError>) {
    let mut rows = Vec::with_capacity(records.len());
    let mut errors = Vec::new();
    for r in records {
        let [source, target, code] = r.fields.as_slice() else {
            errors.push((
                r.line,
                format!(
                    "expected 3 fields (source,target,code), found {}",
                    r.fields.len()
                ),
            ));
            continue;
        };
        match code.trim().parse::<u16>() {
            Ok(code) => rows.push((source.clone(), target.clone(), code)),
            Err(_) => errors.push((r.line, format!("code {code:?} is not a number"))),
        }
    }
    (rows, errors)
}

/// `422 validation` with `errors[] = { field: "line <n>", detail }` (at most
/// [`MAX_LINE_ERRORS`], by line).
fn line_errors(detail: &str, mut errors: Vec<LineError>) -> Problem {
    errors.sort_by_key(|(line, _)| *line);
    let total = errors.len();
    let mut p = Problem::validation(if total > MAX_LINE_ERRORS {
        format!("{detail} ({total} errors, the first {MAX_LINE_ERRORS} listed)")
    } else {
        detail.to_owned()
    });
    for (line, message) in errors.into_iter().take(MAX_LINE_ERRORS) {
        p = p.with_error(Some(&format!("line {line}")), message);
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(line: usize, fields: &[&str]) -> CsvRecord {
        CsvRecord {
            line,
            fields: fields.iter().map(|f| (*f).to_owned()).collect(),
        }
    }

    #[test]
    fn header_and_rows() {
        assert!(is_header(&record(1, &["Source", " target ", "CODE"])));
        assert!(!is_header(&record(1, &["/a", "/b", "301"])));
        let (rows, errors) = to_rows(&[
            record(2, &["/a", "/b", " 302 "]),
            record(3, &["/a", "/b"]),
            record(5, &["/a", "/b", "x"]),
        ]);
        assert_eq!(rows, [("/a".to_owned(), "/b".to_owned(), 302)]);
        let lines: Vec<usize> = errors.iter().map(|e| e.0).collect();
        assert_eq!(lines, [3, 5]);
    }

    #[test]
    fn line_errors_are_capped() {
        let p = line_errors(
            "bad",
            (1..=150).rev().map(|l| (l, "x".to_owned())).collect(),
        );
        assert_eq!(p.errors.len(), MAX_LINE_ERRORS);
        assert_eq!(p.errors[0].field.as_deref(), Some("line 1"));
        assert!(p.detail.contains("150 errors"), "{}", p.detail);
    }
}
