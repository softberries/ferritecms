//! RSS feeds, sitemaps and robots.txt: well-formed XML (parsed with quick-xml),
//! published-only, cached and refreshed by one poll.

use axum::http::StatusCode;
use ferrite_core::OptionService;
use ferrite_domain::ContentStatus;
use ferrite_http::app::feeds::DISCOURAGE_INDEXING_OPTION;
use quick_xml::{Reader, events::Event};
use serde_json::json;

use crate::app_tests;
use std::sync::Arc;

use axum::http::Method;

use crate::harness::{App, request, send};

/// Parses `xml` to the end (end tags must match); returns the root element's name.
fn well_formed(xml: &str) -> String {
    let mut reader = Reader::from_str(xml);
    let mut root = None;
    let mut depth = 0usize;
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                if depth == 0 {
                    root = Some(String::from_utf8(e.name().as_ref().to_vec()).unwrap());
                }
                depth += 1;
            }
            Ok(Event::End(_)) => depth -= 1,
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(e) => panic!("malformed XML at {}: {e}\n{xml}", reader.buffer_position()),
        }
    }
    assert_eq!(depth, 0, "unclosed elements");
    root.expect("a root element")
}

async fn rss_feeds(app: App) -> App {
    let news = app.category("news").await;
    app.post("Tom & Jerry <3", "tom-jerry", &[news]).await;
    app.post("Odd\u{1}chars\u{b}here", "odd", &[]).await;
    app.draft_post("Secret Draft", "secret", &[news]).await;

    let r = app.get("/feed/").await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert_eq!(
        r.header("content-type"),
        Some("application/rss+xml; charset=utf-8")
    );
    assert_eq!(well_formed(&r.body), "rss");
    assert!(
        r.body
            .starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<rss version=\"2.0\" xmlns:atom=\"http://www.w3.org/2005/Atom\">")
    );
    assert!(
        r.body.contains("<title>Tom &amp; Jerry &lt;3</title>"),
        "{}",
        r.body
    );
    assert!(
        r.body.contains("<title>Oddcharshere</title>"),
        "control chars dropped"
    );
    assert!(
        r.body
            .contains("<link>https://example.com/tom-jerry/</link>")
    );
    assert!(
        r.body
            .contains("<guid isPermaLink=\"true\">https://example.com/tom-jerry/</guid>")
    );
    assert!(r.body.contains("<pubDate>") && r.body.contains("+0000</pubDate>"));
    assert!(
        r.body
            .contains("<description>Body of tom-jerry</description>"),
        "{}",
        r.body
    );
    assert!(!r.body.contains("Secret"));

    assert!(r.body.contains("<title>Test Site</title>"), "{}", r.body);
    assert!(r.body.contains(
        "<atom:link href=\"https://example.com/feed/\" rel=\"self\" type=\"application/rss+xml\"/>"
    ));
    assert!(r.body.contains("<lastBuildDate>"));

    let r = app.get("/category/news/feed/").await;
    assert_eq!(r.status, StatusCode::OK);
    well_formed(&r.body);
    assert!(
        r.body.contains("<title>news – Test Site</title>"),
        "{}",
        r.body
    );
    assert!(
        r.body
            .contains("<atom:link href=\"https://example.com/category/news/feed/\" rel=\"self\"")
    );
    assert!(r.body.contains("Tom &amp; Jerry"));
    assert!(
        !r.body.contains("Odd"),
        "other posts are not in the term feed"
    );
    assert!(
        r.body
            .contains("<link>https://example.com/category/news/</link>")
    );
    assert!(!r.body.contains("Secret"));

    // Unknown term, and a term with only drafts (R7): 404.
    let hidden = app.category("hidden").await;
    app.draft_post("Hidden Post", "hidden-post", &[hidden])
        .await;
    for path in ["/category/nope/feed/", "/category/hidden/feed/"] {
        assert_eq!(app.get(path).await.status, StatusCode::NOT_FOUND, "{path}");
    }
    app
}

