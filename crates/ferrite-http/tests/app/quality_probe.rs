//! M6-4 task 6: `SiteProbe` (design 16 §6.2): what a path answers, without rendering,
//! caching, counting or calling a plugin.

use std::sync::Arc;

use ferrite_core::{
    RedirectService,
    quality::{PathProbe, PathStatus},
};
use ferrite_http::{SiteProbe, app::hits::HitCounter, plugins::ServedBuffer};

use crate::app_tests;
use crate::harness::App;

app_tests!(
    published_page_ok_with_item_id,
    draft_page_not_found,
    trashed_media_not_found,
    stored_redirect_followed_and_reported,
    redirect_410_is_gone,
    redirect_to_external_is_external,
    canonical_trailing_slash_redirected,
    term_and_archive_routes_ok,
    plugin_and_admin_paths_unchecked,
    probe_records_no_hit_view_or_cache_entry,
    hostile_paths,
);

fn probe(app: &App) -> SiteProbe {
    SiteProbe::new(app.state.clone())
}

async fn published_page_ok_with_item_id(app: App) -> App {
    let p = app.post("Hello", "hello-world", &[]).await;
    let page = app.page("About", "about", None).await;
    assert_eq!(
        probe(&app).probe("/hello-world/").await,
        PathStatus::Ok { item: Some(p.id) }
    );
    assert_eq!(
        probe(&app).probe("/about/?x=1").await,
        PathStatus::Ok {
            item: Some(page.id)
        }
    );
    assert_eq!(probe(&app).probe("/").await, PathStatus::Ok { item: None });
    app
}

async fn draft_page_not_found(app: App) -> App {
    app.draft_post("Draft", "draft-one", &[]).await;
    assert_eq!(probe(&app).probe("/draft-one/").await, PathStatus::NotFound);
    assert_eq!(probe(&app).probe("/no-such/").await, PathStatus::NotFound);
    app
}

async fn trashed_media_not_found(app: App) -> App {
    let hash = "a".repeat(64);
    assert_eq!(
        probe(&app).probe(&format!("/_media/{hash}/x.png")).await,
        PathStatus::NotFound
    );
    assert_eq!(
        probe(&app).probe("/_media/not-a-hash/x.png").await,
        PathStatus::NotFound
    );
    app
}

async fn stored_redirect_followed_and_reported(app: App) -> App {
    let p = app.post("Hello", "hello-world", &[]).await;
    let r = app.redirects();
    r.upsert(&app.admin, "/a/", "/b/", 301).await.unwrap();
    r.upsert(&app.admin, "/b/", "https://example.com/hello-world/", 302)
        .await
        .unwrap();
    assert_eq!(
        probe(&app).probe("/a/").await,
        PathStatus::Redirected {
            to: "/hello-world/".into(),
            item: Some(p.id)
        }
    );
    r.upsert(&app.admin, "/c/", "/nowhere/", 301).await.unwrap();
    assert_eq!(probe(&app).probe("/c/").await, PathStatus::NotFound);
    app
}

async fn redirect_410_is_gone(app: App) -> App {
    app.redirects()
        .upsert(&app.admin, "/old/", "/gone-target/", 410)
        .await
        .unwrap();
    assert_eq!(probe(&app).probe("/old/").await, PathStatus::Gone);
    app
}

async fn redirect_to_external_is_external(app: App) -> App {
    app.redirects()
        .upsert(&app.admin, "/out/", "https://other.example/x", 301)
        .await
        .unwrap();
    assert_eq!(
        probe(&app).probe("/out/").await,
        PathStatus::External("https://other.example/x".into())
    );
    app
}

async fn canonical_trailing_slash_redirected(app: App) -> App {
    let p = app.post("Hello", "hello-world", &[]).await;
    assert_eq!(
        probe(&app).probe("/hello-world").await,
        PathStatus::Redirected {
            to: "/hello-world/".into(),
            item: Some(p.id)
        }
    );
    app
}

async fn term_and_archive_routes_ok(app: App) -> App {
    let news = app.category("news").await;
    app.post("Hello", "hello-world", &[news]).await;
    for path in ["/category/news/", "/post/", "/robots.txt", "/sitemap.xml"] {
        assert_eq!(
            probe(&app).probe(path).await,
            PathStatus::Ok { item: None },
            "{path}"
        );
    }
    assert_eq!(
        probe(&app).probe("/category/nope/").await,
        PathStatus::NotFound
    );
    app
}

async fn plugin_and_admin_paths_unchecked(app: App) -> App {
    for path in [
        "/_p/analytics/",
        "/admin/",
        "/API/v1/types",
        "/mcp",
        "/oauth/authorize",
        "/.well-known/oauth-authorization-server",
    ] {
        assert_eq!(
            probe(&app).probe(path).await,
            PathStatus::Unchecked,
            "{path}"
        );
    }
    app.redirects()
        .upsert(&app.admin, "/to-admin/", "/admin/", 301)
        .await
        .unwrap();
    assert_eq!(probe(&app).probe("/to-admin/").await, PathStatus::Unchecked);
    assert_eq!(
        probe(&app)
            .probe("/_preview/0190f1c0-0000-7000-8000-000000000001")
            .await,
        PathStatus::NotFound
    );
    app
}

/// Review Focus: the probe never fills the page cache, counts a redirect hit, a page
/// view or a `request.served` event.
async fn probe_records_no_hit_view_or_cache_entry(app: App) -> App {
    app.post("Hello", "hello-world", &[]).await;
    app.redirects()
        .upsert(&app.admin, "/old/", "/hello-world/", 301)
        .await
        .unwrap();
    let hits = Arc::new(HitCounter::new(Arc::new(RedirectService::new(
        app.site.store.clone(),
    ))));
    let served = Arc::new(ServedBuffer::new(
        app.site.store.clone(),
        app.site.host.clone(),
    ));
    let metrics = Arc::new(ferrite_http::metrics::Metrics::new("9.9.9", "t"));
    let app = app
        .rebuilt(App::config(), |s| {
            s.with_hits(hits.clone())
                .with_served(served.clone())
                .with_metrics(metrics.clone())
        })
        .await;
    let before = metrics.snapshot();
    let p = probe(&app);
    for _ in 0..50 {
        p.probe("/old/").await;
        p.probe("/hello-world/").await;
    }
    assert_eq!(app.cached().await, 0, "no page cache entry");
    assert!(hits.is_empty(), "no redirect hit");
    assert!(served.is_empty(), "no request.served event");
    assert_eq!(
        metrics.snapshot(),
        before,
        "no page view or request counted"
    );
    app
}

async fn hostile_paths(app: App) -> App {
    let long = format!("/{}", "a".repeat(10 * 1024));
    for path in [
        "//evil.example/x",
        "/%2F%2Fevil",
        "/a\u{0}b",
        "/a\nb",
        "relative",
        "",
        long.as_str(),
        "/%ff%fe/",
        "/../../etc/passwd",
    ] {
        let s = probe(&app).probe(path).await;
        assert!(
            matches!(s, PathStatus::NotFound | PathStatus::Unchecked),
            "{path:?}: {s:?}"
        );
    }
    app
}
