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
    assert!(r.body.contains("Nothing here (410)"), "{}", r.body);
    assert!(r.body.contains("<title>Page gone – Test Site</title>"));
    // The shared 410 body, cached under its own internal key.
    let before = app.state.renders();
    assert_eq!(app.get("/removed").await.status, StatusCode::GONE);
    assert_eq!(app.state.renders(), before, "410 body rendered once");

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
    assert!(r.body.contains("Nothing here (404)"), "{}", r.body);
    // The template gets no request path; the head has no canonical URL.
    assert!(r.body.contains("<p class=\"path\">[]</p>"), "{}", r.body);
    assert!(!r.body.contains("no-such-thing"), "{}", r.body);
    assert!(!r.body.contains("rel=\"canonical\""));
    assert!(
        r.body
            .contains("<meta name=\"robots\" content=\"noindex\">")
    );
    assert!(r.body.contains("<title>Page not found – Test Site</title>"));
    assert!(r.header("etag").is_none(), "error pages carry no ETag");
    assert_eq!(r.header("vary"), Some("accept-encoding"));
    // Path tricks are 404 too, never a 500, and share the same body.
    for path in ["/%2e%2e/", "//x", "/a/../b/", "/%FF/"] {
        let t = app.get(path).await;
        assert_eq!(t.status, StatusCode::NOT_FOUND, "{path}");
        assert_eq!(t.body, r.body, "{path}");
    }
    assert_eq!(
        app.cached().await,
        1,
        "one shared 404 entry, no per-path entry"
    );
    // Content published at a missing path shows at once (no poll): nothing was
    // cached under its path.
    app.post("No Such Thing", "no-such-thing", &[]).await;
    assert_eq!(app.get("/no-such-thing/").await.status, StatusCode::OK);
    app
}

/// 100 random missing URLs render the 404 body once (R26).
async fn random_404s_render_once(app: App) -> App {
    let before = app.state.renders();
    for i in 0..100u32 {
        let path = format!("/missing-{}-{i}/", ferrite_domain::ContentId::new());
        let r = app.get(&path).await;
        assert_eq!(r.status, StatusCode::NOT_FOUND, "{path}");
    }
    assert_eq!(app.state.renders() - before, 1, "rendered once");
    assert_eq!(app.cached().await, 1);
    app
}

/// The shared 404 body records its deps: a menu change refreshes it after a poll.
async fn shared_404_follows_its_deps(app: App) -> App {
    let one = app.post("Menu Target", "menu-target", &[]).await;
    let mut poller = app.poller().await;
    assert!(!app.get("/nope/").await.body.contains("Menu Target"));
    crate::freshness::menu(&app, &[one.id]).await;
    poller.poll_once().await.unwrap();
    let r = app.get("/nope/").await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    assert!(r.body.contains(">Menu Target<"), "{}", r.body);
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
    // The built-in 404 is the shared body too (short TTL); no 500 is cached.
    assert_eq!(state.cache.len().await, 1);
    app
}

app_tests!(
    canonical_and_redirects,
    not_found_page,
    random_404s_render_once,
    shared_404_follows_its_deps,
    task_8b_routes_are_404,
    broken_template_is_500_and_server_keeps_serving,
    builtin_error_pages,
);
