//! The admin SPA under `/admin/` (M2b-1 Task 1): the bare-path redirect, the SPA
//! fallback, immutable hashed assets, 404s that never fall back, the CSP and the other
//! admin headers (Review Focus 1 and 2), methods, traversal, the placeholder, and that
//! admin paths never reach redirect rows or the page cache. Served from an in-memory
//! asset set through `AppState::with_admin_assets`.

use std::{borrow::Cow, collections::HashMap, sync::Arc};

use axum::http::{Method, StatusCode, header};
use ferrite_http::admin::{AdminAssets, AdminFile};
use sha2::{Digest, Sha256};

use crate::app_tests;
use crate::harness::{App, Resp, request};

const INDEX: &str = "<!doctype html><html><head><script type=\"module\" src=\"/admin/assets/index-abc.js\"></script></head><body><div id=\"root\"></div></body></html>";
const JS: &str = "console.log('admin');";
const ROBOTS: &str = "User-agent: *\nDisallow: /\n";

/// The exact CSP without a Direct-S3 media origin.
const CSP: &str = "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob: https:; font-src 'self'; connect-src 'self'; frame-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'; object-src 'none'; worker-src 'self'";
/// The exact CSP with `https://cdn.example.com` as the media origin.
const CSP_CDN: &str = "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob: https: https://cdn.example.com; font-src 'self'; connect-src 'self'; frame-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'; object-src 'none'; worker-src 'self'";

/// An in-memory admin build.
struct Fixture(HashMap<&'static str, &'static [u8]>);

impl AdminAssets for Fixture {
    fn get(&self, path: &str) -> Option<AdminFile> {
        self.0.get(path).map(|b| AdminFile {
            bytes: Cow::Borrowed(*b),
            sha256: Sha256::digest(b).into(),
        })
    }
}

fn built() -> Arc<dyn AdminAssets> {
    Arc::new(Fixture(HashMap::from([
        ("index.html", INDEX.as_bytes()),
        ("assets/index-abc.js", JS.as_bytes()),
        ("assets/font-x.woff2", b"wOF2 bytes".as_slice()),
        ("robots.txt", ROBOTS.as_bytes()),
        (
            "favicon.svg",
            b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>".as_slice(),
        ),
    ])))
}

fn empty() -> Arc<dyn AdminAssets> {
    Arc::new(Fixture(HashMap::new()))
}

async fn with_build(app: App) -> App {
    app.with_admin(App::config(), built()).await
}

fn etag_of(body: &[u8]) -> String {
    let digest = Sha256::digest(body);
    let hex: String = digest[..8].iter().map(|b| format!("{b:02x}")).collect();
    format!("W/\"{hex}\"")
}

/// The headers every admin response carries.
fn assert_admin_headers(r: &Resp, csp: &str) {
    assert_eq!(r.header("content-security-policy"), Some(csp), "{r:?}");
    assert_eq!(r.header("referrer-policy"), Some("no-referrer"), "{r:?}");
    assert_eq!(r.header("x-frame-options"), Some("DENY"), "{r:?}");
    assert_eq!(
        r.header("cross-origin-opener-policy"),
        Some("same-origin"),
        "{r:?}"
    );
    assert_eq!(r.header("x-content-type-options"), Some("nosniff"), "{r:?}");
    assert_eq!(
        r.header("permissions-policy"),
        Some("camera=(), microphone=(), geolocation=()"),
        "{r:?}"
    );
    assert_eq!(r.header("x-robots-tag"), Some("noindex"), "{r:?}");
}

async fn admin_redirects_bare_path(app: App) -> App {
    let app = with_build(app).await;
    for (from, to) in [("/admin", "/admin/"), ("/admin?tab=x", "/admin/?tab=x")] {
        let r = app.get(from).await;
        assert_eq!(r.status, StatusCode::PERMANENT_REDIRECT, "{from}");
        assert_eq!(r.header("location"), Some(to), "{from}");
        assert_admin_headers(&r, CSP);
    }
    let r = app.send(request(Method::HEAD, "/admin")).await;
    assert_eq!(r.status, StatusCode::PERMANENT_REDIRECT);
    app
}

