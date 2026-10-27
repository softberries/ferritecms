//! The admin SPA under [`ADMIN_BASE`] (`/admin/`, design 07 §3 "Embedding & serving").
//!
//! `admin/` builds to `admin/dist/`, which [`EmbeddedAdmin`] compiles into the binary
//! (rust-embed, `allow_missing`): `cargo build` never runs pnpm, and without a build the
//! set is empty and every page path answers a static placeholder page ("admin not
//! built"). Tests swap the set through `AppState::with_admin_assets` (`test-seams`).
//!
//! Routing ([`admin_routes`], merged in front of the public fallback, so admin paths
//! never reach redirect rows or the page cache):
//!
//! - `GET|HEAD /admin` → `308` to `/admin/` (query kept).
//! - Methods other than GET and HEAD → `405` with `Allow: GET, HEAD`.
//! - The path after `/admin/` is percent-decoded once. A path that does not decode to
//!   UTF-8, or holds a `.` or `..` segment, a backslash or a NUL, is a plain `404`
//!   (pre-flight F3).
//! - A path under `assets/` is that file or a plain `404`.
//! - Any other path is the file if it exists and its last segment has an extension
//!   (`favicon.svg`, `robots.txt`); a missing path whose extension is in the content-type
//!   table is `404`; everything else is `index.html` (the SPA fallback), or the
//!   placeholder when no `index.html` is embedded. `/admin/index.html` itself is served
//!   as the fallback is (themed, below), so without a build it is the placeholder (`200`).
//!
//! Headers: `assets/*` are `Cache-Control: public, max-age=31536000, immutable` (their
//! names carry a content hash); `index.html`, the placeholder and other root files are
//! `no-cache`. Each file has a **weak** ETag of the first 16 hex digits of its SHA-256
//! (`W/"…"`, pre-flight F2: the compression layer may change the bytes per request, and
//! a strong validator must differ per content-coding, RFC 9110 §8.8.1); `If-None-Match`
//! uses the weak comparison and answers `304`; `Vary: Accept-Encoding` on both.
//!
//! `index.html` (the fallback and `/admin/index.html`) gets `class="dark"` or
//! `class="light"` on `<html>` when the request's [`THEME_COOKIE`] is exactly `dark` or
//! `light` (P57: the explicit theme paints before any JS; no inline script, so the CSP
//! is unchanged). Any other value is ignored and the class is a fixed string, never the
//! cookie's text. `index.html` always carries `Vary: Accept-Encoding, Cookie`, and a
//! rewritten page the ETag of its own bytes, so a `304` never revalidates one theme's
//! copy for the other.
//!
//! Every admin response, errors and redirects included, carries the admin CSP
//! ([`admin_csp`]), `Referrer-Policy: no-referrer` (P24), `X-Frame-Options: DENY`,
//! `Cross-Origin-Opener-Policy: same-origin`, `X-Content-Type-Options: nosniff`,
//! `Permissions-Policy: camera=(), microphone=(), geolocation=()` and
//! `X-Robots-Tag: noindex`. Text formats go through the shared compression layer; fonts,
//! images and unknown types are not compressed.

use std::{
    borrow::Cow,
    sync::{Arc, LazyLock},
};

use axum::{
    Router,
    body::Body,
    extract::State,
    http::{HeaderMap, HeaderName, HeaderValue, Method, Response, StatusCode, Uri, header},
    routing::any,
};
use percent_encoding::percent_decode_str;
use sha2::{Digest, Sha256};

use crate::app::AppState;

/// Where the admin is served (with the trailing slash; Vite's `base`).
pub const ADMIN_BASE: &str = "/admin/";

const IMMUTABLE: &str = "public, max-age=31536000, immutable";
const NO_CACHE: &str = "no-cache";
const INDEX: &str = "index.html";
const ASSETS: &str = "assets/";
const VARY_ENCODING: &str = "accept-encoding";
/// `index.html` depends on the theme cookie too (P57), so no cache may hand one
/// viewer's copy to another.
const VARY_COOKIE: &str = "accept-encoding, cookie";

/// The cookie the admin writes for an explicit theme choice (`dark` or `light`; P57,
/// design 11 §4); `index.html` gets the matching class on `<html>`.
pub const THEME_COOKIE: &str = "ferrite_admin_theme";

/// The static page served for every page path when no admin build is embedded.
const PLACEHOLDER: &str = include_str!("admin/placeholder.html");

