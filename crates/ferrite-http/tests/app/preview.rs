//! Previews at `/_preview/<id>` (P20) and logged-in requests on the public site (P21).

use std::{path::PathBuf, sync::Arc};

use axum::http::{Method, StatusCode};
use ferrite_cache::InvalidationLoop;
use ferrite_core::{PermalinkRules, SiteReader};
use ferrite_domain::{ArtifactHash, ContentId, Role, Timestamp};
use ferrite_http::{AppParts, AppState, HttpConfig, LoggedIn, build_app, load_route_table};
use ferrite_store::Store;
use ferrite_store_tests::CountingStore;

use crate::app_tests;
use crate::common::SITE_URL;
use crate::harness::{
    App, Resp, SITE_TITLE, draft, fixture, load_theme, request, send, with_cookie,
};

fn preview_path(id: ContentId) -> String {
    format!("/_preview/{id}")
}

fn meta_value<'a>(html: &'a str, name: &str) -> Option<&'a str> {
    let start = format!("<meta name=\"{name}\" content=\"");
    let rest = &html[html.find(&start)? + start.len()..];
    rest.split('"').next()
}

/// Headers every preview response carries, the 404 included.
fn assert_private_noindex(r: &Resp) {
    assert_eq!(
        r.header("cache-control"),
        Some("private, no-store"),
        "{r:?}"
    );
    assert_eq!(r.header("x-robots-tag"), Some("noindex"), "{r:?}");
}

/// The shared 404 page.
fn assert_shared_404(r: &Resp, what: &str) {
    assert_eq!(r.status, StatusCode::NOT_FOUND, "{what}: {r:?}");
    assert_eq!(r.tpl(), "404", "{what}");
    assert!(r.body.contains("Nothing here (404)"), "{what}: {}", r.body);
    assert!(
        r.body.contains("<p class=\"path\">[]</p>"),
        "{what}: shared body"
    );
}

async fn preview_renders_draft_for_editor(app: App) -> App {
    let draft = app.draft_post("Secret draft", "secret-draft", &[]).await;
    let editor = app.site.user("eddie", Role::Editor).await;
    let cookie = app.session(&editor).await;
    let before = app.cached().await;
    for who in [cookie.clone(), app.session(&app.ada_user).await] {
        let r = app.get_with_cookie(&preview_path(draft.id), &who).await;
        assert_eq!(r.status, StatusCode::OK, "{r:?}");
        r.assert_html();
        assert_eq!(r.tpl(), "single");
        assert!(r.body.contains("<h1>Secret draft</h1>"), "{}", r.body);
        assert!(r.body.contains("class=\"preview-notice\""), "preview flag");
        assert_private_noindex(&r);
    }
    assert_eq!(app.cached().await, before, "previews are never cached");
    // The draft is still not public.
    let r = app.get("/secret-draft/").await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    // HEAD is answered like GET.
    let mut head = with_cookie(&preview_path(draft.id), &cookie);
    *head.method_mut() = Method::HEAD;
    let r = app.send(head).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_private_noindex(&r);
    // No validator, and `If-None-Match` never yields a 304 (no-store).
    let r = app.get_with_cookie(&preview_path(draft.id), &cookie).await;
    assert_eq!(r.header("etag"), None, "{r:?}");
    let mut conditional = with_cookie(&preview_path(draft.id), &cookie);
    conditional
        .headers_mut()
        .insert("if-none-match", "*".parse().unwrap());
    let r = app.send(conditional).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert!(r.body.contains("<h1>Secret draft</h1>"));
    assert_private_noindex(&r);
    app
}