async fn spa_fallback_serves_index(app: App) -> App {
    let app = with_build(app).await;
    for path in [
        "/admin/",
        "/admin/content/post",
        "/admin/reset-password",
        "/admin/content/post/",
        "/admin/users/john.doe",
    ] {
        let r = app.get(path).await;
        assert_eq!(r.status, StatusCode::OK, "{path}");
        assert_eq!(r.body, INDEX, "{path}");
        assert_eq!(r.header("cache-control"), Some("no-cache"), "{path}");
        assert_eq!(
            r.header("content-type"),
            Some("text/html; charset=utf-8"),
            "{path}"
        );
        assert_eq!(r.header("vary"), Some("accept-encoding, cookie"), "{path}");
        assert_eq!(
            r.header("etag"),
            Some(etag_of(INDEX.as_bytes()).as_str()),
            "{path}"
        );
        assert_admin_headers(&r, CSP);
    }
    // Compression goes through the shared layer; the weak tag stays the same.
    let mut req = request(Method::GET, "/admin/content/post");
    req.headers_mut()
        .insert(header::ACCEPT_ENCODING, "gzip".parse().unwrap());
    let r = app.send(req).await;
    assert_eq!(r.header("content-encoding"), Some("gzip"));
    assert_eq!(r.header("etag"), Some(etag_of(INDEX.as_bytes()).as_str()));
    assert_admin_headers(&r, CSP);
    // HEAD: same status and headers, no body.
    let r = app.send(request(Method::HEAD, "/admin/content/post")).await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(r.raw.is_empty());
    assert_eq!(r.header("cache-control"), Some("no-cache"));
    // Root files with an extension are served as themselves, not cached long.
    let r = app.get("/admin/robots.txt").await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.body, ROBOTS);
    assert_eq!(r.header("content-type"), Some("text/plain; charset=utf-8"));
    assert_eq!(r.header("cache-control"), Some("no-cache"));
    let r = app.get("/admin/favicon.svg").await;
    assert_eq!(r.header("content-type"), Some("image/svg+xml"));
    // `index.html` itself.
    let r = app.get("/admin/index.html").await;
    assert_eq!(r.body, INDEX);
    assert_eq!(r.header("cache-control"), Some("no-cache"));
    app
}

/// `GET /admin/…` with the theme cookie (P57).
async fn get_with_theme(app: &App, path: &str, cookie: &str) -> Resp {
    let mut req = request(Method::GET, path);
    req.headers_mut()
        .insert(header::COOKIE, cookie.parse().unwrap());
    app.send(req).await
}

async fn index_html_gets_theme_class_from_cookie(app: App) -> App {
    let app = with_build(app).await;
    for (cookie, class) in [
        ("ferrite_admin_theme=dark", "dark"),
        ("ferrite_admin_theme=light", "light"),
        ("a=1; ferrite_admin_theme=dark; b=2", "dark"),
        ("ferrite_admin_theme=light;other=x", "light"),
    ] {
        let themed = INDEX.replacen("<html>", &format!("<html class=\"{class}\">"), 1);
        for path in ["/admin/", "/admin/content/post", "/admin/index.html"] {
            let r = get_with_theme(&app, path, cookie).await;
            assert_eq!(r.status, StatusCode::OK, "{cookie} {path}");
            assert_eq!(r.body, themed, "{cookie} {path}");
            assert_eq!(r.header("cache-control"), Some("no-cache"), "{path}");
            assert_eq!(r.header("vary"), Some("accept-encoding, cookie"), "{path}");
            // Its own validator: the plain page's tag does not revalidate it.
            let etag = etag_of(themed.as_bytes());
            assert_eq!(r.header("etag"), Some(etag.as_str()), "{cookie} {path}");
            assert_admin_headers(&r, CSP);
            let mut req = request(Method::GET, path);
            req.headers_mut()
                .insert(header::COOKIE, cookie.parse().unwrap());
            req.headers_mut().insert(
                header::IF_NONE_MATCH,
                etag_of(INDEX.as_bytes()).parse().unwrap(),
            );
            assert_eq!(
                app.send(req).await.status,
                StatusCode::OK,
                "{cookie} {path}"
            );
        }
    }
    // Any other value, or a cookie of another name, is ignored: the page is unchanged.
    for cookie in [
        "ferrite_admin_theme=system",
        "ferrite_admin_theme=Dark",
        "ferrite_admin_theme=",
        "ferrite_admin_theme=dark%22%3E%3Cscript%3Ealert(1)%3C%2Fscript%3E",
        "ferrite_admin_theme=\"dark\"",
        "ferrite_admin_theme=dark\" onload=\"x",
        "xferrite_admin_theme=dark",
        "ferrite_admin_theme_x=dark",
    ] {
        let r = get_with_theme(&app, "/admin/", cookie).await;
        assert_eq!(r.status, StatusCode::OK, "{cookie}");
        assert_eq!(r.body, INDEX, "{cookie}");
        assert_eq!(
            r.header("etag"),
            Some(etag_of(INDEX.as_bytes()).as_str()),
            "{cookie}"
        );
        assert_admin_headers(&r, CSP);
    }
    // Assets are not rewritten and do not vary on the cookie.
    let r = get_with_theme(
        &app,
        "/admin/assets/index-abc.js",
        "ferrite_admin_theme=dark",
    )
    .await;
    assert_eq!(r.body, JS);
    assert_eq!(r.header("vary"), Some("accept-encoding"));
    app
}