/// Headers on every admin response besides the CSP.
const ADMIN_HEADERS: [(HeaderName, &str); 6] = [
    (header::REFERRER_POLICY, "no-referrer"),
    (header::X_FRAME_OPTIONS, "DENY"),
    (
        HeaderName::from_static("cross-origin-opener-policy"),
        "same-origin",
    ),
    (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
    (
        HeaderName::from_static("permissions-policy"),
        "camera=(), microphone=(), geolocation=()",
    ),
    (HeaderName::from_static("x-robots-tag"), "noindex"),
];

/// One file of the admin build.
#[derive(Debug, Clone)]
pub struct AdminFile {
    pub bytes: Cow<'static, [u8]>,
    pub sha256: [u8; 32],
}

/// The admin build's files by path relative to `admin/dist/` (`index.html`,
/// `assets/index-abc.js`).
pub trait AdminAssets: Send + Sync {
    fn get(&self, path: &str) -> Option<AdminFile>;
}

/// `admin/dist/` compiled into the binary; empty when the folder is missing at build
/// time (`build.rs` makes Cargo rebuild when it appears or changes).
#[derive(rust_embed::Embed)]
#[folder = "../../admin/dist/"]
#[allow_missing = true]
pub struct EmbeddedAdmin;

impl AdminAssets for EmbeddedAdmin {
    fn get(&self, path: &str) -> Option<AdminFile> {
        <Self as rust_embed::Embed>::get(path).map(|f| AdminFile {
            sha256: f.metadata.sha256_hash(),
            bytes: f.data,
        })
    }
}

/// The admin's `Content-Security-Policy`. `media_origin` is the origin of
/// `[media] public_base_url` when media is served directly from S3/a CDN (else `None`):
/// the editor shows those images. `img-src` also allows any `https:` image, which body
/// images may reference (pre-flight F5; `no-referrer` keeps the admin path from the
/// remote host). `style-src 'unsafe-inline'` is needed by the injected `<style>`
/// elements of sonner, Radix and the shadcn chart (P8); scripts are `'self'` only.
pub fn admin_csp(media_origin: Option<&str>) -> String {
    let media = media_origin.map(|o| format!(" {o}")).unwrap_or_default();
    format!(
        "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; \
         img-src 'self' data: blob: https:{media}; font-src 'self'; connect-src 'self'; \
         frame-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'; \
         object-src 'none'; worker-src 'self'"
    )
}

/// A header value of [`AdminHeaders`] that is not a valid header value (in practice a
/// media origin with characters a header cannot carry).
#[derive(Debug, thiserror::Error)]
#[error("invalid admin {name} header value {value:?}")]
pub struct InvalidAdminHeader {
    pub name: HeaderName,
    pub value: String,
}

/// The CSP ([`admin_csp`]) and [`ADMIN_HEADERS`] as header values, built once with the
/// app state: an invalid value fails there, so no admin response goes out without its
/// CSP.
#[derive(Debug, Clone)]
pub struct AdminHeaders(Vec<(HeaderName, HeaderValue)>);

impl AdminHeaders {
    pub fn new(media_origin: Option<&str>) -> Result<Self, InvalidAdminHeader> {
        let csp = (header::CONTENT_SECURITY_POLICY, admin_csp(media_origin));
        let fixed = ADMIN_HEADERS.map(|(name, value)| (name, value.to_owned()));
        std::iter::once(csp)
            .chain(fixed)
            .map(|(name, value)| match HeaderValue::from_str(&value) {
                Ok(v) => Ok((name, v)),
                Err(_) => Err(InvalidAdminHeader { name, value }),
            })
            .collect::<Result<_, _>>()
            .map(Self)
    }
}

/// `/admin`, `/admin/` and `/admin/{*path}` (see the module docs).
pub fn admin_routes(state: &Arc<AppState>) -> Router {
    Router::new()
        .route("/admin", any(bare))
        .route(ADMIN_BASE, any(serve))
        .route("/admin/{*path}", any(serve))
        .with_state(state.clone())
}

async fn bare(State(state): State<Arc<AppState>>, method: Method, uri: Uri) -> Response<Body> {
    let mut res = if is_read(&method) {
        let location = match uri.query() {
            Some(q) => format!("{ADMIN_BASE}?{q}"),
            None => ADMIN_BASE.to_owned(),
        };
        let mut res = Response::new(Body::empty());
        *res.status_mut() = StatusCode::PERMANENT_REDIRECT;
        set(&mut res, header::LOCATION, &location);
        res
    } else {
        method_not_allowed()
    };
    secure(&mut res, &state);
    res
}

async fn serve(
    State(state): State<Arc<AppState>>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
) -> Response<Body> {
    let mut res = respond(&state, &method, &uri, &headers);
    secure(&mut res, &state);
    res
}

fn respond(state: &AppState, method: &Method, uri: &Uri, headers: &HeaderMap) -> Response<Body> {
    if !is_read(method) {
        return method_not_allowed();
    }
    let raw = uri.path().strip_prefix(ADMIN_BASE).unwrap_or_default();
    let Some(path) = decode(raw) else {
        return not_found();
    };
    if path.starts_with(ASSETS) {
        return match state.admin.get(&path) {
            Some(file) => file_response(&file, extension(&path), IMMUTABLE, VARY_ENCODING, headers),
            None => not_found(),
        };
    }
    let ext = extension(&path);
    if path != INDEX
        && ext.is_some()
        && let Some(file) = state.admin.get(&path)
    {
        return file_response(&file, ext, NO_CACHE, VARY_ENCODING, headers);
    }
    if path != INDEX && ext.is_some_and(|e| content_type(e).is_some()) {
        return not_found();
    }
    match state.admin.get(INDEX) {
        Some(file) => match theme_class(headers) {
            Some(attr) => {
                let themed = with_theme_class(&file, attr);
                file_response(&themed, Some("html"), NO_CACHE, VARY_COOKIE, headers)
            }
            None => file_response(&file, Some("html"), NO_CACHE, VARY_COOKIE, headers),
        },
        None => file_response(
            &PLACEHOLDER_FILE,
            Some("html"),
            NO_CACHE,
            VARY_ENCODING,
            headers,
        ),
    }
}

/// The `<html>` attribute for the viewer's explicit theme (P57): the cookie
/// [`THEME_COOKIE`] (first occurrence) with exactly `dark` or `light`; any other value,
/// or none, gives `None` (no class: `tokens.css` follows the system scheme). The
/// attribute is a fixed string; the cookie value is never copied into the page.
fn theme_class(headers: &HeaderMap) -> Option<&'static str> {
    let value = headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .find_map(|pair| {
            let (name, value) = pair.trim().split_once('=')?;
            (name == THEME_COOKIE).then_some(value)
        })?;
    match value {
        "dark" => Some(" class=\"dark\""),
        "light" => Some(" class=\"light\""),
        _ => None,
    }
}

