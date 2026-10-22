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
    let etag = first.header("etag").expect("etag").to_owned();
    assert!(etag.starts_with('"') && etag.ends_with('"'), "{etag}");
    for value in [
        etag.clone(),
        format!("W/{etag}"),
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
        assert_eq!(r.header("etag"), Some(etag.as_str()));
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
        let weak = format!("W/{}", plain.header("etag").unwrap());
        assert_eq!(r.header("etag"), Some(weak.as_str()), "{enc}");
        // Revalidating the compressed variant with its weak tag: 304.
        let mut req = with_header(Method::GET, "/hello-world/", header::ACCEPT_ENCODING, enc);
        req.headers_mut()
            .insert(header::IF_NONE_MATCH, weak.parse().unwrap());
        assert_eq!(
            app.send(req).await.status,
            StatusCode::NOT_MODIFIED,
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

app_tests!(
    head_is_get_without_body,
    etag_and_304,
    other_methods_are_405,
    health_and_readiness,
    compresses_html,
    real_server,
);