async fn hashed_assets_are_immutable(app: App) -> App {
    let app = with_build(app).await;
    let r = app.get("/admin/assets/index-abc.js").await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.body, JS);
    assert_eq!(
        r.header("cache-control"),
        Some("public, max-age=31536000, immutable")
    );
    assert_eq!(
        r.header("content-type"),
        Some("text/javascript; charset=utf-8")
    );
    let etag = etag_of(JS.as_bytes());
    assert_eq!(r.header("etag"), Some(etag.as_str()));
    assert_eq!(r.header("vary"), Some("accept-encoding"));
    assert_admin_headers(&r, CSP);
    // Weak comparison: the strong form of the tag matches too.
    for inm in [etag.clone(), etag.trim_start_matches("W/").to_owned()] {
        let mut req = request(Method::GET, "/admin/assets/index-abc.js");
        req.headers_mut()
            .insert(header::IF_NONE_MATCH, inm.parse().unwrap());
        let r = app.send(req).await;
        assert_eq!(r.status, StatusCode::NOT_MODIFIED, "{inm}");
        assert!(r.raw.is_empty());
        assert_eq!(r.header("etag"), Some(etag.as_str()));
        assert_eq!(
            r.header("cache-control"),
            Some("public, max-age=31536000, immutable")
        );
        assert_admin_headers(&r, CSP);
    }
    let mut req = request(Method::GET, "/admin/assets/index-abc.js");
    req.headers_mut()
        .insert(header::IF_NONE_MATCH, "W/\"0000\"".parse().unwrap());
    assert_eq!(app.send(req).await.status, StatusCode::OK);
    let r = app.get("/admin/assets/font-x.woff2").await;
    assert_eq!(r.header("content-type"), Some("font/woff2"));
    app
}

async fn missing_asset_is_404_not_index(app: App) -> App {
    let app = with_build(app).await;
    for path in [
        "/admin/assets/index-missing.js",
        "/admin/assets/",
        "/admin/assets/sub/x",
        "/admin/favicon.ico",
        "/admin/manifest.webmanifest",
        "/admin/content/post.json",
    ] {
        let r = app.get(path).await;
        assert_eq!(r.status, StatusCode::NOT_FOUND, "{path}");
        assert_eq!(r.body, "not found\n", "{path}");
        assert!(!r.body.contains("root"), "{path}");
        assert_admin_headers(&r, CSP);
    }
    app
}

async fn admin_index_has_the_csp(app: App) -> App {
    let app = with_build(app).await;
    let r = app.get("/admin/").await;
    assert_eq!(r.header("content-security-policy"), Some(CSP));
    assert_eq!(ferrite_http::admin::admin_csp(None), CSP);
    let mut config = App::config();
    config.media_origin = Some("https://cdn.example.com".into());
    let app = app.with_admin(config, built()).await;
    for path in [
        "/admin/",
        "/admin/content/post",
        "/admin/assets/index-abc.js",
    ] {
        let r = app.get(path).await;
        assert_eq!(r.status, StatusCode::OK, "{path}");
        assert_eq!(r.header("content-security-policy"), Some(CSP_CDN), "{path}");
    }
    assert_eq!(
        ferrite_http::admin::admin_csp(Some("https://cdn.example.com")),
        CSP_CDN
    );
    app
}

