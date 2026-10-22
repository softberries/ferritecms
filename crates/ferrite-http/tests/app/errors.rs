//! Canonical and redirect responses, the 404/410/500 pages and their fallbacks.

use std::fs;

use axum::http::StatusCode;

use crate::app_tests;
use crate::harness::{App, SITE_TITLE, load_theme, send};

async fn canonical_and_redirects(app: App) -> App {
    app.post("Hello World", "hello-world", &[]).await;
    app.post("Second Post", "second", &[]).await;

    for (from, to) in [
        ("/hello-world", "/hello-world/"),
        ("/Hello-World/", "/hello-world/"),
        ("/hello-world?x=1", "/hello-world/?x=1"),
        ("/page/1/?s=a", "/?s=a"),
    ] {
        let r = app.get(from).await;
        assert_eq!(r.status, StatusCode::MOVED_PERMANENTLY, "{from}");
        assert_eq!(r.header("location"), Some(to), "{from}");
    }

    let redirects = app.redirects();
    redirects
        .upsert(&app.admin, "/old-hello/", "/hello-world/", 301)
        .await
        .unwrap();
    redirects
        .upsert(&app.admin, "/temp/", "/second/", 302)
        .await
        .unwrap();
    redirects
        .upsert(&app.admin, "/removed/", "/removed-target/", 410)
        .await
        .unwrap();
    for (from, status, to) in [
        (
            "/old-hello/",
            StatusCode::MOVED_PERMANENTLY,
            "/hello-world/",
        ),
        // Trailing slash toggled.
        ("/old-hello", StatusCode::MOVED_PERMANENTLY, "/hello-world/"),
        ("/temp/", StatusCode::FOUND, "/second/"),
    ] {
        let r = app.get(from).await;
        assert_eq!(r.status, status, "{from}");
        assert_eq!(r.header("location"), Some(to), "{from}");
    }
    let r = app.get("/removed/").await;
    assert_eq!(r.status, StatusCode::GONE);
    r.assert_html();
    // MiniJinja escapes `/` in HTML.
    assert!(
        r.body.contains("Nothing at &#x2f;removed&#x2f; (410)"),
        "{}",
        r.body
    );

    // Redirects are checked before the cache: a cached page is redirected at once.
    assert_eq!(app.get("/second/").await.status, StatusCode::OK);
    assert!(app.cached().await >= 1);
    redirects
        .upsert(&app.admin, "/second/", "/hello-world/", 301)
        .await
        .unwrap();
    let r = app.get("/second/").await;
    assert_eq!(r.status, StatusCode::MOVED_PERMANENTLY);
    assert_eq!(r.header("location"), Some("/hello-world/"));
    app
}

async fn not_found_page(app: App) -> App {
    let r = app.get("/no-such-thing/").await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    r.assert_html();
    assert_eq!(r.tpl(), "404");
    assert!(
        r.body
            .contains("Nothing at &#x2f;no-such-thing&#x2f; (404)"),
        "{}",
        r.body
    );
    assert!(r.body.contains("<title>Page not found – Test Site</title>"));
    assert!(r.header("etag").is_none(), "error pages carry no ETag");
    // Path tricks are 404 too, never a 500.
    for path in ["/%2e%2e/", "//x", "/a/../b/", "/%FF/"] {
        assert_eq!(app.get(path).await.status, StatusCode::NOT_FOUND, "{path}");
    }
    assert_eq!(app.cached().await, 0, "404s are never cached");
    app
}

/// Task 8b routes answer 404 until they exist.
async fn task_8b_routes_are_404(app: App) -> App {
    for path in [
        "/feed/",
        "/robots.txt",
        "/sitemap.xml",
        "/_theme/abc/css/site.css",
        &format!("/_media/{}/a.png", "a".repeat(64)),
    ] {
        let r = app.get(path).await;
        assert_eq!(r.status, StatusCode::NOT_FOUND, "{path}");
        assert_eq!(r.tpl(), "404", "{path}");
    }
    app
}

async fn broken_template_is_500_and_server_keeps_serving(app: App) -> App {
    app.post("Hello World", "hello-world", &[]).await;
    // single-post-broken.html includes a template that does not exist.
    app.post("Broken", "broken", &[]).await;
    for _ in 0..2 {
        let r = app.get("/broken/").await;
        assert_eq!(r.status, StatusCode::INTERNAL_SERVER_ERROR, "{r:?}");
        r.assert_html();
        assert_eq!(r.tpl(), "500");
        assert!(r.body.contains("Broken (500)"));
    }
    assert_eq!(app.cached().await, 0, "500s are never cached");
    let r = app.get("/hello-world/").await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(r.body.contains("<h1>Hello World</h1>"));
    app
}

/// A theme without `404.html`/`500.html` whose every template fails: the built-in
/// pages answer.
async fn builtin_error_pages(app: App) -> App {
    app.post("Hello World", "hello-world", &[]).await;
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("templates")).unwrap();
    fs::write(
        dir.path().join("theme.toml"),
        "[theme]\nname = \"broken\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    fs::write(
        dir.path().join("templates/index.html"),
        "{% include \"missing.html\" %}",
    )
    .unwrap();
    let state = App::state(&app.site, load_theme(dir.path().to_path_buf()), true).await;
    let router = ferrite_http::build_app(state.clone());

    let r = send(
        &router,
        crate::harness::request(axum::http::Method::GET, "/hello-world/"),
    )
    .await;
    assert_eq!(r.status, StatusCode::INTERNAL_SERVER_ERROR);
    r.assert_html();
    assert!(
        r.body.contains("<h1>Something went wrong</h1>"),
        "{}",
        r.body
    );
    assert!(r.body.contains(&format!(
        "<title>Something went wrong – {SITE_TITLE}</title>"
    )));

    // 404 falls back to index.html, which fails too: the built-in 404.
    let r = send(
        &router,
        crate::harness::request(axum::http::Method::GET, "/nope/"),
    )
    .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    assert!(r.body.contains("<h1>Page not found</h1>"), "{}", r.body);
    assert_eq!(state.cache.len().await, 0);
    app
}

app_tests!(
    canonical_and_redirects,
    not_found_page,
    task_8b_routes_are_404,
    broken_template_is_500_and_server_keeps_serving,
    builtin_error_pages,
);