async fn preview_is_404_without_session_or_rights(app: App) -> App {
    let draft = app.draft_post("Ada's draft", "adas-draft", &[]).await;
    let trashed = app.draft_post("Trashed", "trashed", &[]).await;
    app.svc
        .trash(&app.ada, trashed.id, trashed.version)
        .await
        .expect("trash");
    let contributor = app.site.user("carl", Role::Contributor).await;
    let carl = app.session(&contributor).await;
    let admin = app.site.user("root", Role::Admin).await;
    let root = app.session(&admin).await;
    let mut reset_user = app.site.user("rita", Role::Editor).await;
    let rita = app.session(&reset_user).await;
    reset_user.must_reset = true;
    app.site
        .store
        .read()
        .users()
        .update(&reset_user)
        .await
        .expect("must_reset");

    let anon = app.get(&preview_path(draft.id)).await;
    assert_shared_404(&anon, "anonymous");
    assert_private_noindex(&anon);
    let cases = [
        (
            "contributor, another author's draft",
            preview_path(draft.id),
            carl.clone(),
        ),
        ("trashed item", preview_path(trashed.id), root.clone()),
        ("missing item", preview_path(ContentId::new()), root.clone()),
        ("must_reset session", preview_path(draft.id), rita),
        (
            "unknown session",
            preview_path(draft.id),
            "__Host-ferrite_session=nope".into(),
        ),
        ("malformed id", "/_preview/not-an-id".into(), root.clone()),
        (
            "upper-case id",
            preview_path(draft.id).to_uppercase(),
            root.clone(),
        ),
        (
            "trailing slash",
            format!("{}/", preview_path(draft.id)),
            root.clone(),
        ),
    ];
    for (what, path, cookie) in cases {
        let r = app.get_with_cookie(&path, &cookie).await;
        assert_shared_404(&r, what);
        assert_eq!(r.body, anon.body, "{what}: the same shared body");
    }
    // A bearer token is not accepted for previews: no `Authorization` support at all.
    let mut bearer = request(Method::GET, &preview_path(draft.id));
    bearer
        .headers_mut()
        .insert("authorization", "Bearer fer_x".parse().unwrap());
    assert_shared_404(&app.send(bearer).await, "bearer");
    // A redirect row on the preview path (the service refuses one; written straight to
    // the store here) never shadows it (F27).
    app.site
        .store
        .read()
        .redirects()
        .upsert(&ferrite_domain::Redirect {
            id: ferrite_domain::RedirectId::new(),
            source_path: preview_path(draft.id),
            target: "/elsewhere/".into(),
            code: 301,
            hits: 0,
            origin: ferrite_domain::RedirectOrigin::Manual,
            created_at: Timestamp::now(),
            last_hit_at: None,
        })
        .await
        .expect("redirect row");
    let r = app.get_with_cookie(&preview_path(draft.id), &root).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    app
}

async fn preview_plugins_see_the_item(app: App) -> App {
    app.site.install("reader").await;
    app.site.load().await;
    let draft = app.draft_post("Plugin draft", "plugin-draft", &[]).await;
    let cookie = app.session(&app.ada_user).await;
    let r = app.get_with_cookie(&preview_path(draft.id), &cookie).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert_eq!(
        meta_value(&r.body, "reader-current"),
        Some("Plugin draft"),
        "{}",
        r.body
    );
    app
}

async fn logged_in_bypasses_cache(app: App) -> App {
    app.post("Hello", "hello", &[]).await;
    app.post("Other", "other", &[]).await;
    let cookie = app.session(&app.ada_user).await;

    // Anonymous fills the cache; a logged-in request neither reads nor fills it.
    let r = app.get("/hello/").await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(
        r.header("cache-control"),
        None,
        "anonymous pages: no header"
    );
    let cached = app.cached().await;
    let renders = app.state.renders();
    let r = app.get_with_cookie("/hello/", &cookie).await;
    assert_eq!(r.status, StatusCode::OK, "{r:?}");
    assert!(r.body.contains("<h1>Hello</h1>"));
    assert_eq!(r.header("cache-control"), Some("private, no-store"));
    assert!(r.header("set-cookie").is_none());
    let etag = r.header("etag").unwrap().to_owned();
    assert_eq!(
        app.state.renders(),
        renders + 1,
        "rendered, not the cached page"
    );
    assert_eq!(app.cached().await, cached, "nothing stored");
    let r = app.get("/hello/").await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(app.state.renders(), renders + 1, "anonymous: cache hit");

    // A conditional logged-in request: 304, still `private, no-store`, nothing stored.
    let mut conditional = with_cookie("/hello/", &cookie);
    conditional
        .headers_mut()
        .insert("if-none-match", etag.parse().unwrap());
    let r = app.send(conditional).await;
    assert_eq!(r.status, StatusCode::NOT_MODIFIED, "{r:?}");
    assert_eq!(r.header("cache-control"), Some("private, no-store"));
    assert_eq!(app.cached().await, cached);

    // The other direction: a logged-in render is not stored for anonymous visitors.
    let r = app.get_with_cookie("/other/", &cookie).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(app.cached().await, cached);
    let renders = app.state.renders();
    let r = app.get("/other/").await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(
        app.state.renders(),
        renders + 1,
        "anonymous renders its own page"
    );
    assert_eq!(r.header("cache-control"), None);
    app
}