/// Review Focus 2 for the feed: an edit shows after one poll; trash removes the item.
async fn feed_follows_edits(app: App) -> App {
    let hello = app.post("Hello World", "hello-world", &[]).await;
    let mut poller = app.poller().await;
    assert!(app.get("/feed/").await.body.contains("Hello World"));
    let edited = app
        .svc
        .update(
            &app.ada,
            hello.id,
            hello.version,
            crate::harness::draft("post", "Hello Again", "hello-world", None, &[]),
        )
        .await
        .unwrap();
    assert!(
        app.get("/feed/").await.body.contains("Hello World"),
        "cached until the poll"
    );
    poller.poll_once().await.unwrap();
    let r = app.get("/feed/").await;
    assert!(r.body.contains("Hello Again") && !r.body.contains("Hello World"));
    app.svc
        .trash(&app.ada, edited.id, edited.version)
        .await
        .unwrap();
    poller.poll_once().await.unwrap();
    assert!(!app.get("/feed/").await.body.contains("Hello Again"));
    app
}

async fn sitemaps(app: App) -> App {
    app.post("Public Post", "public-post", &[]).await;
    app.page("About", "about", None).await;
    app.draft_post("Secret Draft", "secret", &[]).await;
    app.site
        .post(
            &app.ada_user,
            "secret-future",
            "Future",
            ContentStatus::Published,
            Some(crate::common::HOUR),
        )
        .await;
    let mut poller = app.poller().await;

    let r = app.get("/sitemap.xml").await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(
        r.header("content-type"),
        Some("application/xml; charset=utf-8")
    );
    assert_eq!(well_formed(&r.body), "sitemapindex");
    assert!(
        r.body
            .contains("<loc>https://example.com/sitemap-post-1.xml</loc>"),
        "{}",
        r.body
    );
    assert!(
        r.body
            .contains("<loc>https://example.com/sitemap-page-1.xml</loc>")
    );
    assert!(!r.body.contains("sitemap-post-2.xml"));
    assert!(
        !r.body.contains("sitemap-media"),
        "media is never in a sitemap"
    );

    let r = app.get("/sitemap-post-1.xml").await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(well_formed(&r.body), "urlset");
    assert!(
        r.body
            .contains("<loc>https://example.com/public-post/</loc>"),
        "{}",
        r.body
    );
    assert!(r.body.contains("<lastmod>") && r.body.contains("Z</lastmod>"));
    assert!(!r.body.contains("secret"), "{}", r.body);
    let r = app.get("/sitemap-page-1.xml").await;
    assert!(r.body.contains("<loc>https://example.com/about/</loc>"));
    for path in ["/sitemap-post-2.xml", "/sitemap-nope-1.xml"] {
        assert_eq!(app.get(path).await.status, StatusCode::NOT_FOUND, "{path}");
    }

    // Publishing the draft reaches the cached sitemap after one poll.
    let item = app
        .site
        .store
        .read()
        .content()
        .get_by_slug("post", None, "secret")
        .await
        .unwrap()
        .unwrap();
    app.svc
        .publish(&app.ada, item.id, item.version)
        .await
        .unwrap();
    assert!(
        !app.get("/sitemap-post-1.xml")
            .await
            .body
            .contains("/secret/")
    );
    poller.poll_once().await.unwrap();
    assert!(
        app.get("/sitemap-post-1.xml")
            .await
            .body
            .contains("<loc>https://example.com/secret/</loc>")
    );
    app
}

async fn robots_txt(app: App) -> App {
    let mut poller = app.poller().await;
    let r = app.get("/robots.txt").await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.header("content-type"), Some("text/plain; charset=utf-8"));
    assert_eq!(
        r.body,
        "User-agent: *\nDisallow:\n\nSitemap: https://example.com/sitemap.xml\n"
    );
    OptionService::new(app.site.store.clone())
        .set(&app.admin, DISCOURAGE_INDEXING_OPTION, &json!(true), false)
        .await
        .unwrap();
    poller.poll_once().await.unwrap();
    let r = app.get("/robots.txt").await;
    assert!(
        r.body.starts_with("User-agent: *\nDisallow: /\n"),
        "{}",
        r.body
    );
    assert!(r.body.contains("Sitemap: https://example.com/sitemap.xml"));
    app
}

