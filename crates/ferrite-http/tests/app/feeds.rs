//! RSS feeds, sitemaps and robots.txt: well-formed XML (parsed with quick-xml),
//! published-only, cached and refreshed by one poll.

use axum::http::StatusCode;
use ferrite_core::{OptionService, site::FRONT_PAGE_OPTION};
use ferrite_domain::{ContentId, ContentStatus, NewContent, Timestamp};
use ferrite_http::app::feeds::DISCOURAGE_INDEXING_OPTION;
use ferrite_store::Store;
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
        .get_by_slug("post", None, None, "secret")
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

/// URLs per sitemap file in the golden site.
const GOLDEN_FILE_LEN: usize = 3;
/// The checked-in output of [`sitemaps_are_unchanged`]; `FERRITE_UPDATE_GOLDEN=1`
/// rewrites it.
const GOLDEN: &str = "tests/fixtures/sitemaps.golden.txt";

/// 2023-11-14T22:13:20Z plus `secs` seconds.
fn at(secs: i64) -> Timestamp {
    Timestamp::from_micros(1_700_000_000_000_000 + secs * 1_000_000).unwrap()
}

/// Published at `secs` after [`at`]'s base (a [`fixed_row`] state).
fn public(secs: i64) -> Option<(ContentStatus, Timestamp)> {
    Some((ContentStatus::Published, at(secs)))
}

/// Inserts a row with the fixed id `n` and fixed times, so the sitemaps are the same on
/// every run and backend: `state` is the status and `published_at`, `None` a draft
/// without a publish time.
async fn fixed_row(
    app: &App,
    n: u128,
    kind: &str,
    slug: &str,
    parent: Option<ContentId>,
    state: Option<(ContentStatus, Timestamp)>,
    updated: i64,
) -> ContentId {
    let (status, published) = state.map_or((ContentStatus::Draft, None), |(s, t)| (s, Some(t)));
    let id = ContentId::from_uuid(uuid::Uuid::from_u128(
        0x0190_0000_0000_7000_8000_0000_0000_0000 + n,
    ));
    app.site
        .store
        .read()
        .content()
        .insert(&NewContent {
            id,
            kind: kind.into(),
            status,
            slug: slug.into(),
            parent_id: parent,
            author_id: app.ada_user.id,
            title: slug.into(),
            excerpt: String::new(),
            body: json!({"type": "doc", "content": []}),
            body_html: None,
            fields: json!({}),
            menu_order: 0,
            published_at: published,
            scheduled_at: published.filter(|_| status == ContentStatus::Scheduled),
            created_at: Some(at(0)),
            updated_at: Some(at(updated)),
            locale: "en".into(),
            translation_group: None,
        })
        .await
        .unwrap();
    id
}

/// Posts and pages that split into several files of [`GOLDEN_FILE_LEN`]: two posts
/// share `published_at` across the first file boundary (ordered by id), the newest
/// `updated_at` of a file is not its first row's, and the static front page `home`
/// (with a child) sits in the second page file. Plus rows no sitemap may list, with
/// publish times among the listed ones and the newest `updated_at`s (a leak would move
/// the split or a `lastmod`): a draft, a future post, and trashed, private, scheduled
/// (with a past publish time) and pending posts and pages.
async fn golden_site(app: &App) {
    for (n, slug, published, updated) in [
        (1, "p-a", 100, 500),
        (2, "p-b", 90, 95),
        (3, "p-c", 80, 900),
        (4, "p-d", 80, 81),
        (5, "p-e", 70, 70),
        (6, "p-f", 60, 1000),
        (7, "p-g", 50, 55),
    ] {
        fixed_row(app, n, "post", slug, None, public(published), updated).await;
    }
    fixed_row(app, 8, "post", "p-draft", None, None, 2000).await;
    let future =
        Timestamp::from_micros(Timestamp::now().as_micros() + crate::common::HOUR).unwrap();
    let future = Some((ContentStatus::Published, future));
    fixed_row(app, 9, "post", "p-future", None, future, 3000).await;
    for (n, kind, slug, status, published, updated) in [
        (10, "post", "p-trash", ContentStatus::Trash, 75, 5000),
        (11, "post", "p-private", ContentStatus::Private, 85, 5001),
        (
            12,
            "post",
            "p-scheduled",
            ContentStatus::Scheduled,
            65,
            5002,
        ),
        (13, "post", "p-pending", ContentStatus::Pending, 95, 5003),
        (26, "page", "old-page", ContentStatus::Trash, 35, 5004),
        (27, "page", "private-page", ContentStatus::Private, 15, 5005),
        (
            28,
            "page",
            "scheduled-page",
            ContentStatus::Scheduled,
            25,
            5006,
        ),
    ] {
        let state = Some((status, at(published)));
        fixed_row(app, n, kind, slug, None, state, updated).await;
    }

    let home = fixed_row(app, 21, "page", "home", None, public(10), 20).await;
    fixed_row(app, 22, "page", "garden", Some(home), public(40), 45).await;
    let about = fixed_row(app, 23, "page", "about", None, public(30), 300).await;
    fixed_row(app, 24, "page", "team", Some(about), public(20), 25).await;
    fixed_row(app, 25, "page", "contact", None, public(5), 6).await;
    app.site
        .store
        .read()
        .options()
        .set(FRONT_PAGE_OPTION, &json!(home.to_string()), false)
        .await
        .unwrap();
}

