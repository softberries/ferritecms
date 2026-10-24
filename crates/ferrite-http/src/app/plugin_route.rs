//! Plugin routes: `/_p/<plugin>/<path>` for every method (05 §4b, M2a-2 P15, P16).
//!
//! Mounted as axum routes in front of the public fallback (also `/_p/<plugin>/`, so a
//! manifest route `"/"` is reachable, pre-flight F25), so nothing here passes through
//! redirects or the page cache: responses are **never cached**. Every error is plain
//! text (these are not API endpoints). Checks, in order:
//!
//! 1. The path: the raw `<plugin>` segment must be a plugin name (`[a-z0-9-]{2,48}`,
//!    never percent-decoded), and the rest, percent-decoded as UTF-8, is the route
//!    path. Dot segments and `%2F` are not resolved, so a path can only ever name a
//!    route of that one plugin, never another plugin's or a core route.
//! 2. `404` when the plugin is not loaded on this node, is disabled on this node
//!    (after repeated failures), lacks the `routes` grant, or has no such path; also
//!    when its `plugins` row is missing or disabled (read per request, so a disable or
//!    delete on any node answers `404` everywhere at once, before the reload). A path
//!    whose route lacks the method: `405` with `Allow`.
//! 3. Methods other than GET and HEAD need the site's origin (`Origin`, else the
//!    `Referer`'s origin, as the API's rule) unless the route has `cross_origin =
//!    true`: else `403`.
//! 4. The route's rate limit (`rate_limit`, default `10/min`) per client address (/64
//!    for IPv6) through [`AuthService::hit_limit`](ferrite_core::AuthService::hit_limit)
//!    on bucket `route:<plugin>:<32 hex of SHA-256(path)>` (F24), counted in the
//!    database so it holds across nodes: `429` with `Retry-After`.
//! 5. The body: at most [`MAX_ROUTE_BODY_BYTES`] (1 MiB), else `413`; at most
//!    [`MAX_QUERY_PAIRS`] query pairs, else `400`.
//!
//! The plugin then gets a `route-request` with only the `content-type`, `accept`,
//! `accept-language`, `user-agent` and `referer` request headers (never `Cookie` or
//! `Authorization`), its own client-address hash
//! ([`AuthService::plugin_ip_hash`](ferrite_core::AuthService::plugin_ip_hash); empty
//! when the address is unknown), and `user` = the id behind the `ferrite_session`
//! cookie (no bearer tokens; none for an account that must reset its password).
//!
//! The call is bounded by the plugin host (fuel, memory, the 2 s route deadline, the
//! response checks of `validate_route_response`); any failure is `502`. The visitor
//! gets the plugin's status and only its `Content-Type`, `Content-Language` and, on a
//! 3xx, a root-relative `Location` (`/…` in visible ASCII, not `//…` or `/\…`; any other
//! `Location` on a 3xx makes the answer `502`). Every handler answer carries
//! `Content-Security-Policy: sandbox allow-forms` (an opaque origin: script in it
//! cannot read the site's cookies or call the API as the visitor),
//! `X-Content-Type-Options: nosniff`, `Cache-Control: no-store` and
//! `X-Robots-Tag: noindex`; never `Set-Cookie`.
//!
//! Without an [`AuthService`](ferrite_core::AuthService) (an app built without
//! `with_auth`) the rate limit cannot be counted, so routes answer `503`.

use std::sync::Arc;

use axum::{
    body::Body,
    extract::{Request, State},
    http::{HeaderMap, HeaderName, HeaderValue, Method, Response, StatusCode, header},
};
use ferrite_core::CoreError;
use ferrite_plugin_host::{MAX_ROUTE_BODY_BYTES, PluginError, RouteRequest, RouteResponse};
use percent_encoding::percent_decode_str;
use sha2::{Digest, Sha256};

use super::{AppState, client::ClientIp, handler, response::text};
use crate::api::auth::same_origin;

/// Most query pairs passed to a plugin (`400` above).
pub const MAX_QUERY_PAIRS: usize = 100;

/// Request headers a plugin may see (05 §4b; the WIT `route-request` docs).
const PASSED_HEADERS: [HeaderName; 5] = [
    header::CONTENT_TYPE,
    header::ACCEPT,
    header::ACCEPT_LANGUAGE,
    header::USER_AGENT,
    header::REFERER,
];

