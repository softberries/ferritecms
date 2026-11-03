//! Live, link and whole-site previews (M6-3 task 10, design 15 §3, §5, §7).

use std::sync::Arc;

use axum::http::StatusCode;
use ferrite_core::{
    AutosaveDraft, ContentService, PermalinkRules,
    preview_link::PreviewLinkService,
    release::{EntryInput, NewRelease, ReleaseService},
};
use ferrite_domain::{ContentId, EntryTransition, PreviewTarget, Role};
use ferrite_http::build_app;
use serde_json::json;

use crate::app_tests;
use crate::harness::{App, Resp, fixture, load_theme, with_cookie};

struct Editorial {
    releases: Arc<ReleaseService>,
    links: Arc<PreviewLinkService>,
}

/// `app` with a state that serves link and whole-site previews.
async fn editorial(mut app: App) -> (App, Editorial) {
    let store = app.site.store.clone();
    let rules = Arc::new(PermalinkRules::default());
    let content = Arc::new(ContentService::new(store.clone(), rules.clone()));
    let releases = Arc::new(ReleaseService::new(store.clone(), rules, content));
    let links = Arc::new(PreviewLinkService::new(store));
    let state = App::build_state(
        &app.site,
        app.site.reader.clone(),
        load_theme(fixture("app-theme")),
        true,
        App::config(),
    )
    .await
    .with_editorial(releases.clone(), links.clone());
    let state = Arc::new(state);
    app.router = build_app(state.clone());
    app.state = state;
    (app, Editorial { releases, links })
}

fn preview_cookie(r: &Resp) -> String {
    let set = r.header("set-cookie").expect("set-cookie");
    set.split(';').next().unwrap().to_owned()
}

fn assert_preview_headers(r: &Resp) {
    assert_eq!(
        r.header("cache-control"),
        Some("private, no-store"),
        "{r:?}"
    );
    assert_eq!(r.header("x-robots-tag"), Some("noindex"), "{r:?}");
}

/// A release publishing a new post `slug`, with a share link of it.
async fn release_with_link(app: &App, ed: &Editorial, slug: &str) -> (ContentId, String) {
    let it = app.draft_post("Released post", slug, &[]).await;
    let r = ed
        .releases
        .create(
            &app.admin,
            NewRelease {
                title: "Spring launch".into(),
                description: String::new(),
            },
        )
        .await
        .unwrap();
    ed.releases
        .stage(
            &app.admin,
            r.id,
            r.version,
            EntryInput {
                item: it.id,
                transition: EntryTransition::Publish,
                changes: None,
            },
        )
        .await
        .unwrap();
    let (_, token) = ed
        .links
        .create(&app.admin, PreviewTarget::Release(r.id), Some(1))
        .await
        .unwrap();
    (it.id, token)
}

async fn working_preview_shows_autosave(app: App) -> App {
    let (app, _) = editorial(app).await;
    let it = app.draft_post("Saved title", "saved", &[]).await;
    app.svc
        .put_autosave(
            &app.ada,
            it.id,
            AutosaveDraft {
                base_version: it.version,
                title: "Typed title".into(),
                excerpt: None,
                slug: None,
                parent_id: None,
                menu_order: None,
                terms: None,
                body: json!({"type": "doc", "content": []}),
                fields: json!({}),
                seo: None,
            },
        )
        .await
        .unwrap();
    let cookie = app.session(&app.ada_user).await;
    let r = app
        .get_with_cookie(&format!("/_preview/{}?working=1", it.id), &cookie)
        .await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert!(r.body.contains("<h1>Typed title</h1>"), "{}", r.body);
    assert!(
        !r.body.contains("aria-label=\"Preview\""),
        "the editor iframe has no bar"
    );
    let r = app
        .get_with_cookie(&format!("/_preview/{}", it.id), &cookie)
        .await;
    assert!(r.body.contains("<h1>Saved title</h1>"), "{}", r.body);
    app
}

async fn release_link_sets_cookie_and_redirects(app: App) -> App {
    let (app, ed) = editorial(app).await;
    let (_, token) = release_with_link(&app, &ed, "launch").await;
    let r = app
        .get(&format!("/_preview/link/{token}?to=%2Flaunch%2F"))
        .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{r:?}");
    assert_eq!(r.header("location"), Some("/launch/"));
    let set = r.header("set-cookie").unwrap();
    assert!(
        set.starts_with(&format!("__Host-ferrite_preview={token};")),
        "{set}"
    );
    assert!(set.contains("HttpOnly") && set.contains("SameSite=Lax"));
    let r = app
        .get(&format!("/_preview/link/{token}?to=//evil.example/"))
        .await;
    assert_eq!(r.header("location"), Some("/"));
    // An unknown token is the 404 page.
    let r = app
        .get("/_preview/link/ferp_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA")
        .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    app
}