/// lastBuildDate follows the newest `updated_at`: an edit moves it.
async fn feed_last_build_date_follows_updates(app: App) -> App {
    let a = app.post("First", "first", &[]).await;
    app.post("Second", "second", &[]).await;
    let build = |body: &str| {
        body.split_once("<lastBuildDate>")
            .and_then(|(_, r)| r.split_once("</lastBuildDate>"))
            .map(|(d, _)| chrono::DateTime::parse_from_rfc2822(d).unwrap())
            .unwrap()
    };
    let mut poller = app.poller().await;
    let before = build(&app.get("/feed/").await.body);
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    app.svc
        .update(
            &app.ada,
            a.id,
            a.version,
            crate::harness::draft("post", "First Edited", "first", None, &[]),
        )
        .await
        .unwrap();
    poller.poll_once().await.unwrap();
    let after = build(&app.get("/feed/").await.body);
    assert!(after > before, "{before} -> {after}");
    app
}

/// Sitemaps split at the file length (overridden to 2 here); a file number past the
/// last is 404; `&` in the site URL is escaped in `<loc>` and `<link>`.
async fn sitemap_split_and_escaping(app: App) -> App {
    for slug in ["p1", "p2", "p3", "p4", "p5"] {
        app.post(slug, slug, &[]).await;
    }
    let reader = Arc::new(
        ferrite_core::SiteReader::new(
            app.site.store.clone(),
            Arc::new(ferrite_core::PermalinkRules::default()),
            Arc::new(|hash: &ferrite_domain::ArtifactHash, name: &str| {
                format!("/_media/{hash}/{name}")
            }),
            Arc::new(ferrite_domain::Timestamp::now),
        )
        .with_sitemap_file_len(2),
    );
    let config = ferrite_http::HttpConfig::new("https://example.com/a&b", "A & B", "en");
    let theme = crate::harness::load_theme(crate::harness::fixture("app-theme"));
    let state = App::state_with_reader(&app.site, reader, theme, true, config).await;
    let router = ferrite_http::build_app(state);
    let get = |path: &str| send(&router, request(Method::GET, path));

    let r = get("/sitemap.xml").await;
    assert_eq!(well_formed(&r.body), "sitemapindex");
    for n in 1..=3 {
        assert!(
            r.body.contains(&format!(
                "<loc>https://example.com/a&amp;b/sitemap-post-{n}.xml</loc>"
            )),
            "{n}: {}",
            r.body
        );
    }
    assert!(!r.body.contains("sitemap-post-4.xml"));
    let mut total = 0;
    for n in 1..=3 {
        let r = get(&format!("/sitemap-post-{n}.xml")).await;
        assert_eq!(r.status, StatusCode::OK, "{n}");
        assert_eq!(well_formed(&r.body), "urlset");
        let count = r.body.matches("<url>").count();
        assert_eq!(count, if n == 3 { 1 } else { 2 }, "{n}");
        assert!(
            r.body.contains("<loc>https://example.com/a&amp;b/"),
            "{}",
            r.body
        );
        total += count;
    }
    assert_eq!(total, 5);
    assert_eq!(
        get("/sitemap-post-4.xml").await.status,
        StatusCode::NOT_FOUND
    );

    let r = get("/feed/").await;
    assert_eq!(well_formed(&r.body), "rss");
    assert!(r.body.contains("<title>A &amp; B</title>"));
    assert!(r.body.contains("<link>https://example.com/a&amp;b/</link>"));
    assert!(
        r.body
            .contains("href=\"https://example.com/a&amp;b/feed/\"")
    );
    assert!(
        r.body
            .contains("<link>https://example.com/a&amp;b/p1/</link>"),
        "{}",
        r.body
    );
    app
}

app_tests!(
    feed_last_build_date_follows_updates,
    sitemap_split_and_escaping,
    rss_feeds,
    feed_follows_edits,
    sitemaps,
    robots_txt
);