async fn admin_responses_send_no_referrer(app: App) -> App {
    let app = with_build(app).await;
    for path in [
        "/admin/",
        "/admin/reset-password",
        "/admin/assets/index-abc.js",
        "/admin/assets/nope.js",
        "/admin",
    ] {
        let r = app.get(path).await;
        assert_eq!(r.header("referrer-policy"), Some("no-referrer"), "{path}");
    }
    let app = app.with_admin(App::config(), empty()).await;
    let r = app.get("/admin/reset-password").await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(r.body.contains("FerriteCMS admin not built"));
    assert_eq!(r.header("referrer-policy"), Some("no-referrer"));
    // The public site keeps its own policy.
    let r = app.get("/").await;
    assert_eq!(
        r.header("referrer-policy"),
        Some("strict-origin-when-cross-origin")
    );
    assert_eq!(r.header("content-security-policy"), None);
    app
}

async fn admin_rejects_unsafe_methods(app: App) -> App {
    let app = with_build(app).await;
    for method in [Method::POST, Method::PUT, Method::PATCH, Method::DELETE] {
        for path in [
            "/admin",
            "/admin/",
            "/admin/content/post",
            "/admin/assets/index-abc.js",
        ] {
            let r = app.send(request(method.clone(), path)).await;
            assert_eq!(r.status, StatusCode::METHOD_NOT_ALLOWED, "{method} {path}");
            assert_eq!(r.header("allow"), Some("GET, HEAD"), "{method} {path}");
            assert_admin_headers(&r, CSP);
        }
    }
    app
}

async fn admin_path_traversal_is_404(app: App) -> App {
    let app = with_build(app).await;
    for path in [
        "/admin/assets/../../secret.key",
        "/admin/../secret.key",
        "/admin/%2e%2e/secret.key",
        "/admin/assets/%2e%2e/%2e%2e/secret.key",
        "/admin/assets/%2E%2E/index-abc.js",
        "/admin/./index.html",
        "/admin/assets/..%5c..%5csecret.key",
        "/admin/content%5cpost",
        "/admin/content%00post",
        "/admin/%ff",
    ] {
        let r = app.get(path).await;
        assert_eq!(r.status, StatusCode::NOT_FOUND, "{path}");
        assert_eq!(r.body, "not found\n", "{path}");
        assert_admin_headers(&r, CSP);
    }
    app
}

async fn placeholder_without_dist(app: App) -> App {
    let app = app.with_admin(App::config(), empty()).await;
    for path in ["/admin/", "/admin/content/post", "/admin/reset-password"] {
        let r = app.get(path).await;
        assert_eq!(r.status, StatusCode::OK, "{path}");
        assert!(
            r.body.contains("<title>FerriteCMS admin not built</title>"),
            "{path}"
        );
        assert!(
            r.body
                .contains("pnpm -C admin install &amp;&amp; pnpm -C admin build"),
            "{path}"
        );
        assert!(!r.body.to_ascii_lowercase().contains("<script"), "{path}");
        assert_eq!(r.header("cache-control"), Some("no-cache"), "{path}");
        assert_eq!(
            r.header("content-type"),
            Some("text/html; charset=utf-8"),
            "{path}"
        );
        assert!(r.header("etag").is_some_and(|e| e.starts_with("W/\"")));
        assert_admin_headers(&r, CSP);
    }
    // Assets and files with a known extension are still plain 404s.
    for path in ["/admin/assets/index-abc.js", "/admin/favicon.svg"] {
        assert_eq!(app.get(path).await.status, StatusCode::NOT_FOUND, "{path}");
    }
    app
}

async fn admin_is_not_cached_or_redirected(app: App) -> App {
    let app = with_build(app).await;
    // Redirect rows cannot be written under the reserved `admin` word.
    for source in ["/admin/x", "/admin/", "/admin"] {
        let res = app
            .redirects()
            .upsert(&app.admin, source, "/elsewhere/", 301)
            .await;
        assert!(res.is_err(), "{source}: {res:?}");
    }
    let before = app.state.cache.stats().await;
    for path in [
        "/admin/",
        "/admin/content/post",
        "/admin/assets/index-abc.js",
        "/admin/assets/missing.js",
        "/admin",
    ] {
        app.get(path).await;
        app.get(path).await;
    }
    assert_eq!(app.state.cache.stats().await, before);
    assert_eq!(app.state.renders(), 0);
    app
}

app_tests!(
    admin_redirects_bare_path,
    spa_fallback_serves_index,
    index_html_gets_theme_class_from_cookie,
    hashed_assets_are_immutable,
    missing_asset_is_404_not_index,
    admin_index_has_the_csp,
    admin_responses_send_no_referrer,
    admin_rejects_unsafe_methods,
    admin_path_traversal_is_404,
    placeholder_without_dist,
    admin_is_not_cached_or_redirected,
);
