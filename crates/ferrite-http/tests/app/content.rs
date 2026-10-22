//! Every content route type, end to end, plus a plugin's head tag in a served page.

use axum::http::StatusCode;
use ferrite_core::site::FRONT_PAGE_OPTION;
use serde_json::json;

use crate::app_tests;
use crate::harness::App;

/// `/YYYY/` and `/YYYY/MM/` of an RFC 3339 UTC timestamp.
fn date_paths(rfc3339: &str) -> (String, String) {
    (
        format!("/{}/", &rfc3339[..4]),
        format!("/{}/{}/", &rfc3339[..4], &rfc3339[5..7]),
    )
}

async fn serves_every_content_route(app: App) -> App {
    let news = app.category("news").await;
    let hello = app.post("Hello World", "hello-world", &[news]).await;
    app.post("Second Post", "second", &[]).await;
    let about = app.page("About Us", "about", None).await;
    app.page("Our Team", "team", Some(about.id)).await;

    let r = app.get("/hello-world/").await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    r.assert_html();
    assert_eq!(r.tpl(), "single");
    assert!(r.body.contains("<h1>Hello World</h1>"), "{}", r.body);
    assert!(r.body.contains("Body of hello-world"), "{}", r.body);
    assert!(r.header("etag").is_some());

    let r = app.get("/about/").await;
    assert_eq!((r.status, r.tpl()), (StatusCode::OK, "page"), "{r:?}");
    assert!(r.body.contains("<h1>About Us</h1>"));

    let r = app.get("/about/team/").await;
    assert_eq!((r.status, r.tpl()), (StatusCode::OK, "page"), "{r:?}");
    assert!(r.body.contains("<h1>Our Team</h1>"));

    // Front page listing posts: front-page.html → home.html → index.html.
    let r = app.get("/").await;
    assert_eq!((r.status, r.tpl()), (StatusCode::OK, "index"), "{r:?}");
    assert!(r.body.contains("Hello World") && r.body.contains("Second Post"));
    assert!(
        !r.body.contains("About Us"),
        "pages are not in the post listing"
    );

    let r = app.get("/post/").await;
    assert_eq!((r.status, r.tpl()), (StatusCode::OK, "archive"), "{r:?}");
    assert!(r.body.contains("Hello World") && r.body.contains("Second Post"));

    let r = app.get("/category/news/").await;
    assert_eq!((r.status, r.tpl()), (StatusCode::OK, "archive"), "{r:?}");
    assert!(r.body.contains("Hello World"));
    assert!(!r.body.contains("Second Post"));

    let r = app.get("/author/ada/").await;
    assert_eq!((r.status, r.tpl()), (StatusCode::OK, "archive"), "{r:?}");
    assert!(r.body.contains("Hello World") && r.body.contains("Second Post"));

    let published = hello.published_at.expect("published").to_rfc3339();
    let (year, month) = date_paths(&published);
    for path in [&year, &month] {
        let r = app.get(path).await;
        assert_eq!(
            (r.status, r.tpl()),
            (StatusCode::OK, "archive"),
            "{path}: {r:?}"
        );
        assert!(r.body.contains("Hello World"), "{path}");
    }
    // R27: a date archive without public posts is 404, like an empty term.
    assert_eq!(
        app.get("/1999/").await.status,
        StatusCode::NOT_FOUND,
        "empty year"
    );

    let r = app.get("/?s=Hello").await;
    assert_eq!((r.status, r.tpl()), (StatusCode::OK, "search"), "{r:?}");
    assert!(
        r.body.contains("<p class=\"query\">Hello</p>"),
        "{}",
        r.body
    );
    assert!(r.body.contains("Hello World"));
    assert!(!r.body.contains("Second Post"));

    // Paging past the end is 404; page 1 under /page/1/ redirects.
    assert_eq!(app.get("/page/2/").await.status, StatusCode::NOT_FOUND);
    let r = app.get("/page/1/").await;
    assert_eq!(r.status, StatusCode::MOVED_PERMANENTLY);
    assert_eq!(r.header("location"), Some("/"));
    app
}

async fn static_front_page(app: App) -> App {
    let home = app.page("Welcome Home", "home", None).await;
    app.post("Some Post", "some-post", &[]).await;
    app.site
        .store
        .read()
        .options()
        .set(FRONT_PAGE_OPTION, &json!(home.id.to_string()), false)
        .await
        .unwrap();
    let r = app.get("/").await;
    assert_eq!((r.status, r.tpl()), (StatusCode::OK, "page"), "{r:?}");
    assert!(r.body.contains("<h1>Welcome Home</h1>"));
    assert!(!r.body.contains("Some Post"));
    assert_eq!(app.get("/page/2/").await.status, StatusCode::NOT_FOUND);
    app
}

async fn plugin_head_meta_is_served(app: App) -> App {
    app.post("Hello World", "hello-world", &[]).await;
    app.site.install("hello").await;
    assert_eq!(app.site.load().await.loaded, ["hello"]);
    let r = app.get("/hello-world/").await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(
        r.body
            .contains("<meta name=\"generator\" content=\"ferrite-hello\">"),
        "{}",
        r.body
    );
    // Core head too: title and canonical link.
    assert!(
        r.body.contains("<title>Hello World – Test Site</title>"),
        "{}",
        r.body
    );
    assert!(
        r.body
            .contains("<link rel=\"canonical\" href=\"https://example.com/hello-world/\">")
    );
    app
}

/// A plugin whose artifact was tampered with is skipped at load; the page still
/// renders, without its tag.
async fn tampered_plugin_does_not_break_the_page(app: App) -> App {
    app.post("Hello World", "hello-world", &[]).await;
    app.site.install("hello").await;
    let row = app
        .site
        .store
        .read()
        .plugins()
        .list()
        .await
        .unwrap()
        .into_iter()
        .find(|p| p.name == "hello")
        .unwrap();
    app.site.tamper(&row.artifact_hash, b"\0asm tampered");
    let summary = app.site.load().await;
    assert!(summary.loaded.is_empty(), "{summary:?}");
    let r = app.get("/hello-world/").await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(r.body.contains("<h1>Hello World</h1>"));
    assert!(!r.body.contains("ferrite-hello"));
    app
}

app_tests!(
    tampered_plugin_does_not_break_the_page,
    serves_every_content_route,
    static_front_page,
    plugin_head_meta_is_served,
);
