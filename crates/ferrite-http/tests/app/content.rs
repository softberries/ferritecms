//! Every content route type, end to end, plus a plugin's head tag in a served page.

use std::{sync::Arc, time::Duration};

use axum::http::StatusCode;
use ferrite_core::{
    RedirectService, TaxonomyService,
    site::{FRONT_PAGE_OPTION, PER_PAGE_OPTION},
};
use ferrite_domain::RedirectId;
use ferrite_http::app::hits::{HitCounter, MAX_BUFFERED_REDIRECTS};
use ferrite_store_tests::CountingStore;
use serde_json::json;
use tokio_util::sync::CancellationToken;

use crate::app_tests;
use crate::common::plugin_wasm;
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

/// M4-4 P67: the front page's own permalink answers a cacheable 301 to `/` (keeping
/// `?cpage=N` only), for visitors and logged-in users alike; `/`, the front page's
/// children, a page of the same slug elsewhere and the preview stay 200, and changing
/// `reading.front_page` gives the old front page its path back after one poll.
async fn the_front_pages_permalink_redirects_to_root(app: App) -> App {
    let home = app.page("Welcome Home", "home", None).await;
    let child = app.page("Child", "x", Some(home.id)).await;
    let about = app.page("About", "about", None).await;
    app.page("Another Home", "home", Some(about.id)).await;
    let mut poller = app.poller().await;

    // Without a static front page `/home/` is an ordinary page.
    let r = app.get("/home/").await;
    assert_eq!((r.status, r.tpl()), (StatusCode::OK, "page"), "{r:?}");

    ferrite_core::OptionService::new(app.site.store.clone())
        .set(
            &app.admin,
            FRONT_PAGE_OPTION,
            &json!(home.id.to_string()),
            false,
        )
        .await
        .unwrap();
    poller.poll_once().await.unwrap();

    let moved = |r: &crate::harness::Resp, to: &str| {
        assert_eq!(r.status, StatusCode::MOVED_PERMANENTLY, "{r:?}");
        assert_eq!(r.header("location"), Some(to), "{r:?}");
    };
    moved(&app.get("/home/").await, "/");
    // Cached like a page: the second request is a hit, still the redirect.
    let key = ferrite_cache::CacheKey::anon("example.com", "/home/", "");
    let cached = app.state.cache.get(&key).await.expect("the 301 is cached");
    assert_eq!(cached.status, 301);
    moved(&app.get("/home/").await, "/");
    moved(&app.get("/home/?cpage=2").await, "/?cpage=2");
    // Only `cpage` is part of the page: other parameters are dropped.
    moved(&app.get("/home/?utm_source=x&cpage=3").await, "/?cpage=3");
    moved(&app.get("/home/?utm_source=x").await, "/");
    // Logged in, the same answer.
    let cookie = app.session(&app.ada_user).await;
    moved(&app.get_with_cookie("/home/", &cookie).await, "/");

    // `/` itself is the page, never a redirect, with any comment page.
    for path in ["/", "/?cpage=2"] {
        let r = app.get(path).await;
        assert_eq!(
            (r.status, r.tpl()),
            (StatusCode::OK, "page"),
            "{path}: {r:?}"
        );
        assert!(r.body.contains("<h1>Welcome Home</h1>"), "{path}");
    }
    let r = app.get_with_cookie("/", &cookie).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    // A child keeps its path; a page of the same slug under another parent is not the
    // front page.
    let r = app.get("/home/x/").await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert!(r.body.contains("<h1>Child</h1>"));
    let r = app.get("/about/home/").await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert!(r.body.contains("<h1>Another Home</h1>"));
    // The preview renders the item itself.
    let r = app
        .get_with_cookie(&format!("/_preview/{}", home.id), &cookie)
        .await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert!(r.body.contains("<h1>Welcome Home</h1>"), "{}", r.body);

    // Another front page: after one poll `/home/` is a page again and the child's
    // permalink redirects.
    ferrite_core::OptionService::new(app.site.store.clone())
        .set(
            &app.admin,
            FRONT_PAGE_OPTION,
            &json!(child.id.to_string()),
            false,
        )
        .await
        .unwrap();
    poller.poll_once().await.unwrap();
    let r = app.get("/home/").await;
    assert_eq!((r.status, r.tpl()), (StatusCode::OK, "page"), "{r:?}");
    moved(&app.get("/home/x/").await, "/");
    let r = app.get("/").await;
    assert!(r.body.contains("<h1>Child</h1>"), "{}", r.body);
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

/// The text of `<p class="{class}">` in `body`.
fn para<'a>(body: &'a str, class: &str) -> &'a str {
    let open = format!("<p class=\"{class}\">");
    let start = body
        .find(&open)
        .map(|i| i + open.len())
        .unwrap_or_else(|| panic!("no {class}: {body}"));
    let end = body[start..].find("</p>").unwrap() + start;
    &body[start..end]
}

