//! Review Focus 1 (no draft leaks) and 2 (no stale pages) at the HTTP level, and the
//! page cache serving repeat requests.

use axum::http::StatusCode;
use ferrite_core::{
    MenuService,
    events::{PLUGINS_KEY, THEME_KEY, invalidate},
};
use ferrite_domain::{ContentStatus, Menu, MenuItem, MenuItemId, MenuItemKind};

use crate::app_tests;
use crate::common::HOUR;
use crate::harness::{App, draft};

async fn menu(app: &App, targets: &[ferrite_domain::ContentId]) {
    let items = targets
        .iter()
        .enumerate()
        .map(|(i, id)| MenuItem {
            id: MenuItemId::new(),
            parent_id: None,
            position: i32::try_from(i).unwrap(),
            kind: MenuItemKind::Item,
            target_id: Some(id.as_uuid()),
            url: None,
            label: String::new(),
        })
        .collect();
    MenuService::new(app.site.store.clone())
        .put(
            &app.admin,
            &Menu {
                name: "primary".into(),
                label: "Primary".into(),
                items,
            },
        )
        .await
        .unwrap();
}

const SECRETS: [&str; 7] = [
    "Secret Draft",
    "Secret Pending",
    "Secret Scheduled",
    "Secret Trashed",
    "Secret Private",
    "Secret Future",
    "Secret Page",
];

/// Draft, pending, scheduled, private, future-dated and trashed items never render:
/// not at their URL, not in any listing, search, menu or `related()`.
async fn unpublished_items_never_render(app: App) -> App {
    let news = app.category("news").await;
    let public = app.post("Public Post", "public-post", &[news]).await;
    app.post("Public Neighbour", "public-neighbour", &[news])
        .await;

    let draft_item = app
        .draft_post("Secret Draft", "secret-draft", &[news])
        .await;
    let pending = app
        .draft_post("Secret Pending", "secret-pending", &[news])
        .await;
    app.svc
        .submit(&app.ada, pending.id, pending.version)
        .await
        .unwrap();
    let scheduled = app
        .draft_post("Secret Scheduled", "secret-scheduled", &[news])
        .await;
    let at =
        ferrite_domain::Timestamp::from_micros(ferrite_domain::Timestamp::now().as_micros() + HOUR)
            .unwrap();
    app.svc
        .schedule(&app.ada, scheduled.id, scheduled.version, at)
        .await
        .unwrap();
    let trashed = app.post("Secret Trashed", "secret-trashed", &[news]).await;
    app.svc
        .trash(&app.ada, trashed.id, trashed.version)
        .await
        .unwrap();
    app.site
        .post(
            &app.ada_user,
            "secret-private",
            "Secret Private",
            ContentStatus::Private,
            Some(-HOUR),
        )
        .await;
    app.site
        .post(
            &app.ada_user,
            "secret-future",
            "Secret Future",
            ContentStatus::Published,
            Some(HOUR),
        )
        .await;
    let secret_page = app
        .svc
        .create(
            &app.admin,
            draft("page", "Secret Page", "secret-page", None, &[]),
        )
        .await
        .unwrap();
    menu(&app, &[public.id, draft_item.id, secret_page.id]).await;

    for slug in [
        "secret-draft",
        "secret-pending",
        "secret-scheduled",
        "secret-trashed",
        "secret-private",
        "secret-future",
        "secret-page",
    ] {
        let r = app.get(&format!("/{slug}/")).await;
        assert_eq!(r.status, StatusCode::NOT_FOUND, "{slug}");
        assert!(!r.body.contains("Secret"), "{slug}: {}", r.body);
    }
    let year = &public.published_at.unwrap().to_rfc3339()[..4];
    for path in [
        "/".to_owned(),
        "/post/".to_owned(),
        "/category/news/".to_owned(),
        "/author/ada/".to_owned(),
        format!("/{year}/"),
        "/?s=Secret".to_owned(),
        "/?s=Post".to_owned(),
        // Single page: menu and related().
        "/public-post/".to_owned(),
    ] {
        let r = app.get(&path).await;
        assert_eq!(r.status, StatusCode::OK, "{path}");
        // Titles, not the word: search echoes its query.
        for title in SECRETS {
            assert!(!r.body.contains(title), "{path}: {title}: {}", r.body);
        }
    }
    // Plugins' `content.read` sees the same public view as the site.
    use ferrite_plugin_host::HostServices;
    let listed = app.site.services.list_posts("post", 100, 0).await;
    assert!(
        listed.iter().all(|p| !p.title.contains("Secret")),
        "{listed:?}"
    );
    assert_eq!(listed.len(), 2);
    // URLs are HTML-escaped (`/` as `&#x2f;`).
    let r = app.get("/public-post/").await;
    assert!(
        r.body
            .contains("class=\"menu\" href=\"&#x2f;public-post&#x2f;\">Public Post<"),
        "{}",
        r.body
    );
    assert!(
        r.body
            .contains("class=\"related\" href=\"&#x2f;public-neighbour&#x2f;\""),
        "{}",
        r.body
    );
    app
}

