//! `/_media/{hash}/{name}` (03 §5, Review Focus 5): app-served media files.
//!
//! * **Published only.** The bytes are served only when `hash` is the original or a
//!   variant of a public media item ([`SiteReader::media_file`]); anything else is a
//!   plain `404` (no theme render: these are asset URLs).
//! * **Name.** The last segment must be the item's `original_name` (variants share it;
//!   `file` stands in for an empty, `.` or `..` name, as `MediaStore::public_url` writes
//!   it). Another name is a `301` to the canonical one, so old or hand-typed names still
//!   work and every file has one URL.
//! * **Streaming** through `MediaStore::open_read`; never buffered, never in the page
//!   cache (the HTTP cache headers are enough: the content is immutable). The small
//!   in-memory LRU of 03 §5 is deferred.
//! * **Headers:** `ETag: "<hash>"` (strong: the bytes never change), `If-None-Match` →
//!   `304`, `Cache-Control: public, max-age=31536000, immutable`, `Accept-Ranges: bytes`,
//!   `X-Content-Type-Options: nosniff`. Types that are [`inline_safe`] are served inline
//!   with their type, PDF additionally with `Content-Security-Policy: sandbox`;
//!   everything else (SVG included: M1b stores no sanitised marker) as
//!   `application/octet-stream` with `Content-Disposition: attachment`. Not compressed.
//! * **Ranges:** one range only. `bytes=a-b`, `bytes=a-` and the suffix `bytes=-n` give
//!   `206` with `Content-Range`; a start at or past the end, or `bytes=-0`, is `416`
//!   with `Content-Range: bytes */<size>`. A request for **several ranges is answered
//!   with `200` and the full body** (no multipart responses), as is a malformed or
//!   non-`bytes` range, and a range whose `If-Range` does not equal the ETag (strong
//!   comparison; dates never match).

use std::{ops::Range, sync::Arc};

use axum::{
    body::Body,
    http::{HeaderMap, HeaderValue, Method, Response, StatusCode, header},
};
use ferrite_core::{MediaFile, permalink::encode_segment};
use ferrite_domain::ArtifactHash;
use ferrite_media::{MediaError, inline_safe};
use futures::TryStreamExt;
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};

use super::{AppState, response::NoCompress};

/// `Cache-Control` of every media file.
const IMMUTABLE: &str = "public, max-age=31536000, immutable";
const PDF: &str = "application/pdf";

/// What a `Range` header asks for, against a file of `size` bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Ranged {
    /// No (usable) range: the whole file.
    Full,
    /// One satisfiable range (end exclusive).
    Part(Range<u64>),
    /// `416`.
    Unsatisfiable,
}