/// Ruling R30: listing routes get `archive` (what they list) and `pagination`
/// (canonical neighbour URLs, the search query encoded) from the server; 404 gets
/// neither.
async fn listings_carry_archive_and_pagination(app: App) -> App {
    let news = app.category("news").await;
    app.post("Fish Chips One", "fish-one", &[news]).await;
    let second = app.post("Fish Chips Two", "fish-two", &[news]).await;
    app.site
        .store
        .read()
        .options()
        .set(PER_PAGE_OPTION, &json!(1), false)
        .await
        .unwrap();

    let r = app.get("/category/news/").await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert_eq!(para(&r.body, "archive"), "term|post|news|||");
    assert_eq!(
        para(&r.body, "nav"),
        "1/2 prev= next=&#x2f;category&#x2f;news&#x2f;page&#x2f;2&#x2f;"
    );
    let r = app.get("/category/news/page/2/").await;
    assert_eq!(
        para(&r.body, "nav"),
        "2/2 prev=&#x2f;category&#x2f;news&#x2f; next="
    );

    let r = app.get("/author/ada/").await;
    assert_eq!(para(&r.body, "archive"), "author|post||ada display||");
    assert!(para(&r.body, "nav").ends_with("next=&#x2f;author&#x2f;ada&#x2f;page&#x2f;2&#x2f;"));

    let published = second.published_at.expect("published").to_rfc3339();
    let (year, month) = date_paths(&published);
    let r = app.get(&month).await;
    let (y, m) = (&published[..4], published[5..7].trim_start_matches('0'));
    assert_eq!(para(&r.body, "archive"), format!("date|post|||{y}|{m}"));
    let r = app.get(&year).await;
    assert_eq!(para(&r.body, "archive"), format!("date|post|||{y}|"));

    let r = app.get("/post/").await;
    assert_eq!(para(&r.body, "archive"), "type|post||||");
    let r = app.get("/").await;
    assert_eq!(para(&r.body, "archive"), "front|post||||");
    assert!(para(&r.body, "nav").ends_with("next=&#x2f;page&#x2f;2&#x2f;"));

    // Search: the query is percent-encoded into the links, and they round-trip.
    let r = app.get("/?s=fish%20chips").await;
    assert_eq!(r.tpl(), "search", "{r:?}");
    assert_eq!(para(&r.body, "archive"), "search|||||");
    assert_eq!(
        para(&r.body, "nav"),
        "1/2 prev= next=&#x2f;page&#x2f;2&#x2f;?s=fish%20chips"
    );
    let r = app.get("/page/2/?s=fish%20chips").await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert_eq!(para(&r.body, "query"), "fish chips");
    assert_eq!(para(&r.body, "nav"), "2/2 prev=&#x2f;?s=fish%20chips next=");

    // Error pages get neither.
    let r = app.get("/no-such-page/").await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    assert_eq!(para(&r.body, "ctx"), "[False|False]");
    app
}

/// A custom menu URL that bypassed `MenuService` validation (written straight to the
/// store) with a script scheme is dropped by the bridge; safe ones stay.
async fn unsafe_menu_urls_are_dropped(app: App) -> App {
    use ferrite_domain::{Menu, MenuItem, MenuItemId, MenuItemKind};
    app.post("Hello World", "hello-world", &[]).await;
    let entry = |i: i32, url: &str, label: &str| MenuItem {
        id: MenuItemId::new(),
        parent_id: None,
        position: i,
        kind: MenuItemKind::Url,
        target_id: None,
        url: Some(url.into()),
        label: label.into(),
    };
    let menu = Menu {
        name: "primary".into(),
        label: "Primary".into(),
        items: vec![
            entry(0, "javascript:alert(1)", "Evil"),
            entry(1, "data:text/html,x", "Data"),
            entry(2, "/about/", "About"),
            entry(3, "mailto:hi@example.com", "Mail"),
        ],
    };
    app.site.store.read().menus().put(&menu).await.unwrap();
    let r = app.get("/hello-world/").await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert!(!r.body.contains("javascript"), "{}", r.body);
    assert!(
        !r.body.contains(">Evil<") && !r.body.contains(">Data<"),
        "{}",
        r.body
    );
    assert!(
        r.body.contains(">About<") && r.body.contains(">Mail<"),
        "{}",
        r.body
    );
    app
}