/// Editing a published post's title updates its page, its category archive, the home
/// listing and the type archive after one poll of the invalidation loop; trashing it
/// removes it from all of them. Until the poll, the cache answers (the old title).
async fn edits_and_trash_reach_every_page(app: App) -> App {
    let news = app.category("news").await;
    let hello = app.post("Hello World", "hello-world", &[news]).await;
    app.post("Other Post", "other-post", &[news]).await;
    let mut poller = app.poller().await;
    let pages = ["/hello-world/", "/category/news/", "/", "/post/"];
    for path in pages {
        let r = app.get(path).await;
        assert_eq!(r.status, StatusCode::OK, "{path}");
        assert!(r.body.contains("Hello World"), "{path}");
    }
    assert_eq!(app.cached().await, 4);

    let edited = app
        .svc
        .update(
            &app.ada,
            hello.id,
            hello.version,
            draft("post", "Hello Again", "hello-world", None, &[news]),
        )
        .await
        .unwrap();
    // No poll yet: cache hits, old title.
    for path in pages {
        assert!(
            app.get(path).await.body.contains("Hello World"),
            "{path} cached"
        );
    }
    poller.poll_once().await.unwrap();
    assert_eq!(app.cached().await, 0, "every page showed the post");
    for path in pages {
        let r = app.get(path).await;
        assert_eq!(r.status, StatusCode::OK, "{path}");
        assert!(r.body.contains("Hello Again"), "{path}: {}", r.body);
        assert!(!r.body.contains("Hello World"), "{path}");
    }

    app.svc
        .trash(&app.ada, edited.id, edited.version)
        .await
        .unwrap();
    poller.poll_once().await.unwrap();
    assert_eq!(app.get("/hello-world/").await.status, StatusCode::NOT_FOUND);
    for path in ["/category/news/", "/", "/post/", "/?s=Hello"] {
        let r = app.get(path).await;
        assert_eq!(r.status, StatusCode::OK, "{path}");
        assert!(!r.body.contains("Hello Again"), "{path}: {}", r.body);
        assert!(
            r.body.contains("Other Post") || path.contains("?s="),
            "{path}"
        );
    }
    app
}

/// The second request for a page is a cache hit (one entry, same body and ETag); the
/// search page is cached too (with its short TTL), keyed by the normalised query.
async fn second_request_is_a_cache_hit(app: App) -> App {
    app.post("Hello World", "hello-world", &[]).await;
    let first = app.get("/hello-world/").await;
    assert_eq!(app.cached().await, 1);
    // A write that bypasses the outbox: only a re-render would show it.
    let tx = app.site.store.begin().await.unwrap();
    let mut row = tx
        .content()
        .get_by_slug("post", None, "hello-world")
        .await
        .unwrap()
        .unwrap();
    row.title = "Changed Behind The Cache".into();
    tx.content().update(&row).await.unwrap();
    tx.commit().await.unwrap();
    let second = app.get("/hello-world/").await;
    assert_eq!(second.body, first.body, "served from the cache");
    assert_eq!(second.header("etag"), first.header("etag"));
    assert_eq!(app.cached().await, 1);

    app.get("/?s=Hello").await;
    app.get("/?s=Hello&utm_source=x").await;
    assert_eq!(app.cached().await, 2, "ignored params share the key");

    // Every page depends on `theme` and on `plugins`.
    for key in [PLUGINS_KEY, THEME_KEY] {
        let mut poller = app.poller().await;
        app.get("/hello-world/").await;
        app.get("/?s=Hello").await;
        assert_eq!(app.cached().await, 2);
        app.site
            .store
            .read()
            .events()
            .append(&invalidate([key.to_owned()]))
            .await
            .unwrap();
        poller.poll_once().await.unwrap();
        assert_eq!(app.cached().await, 0, "{key}");
    }
    app
}

/// Menu dependencies come from the render (the bridge), not from the page's own
/// reads: renaming a menu target, changing the menu, and publishing a hidden target
/// each refresh a cached page that shows the menu, after one poll.
async fn menu_changes_reach_cached_pages(app: App) -> App {
    let one = app.post("Target One", "one", &[]).await;
    let hidden = app.draft_post("Target Hidden", "hidden", &[]).await;
    app.page("About", "about", None).await;
    menu(&app, &[one.id, hidden.id]).await;
    let mut poller = app.poller().await;
    let menu_of = |body: &str| body.matches("class=\"menu\"").count();

    let r = app.get("/about/").await;
    assert!(r.body.contains(">Target One<") && !r.body.contains("Target Hidden"));
    assert_eq!(menu_of(&r.body), 1);

    app.svc
        .update(
            &app.ada,
            one.id,
            one.version,
            draft("post", "Target Renamed", "one", None, &[]),
        )
        .await
        .unwrap();
    poller.poll_once().await.unwrap();
    assert!(app.get("/about/").await.body.contains(">Target Renamed<"));

    app.svc
        .publish(&app.ada, hidden.id, hidden.version)
        .await
        .unwrap();
    poller.poll_once().await.unwrap();
    let r = app.get("/about/").await;
    assert!(r.body.contains(">Target Hidden<"), "{}", r.body);

    menu(&app, &[hidden.id]).await;
    poller.poll_once().await.unwrap();
    let r = app.get("/about/").await;
    assert_eq!(menu_of(&r.body), 1);
    assert!(!r.body.contains("Target Renamed"));
    app
}

app_tests!(
    menu_changes_reach_cached_pages,
    unpublished_items_never_render,
    edits_and_trash_reach_every_page,
    second_request_is_a_cache_hit,
);