/// A router over `store` whose sitemaps split every [`GOLDEN_FILE_LEN`] URLs.
async fn golden_router(app: &App, store: Arc<dyn Store>) -> axum::Router {
    let reader = Arc::new(
        ferrite_core::SiteReader::new(
            store.clone(),
            Arc::new(ferrite_core::PermalinkRules::default()),
            Arc::new(|hash: &ferrite_domain::ArtifactHash, name: &str| {
                format!("/_media/{hash}/{name}")
            }),
            Arc::new(Timestamp::now),
        )
        .with_sitemap_file_len(GOLDEN_FILE_LEN),
    );
    let theme = crate::harness::load_theme(crate::harness::fixture("app-theme"));
    let mut state = App::build_state(&app.site, reader, theme, true, App::config()).await;
    state.store = store;
    ferrite_http::build_app(Arc::new(state))
}

/// The `<loc>` texts of a sitemap index.
fn locs(index: &str) -> Vec<String> {
    index
        .split("<loc>")
        .skip(1)
        .filter_map(|s| s.split_once("</loc>").map(|(l, _)| l.to_owned()))
        .collect()
}

/// The index, every file it names and the first file past each kind's last, as one
/// text: `== <path> <status>` lines, each followed by the body of a 200.
async fn render_sitemaps(router: &axum::Router) -> String {
    let index = send(router, request(Method::GET, "/sitemap.xml")).await;
    assert_eq!(index.status, StatusCode::OK);
    assert_eq!(well_formed(&index.body), "sitemapindex");
    let mut paths: Vec<String> = locs(&index.body)
        .iter()
        .map(|l| l.strip_prefix(crate::common::SITE_URL).unwrap().to_owned())
        .collect();
    paths.extend(["/sitemap-post-4.xml".into(), "/sitemap-page-3.xml".into()]);
    let mut out = format!("== /sitemap.xml 200\n{}", index.body);
    for path in paths {
        let r = send(router, request(Method::GET, &path)).await;
        out.push_str(&format!("== {path} {}\n", r.status.as_u16()));
        if r.status == StatusCode::OK {
            assert_eq!(well_formed(&r.body), "urlset");
            out.push_str(&r.body);
        }
    }
    out
}

/// The sitemaps of [`golden_site`] (the index and every file) match the checked-in
/// [`GOLDEN`] text, captured before the M4-4 rewrite of the sitemap reads: the same
/// URLs, `lastmod`s and file split on both backends.
async fn sitemaps_are_unchanged(app: App) -> App {
    golden_site(&app).await;
    let router = golden_router(&app, app.site.store.clone()).await;
    let got = render_sitemaps(&router).await;
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(GOLDEN);
    if std::env::var_os("FERRITE_UPDATE_GOLDEN").is_some() {
        std::fs::write(&path, &got).unwrap();
    }
    let want = std::fs::read_to_string(&path).unwrap();
    assert_eq!(
        got, want,
        "regenerate with FERRITE_UPDATE_GOLDEN=1 if the change is intended"
    );
    app
}

/// The index reads no content rows (one aggregate per type), and file `n` lists only
/// its own rows: it seeks to its start instead of walking the files before it.
async fn sitemap_reads_seek(app: App) -> App {
    golden_site(&app).await;
    let counting = Arc::new(ferrite_store_tests::CountingStore::new(
        app.site.store.clone(),
    ));
    let router = golden_router(&app, counting.clone()).await;
    for (path, rows) in [
        ("/sitemap.xml", 0),
        ("/sitemap-post-1.xml", 3),
        ("/sitemap-post-2.xml", 3),
        ("/sitemap-post-3.xml", 1),
        ("/sitemap-page-2.xml", 2),
        ("/sitemap-post-4.xml", 0),
    ] {
        counting.reset();
        let r = send(&router, request(Method::GET, path)).await;
        assert!(
            matches!(r.status, StatusCode::OK | StatusCode::NOT_FOUND),
            "{path}: {r:?}"
        );
        eprintln!(
            "{path}: {} store calls, {} listed rows",
            counting.calls(),
            counting.listed_rows()
        );
        assert_eq!(counting.listed_rows(), rows, "{path}");
    }
    app
}

app_tests!(
    sitemaps_are_unchanged,
    sitemap_reads_seek,
    feed_last_build_date_follows_updates,
    sitemap_split_and_escaping,
    rss_feeds,
    feed_follows_edits,
    sitemaps,
    robots_txt
);
