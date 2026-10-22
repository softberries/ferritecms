//! HTTP mechanics: HEAD, ETag/304, 405, health checks, compression, a real server.

use axum::{
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::app_tests;
use crate::harness::{App, fixture, load_theme, request, send};

fn with_header(method: Method, path: &str, name: header::HeaderName, value: &str) -> Request<Body> {
    let mut req = request(method, path);
    req.headers_mut().insert(name, value.parse().unwrap());
    req
}

async fn head_is_get_without_body(app: App) -> App {
    app.post("Hello World", "hello-world", &[]).await;
    let get = app.get("/hello-world/").await;
    let head = app.send(request(Method::HEAD, "/hello-world/")).await;
    assert_eq!(head.status, StatusCode::OK);
    assert!(head.body.is_empty(), "{:?}", head.body);
    for name in [
        "content-type",
        "etag",
        "x-content-type-options",
        "x-frame-options",
    ] {
        assert_eq!(head.header(name), get.header(name), "{name}");
    }
    // Compressed like GET.
    let head = app
        .send(with_header(
            Method::HEAD,
            "/hello-world/",
            header::ACCEPT_ENCODING,
            "gzip",
        ))
        .await;
    assert_eq!(head.header("content-encoding"), Some("gzip"));
    assert!(head.body.is_empty());
    // HEAD of a 404.
    let head = app.send(request(Method::HEAD, "/missing/")).await;
    assert_eq!(head.status, StatusCode::NOT_FOUND);
    assert!(head.body.is_empty());
    app
}

async fn etag_and_304(app: App) -> App {
    app.post("Hello World", "hello-world", &[]).await;
    let first = app.get("/hello-world/").await;
    // Weak on every HTML page; Vary on the 200 and the 304.
    let etag = first.header("etag").expect("etag").to_owned();
    assert!(etag.starts_with("W/\"") && etag.ends_with('"'), "{etag}");
    assert_eq!(first.header("vary"), Some("accept-encoding"));
    let strong = etag.trim_start_matches("W/").to_owned();
    for value in [
        etag.clone(),
        strong.clone(),
        format!("\"x\", {etag}"),
        "*".into(),
    ] {
        let r = app
            .send(with_header(
                Method::GET,
                "/hello-world/",
                header::IF_NONE_MATCH,
                &value,
            ))
            .await;
        assert_eq!(r.status, StatusCode::NOT_MODIFIED, "{value}");
        assert!(r.body.is_empty());
        assert_eq!(r.header("etag"), Some(etag.as_str()), "{value}");
        assert_eq!(r.header("vary"), Some("accept-encoding"), "{value}");
    }
    let r = app
        .send(with_header(
            Method::GET,
            "/hello-world/",
            header::IF_NONE_MATCH,
            "\"other\"",
        ))
        .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.body, first.body);
    // Never 304 for an error page.
    let r = app
        .send(with_header(
            Method::GET,
            "/missing/",
            header::IF_NONE_MATCH,
            "*",
        ))
        .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    app
}

async fn other_methods_are_405(app: App) -> App {
    app.post("Hello World", "hello-world", &[]).await;
    for (method, path) in [
        (Method::POST, "/"),
        (Method::PUT, "/hello-world/"),
        (Method::DELETE, "/hello-world/"),
        (Method::PATCH, "/healthz"),
        (Method::OPTIONS, "/missing/"),
    ] {
        let r = app.send(request(method.clone(), path)).await;
        assert_eq!(r.status, StatusCode::METHOD_NOT_ALLOWED, "{method} {path}");
        assert_eq!(r.header("allow"), Some("GET, HEAD"));
        r.assert_security_headers();
    }
    assert_eq!(app.cached().await, 0);
    app
}

async fn health_and_readiness(app: App) -> App {
    let r = app.get("/healthz").await;
    assert_eq!((r.status, r.body.as_str()), (StatusCode::OK, "ok\n"));
    assert_eq!(r.header("cache-control"), Some("no-store"));
    let r = app.get("/readyz").await;
    assert_eq!((r.status, r.body.as_str()), (StatusCode::OK, "ready\n"));
    // Exact paths only.
    assert_eq!(app.get("/healthz/").await.status, StatusCode::NOT_FOUND);

    let state = App::state(&app.site, load_theme(fixture("app-theme")), false).await;
    let r = send(
        &ferrite_http::build_app(state),
        request(Method::GET, "/readyz"),
    )
    .await;
    assert_eq!(
        r.status,
        StatusCode::SERVICE_UNAVAILABLE,
        "incompatible migrations"
    );
    app
}