/// P25: renaming a term serves a 301 from its old archive to the new one.
async fn term_redirect_served(app: App) -> App {
    let old = app.category("old").await;
    app.post("Filed", "filed", &[old]).await;
    let mut poller = app.poller().await;
    let r = app.get("/category/old/").await;
    assert_eq!((r.status, r.tpl()), (StatusCode::OK, "archive"), "{r:?}");

    TaxonomyService::new(app.site.store.clone())
        .update_term(&app.admin, old, "New", Some("new".into()), None, "")
        .await
        .expect("rename");
    poller.poll_once().await.unwrap();

    for path in ["/category/old/", "/category/old"] {
        let r = app.get(path).await;
        assert_eq!(r.status, StatusCode::MOVED_PERMANENTLY, "{path}: {r:?}");
        assert_eq!(r.header("location"), Some("/category/new/"), "{path}");
    }
    let r = app.get("/category/new/").await;
    assert_eq!((r.status, r.tpl()), (StatusCode::OK, "archive"), "{r:?}");
    assert!(r.body.contains("Filed"), "{}", r.body);
    app
}

/// A new term taking a renamed term's old slug serves its own archive: creating it
/// cleared the rename's redirect.
async fn new_term_reclaims_renamed_path(app: App) -> App {
    let old = app.category("old").await;
    app.post("First", "first", &[old]).await;
    TaxonomyService::new(app.site.store.clone())
        .update_term(&app.admin, old, "New", Some("new".into()), None, "")
        .await
        .expect("rename");
    let r = app.get("/category/old/").await;
    assert_eq!(r.status, StatusCode::MOVED_PERMANENTLY, "{r:?}");

    let again = app.category("old").await;
    app.post("Second", "second", &[again]).await;
    let mut poller = app.poller().await;
    poller.poll_once().await.unwrap();
    let r = app.get("/category/old/").await;
    assert_eq!((r.status, r.tpl()), (StatusCode::OK, "archive"), "{r:?}");
    assert!(r.body.contains("Second"), "{}", r.body);
    assert!(!r.body.contains("First"), "{}", r.body);
    app
}

/// Task 14: hits on one redirect coalesce into one `add_hits` per flush (counted
/// through a counting store that only the counter uses); serving the redirect does no
/// store write.
async fn redirect_hits_coalesce(app: App) -> App {
    let r = app
        .redirects()
        .upsert(&app.admin, "/old-path", "/new-path", 301)
        .await
        .unwrap();
    let counting = Arc::new(CountingStore::new(app.site.store.clone()));
    let hits = Arc::new(HitCounter::new(Arc::new(RedirectService::new(
        counting.clone(),
    ))));
    let app = app
        .rebuilt(App::config(), |s| s.with_hits(hits.clone()))
        .await;
    for _ in 0..50 {
        let res = app.get("/old-path").await;
        assert_eq!(res.status, StatusCode::MOVED_PERMANENTLY, "{res:?}");
    }
    assert_eq!(counting.calls(), 0, "no store call on the request path");
    assert_eq!(hits.len(), 1);

    // The flusher writes once on stop.
    let stop = CancellationToken::new();
    let flusher = hits
        .clone()
        .spawn_flusher(Duration::from_secs(3600), stop.clone());
    stop.cancel();
    flusher.await.unwrap();
    assert_eq!(counting.calls(), 1, "one write for 50 hits");
    let stored = app.redirects().resolve("/old-path").await.unwrap().unwrap();
    assert_eq!(stored.hits, 50);
    let first = stored.last_hit_at.expect("last hit time");
    assert!(hits.is_empty());

    // The next batch: one more write, added to the count, time moves forward.
    for _ in 0..3 {
        app.get("/old-path").await;
    }
    assert_eq!(hits.flush().await, 1);
    assert_eq!(counting.calls(), 2);
    let stored = app.redirects().resolve("/old-path").await.unwrap().unwrap();
    assert_eq!((stored.id, stored.hits), (r.id, 53));
    assert!(stored.last_hit_at.unwrap() >= first);
    // Nothing buffered: a flush writes nothing.
    assert_eq!(hits.flush().await, 0);
    assert_eq!(counting.calls(), 2);
    app
}