async fn per_user_variant_caches_per_user(app: App) -> App {
    let post = app.post("Shared", "shared", &[]).await;
    let mut config = HttpConfig::new(SITE_URL, SITE_TITLE, "en");
    config.logged_in = LoggedIn::PerUser;
    let state = App::state_with(&app.site, load_theme(fixture("app-theme")), true, config).await;
    let router = build_app(state.clone());
    let mut poller = InvalidationLoop::for_store(app.site.store.clone(), state.cache.clone())
        .await
        .expect("invalidation loop");
    let editor = app.site.user("eddie", Role::Editor).await;
    let (ada, eddie) = (app.session(&app.ada_user).await, app.session(&editor).await);
    let get = async |cookie: Option<&str>| match cookie {
        Some(c) => send(&router, with_cookie("/shared/", c)).await,
        None => send(&router, request(Method::GET, "/shared/")).await,
    };

    // The anonymous entry exists first; a user under per-user renders their own.
    let renders = state.renders();
    let anon = get(None).await;
    assert_eq!(anon.status, StatusCode::OK);
    assert_eq!(state.cache.len().await, 1);
    for c in [&ada, &eddie] {
        let r = get(Some(c)).await;
        assert_eq!(r.status, StatusCode::OK);
        assert_eq!(r.header("cache-control"), Some("private, no-store"));
    }
    assert_eq!(
        state.renders(),
        renders + 3,
        "one render per user, not the anon page"
    );
    assert_eq!(
        state.cache.len().await,
        3,
        "one entry per user next to anonymous"
    );
    for c in [&ada, &eddie] {
        get(Some(c)).await;
    }
    get(None).await;
    assert_eq!(state.renders(), renders + 3, "per-user and anonymous hits");
    // A conditional request from a logged-in user: 304 still `private, no-store`.
    let etag = get(Some(&ada)).await.header("etag").unwrap().to_owned();
    let mut conditional = with_cookie("/shared/", &ada);
    conditional
        .headers_mut()
        .insert("if-none-match", etag.parse().unwrap());
    let r = send(&router, conditional).await;
    assert_eq!(r.status, StatusCode::NOT_MODIFIED, "{r:?}");
    assert_eq!(r.header("cache-control"), Some("private, no-store"));
    // An invalid cookie is anonymous: the anonymous entry.
    let r = get(Some("__Host-ferrite_session=bogus")).await;
    assert_eq!(r.body, anon.body);
    assert_eq!(state.renders(), renders + 3);
    assert_eq!(r.header("cache-control"), None);

    // An edit invalidates every variant.
    let mut d = draft("post", "Shared v2", "shared", None, &[]);
    d.slug = Some("shared".into());
    app.svc
        .update(&app.ada, post.id, post.version, d)
        .await
        .expect("update");
    poller.poll_once().await.unwrap();
    assert_eq!(state.cache.len().await, 0, "every variant dropped");
    for c in [&ada, &eddie] {
        let r = get(Some(c)).await;
        assert!(r.body.contains("<h1>Shared v2</h1>"), "{}", r.body);
    }
    assert!(get(None).await.body.contains("<h1>Shared v2</h1>"));
    app
}