async fn compresses_html(app: App) -> App {
    app.post("Hello World", "hello-world", &[]).await;
    let plain = app.get("/hello-world/").await;
    assert_eq!(plain.header("content-encoding"), None);
    for enc in ["br", "gzip"] {
        let r = app
            .send(with_header(
                Method::GET,
                "/hello-world/",
                header::ACCEPT_ENCODING,
                enc,
            ))
            .await;
        assert_eq!(r.status, StatusCode::OK);
        assert_eq!(r.header("content-encoding"), Some(enc), "{enc}");
        assert_ne!(r.body, plain.body, "{enc}: compressed body");
        // Same weak tag as the identity body; one `Vary` value (not duplicated by
        // the compression layer).
        let etag = plain.header("etag").unwrap();
        assert!(etag.starts_with("W/"), "{etag}");
        assert_eq!(r.header("etag"), Some(etag), "{enc}");
        let vary: Vec<_> = r.headers.get_all("vary").iter().collect();
        assert_eq!(vary, ["accept-encoding"], "{enc}");
        // Revalidating the compressed variant: 304 with ETag and Vary.
        let mut req = with_header(Method::GET, "/hello-world/", header::ACCEPT_ENCODING, enc);
        req.headers_mut()
            .insert(header::IF_NONE_MATCH, etag.parse().unwrap());
        let not_modified = app.send(req).await;
        assert_eq!(not_modified.status, StatusCode::NOT_MODIFIED, "{enc}");
        assert_eq!(not_modified.header("etag"), Some(etag), "{enc}");
        assert_eq!(
            not_modified.header("vary"),
            Some("accept-encoding"),
            "{enc}"
        );
        r.assert_security_headers();
    }
    app
}

/// Over a real socket with `axum::serve`, then a graceful shutdown.
async fn real_server(app: App) -> App {
    app.post("Hello World", "hello-world", &[]).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(
        axum::serve(listener, app.router.clone())
            .with_graceful_shutdown(async {
                let _ = stopped.await;
            })
            .into_future(),
    );
    for (method, want_body) in [("GET", true), ("HEAD", false)] {
        let mut sock = tokio::net::TcpStream::connect(addr).await.unwrap();
        sock.write_all(
            format!(
                "{method} /hello-world/ HTTP/1.1\r\nHost: example.com\r\nConnection: close\r\n\r\n"
            )
            .as_bytes(),
        )
        .await
        .unwrap();
        let mut raw = String::new();
        sock.read_to_string(&mut raw).await.unwrap();
        assert!(raw.starts_with("HTTP/1.1 200 OK\r\n"), "{raw}");
        assert!(
            raw.to_ascii_lowercase()
                .contains("x-frame-options: sameorigin")
        );
        assert_eq!(
            raw.contains("<h1>Hello World</h1>"),
            want_body,
            "{method}: {raw}"
        );
    }
    stop.send(()).unwrap();
    server.await.unwrap().unwrap();
    app
}

/// With no render slot free, a render waits `render_wait` and answers 503 with
/// `Retry-After` (R28), never cached; health checks need no slot.
async fn saturated_renders_are_503(app: App) -> App {
    app.post("Hello World", "hello-world", &[]).await;
    let mut config = ferrite_http::HttpConfig::new(crate::common::SITE_URL, "Test Site", "en");
    config.max_concurrent_renders = 0;
    config.render_wait = std::time::Duration::from_millis(50);
    let state = App::state_with(&app.site, load_theme(fixture("app-theme")), true, config).await;
    let router = ferrite_http::build_app(state.clone());
    for path in ["/hello-world/", "/missing/"] {
        let r = send(&router, request(Method::GET, path)).await;
        assert_eq!(r.status, StatusCode::SERVICE_UNAVAILABLE, "{path}");
        assert_eq!(r.header("retry-after"), Some("5"), "{path}");
        r.assert_html();
    }
    assert_eq!(state.cache.len().await, 0, "503s are never cached");
    assert_eq!(state.renders(), 0);
    let r = send(&router, request(Method::GET, "/healthz")).await;
    assert_eq!(r.status, StatusCode::OK);
    app
}

/// The cache key's host is the site's host (R25): `Host` values share one entry.
async fn host_header_does_not_split_the_cache(app: App) -> App {
    app.post("Hello World", "hello-world", &[]).await;
    for host in ["example.com", "EVIL.example", "a.b.c", "localhost:1"] {
        let mut req = request(Method::GET, "/hello-world/");
        req.headers_mut()
            .insert(header::HOST, host.parse().unwrap());
        assert_eq!(app.send(req).await.status, StatusCode::OK, "{host}");
    }
    assert_eq!(app.cached().await, 1);
    app
}

app_tests!(
    saturated_renders_are_503,
    host_header_does_not_split_the_cache,
    head_is_get_without_body,
    etag_and_304,
    other_methods_are_405,
    health_and_readiness,
    compresses_html,
    real_server,
);
