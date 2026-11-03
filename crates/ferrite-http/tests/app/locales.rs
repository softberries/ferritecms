//! Multilingual pages over HTTP (M6-1 Task 10, design 13 §4–§5): locale prefixes,
//! `<html lang>`, `hreflang` alternates, per-locale front pages, feeds and sitemaps.

use axum::http::StatusCode;
use ferrite_core::{ContentDraft, OptionService, site::FRONT_PAGE_OPTION};
use ferrite_domain::ContentItem;
use serde_json::json;

use crate::app_tests;
use crate::harness::App;

app_tests!(
    de_post_served_under_prefix_with_html_lang_and_hreflang,
    missing_translation_is_404,
    de_front_page_is_translation_of_front_page,
    de_feed_lists_de_items_with_language_de,
    sitemap_alternates_and_namespace_only_when_multilingual,
    locale_texts_title_in_head_and_feed,
    adding_translation_refreshes_cached_original,
);

async fn de(app: App) -> App {
    app.localized(json!([{"tag": "en"}, {"tag": "de"}])).await
}

/// A published translation of `item` into `locale`.
async fn translated(app: &App, item: &ContentItem, locale: &str, title: &str) -> ContentItem {
    let t = app
        .svc
        .translate(&app.admin, item.id, locale)
        .await
        .expect("translate")
        .item;
    let mut d = ContentDraft {
        kind: t.kind.clone(),
        title: title.into(),
        slug: None,
        parent_id: t.parent_id,
        excerpt: None,
        body: t.body.clone(),
        fields: t.fields.clone(),
        menu_order: t.menu_order,
        terms: Vec::new(),
        seo: None,
    };
    d.slug = Some(t.slug.clone());
    let t = app
        .svc
        .update(&app.admin, t.id, t.version, d)
        .await
        .expect("update");
    app.svc
        .publish(&app.admin, t.id, t.version)
        .await
        .expect("publish")
}

async fn de_post_served_under_prefix_with_html_lang_and_hreflang(app: App) -> App {
    let app = de(app).await;
    let en = app.page("Garden", "garden", None).await;
    translated(&app, &en, "de", "Garten").await;
    let r = app.get("/de/garden/").await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert!(r.body.contains("Garten"), "{}", r.body);
    for want in [
        "<link rel=\"alternate\" href=\"https://example.com/garden/\" hreflang=\"en\">",
        "<link rel=\"alternate\" href=\"https://example.com/de/garden/\" hreflang=\"de\">",
        "<link rel=\"alternate\" href=\"https://example.com/garden/\" hreflang=\"x-default\">",
    ] {
        assert!(r.body.contains(want), "{want}\n{}", r.body);
    }
    // The default-locale page names the same alternates.
    let r = app.get("/garden/").await;
    assert!(r.body.contains("hreflang=\"de\""), "{}", r.body);
    // `<html lang>` follows the request's locale (the fixture's index template has it).
    assert!(app.get("/de/").await.body.contains("<html lang=\"de\">"));
    assert!(app.get("/").await.body.contains("<html lang=\"en\">"));
    // Case and slash canonicalise.
    let r = app.get("/DE/garden").await;
    assert_eq!(r.status, StatusCode::MOVED_PERMANENTLY);
    assert_eq!(r.header("location"), Some("/de/garden/"));
    // Files and internal routes do not exist under a prefix.
    assert_eq!(
        app.get("/de/robots.txt").await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        app.get("/de/sitemap.xml").await.status,
        StatusCode::NOT_FOUND
    );
    app
}

async fn missing_translation_is_404(app: App) -> App {
    let app = de(app).await;
    app.page("Only English", "only", None).await;
    assert_eq!(app.get("/only/").await.status, StatusCode::OK);
    assert_eq!(app.get("/de/only/").await.status, StatusCode::NOT_FOUND);
    // No translations: no alternates at all.
    let r = app.get("/only/").await;
    assert!(!r.body.contains("hreflang"), "{}", r.body);
    app
}