/// `file` with `attr` inserted into its `<html` start tag (unchanged when it has none),
/// with the digest of the new bytes, so the ETag differs per theme.
fn with_theme_class(file: &AdminFile, attr: &str) -> AdminFile {
    let bytes = &file.bytes[..];
    let at = bytes
        .windows(6)
        .position(|w| {
            w[..5].eq_ignore_ascii_case(b"<html") && (w[5] == b'>' || w[5].is_ascii_whitespace())
        })
        .map(|i| i + 5);
    let Some(at) = at else {
        return file.clone();
    };
    let mut out = Vec::with_capacity(bytes.len() + attr.len());
    out.extend_from_slice(&bytes[..at]);
    out.extend_from_slice(attr.as_bytes());
    out.extend_from_slice(&bytes[at..]);
    AdminFile {
        sha256: Sha256::digest(&out).into(),
        bytes: Cow::Owned(out),
    }
}

static PLACEHOLDER_FILE: LazyLock<AdminFile> = LazyLock::new(|| AdminFile {
    bytes: Cow::Borrowed(PLACEHOLDER.as_bytes()),
    sha256: Sha256::digest(PLACEHOLDER.as_bytes()).into(),
});

fn is_read(method: &Method) -> bool {
    *method == Method::GET || *method == Method::HEAD
}

/// `raw` percent-decoded once; `None` when it is not UTF-8 or holds a `.`/`..` segment,
/// a backslash or a NUL.
fn decode(raw: &str) -> Option<String> {
    let path = percent_decode_str(raw).decode_utf8().ok()?;
    if path.contains(['\\', '\0']) || path.split('/').any(|s| s == "." || s == "..") {
        return None;
    }
    Some(path.into_owned())
}

/// The extension of the last segment (after its last `.`, not a leading one).
fn extension(path: &str) -> Option<&str> {
    let name = path.rsplit('/').next().unwrap_or_default();
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && !ext.is_empty() => Some(ext),
        _ => None,
    }
}

/// `Content-Type` by extension; `None`: not in the table (served as
/// `application/octet-stream`).
fn content_type(ext: &str) -> Option<&'static str> {
    Some(match ext {
        "html" => "text/html; charset=utf-8",
        "js" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "woff2" => "font/woff2",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "ico" => "image/x-icon",
        "json" => "application/json",
        "txt" => "text/plain; charset=utf-8",
        "webmanifest" => "application/manifest+json",
        _ => return None,
    })
}