/// Parses `value` (one `Range` header) for a file of `size` bytes; see the module docs.
pub(crate) fn parse_range(value: &str, size: u64) -> Ranged {
    let Some(spec) = value.trim().strip_prefix("bytes=") else {
        return Ranged::Full;
    };
    if spec.contains(',') {
        return Ranged::Full;
    }
    let Some((first, last)) = spec.trim().split_once('-') else {
        return Ranged::Full;
    };
    let number = |s: &str| -> Option<u64> {
        let s = s.trim();
        (!s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
            .then(|| s.parse().ok())
            .flatten()
    };
    match (first.trim().is_empty(), number(first), number(last)) {
        // Suffix: the last `n` bytes.
        (true, _, Some(n)) => {
            if n == 0 || size == 0 {
                Ranged::Unsatisfiable
            } else {
                Ranged::Part(size.saturating_sub(n)..size)
            }
        }
        (false, Some(start), end) => {
            if !last.trim().is_empty() && end.is_none() {
                return Ranged::Full;
            }
            if end.is_some_and(|e| e < start) {
                return Ranged::Full;
            }
            if start >= size {
                return Ranged::Unsatisfiable;
            }
            let end = end.map_or(size, |e| e.saturating_add(1).min(size));
            Ranged::Part(start..end)
        }
        _ => Ranged::Full,
    }
}

/// The name in the canonical URL of `file` (see the module docs).
fn canonical_name(file: &MediaFile) -> &str {
    match file.name.as_str() {
        "" | "." | ".." => "file",
        other => other,
    }
}

/// Serves `/_media/{hash}/{name}` (`name` decoded once by the router).
pub(crate) async fn serve(
    state: &Arc<AppState>,
    method: &Method,
    headers: &HeaderMap,
    hash: &str,
    name: &str,
) -> Response<Body> {
    let Ok(hash) = ArtifactHash::parse(hash) else {
        return not_found();
    };
    let file = match state.reader.media_file(&hash).await {
        Ok(Some(file)) => file,
        Ok(None) => return not_found(),
        Err(e) => {
            tracing::error!(error = %e, "media lookup failed");
            return status_only(StatusCode::INTERNAL_SERVER_ERROR);
        }
    };
    let canonical = canonical_name(&file);
    if name != canonical {
        let location = format!("/_media/{hash}/{}", encode_segment(canonical));
        let mut res = status_only(StatusCode::MOVED_PERMANENTLY);
        if let Ok(v) = HeaderValue::from_str(&location) {
            res.headers_mut().insert(header::LOCATION, v);
        }
        return res;
    }
    let etag = format!("\"{hash}\"");
    if headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| ferrite_cache::if_none_match_matches(v, &etag))
    {
        let mut res = status_only(StatusCode::NOT_MODIFIED);
        file_headers(&mut res, &etag);
        return res;
    }
    let range_header = headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .filter(|_| if_range_ok(headers, &etag));
    match open(state, method, &hash, range_header).await {
        Ok(Opened::Body {
            size,
            range,
            partial,
            body,
        }) => {
            let status = if partial {
                StatusCode::PARTIAL_CONTENT
            } else {
                StatusCode::OK
            };
            let mut res = Response::new(body);
            *res.status_mut() = status;
            file_headers(&mut res, &etag);
            content_headers(&mut res, &file);
            set(
                &mut res,
                header::CONTENT_LENGTH,
                &(range.end - range.start).to_string(),
            );
            if partial {
                set(
                    &mut res,
                    header::CONTENT_RANGE,
                    &format!("bytes {}-{}/{size}", range.start, range.end - 1),
                );
            }
            res
        }
        Ok(Opened::Unsatisfiable { size }) => {
            let mut res = status_only(StatusCode::RANGE_NOT_SATISFIABLE);
            file_headers(&mut res, &etag);
            set(&mut res, header::CONTENT_RANGE, &format!("bytes */{size}"));
            res
        }
        Err(MediaError::NotFound) => {
            tracing::warn!(%hash, "media row without a stored blob");
            not_found()
        }
        Err(e) => {
            tracing::error!(%hash, error = %e, "media read failed");
            status_only(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

/// Whether a `Range` may apply: no `If-Range`, or one strongly equal to the ETag.
fn if_range_ok(headers: &HeaderMap, etag: &str) -> bool {
    match headers.get(header::IF_RANGE) {
        None => true,
        Some(v) => v.to_str().is_ok_and(|v| v.trim() == etag),
    }
}

enum Opened {
    Body {
        size: u64,
        range: Range<u64>,
        partial: bool,
        body: Body,
    },
    Unsatisfiable {
        size: u64,
    },
}

/// Opens the file for `range_header`. A ranged request (and HEAD) needs the size first
/// (one metadata request); a plain GET opens the whole object in one request.
async fn open(
    state: &AppState,
    method: &Method,
    hash: &ArtifactHash,
    range_header: Option<&str>,
) -> Result<Opened, MediaError> {
    if range_header.is_none() && method != Method::HEAD {
        let read = state.media.open_read(hash, None).await?;
        return Ok(Opened::Body {
            size: read.size,
            range: read.range,
            partial: false,
            body: Body::from_stream(read.stream.map_err(std::io::Error::other)),
        });
    }
    let size = state.media.head(hash).await?.ok_or(MediaError::NotFound)?;
    let (range, partial) = match range_header.map_or(Ranged::Full, |v| parse_range(v, size)) {
        Ranged::Full => (0..size, false),
        Ranged::Part(r) => (r, true),
        Ranged::Unsatisfiable => return Ok(Opened::Unsatisfiable { size }),
    };
    if method == Method::HEAD {
        return Ok(Opened::Body {
            size,
            range,
            partial,
            body: Body::empty(),
        });
    }
    let read = state.media.open_read(hash, Some(range)).await?;
    Ok(Opened::Body {
        size: read.size,
        range: read.range,
        partial,
        body: Body::from_stream(read.stream.map_err(std::io::Error::other)),
    })
}

fn file_headers(res: &mut Response<Body>, etag: &str) {
    set(res, header::ETAG, etag);
    set(res, header::CACHE_CONTROL, IMMUTABLE);
    set(res, header::ACCEPT_RANGES, "bytes");
    set(res, header::X_CONTENT_TYPE_OPTIONS, "nosniff");
    res.extensions_mut().insert(NoCompress);
}

fn content_headers(res: &mut Response<Body>, file: &MediaFile) {
    if inline_safe(&file.mime) {
        set(res, header::CONTENT_TYPE, &file.mime);
        set(res, header::CONTENT_DISPOSITION, "inline");
        if file.mime == PDF {
            set(res, header::CONTENT_SECURITY_POLICY, "sandbox");
        }
    } else {
        set(res, header::CONTENT_TYPE, "application/octet-stream");
        let encoded = utf8_percent_encode(canonical_name(file), NON_ALPHANUMERIC);
        set(
            res,
            header::CONTENT_DISPOSITION,
            &format!("attachment; filename*=UTF-8''{encoded}"),
        );
    }
}

fn set(res: &mut Response<Body>, name: header::HeaderName, value: &str) {
    if let Ok(v) = HeaderValue::from_str(value) {
        res.headers_mut().insert(name, v);
    }
}

fn status_only(status: StatusCode) -> Response<Body> {
    let mut res = Response::new(Body::empty());
    *res.status_mut() = status;
    res.extensions_mut().insert(NoCompress);
    res
}

/// The plain 404 of asset URLs (`/_media`, `/_theme`).
pub(crate) fn not_found() -> Response<Body> {
    let mut res = Response::new(Body::from("not found\n"));
    *res.status_mut() = StatusCode::NOT_FOUND;
    set(&mut res, header::CONTENT_TYPE, "text/plain; charset=utf-8");
    set(&mut res, header::CACHE_CONTROL, "no-store");
    res.extensions_mut().insert(NoCompress);
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges() {
        let p = |v: &str| parse_range(v, 1000);
        assert_eq!(p("bytes=0-0"), Ranged::Part(0..1));
        assert_eq!(p("bytes=0-499"), Ranged::Part(0..500));
        assert_eq!(p("bytes=500-"), Ranged::Part(500..1000));
        assert_eq!(p("bytes=990-5000"), Ranged::Part(990..1000));
        assert_eq!(p("bytes=-100"), Ranged::Part(900..1000));
        assert_eq!(p("bytes=-5000"), Ranged::Part(0..1000));
        assert_eq!(p("bytes=999-"), Ranged::Part(999..1000));
        // Unsatisfiable.
        assert_eq!(p("bytes=-0"), Ranged::Unsatisfiable);
        assert_eq!(p("bytes=1000-"), Ranged::Unsatisfiable);
        assert_eq!(p("bytes=99999999-"), Ranged::Unsatisfiable);
        assert_eq!(parse_range("bytes=0-", 0), Ranged::Unsatisfiable);
        assert_eq!(parse_range("bytes=-1", 0), Ranged::Unsatisfiable);
        // Ignored: the full body.
        assert_eq!(p("bytes=0-0,1-1"), Ranged::Full, "multi-range");
        assert_eq!(p("bytes=0-0,1-1,2-2,3-3,4-4"), Ranged::Full);
        assert_eq!(p("bytes=5-1"), Ranged::Full, "inverted");
        assert_eq!(p("items=0-1"), Ranged::Full);
        assert_eq!(p("bytes=a-b"), Ranged::Full);
        assert_eq!(p("bytes=1-x"), Ranged::Full);
        assert_eq!(p("bytes=-"), Ranged::Full);
        assert_eq!(p("bytes=+1-2"), Ranged::Full);
        assert_eq!(
            p("bytes=99999999999999999999999-"),
            Ranged::Full,
            "overflow"
        );
    }
}