async fn de_front_page_is_translation_of_front_page(app: App) -> App {
    let app = de(app).await;
    let home = app.page("Home", "home", None).await;
    OptionService::new(app.site.store.clone())
        .set_many(
            &app.admin,
            &[(FRONT_PAGE_OPTION.into(), Some(json!(home.id.to_string())))],
        )
        .await
        .unwrap();
    // No translation yet: the German root lists German posts (none).
    assert_eq!(app.get("/de/").await.status, StatusCode::OK);
    let start = translated(&app, &home, "de", "Startseite").await;
    let app = app.localized(json!([{"tag": "en"}, {"tag": "de"}])).await;
    let r = app.get("/de/").await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(r.body.contains("Startseite"), "{}", r.body);
    assert!(
        r.body
            .contains("href=\"https://example.com/\" hreflang=\"en\""),
        "{}",
        r.body
    );
    // The translation's own permalink answers 301 to the locale root.
    let r = app.get(&format!("/de/{}/", start.slug)).await;
    assert_eq!(r.status, StatusCode::MOVED_PERMANENTLY, "{r:?}");
    assert_eq!(r.header("location"), Some("/de/"));
    app
}

async fn de_feed_lists_de_items_with_language_de(app: App) -> App {
    let app = de(app).await;
    let en = app.post("Hello", "hello", &[]).await;
    translated(&app, &en, "de", "Hallo").await;
    let r = app.get("/de/feed/").await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(r.body.contains("<language>de</language>"), "{}", r.body);
    assert!(
        r.body
            .contains("<link>https://example.com/de/hello/</link>"),
        "{}",
        r.body
    );
    assert!(!r.body.contains("<title>Hello</title>"), "{}", r.body);
    assert!(
        r.body
            .contains("href=\"https://example.com/de/feed/\" rel=\"self\""),
        "{}",
        r.body
    );
    let r = app.get("/feed/").await;
    assert!(r.body.contains("<language>en</language>"), "{}", r.body);
    assert!(!r.body.contains("/de/hello/"), "{}", r.body);
    app
}

async fn sitemap_alternates_and_namespace_only_when_multilingual(app: App) -> App {
    let en = app.post("Hello", "hello", &[]).await;
    let one = app.get("/sitemap-post-1.xml").await;
    assert!(!one.body.contains("xmlns:xhtml"), "{}", one.body);
    let app = de(app).await;
    translated(&app, &en, "de", "Hallo").await;
    let r = app.get("/sitemap-post-1.xml").await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(
        r.body
            .contains("xmlns:xhtml=\"http://www.w3.org/1999/xhtml\""),
        "{}",
        r.body
    );
    for want in [
        "<loc>https://example.com/de/hello/</loc>",
        "<xhtml:link rel=\"alternate\" hreflang=\"de\" href=\"https://example.com/de/hello/\"/>",
        "<xhtml:link rel=\"alternate\" hreflang=\"x-default\" href=\"https://example.com/hello/\"/>",
    ] {
        assert!(r.body.contains(want), "{want}\n{}", r.body);
    }
    app
}

async fn locale_texts_title_in_head_and_feed(app: App) -> App {
    let app = de(app).await;
    OptionService::new(app.site.store.clone())
        .set_many(
            &app.admin,
            &[(
                "site.locale_texts".into(),
                Some(json!({"de": {"title": "Mein Garten", "description": "Über Pflanzen"}})),
            )],
        )
        .await
        .unwrap();
    let en = app.post("Hello", "hello", &[]).await;
    translated(&app, &en, "de", "Hallo").await;
    let r = app.get("/de/hallo-does-not-exist/").await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    let r = app.get("/de/").await;
    assert!(r.body.contains("Mein Garten"), "{}", r.body);
    let feed = app.get("/de/feed/").await;
    assert!(
        feed.body.contains("<title>Mein Garten</title>"),
        "{}",
        feed.body
    );
    let feed = app.get("/feed/").await;
    assert!(!feed.body.contains("Mein Garten"), "{}", feed.body);
    app
}

async fn adding_translation_refreshes_cached_original(app: App) -> App {
    let app = de(app).await;
    let en = app.page("Garden", "garden", None).await;
    let mut poller = app.poller().await;
    let r = app.get("/garden/").await;
    assert!(!r.body.contains("hreflang"), "{}", r.body);
    assert!(app.cached().await > 0);
    translated(&app, &en, "de", "Garten").await;
    poller.poll_once().await.unwrap();
    let r = app.get("/garden/").await;
    assert!(r.body.contains("hreflang=\"de\""), "{}", r.body);
    app
}
