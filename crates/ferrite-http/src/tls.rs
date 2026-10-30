//! What the built-in TLS (M4-2 P33) adds over HTTP: HSTS on the responses served over
//! TLS ([`with_hsts`]; `D01:529`), and the optional plain HTTP listener that only
//! redirects to the https site ([`redirect_app`]). The TLS listener itself (ACME,
//! TLS-ALPN-01) is the binary's.

use axum::{
    Router,
    http::{HeaderValue, StatusCode, Uri, header},
    response::{IntoResponse, Response},
};
use tower_http::set_header::SetResponseHeaderLayer;

/// `Strict-Transport-Security` on every response served over TLS: one year, no
/// `includeSubDomains` (other hosts under the domain are not ours to pin).
pub const HSTS: &str = "max-age=31536000";

/// `app` with [`HSTS`] on every response. Core owns the header: a value set inside (a
/// plugin route strips it anyway) is overridden. Only the TLS listener serves this; the
/// plain `server.listen` (loopback, healthcheck) does not.
pub fn with_hsts(app: Router) -> Router {
    app.layer(SetResponseHeaderLayer::overriding(
        header::STRICT_TRANSPORT_SECURITY,
        HeaderValue::from_static(HSTS),
    ))
}

/// The redirect listener's app (`server.tls.redirect_http`): every request, whatever
/// its method, path or `Host`, gets `301` to the same path and query on `site_origin`
/// (`https://host[:port]`, from `site.url`: never the `Host` header, so it cannot be
/// made to redirect elsewhere). Nothing else is served there.
pub fn redirect_app(site_origin: &str) -> Router {
    let origin = site_origin.trim_end_matches('/').to_owned();
    Router::new().fallback(move |uri: Uri| {
        let origin = origin.clone();
        async move { redirect(&origin, &uri) }
    })
}

fn redirect(origin: &str, uri: &Uri) -> Response {
    let rest = uri.path_and_query().map_or("/", |pq| pq.as_str());
    let rest = if rest.starts_with('/') { rest } else { "/" };
    let location = HeaderValue::from_str(&format!("{origin}{rest}"))
        .or_else(|_| HeaderValue::from_str(&format!("{origin}/")))
        .unwrap_or_else(|_| HeaderValue::from_static("/"));
    (
        StatusCode::MOVED_PERMANENTLY,
        [
            (header::LOCATION, location),
            (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
        ],
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redirect_keeps_path_and_query() {
        let r = redirect(
            "https://example.com",
            &"/a/b?x=1&y=%20".parse::<Uri>().unwrap(),
        );
        assert_eq!(r.status(), StatusCode::MOVED_PERMANENTLY);
        assert_eq!(
            r.headers()[header::LOCATION],
            "https://example.com/a/b?x=1&y=%20"
        );
        // An absolute-form request target does not choose the host.
        let r = redirect(
            "https://example.com:8443",
            &"http://evil.test/p".parse::<Uri>().unwrap(),
        );
        assert_eq!(r.headers()[header::LOCATION], "https://example.com:8443/p");
        let r = redirect("https://example.com", &"*".parse::<Uri>().unwrap());
        assert_eq!(r.headers()[header::LOCATION], "https://example.com/");
    }
}
