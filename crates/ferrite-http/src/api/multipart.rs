//! `multipart/form-data` uploads (P11) with a fixed set of parts, each bounded.
//!
//! Handlers authenticate and authorize first, then [`open`] the body (media type and a
//! declared `Content-Length` checked before anything is read) and [`read_parts`] it.
//! Parts are read chunk by chunk and refused as soon as one passes its limit, so no
//! more than the limits are ever buffered; the route's `DefaultBodyLimit` caps the
//! whole body on top (axum's [`Multipart`] reads through it).
//!
//! | What | Problem |
//! |---|---|
//! | not `multipart/form-data` (or no boundary) | `415 unsupported-media-type` |
//! | `Content-Length` over the route limit, a part over its limit, the body over the route limit | `413 too-large` (naming the part when one is) |
//! | an unknown, unnamed or repeated part, a malformed body | `400 malformed-body` (naming the part) |
//! | a part of the spec missing | `422 validation`, `errors[0].field` = the part |

use std::collections::BTreeMap;

use axum::{
    extract::{
        FromRequest, Multipart, Request,
        multipart::{Field, MultipartError},
    },
    http::{StatusCode, header},
};
use bytes::{Bytes, BytesMut};

use super::problem::Problem;

/// The upload of `req` as multipart parts, if it is `multipart/form-data` and its
/// declared length (if any) is at most `max_body` bytes.
pub async fn open(req: Request, max_body: usize) -> Result<Multipart, Problem> {
    let content_type = req
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    let essence = content_type.split(';').next().unwrap_or_default().trim();
    if !essence.eq_ignore_ascii_case("multipart/form-data") {
        return Err(unsupported());
    }
    let declared = req
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    if declared.is_some_and(|n| n > max_body as u64) {
        return Err(too_large(format!(
            "the body is larger than {max_body} bytes"
        )));
    }
    Multipart::from_request(req, &())
        .await
        .map_err(|_| unsupported())
}

/// Reads every part of `mp`: each must be named in `spec` (name, most bytes) and appear
/// once, and every part of `spec` must be there (see the module docs for the problems).
pub async fn read_parts(
    mp: Multipart,
    spec: &[(&'static str, usize)],
) -> Result<BTreeMap<&'static str, Bytes>, Problem> {
    read_parts_with(mp, spec, &[]).await
}

/// [`read_parts`] where the parts named in `optional` may be left out (M6-2: a plugin's
/// `signature` and `publisher_key`).
pub async fn read_parts_with(
    mut mp: Multipart,
    spec: &[(&'static str, usize)],
    optional: &[&str],
) -> Result<BTreeMap<&'static str, Bytes>, Problem> {
    let mut parts = BTreeMap::new();
    while let Some(mut field) = mp.next_field().await.map_err(body_error)? {
        // Echoed in a problem: cut to 64 characters.
        let given: String = field.name().unwrap_or_default().chars().take(64).collect();
        let Some(&(name, max)) = spec.iter().find(|(n, _)| *n == given) else {
            return Err(malformed(if given.is_empty() {
                "a part has no name".to_owned()
            } else {
                format!("unknown part {given:?}")
            }));
        };
        if parts.contains_key(name) {
            return Err(malformed(format!("part {name:?} is sent more than once")));
        }
        let buf = read_field(&mut field, name, max).await?;
        parts.insert(name, buf);
    }
    if let Some((missing, _)) = spec
        .iter()
        .find(|(n, _)| !parts.contains_key(n) && !optional.contains(n))
    {
        return Err(Problem::validation(format!("part {missing:?} is required"))
            .with_error(Some(missing), "required"));
    }
    Ok(parts)
}

/// The bytes of part `field` (named `name` in problems), refused `413` as soon as they
/// pass `max`.
pub(crate) async fn read_field(
    field: &mut Field<'_>,
    name: &str,
    max: usize,
) -> Result<Bytes, Problem> {
    let mut buf = BytesMut::new();
    while let Some(chunk) = field.chunk().await.map_err(body_error)? {
        if buf.len() + chunk.len() > max {
            return Err(too_large(format!(
                "part {name:?} is larger than {max} bytes"
            )));
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(buf.freeze())
}

/// The problem of a body that failed while it was read: `413` over the route limit,
/// else `400 malformed-body`.
pub(crate) fn body_error(e: MultipartError) -> Problem {
    if e.status() == StatusCode::PAYLOAD_TOO_LARGE {
        too_large("the body is larger than this endpoint accepts".to_owned())
    } else {
        malformed(e.body_text())
    }
}

fn unsupported() -> Problem {
    Problem::new(
        StatusCode::UNSUPPORTED_MEDIA_TYPE,
        "unsupported-media-type",
        "Unsupported media type",
        "the body must be multipart/form-data with a boundary",
    )
}

fn too_large(detail: String) -> Problem {
    Problem::new(
        StatusCode::PAYLOAD_TOO_LARGE,
        "too-large",
        "Request too large",
        detail,
    )
}

pub(crate) fn malformed(detail: String) -> Problem {
    Problem::new(
        StatusCode::BAD_REQUEST,
        "malformed-body",
        "Malformed request body",
        detail,
    )
}
