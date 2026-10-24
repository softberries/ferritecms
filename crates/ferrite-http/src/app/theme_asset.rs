//! `/_theme/{hash}/{*path}`: the active theme's files under `assets/`.
//!
//! Served only when `hash` is the content hash of the active theme or of the theme the
//! last hot swap replaced ([`AppState::previous_theme`], so pages rendered just before
//! a swap keep their assets; ruling P18); any other hash, older themes' included, is
//! `404`. `path` must name an asset exactly. The path arrives
//! decoded once by the router and is never decoded again; templates and `theme.toml`
//! are not assets and cannot be reached. The URL changes with the content, so caching
//! is immutable. The ETag is `"<theme hash>-<16 hex of SHA-256(path)>"`: **weak**
//! (`W/…`) for the text formats (CSS, JS, SVG, JSON, source maps, plain text), which
//! the compression layer may encode per request (a strong tag must differ per
//! content-coding, RFC 9110 §8.8.1; `If-None-Match` compares weakly), and strong for
//! images, fonts and other binaries, which are never compressed.

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
    let current = state.theme.load_full();
    let theme = if hash == current.hash() {
        current
    } else {
        match state.previous_theme.load_full() {
            Some(previous) if hash == previous.hash() => previous,
            _ => return not_found(),
        }
    };
    let Some((data, mime)) = theme.asset(path) else {
        return not_found();
    };
    let digest = Sha256::digest(path.as_bytes());
    let path_hash = ferrite_domain::to_hex(&digest[..8]);
    let essence = mime.split(';').next().unwrap_or_default().trim();
    let compressible = COMPRESSIBLE.contains(&essence);
    let weak = if compressible { "W/" } else { "" };
    let etag = format!("{weak}\"{}-{path_hash}\"", theme.hash());
    let fresh = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| ferrite_cache::if_none_match_matches(v, &etag));
    let mut res = if fresh {
        let mut res = Response::new(Body::empty());
        *res.status_mut() = StatusCode::NOT_MODIFIED;
        res
    } else {
        Response::new(Body::from(data))
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
    if !compressible {
        res.extensions_mut().insert(NoCompress);
    }
    res
}