/// Task 14: at most 10 000 distinct redirects are buffered; a hit on a further new one
/// is dropped and counted, hits on buffered ones still count.
async fn hit_buffer_is_bounded(app: App) -> App {
    let hits = HitCounter::new(Arc::new(app.redirects()));
    let ids: Vec<RedirectId> = (0..MAX_BUFFERED_REDIRECTS + 1)
        .map(|_| RedirectId::new())
        .collect();
    for id in &ids {
        hits.record(*id);
    }
    assert_eq!(MAX_BUFFERED_REDIRECTS, 10_000);
    assert_eq!((hits.len(), hits.dropped()), (10_000, 1));
    hits.record(ids[0]);
    assert_eq!((hits.len(), hits.dropped()), (10_000, 1));
    hits.record(RedirectId::new());
    assert_eq!(hits.dropped(), 2);
    app
}

/// Task 14: `render.footer` output reaches `footer()` sanitised (html-inject's
/// paragraph without its handler, class or script); a failing footer plugin is skipped and
/// the page stays intact.
async fn footer_filter_renders(app: App) -> App {
    app.post("Hello World", "hello-world", &[]).await;
    let plain = app.get("/hello-world/").await;
    assert!(!plain.body.contains("Injected footer"), "{}", plain.body);

    app.site.install("html-inject").await;
    assert_eq!(app.site.load().await.loaded, ["html-inject"]);
    app.state.cache.flush();
    let r = app.get("/hello-world/").await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(
        r.body.contains("<p>Injected footer</p>\n</body>"),
        "{}",
        r.body
    );
    // ammonia drops the script, the handler and the class.
    for bad in ["alert(4)", "alert(5)", "onclick", "injected-footer"] {
        assert!(!r.body.contains(bad), "{bad}: {}", r.body);
    }
    // A working footer plugin alone does not make the page volatile.
    let page = app
        .state
        .cache
        .get(&ferrite_cache::CacheKey::anon(
            "example.com",
            "/hello-world/",
            "",
        ))
        .await
        .expect("cached");
    assert_eq!(page.ttl, None);

    // A footer plugin that answers with the wrong variant (a failure), before
    // html-inject.
    let hash = app.site.put_artifact(plugin_wasm("wrong-variant")).await;
    let manifest = "[plugin]\nname = \"wrong-variant\"\nversion = \"0.1.0\"\n\
                    publisher = \"ferritecms.dev\"\nworld = \"ferrite:plugin/plugin@0.1.0\"\n\
                    [capabilities]\nrender.html = \"sanitized\"\n\
                    [hooks]\n\"render.footer\" = 5\n";
    app.site.row("wrong-variant", manifest, hash, true).await;
    let mut loaded = app.site.load().await.loaded;
    loaded.sort();
    assert_eq!(loaded, ["html-inject", "wrong-variant"]);
    app.state.cache.flush();
    let r = app.get("/hello-world/").await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(r.body.contains("<h1>Hello World</h1>"), "{}", r.body);
    assert!(r.body.contains("<p>Injected footer</p>"), "{}", r.body);
    assert!(!r.body.contains("not a head"), "{}", r.body);
    // The failed footer call makes the page volatile: cached with `volatile_ttl`.
    let page = app
        .state
        .cache
        .get(&ferrite_cache::CacheKey::anon(
            "example.com",
            "/hello-world/",
            "",
        ))
        .await
        .expect("cached");
    assert_eq!(page.ttl, Some(app.state.config.volatile_ttl));
    app
}

app_tests!(
    unsafe_menu_urls_are_dropped,
    listings_carry_archive_and_pagination,
    tampered_plugin_does_not_break_the_page,
    serves_every_content_route,
    static_front_page,
    the_front_pages_permalink_redirects_to_root,
    plugin_head_meta_is_served,
    term_redirect_served,
    new_term_reclaims_renamed_path,
    redirect_hits_coalesce,
    hit_buffer_is_bounded,
    footer_filter_renders,
);
