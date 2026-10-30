//! `page_tree()` over HTTP (M4-3b P56, P57; Review Focus 3): a published child of a
//! draft parent renders but leaves the navigation, and publishing a page refreshes the
//! cached pages that drew the tree.

use axum::http::StatusCode;

use crate::app_tests;
use crate::harness::{App, Resp, draft, fixture, load_theme};

async fn tree_app(app: App) -> App {
    app.with_theme(load_theme(fixture("tree-theme"))).await
}

/// The sidebar (`<nav class="tree">…</nav>`) of a page.
fn nav(r: &Resp) -> &str {
    r.body
        .split_once("<nav class=\"tree\">")
        .and_then(|(_, rest)| rest.split_once("</nav>"))
        .map_or("", |(n, _)| n)
}

/// `prev|next` titles of a page.
fn pager(r: &Resp) -> &str {
    r.body
        .split_once("<p class=\"pager\">")
        .and_then(|(_, rest)| rest.split_once("</p>"))
        .map_or("", |(p, _)| p)
}

/// Alpha, Hidden (draft) with its published Child, and Omega: siblings sort by title.
async fn draft_parent_hides_the_subtree(app: App) -> App {
    let app = tree_app(app).await;
    app.page("Alpha", "alpha", None).await;
    let hidden = app
        .svc
        .create(&app.admin, draft("page", "Hidden", "hidden", None, &[]))
        .await
        .unwrap();
    app.page("Child", "child", Some(hidden.id)).await;
    app.page("Omega", "omega", None).await;

    let child = app.get("/hidden/child/").await;
    assert_eq!(child.status, StatusCode::OK, "{}", child.body);
    assert_eq!(child.tpl(), "page");
    assert!(child.body.contains("<h1>Child</h1>"), "{}", child.body);
    assert_eq!(nav(&child), "[Alpha][Omega]");
    assert_eq!(pager(&child), "|");

    let alpha = app.get("/alpha/").await;
    assert_eq!(nav(&alpha), "[Alpha*][Omega]");
    assert_eq!(pager(&alpha), "|Omega");
    let omega = app.get("/omega/").await;
    assert_eq!(pager(&omega), "Alpha|");
    assert!(!omega.body.contains("Child"), "{}", omega.body);
    app
}

/// A cached page shows a sibling published after it was cached, after one poll of
/// the invalidation loop (`archive:page`).
async fn publishing_refreshes_the_tree(app: App) -> App {
    let app = tree_app(app).await;
    app.page("Alpha", "alpha", None).await;
    app.page("Beta", "beta", None).await;
    let mut poller = app.poller().await;
    assert_eq!(nav(&app.get("/alpha/").await), "[Alpha*][Beta]");
    assert_eq!(app.cached().await, 1);

    app.page("Gamma", "gamma", None).await;
    // No poll yet: the cache answers.
    assert_eq!(nav(&app.get("/alpha/").await), "[Alpha*][Beta]");
    poller.poll_once().await.unwrap();
    let alpha = app.get("/alpha/").await;
    assert_eq!(nav(&alpha), "[Alpha*][Beta][Gamma]");
    assert_eq!(pager(&alpha), "|Beta");
    assert_eq!(pager(&app.get("/gamma/").await), "Beta|");
    app
}

app_tests!(
    draft_parent_hides_the_subtree,
    publishing_refreshes_the_tree
);