/// What every answer from a plugin's handler carries.
const CONTAINMENT: [(HeaderName, &str); 4] = [
    (header::CONTENT_SECURITY_POLICY, "sandbox allow-forms"),
    (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
    (header::CACHE_CONTROL, "no-store"),
    (HeaderName::from_static("x-robots-tag"), "noindex"),
];

fn not_found() -> Response<Body> {
    text(StatusCode::NOT_FOUND, "not found\n")
}

fn bad_gateway() -> Response<Body> {
    text(StatusCode::BAD_GATEWAY, "bad gateway\n")
}

fn unavailable() -> Response<Body> {
    text(StatusCode::SERVICE_UNAVAILABLE, "service unavailable\n")
}

/// Whether `name` can be a plugin name (`[a-z0-9-]{2,48}`, the manifest rule).
fn plugin_name(name: &str) -> bool {
    (2..=48).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// `(plugin, route path)` of a raw `/_p/<plugin>/<rest>` path, or `None` (`404`).
fn split_path(raw: &str) -> Option<(&str, String)> {
    let (name, rest) = raw.strip_prefix("/_p/")?.split_once('/')?;
    if !plugin_name(name) {
        return None;
    }
    let rest = percent_decode_str(rest).decode_utf8().ok()?;
    Some((name, format!("/{rest}")))
}

/// The rate-limit scope of a route: `route:<plugin>:<32 hex of SHA-256(path)>` (F24:
/// manifest paths are unbounded, the bucket is not).
fn limit_scope(plugin: &str, path: &str) -> String {
    let digest = Sha256::digest(path.as_bytes());
    let hex: String = digest[..16].iter().map(|b| format!("{b:02x}")).collect();
    format!("route:{plugin}:{hex}")
}

/// `application/x-www-form-urlencoded` pairs of `query`, or `None` above
/// [`MAX_QUERY_PAIRS`]. Undecodable bytes become U+FFFD.
fn query_pairs(query: Option<&str>) -> Option<Vec<(String, String)>> {
    let decode = |s: &str| {
        let s = s.replace('+', " ");
        percent_decode_str(&s).decode_utf8_lossy().into_owned()
    };
    let mut out = Vec::new();
    for pair in query
        .unwrap_or_default()
        .split('&')
        .filter(|p| !p.is_empty())
    {
        if out.len() == MAX_QUERY_PAIRS {
            return None;
        }
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        out.push((decode(k), decode(v)));
    }
    Some(out)
}

/// The allowlisted request headers, in request order (values that are not visible
/// ASCII are left out).
fn passed_headers(headers: &HeaderMap) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for name in &PASSED_HEADERS {
        for value in headers.get_all(name) {
            if let Ok(v) = value.to_str() {
                out.push((name.as_str().to_owned(), v.to_owned()));
            }
        }
    }
    out
}

/// The handler behind `/_p/{plugin}/` and `/_p/{plugin}/{*path}`; see the module docs.
pub(crate) async fn handle(State(state): State<Arc<AppState>>, req: Request) -> Response<Body> {
    let Some((plugin, path)) = split_path(req.uri().path()) else {
        return not_found();
    };
    let plugin = plugin.to_owned();
    let Ok(spec) = state.plugins.route(&plugin, &path) else {
        return not_found();
    };
    match state.store.read().plugins().get(&plugin).await {
        Ok(Some(row)) if row.enabled => {}
        Ok(_) => return not_found(),
        Err(e) => {
            tracing::error!(plugin, error = %e, "plugin row lookup failed");
            return unavailable();
        }
    }
    let method = req.method().clone();
    if !spec.methods.iter().any(|m| m == method.as_str()) {
        let mut res = text(StatusCode::METHOD_NOT_ALLOWED, "method not allowed\n");
        if let Ok(allow) = HeaderValue::from_str(&spec.methods.join(", ")) {
            res.headers_mut().insert(header::ALLOW, allow);
        }
        return res;
    }
    let safe = method == Method::GET || method == Method::HEAD;
    if !safe && !spec.cross_origin {
        let ok = state
            .origin
            .as_deref()
            .is_some_and(|origin| same_origin(req.headers(), origin));
        if !ok {
            return text(StatusCode::FORBIDDEN, "forbidden\n");
        }
    }
    let Some(auth) = state.auth.clone() else {
        tracing::warn!(plugin, "plugin route without an auth service; refused");
        return unavailable();
    };
    let ip = req.extensions().get::<ClientIp>().map(|c| c.0);
    let (max, window) = spec.rate();
    match auth
        .hit_limit(&limit_scope(&plugin, &path), ip, max, window)
        .await
    {
        Ok(()) => {}
        Err(CoreError::RateLimited { retry_after_secs }) => {
            let mut res = text(StatusCode::TOO_MANY_REQUESTS, "too many requests\n");
            res.headers_mut()
                .insert(header::RETRY_AFTER, HeaderValue::from(retry_after_secs));
            return res;
        }
        Err(e) => {
            tracing::error!(plugin, error = %e, "plugin route rate limit failed");
            return unavailable();
        }
    }
    let Some(query) = query_pairs(req.uri().query()) else {
        return text(StatusCode::BAD_REQUEST, "too many query parameters\n");
    };
    let too_large = || text(StatusCode::PAYLOAD_TOO_LARGE, "payload too large\n");
    let declared = req
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    if declared.is_some_and(|n| n > u64::try_from(MAX_ROUTE_BODY_BYTES).unwrap_or(u64::MAX)) {
        return too_large();
    }
    let headers = passed_headers(req.headers());
    let user = handler::session(&state, req.headers())
        .await
        .filter(|who| !who.user.must_reset)
        .map(|who| who.user.id.to_string());
    let body = match axum::body::to_bytes(req.into_body(), MAX_ROUTE_BODY_BYTES).await {
        Ok(b) => b.to_vec(),
        // Over the limit (or a body that broke off): never passed on.
        Err(_) => return too_large(),
    };
    let route_req = RouteRequest {
        method: method.as_str().to_owned(),
        path,
        query,
        headers,
        body,
        client_ip_hash: ip.map_or_else(String::new, |ip| auth.plugin_ip_hash(&plugin, ip)),
        user,
    };
    match state.plugins.handle_route(&plugin, route_req).await {
        Ok(res) => visitor_response(&plugin, res),
        // Swapped or disabled since the check above.
        Err(
            PluginError::UnknownPlugin(_)
            | PluginError::Disabled { .. }
            | PluginError::RouteNotFound { .. },
        ) => not_found(),
        // The host logged and recorded the failure.
        Err(_) => bad_gateway(),
    }
}

/// Whether `location` is a root-relative path a browser cannot read as another origin:
/// `//host` and `/\host` are network-path references to browsers, and URL parsers drop
/// tabs and newlines first (`/<TAB>/host`), so only visible ASCII without `\` passes.
fn root_relative(location: &str) -> bool {
    location.starts_with('/')
        && !location.starts_with("//")
        && location.bytes().all(|b| b.is_ascii_graphic() && b != b'\\')
}

/// The visitor's response: the plugin's status, `Content-Type`, `Content-Language` and
/// (3xx) root-relative `Location` only, plus the containment headers.
fn visitor_response(plugin: &str, res: RouteResponse) -> Response<Body> {
    let Ok(status) = StatusCode::from_u16(res.status) else {
        return bad_gateway();
    };
    let first = |name: &HeaderName| {
        res.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name.as_str()))
            .map(|(_, v)| v.as_str())
    };
    let mut kept: Vec<(HeaderName, HeaderValue)> = Vec::new();
    for name in [header::CONTENT_TYPE, header::CONTENT_LANGUAGE] {
        if let Some(v) = first(&name).and_then(|v| HeaderValue::from_str(v).ok()) {
            kept.push((name, v));
        }
    }
    if status.is_redirection()
        && let Some(location) = first(&header::LOCATION)
    {
        match HeaderValue::from_str(location) {
            Ok(v) if root_relative(location) => kept.push((header::LOCATION, v)),
            _ => {
                tracing::warn!(
                    plugin,
                    status = res.status,
                    "plugin route redirect off the site refused"
                );
                return bad_gateway();
            }
        }
    }
    let mut out = Response::new(Body::from(res.body));
    *out.status_mut() = status;
    let headers = out.headers_mut();
    for (name, value) in kept {
        headers.insert(name, value);
    }
    for (name, value) in CONTAINMENT {
        headers.insert(name, HeaderValue::from_static(value));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_split_without_resolving() {
        let ok = |raw: &str| split_path(raw).map(|(p, r)| (p.to_owned(), r));
        assert_eq!(
            ok("/_p/router/hello"),
            Some(("router".into(), "/hello".into()))
        );
        assert_eq!(ok("/_p/router/"), Some(("router".into(), "/".into())));
        assert_eq!(
            ok("/_p/router/a%20b"),
            Some(("router".into(), "/a b".into()))
        );
        assert_eq!(
            ok("/_p/router/..%2Fx"),
            Some(("router".into(), "/../x".into()))
        );
        for bad in [
            "/_p/router",
            "/_p/",
            "/_p/Router/x",
            "/_p/r/x",
            "/_p/router%2F..%2Fother/x",
            "/_p/router/%ff",
            "/api/v1/x",
        ] {
            assert_eq!(split_path(bad), None, "{bad}");
        }
        let long = format!("/_p/{}/x", "a".repeat(49));
        assert_eq!(split_path(&long), None);
    }

    #[test]
    fn limit_scope_is_bounded() {
        let s = limit_scope("router", &"/x".repeat(10_000));
        assert_eq!(s.len(), "route:router:".len() + 32);
        assert_ne!(limit_scope("router", "/a"), limit_scope("router", "/b"));
        assert_ne!(limit_scope("router", "/a"), limit_scope("other", "/a"));
        // The longest scope plus `:ip:<32 hex>` fits the store's 200-char bucket cap.
        let max = format!(
            "{}:ip:{}",
            limit_scope(&"a".repeat(48), "/"),
            "0".repeat(32)
        );
        assert!(max.len() <= ferrite_store::MAX_RATE_BUCKET, "{}", max.len());
    }

    #[test]
    fn query_is_decoded_and_capped() {
        assert_eq!(query_pairs(None), Some(Vec::new()));
        assert_eq!(
            query_pairs(Some("a=1&b=x+y%21&c&&d=")),
            Some(vec![
                ("a".into(), "1".into()),
                ("b".into(), "x y!".into()),
                ("c".into(), String::new()),
                ("d".into(), String::new()),
            ])
        );
        let at_cap = vec!["k=v"; MAX_QUERY_PAIRS].join("&");
        assert_eq!(query_pairs(Some(&at_cap)).map(|q| q.len()), Some(100));
        let over = format!("{at_cap}&k=v");
        assert_eq!(query_pairs(Some(&over)), None);
    }

    #[test]
    fn only_root_relative_locations() {
        for ok in ["/", "/thanks/", "/a?b=//c", "/a#x"] {
            assert!(root_relative(ok), "{ok}");
        }
        for bad in [
            "https://evil.example/",
            "//evil.example/",
            "/\\evil.example",
            "/a\\b",
            "/\t/evil.example",
            "/ /evil.example",
            "evil",
            "",
            "javascript:alert(1)",
        ] {
            assert!(!root_relative(bad), "{bad}");
        }
    }

    #[test]
    fn response_keeps_only_safe_headers() {
        let res = RouteResponse {
            status: 200,
            headers: vec![
                ("content-type".into(), "text/html".into()),
                ("Set-Cookie".into(), "a=1".into()),
                ("Content-Security-Policy".into(), "default-src *".into()),
                ("X-Content-Type-Options".into(), "".into()),
                ("Location".into(), "https://evil.example/".into()),
                ("Content-Language".into(), "pl".into()),
                ("Access-Control-Allow-Origin".into(), "*".into()),
            ],
            body: b"<p>x</p>".to_vec(),
        };
        let out = visitor_response("p", res);
        let names: Vec<&str> = out.headers().keys().map(HeaderName::as_str).collect();
        assert_eq!(
            names,
            [
                "content-type",
                "content-language",
                "content-security-policy",
                "x-content-type-options",
                "cache-control",
                "x-robots-tag",
            ]
        );
        assert_eq!(
            out.headers()[header::CONTENT_SECURITY_POLICY],
            "sandbox allow-forms"
        );
        // A cross-site Location on a redirect: 502, not followed.
        let redirect = |loc: &str| RouteResponse {
            status: 302,
            headers: vec![("location".into(), loc.into())],
            body: Vec::new(),
        };
        assert_eq!(
            visitor_response("p", redirect("//evil.example")).status(),
            StatusCode::BAD_GATEWAY
        );
        let ok = visitor_response("p", redirect("/thanks/"));
        assert_eq!(ok.status(), StatusCode::FOUND);
        assert_eq!(ok.headers()[header::LOCATION], "/thanks/");
    }
}