async fn cookie_preview_renders_release_and_is_never_cached(app: App) -> App {
    let (app, ed) = editorial(app).await;
    let (_, token) = release_with_link(&app, &ed, "launch").await;
    let link = app.get(&format!("/_preview/link/{token}")).await;
    let cookie = preview_cookie(&link);
    let before = app.cached().await;
    let seq = app.site.store.read().events().max_seq().await.unwrap();
    let r = app.get_with_cookie("/launch/", &cookie).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert!(r.body.contains("<h1>Released post</h1>"), "{}", r.body);
    assert!(r.body.contains("aria-label=\"Preview\""), "the core bar");
    assert!(r.body.contains("Spring launch"));
    assert_preview_headers(&r);
    assert_eq!(r.header("referrer-policy"), Some("no-referrer"));
    assert_eq!(r.header("etag"), None);
    // Listings see it too.
    let home = app.get_with_cookie("/", &cookie).await;
    assert!(home.body.contains("Released post"), "{}", home.body);
    assert_eq!(app.cached().await, before, "never cached");
    assert_eq!(
        app.site.store.read().events().max_seq().await.unwrap(),
        seq,
        "no outbox rows"
    );
    // Live: still a 404.
    assert_eq!(app.get("/launch/").await.status, StatusCode::NOT_FOUND);
    // Exit clears the cookie.
    let exit = app.get("/_preview/exit").await;
    assert_eq!(exit.status, StatusCode::SEE_OTHER);
    assert!(exit.header("set-cookie").unwrap().contains("Max-Age=0"));
    app
}

async fn revoked_cookie_cleared_and_live_page_served(app: App) -> App {
    let (app, ed) = editorial(app).await;
    app.post("Live post", "live", &[]).await;
    let (_, token) = release_with_link(&app, &ed, "launch").await;
    let link = app.get(&format!("/_preview/link/{token}")).await;
    let cookie = preview_cookie(&link);
    let id = ed.links.resolve(&token).await.unwrap().unwrap().link.id;
    ed.links.revoke(&app.admin, id).await.unwrap();
    let r = app.get_with_cookie("/live/", &cookie).await;
    assert_eq!(r.status, StatusCode::OK);
    assert!(!r.body.contains("aria-label=\"Preview\""));
    assert!(
        r.header("set-cookie")
            .is_some_and(|c| c.starts_with("__Host-ferrite_preview=;") && c.contains("Max-Age=0")),
        "{r:?}"
    );
    app
}

async fn item_link_opens_without_session(app: App) -> App {
    let (app, ed) = editorial(app).await;
    let it = app.draft_post("Shared draft", "shared", &[]).await;
    let editor = app.site.user("eddie", Role::Editor).await;
    let (_, token) = ed
        .links
        .create(
            &ferrite_core::Actor::new(editor.id, Role::Editor),
            PreviewTarget::Item(it.id),
            None,
        )
        .await
        .unwrap();
    let r = app
        .send(with_cookie(&format!("/_preview/link/{token}"), ""))
        .await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert!(r.body.contains("<h1>Shared draft</h1>"));
    assert!(r.body.contains("aria-label=\"Preview\""));
    assert_preview_headers(&r);
    // The editor's live preview: the creator's autosave over the item (P7: the iframe
    // loads through a link).
    let ada = ferrite_core::Actor::new(app.ada_user.id, Role::Author);
    app.svc
        .put_autosave(
            &ada,
            it.id,
            AutosaveDraft {
                base_version: it.version,
                title: "Typed in the editor".into(),
                excerpt: None,
                slug: None,
                parent_id: None,
                menu_order: None,
                terms: None,
                body: json!({"type": "doc", "content": []}),
                fields: json!({}),
                seo: None,
            },
        )
        .await
        .unwrap();
    let (_, mine) = ed
        .links
        .create(&ada, PreviewTarget::Item(it.id), Some(1))
        .await
        .unwrap();
    let r = app.get(&format!("/_preview/link/{mine}?working=1")).await;
    assert!(
        r.body.contains("<h1>Typed in the editor</h1>"),
        "{}",
        r.body
    );
    app
}

app_tests!(
    working_preview_shows_autosave,
    release_link_sets_cookie_and_redirects,
    cookie_preview_renders_release_and_is_never_cached,
    revoked_cookie_cleared_and_live_page_served,
    item_link_opens_without_session,
);