/// Text formats the compression layer may encode.
fn compressible(ext: &str) -> bool {
    matches!(
        ext,
        "html" | "js" | "css" | "svg" | "json" | "txt" | "webmanifest"
    )
}

fn file_response(
    file: &AdminFile,
    ext: Option<&str>,
    cache_control: &str,
    vary: &str,
    headers: &HeaderMap,
) -> Response<Body> {
    let etag = format!("W/\"{}\"", ferrite_domain::to_hex(&file.sha256[..8]));
    let fresh = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| ferrite_cache::if_none_match_matches(v, &etag));
    let mut res = if fresh {
        let mut res = Response::new(Body::empty());
        *res.status_mut() = StatusCode::NOT_MODIFIED;
        res
    } else {
        let mut res = Response::new(Body::from(file.bytes.clone()));
        let mime = ext
            .and_then(content_type)
            .unwrap_or("application/octet-stream");
        set(&mut res, header::CONTENT_TYPE, mime);
        res
    };
    set(&mut res, header::ETAG, &etag);
    set(&mut res, header::CACHE_CONTROL, cache_control);
    set(&mut res, header::VARY, vary);
    if !ext.is_some_and(compressible) {
        res.extensions_mut().insert(crate::app::NoCompress);
    }
    res
}

fn not_found() -> Response<Body> {
    let mut res = Response::new(Body::from("not found\n"));
    *res.status_mut() = StatusCode::NOT_FOUND;
    set(&mut res, header::CONTENT_TYPE, "text/plain; charset=utf-8");
    set(&mut res, header::CACHE_CONTROL, "no-store");
    res
}

fn method_not_allowed() -> Response<Body> {
    let mut res = Response::new(Body::from("method not allowed\n"));
    *res.status_mut() = StatusCode::METHOD_NOT_ALLOWED;
    set(&mut res, header::CONTENT_TYPE, "text/plain; charset=utf-8");
    set(&mut res, header::CACHE_CONTROL, "no-store");
    set(&mut res, header::ALLOW, "GET, HEAD");
    res
}

/// The CSP and [`ADMIN_HEADERS`] ([`AdminHeaders`]), replacing whatever was set.
fn secure(res: &mut Response<Body>, state: &AppState) {
    for (name, value) in &state.admin_headers.0 {
        res.headers_mut().insert(name.clone(), value.clone());
    }
}

fn set(res: &mut Response<Body>, name: HeaderName, value: &str) {
    if let Ok(v) = HeaderValue::from_str(value) {
        res.headers_mut().insert(name, v);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_refuses_dot_segments_backslashes_and_nul() {
        assert_eq!(decode("content/post").as_deref(), Some("content/post"));
        assert_eq!(decode("").as_deref(), Some(""));
        for bad in [
            "assets/../../secret.key",
            "%2e%2e/secret.key",
            "assets/%2E%2E/x.js",
            "./index.html",
            "assets\\..\\x",
            "assets/%5c..%5cx",
            "a%00b",
            "%ff",
        ] {
            assert_eq!(decode(bad), None, "{bad}");
        }
    }

    #[test]
    fn extensions() {
        assert_eq!(extension("favicon.svg"), Some("svg"));
        assert_eq!(extension("assets/index-abc.js"), Some("js"));
        assert_eq!(extension("content/post"), None);
        assert_eq!(extension("a.b/c"), None);
        assert_eq!(extension(".hidden"), None);
        assert_eq!(extension("x."), None);
    }

    #[test]
    fn admin_headers_hold_the_csp_or_fail() {
        let headers = AdminHeaders::new(Some("https://cdn.example.com")).unwrap();
        let csp = headers
            .0
            .iter()
            .find(|(n, _)| *n == header::CONTENT_SECURITY_POLICY)
            .map(|(_, v)| v.to_str().unwrap().to_owned());
        assert_eq!(csp, Some(admin_csp(Some("https://cdn.example.com"))));
        assert_eq!(headers.0.len(), 1 + ADMIN_HEADERS.len());
        let err = AdminHeaders::new(Some("https://cdn.example.com\nx: y")).unwrap_err();
        assert_eq!(err.name, header::CONTENT_SECURITY_POLICY);
        assert!(AdminHeaders::new(Some("https://cdn\u{7f}.example.com")).is_err());
    }

    #[test]
    fn csp_names_the_media_origin_only_when_given() {
        assert!(admin_csp(None).contains("img-src 'self' data: blob: https:; "));
        assert!(
            admin_csp(Some("https://cdn.example.com"))
                .contains("img-src 'self' data: blob: https: https://cdn.example.com; ")
        );
    }
}
