//! `/_theme/{hash}/{*path}`: the active theme's files under `assets/`.
//!
//! Served only when `hash` is the active theme's content hash (any other, an old
//! theme's included, is `404`) and `path` names an asset exactly. The path arrives
//! decoded once by the router and is never decoded again; templates and `theme.toml`
//! are not assets and cannot be reached. The URL changes with the content, so caching
//! is immutable; the strong ETag is `"<theme hash>-<16 hex of SHA-256(path)>"`. Text
//! formats (CSS, JS, SVG, JSON, source maps, plain text) may be compressed; images,
//! fonts and other binaries are not.

use std::sync::Arc;

use axum::{
    body::Body,
    http::{HeaderMap, HeaderValue, Response, StatusCode, header},
};
use sha2::{Digest, Sha256};

use super::{AppState, media::not_found, response::NoCompress};

const IMMUTABLE: &str = "public, max-age=31536000, immutable";

/// MIME types (without parameters) worth compressing.
const COMPRESSIBLE: [&str; 5] = [
    "text/css",
    "text/javascript",
    "image/svg+xml",
    "application/json",
    "text/plain",
];

pub(crate) fn serve(
    state: &Arc<AppState>,
    headers: &HeaderMap,
    hash: &str,
    path: &str,
) -> Response<Body> {
    let theme = state.theme.load_full();
    if hash != theme.hash() {
        return not_found();
    }
    let Some((data, mime)) = theme.asset(path) else {
        return not_found();
    };
    let digest = Sha256::digest(path.as_bytes());
    let path_hash: String = digest.iter().take(8).map(|b| format!("{b:02x}")).collect();
    let etag = format!("\"{}-{path_hash}\"", theme.hash());
    let fresh = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| ferrite_cache::if_none_match_matches(v, &etag));
    let mut res = if fresh {
        let mut res = Response::new(Body::empty());
        *res.status_mut() = StatusCode::NOT_MODIFIED;
        res
    } else {
        Response::new(Body::from(data.to_vec()))
    };
    let set = |res: &mut Response<Body>, name, value: &str| {
        if let Ok(v) = HeaderValue::from_str(value) {
            res.headers_mut().insert(name, v);
        }
    };
    set(&mut res, header::ETAG, &etag);
    set(&mut res, header::CACHE_CONTROL, IMMUTABLE);
    if !fresh {
        set(&mut res, header::CONTENT_TYPE, mime);
    }
    let essence = mime.split(';').next().unwrap_or_default().trim();
    if !COMPRESSIBLE.contains(&essence) {
        res.extensions_mut().insert(NoCompress);
    }
    res
}
