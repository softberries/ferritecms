//! Store calls per uncached page: a fixed bound with the demo site's menus (final
//! review I3).

use std::{path::PathBuf, sync::Arc};

use axum::http::StatusCode;
use ferrite_core::{MenuService, PermalinkRules, SiteReader};
use ferrite_domain::{ArtifactHash, Menu, MenuItem, MenuItemId, MenuItemKind, Timestamp};
use ferrite_http::{AppParts, AppState, HttpConfig, build_app, load_route_table};
use ferrite_store::Store;
use ferrite_store_tests::CountingStore;

use crate::app_tests;
use crate::common::SITE_URL;
use crate::harness::{App, SITE_TITLE, load_theme, request, send};

/// Most store calls a zero-result search page with both demo menus may take.
const SEARCH_PAGE_BOUND: usize = 20;
/// Most store calls the two demo menus (7 entries) may add to a page: since M4-4 (P67)
/// each menu with a public item entry also reads `reading.front_page` once
/// (`SiteReader::links` names the front page `/`), one call per menu.
const MENUS_BOUND: usize = 10;

fn entry(position: i32, kind: MenuItemKind, target: Option<uuid::Uuid>, url: &str) -> MenuItem {
    MenuItem {
        id: MenuItemId::new(),
        parent_id: None,
        position,
        kind,
        target_id: target,
        url: (!url.is_empty()).then(|| url.to_owned()),
        label: String::new(),
    }
}

/// The demo menus of `ferrite seed demo`: primary (Home, About, News, Contact) and
/// footer (Team, Contact, RSS feed).
async fn demo_menus(app: &App) {
    let about = app.page("About", "about", None).await;
    let team = app.page("Team", "team", Some(about.id)).await;
    let contact = app.page("Contact", "contact", None).await;
    let news = app.category("news").await;
    for i in 0..3 {
        app.post(&format!("News {i}"), &format!("news-{i}"), &[news])
            .await;
    }
    let menus = MenuService::new(app.site.store.clone());
    for (name, items) in [
        (
            "primary",
            vec![
                entry(0, MenuItemKind::Url, None, "/"),
                entry(1, MenuItemKind::Item, Some(about.id.as_uuid()), ""),
                entry(2, MenuItemKind::Term, Some(news.as_uuid()), ""),
                entry(3, MenuItemKind::Item, Some(contact.id.as_uuid()), ""),
            ],
        ),
        (
            "footer",
            vec![
                entry(0, MenuItemKind::Item, Some(team.id.as_uuid()), ""),
                entry(1, MenuItemKind::Item, Some(contact.id.as_uuid()), ""),
                entry(2, MenuItemKind::Url, None, "/feed/"),
            ],
        ),
    ] {
        menus
            .put(
                &app.admin,
                &Menu {
                    name: name.into(),
                    label: name.into(),
                    items,
                    locale: "en".into(),
                    translation_of: None,
                },
            )
            .await
            .unwrap();
    }
}

/// The app over a counting store, with the blog theme (which shows both menus).
async fn counting_app(app: &App) -> (Arc<CountingStore>, axum::Router) {
    counting_app_with(app, HttpConfig::new(SITE_URL, SITE_TITLE, "en")).await
}

async fn counting_app_with(app: &App, config: HttpConfig) -> (Arc<CountingStore>, axum::Router) {
    let counting = Arc::new(CountingStore::new(app.site.store.clone()));
    let store: Arc<dyn Store> = counting.clone();
    let reader = Arc::new(SiteReader::new(
        store.clone(),
        Arc::new(PermalinkRules::default()),
        Arc::new(|hash: &ArtifactHash, name: &str| format!("/_media/{hash}/{name}")),
        Arc::new(Timestamp::now),
    ));
    let routes = load_route_table(store.as_ref(), &PermalinkRules::default())
        .await
        .unwrap();
    let theme = load_theme(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../themes/blog"));
    let state = Arc::new(
        AppState::new(AppParts {
            store,
            reader,
            plugins: app.site.host.clone(),
            media: app.site.media.clone(),
            theme,
            cache: Arc::new(ferrite_cache::PageCache::new(64 * 1024 * 1024)),
            routes,
            config,
            migrations_ok: true,
        })
        .expect("app state"),
    );
    (counting, build_app(state))
}

/// Store calls of one uncached request for `path`.
async fn calls(counting: &CountingStore, router: &axum::Router, path: &str) -> usize {
    counting.reset();
    let r = send(router, request(axum::http::Method::GET, path)).await;
    assert_eq!(r.status, StatusCode::OK, "{path}: {r:?}");
    counting.calls()
}

/// A zero-result search (uncacheable per query) renders both demo menus within a fixed
/// number of store calls; the menus' share does not depend on how they are built.
async fn menu_cost_is_bounded(app: App) -> App {
    let (counting, router) = counting_app(&app).await;
    let without = calls(&counting, &router, "/?s=qqqq").await;
    demo_menus(&app).await;
    let (counting, router) = counting_app(&app).await;
    let with = calls(&counting, &router, "/?s=qqqq").await;
    eprintln!("store calls for /?s=qqqq: {without} without menus, {with} with the demo menus");
    assert!(with <= SEARCH_PAGE_BOUND, "{with} calls");
    assert!(
        with - without <= MENUS_BOUND,
        "menus cost {}",
        with - without
    );
    app
}

/// The render slot covers the whole content pipeline (ruling R32): with no slot free,
/// a request is answered 503 before it loads anything (only the redirect lookup, which
/// runs before the cache, touches the store).
async fn no_slot_no_data_load(app: App) -> App {
    app.post("Hello", "hello", &[]).await;
    let (counting, router) = counting_app(&app).await;
    let rendered = calls(&counting, &router, "/hello/").await;
    let mut config = HttpConfig::new(SITE_URL, SITE_TITLE, "en");
    config.max_concurrent_renders = 0;
    config.render_wait = std::time::Duration::from_millis(20);
    let (counting, router) = counting_app_with(&app, config).await;
    counting.reset();
    let r = send(&router, request(axum::http::Method::GET, "/hello/")).await;
    assert_eq!(r.status, StatusCode::SERVICE_UNAVAILABLE, "{r:?}");
    assert!(r.header("retry-after").is_some());
    eprintln!(
        "store calls for /hello/: {rendered} rendered, {} with no slot",
        counting.calls()
    );
    assert!(
        counting.calls() <= 2,
        "{} calls before the slot",
        counting.calls()
    );
    app
}

/// A path over the router's `MAX_PATH_BYTES` is a 404 without a redirect lookup (final
/// review M4): once the shared 404 body is cached it costs no store call at all.
async fn overlong_path_skips_redirects(app: App) -> App {
    let (counting, router) = counting_app(&app).await;
    let get = |path: String| send(&router, request(axum::http::Method::GET, &path));
    assert_eq!(get("/missing/".into()).await.status, StatusCode::NOT_FOUND);
    let long = format!("/{}/", "a".repeat(ferrite_http::route::MAX_PATH_BYTES));
    counting.reset();
    assert_eq!(get(long).await.status, StatusCode::NOT_FOUND);
    assert_eq!(counting.calls(), 0);
    // A normal missing path still checks the redirect table.
    counting.reset();
    assert_eq!(get("/missing/".into()).await.status, StatusCode::NOT_FOUND);
    assert!(counting.calls() > 0);
    app
}

app_tests!(
    menu_cost_is_bounded,
    no_slot_no_data_load,
    overlong_path_skips_redirects
);