async fn invalid_cookie_is_anonymous(app: App) -> App {
    app.post("Hello", "hello", &[]).await;
    let r = app.get("/hello/").await;
    assert_eq!(r.status, StatusCode::OK);
    let renders = app.state.renders();
    // Unknown token, empty value, garbage, a logged-out session, a disabled user.
    let gone = app.session(&app.ada_user).await;
    App::auth(&app.site)
        .logout(gone.trim_start_matches("__Host-ferrite_session="))
        .await
        .unwrap();
    let mut disabled = app.site.user("dora", Role::Editor).await;
    let dora = app.session(&disabled).await;
    disabled.status = ferrite_domain::UserStatus::Disabled;
    app.site
        .store
        .read()
        .users()
        .update(&disabled)
        .await
        .unwrap();
    for cookie in [
        "__Host-ferrite_session=bogus".to_owned(),
        "__Host-ferrite_session=".to_owned(),
        format!("__Host-ferrite_session={}", "x".repeat(5000)),
        gone,
        dora,
    ] {
        let r = app.get_with_cookie("/hello/", &cookie).await;
        assert_eq!(r.status, StatusCode::OK, "{cookie:.40}: {r:?}");
        assert_eq!(r.header("cache-control"), None, "anonymous response");
        assert!(r.header("set-cookie").is_none(), "cookie left alone");
        assert!(r.header("www-authenticate").is_none());
    }
    assert_eq!(
        app.state.renders(),
        renders,
        "all served from the anonymous cache"
    );
    app
}

/// Anonymous traffic costs no session lookup: a cached hit without a cookie makes the
/// same store calls as before auth existed (the redirect lookup), and a cookie adds
/// exactly the one session read.
async fn anonymous_requests_skip_the_session_lookup(app: App) -> App {
    app.post("Hello", "hello", &[]).await;
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
    let state = Arc::new(
        AppState::new(AppParts {
            store: store.clone(),
            reader,
            plugins: app.site.host.clone(),
            media: app.site.media.clone(),
            theme: load_theme(
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/app-theme"),
            ),
            cache: Arc::new(ferrite_cache::PageCache::new(64 * 1024 * 1024)),
            routes,
            config: HttpConfig::new(SITE_URL, SITE_TITLE, "en"),
            migrations_ok: true,
        })
        .with_auth(Arc::new(ferrite_core::AuthService::new(
            store,
            ferrite_core::auth::SiteKey::generate().unwrap(),
        ))),
    );
    let router = build_app(state);
    assert_eq!(
        send(&router, request(Method::GET, "/hello/")).await.status,
        StatusCode::OK
    );
    counting.reset();
    send(&router, request(Method::GET, "/hello/")).await;
    let anon = counting.calls();
    counting.reset();
    send(&router, with_cookie("/hello/", "other=1; theme=dark")).await;
    assert_eq!(
        counting.calls(),
        anon,
        "an unrelated cookie is not a session"
    );
    // Junk that cannot be one of our tokens is dropped without a store read.
    for junk in ["bogus", &"x".repeat(44), &format!("{}=", "a".repeat(42))] {
        counting.reset();
        send(
            &router,
            with_cookie("/hello/", &format!("__Host-ferrite_session={junk}")),
        )
        .await;
        assert_eq!(
            counting.calls(),
            anon,
            "junk cookie {junk:?}: no session read"
        );
    }
    // A well-formed unknown token costs exactly the one session read.
    counting.reset();
    let unknown = format!("__Host-ferrite_session={}", "A".repeat(43));
    send(&router, with_cookie("/hello/", &unknown)).await;
    assert_eq!(counting.calls(), anon + 1, "one session read");
    app
}

app_tests!(
    preview_renders_draft_for_editor,
    preview_is_404_without_session_or_rights,
    preview_plugins_see_the_item,
    logged_in_bypasses_cache,
    per_user_variant_caches_per_user,
    invalid_cookie_is_anonymous,
    anonymous_requests_skip_the_session_lookup,
);
