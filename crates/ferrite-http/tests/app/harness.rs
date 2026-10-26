//! The public app over a [`Site`] (either profile), with the fixture theme, and
//! helpers to write content through core services and to send requests in-process.

use std::{path::PathBuf, sync::Arc};

use axum::{
    Router,
    body::Body,
    http::{HeaderMap, Method, Request, StatusCode, header},
};
use ferrite_cache::{InvalidationLoop, PageCache};
use ferrite_core::{
    Actor, AuthService, ContentDraft, ContentService, PermalinkRules, RedirectService,
    TaxonomyService, comments::CommentService,
};
use ferrite_domain::{ContentId, ContentItem, Role, TermId, User};
use ferrite_http::{AppParts, AppState, HttpConfig, ServedBuffer, build_app, load_route_table};
use ferrite_theme::{Theme, ThemeSource};
use http_body_util::BodyExt;
use serde_json::json;
use tower::ServiceExt;

use crate::common::{SITE_URL, Site};

pub const SITE_TITLE: &str = "Test Site";

pub fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

pub fn load_theme(dir: PathBuf) -> Arc<Theme> {
    Arc::new(Theme::load(&ThemeSource::Dir(dir)).expect("theme loads"))
}

pub struct App {
    pub site: Site,
    pub state: Arc<AppState>,
    pub router: Router,
    pub svc: ContentService,
    pub admin: Actor,
    /// Author of every post (login `ada`).
    pub ada: Actor,
    pub ada_user: User,
}

/// A response with its body read.
#[derive(Debug)]
pub struct Resp {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: String,
    /// The body bytes (binary responses).
    pub raw: Vec<u8>,
}

impl Resp {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|v| v.to_str().ok())
    }

    /// The template that rendered the page (`data-tpl` on `<body>`).
    pub fn tpl(&self) -> &str {
        self.body
            .split_once("data-tpl=\"")
            .and_then(|(_, rest)| rest.split_once('"'))
            .map_or("", |(t, _)| t)
    }

    /// An HTML page with the security headers.
    pub fn assert_html(&self) {
        assert_eq!(
            self.header("content-type"),
            Some("text/html; charset=utf-8"),
            "{self:?}"
        );
        self.assert_security_headers();
    }

    pub fn assert_security_headers(&self) {
        assert_eq!(self.header("x-content-type-options"), Some("nosniff"));
        assert_eq!(
            self.header("referrer-policy"),
            Some("strict-origin-when-cross-origin")
        );
        assert_eq!(self.header("x-frame-options"), Some("SAMEORIGIN"));
    }
}

impl App {
    pub async fn new(site: Site) -> Self {
        let theme = load_theme(fixture("app-theme"));
        let admin_user = site.user("admin", Role::Admin).await;
        let ada_user = site.user("ada", Role::Author).await;
        let state = Self::state(&site, theme, true).await;
        let svc = ContentService::new(site.store.clone(), Arc::new(PermalinkRules::default()));
        Self {
            router: build_app(state.clone()),
            state,
            svc,
            admin: Site::actor(&admin_user),
            ada: Site::actor(&ada_user),
            ada_user,
            site,
        }
    }

    /// A fresh state over `site` (empty cache).
    pub async fn state(site: &Site, theme: Arc<Theme>, migrations_ok: bool) -> Arc<AppState> {
        let config = HttpConfig::new(SITE_URL, SITE_TITLE, "en");
        Self::state_with(site, theme, migrations_ok, config).await
    }

    pub async fn state_with(
        site: &Site,
        theme: Arc<Theme>,
        migrations_ok: bool,
        config: HttpConfig,
    ) -> Arc<AppState> {
        Self::state_with_reader(site, site.reader.clone(), theme, migrations_ok, config).await
    }

    pub async fn state_with_reader(
        site: &Site,
        reader: Arc<ferrite_core::SiteReader>,
        theme: Arc<Theme>,
        migrations_ok: bool,
        config: HttpConfig,
    ) -> Arc<AppState> {
        Arc::new(Self::build_state(site, reader, theme, migrations_ok, config).await)
    }

    /// The state [`state_with_reader`](Self::state_with_reader) wraps, for callers that
    /// add more (`with_served`).
    pub async fn build_state(
        site: &Site,
        reader: Arc<ferrite_core::SiteReader>,
        theme: Arc<Theme>,
        migrations_ok: bool,
        config: HttpConfig,
    ) -> AppState {
        let routes = load_route_table(site.store.as_ref(), &PermalinkRules::default())
            .await
            .expect("route table");
        AppState::new(AppParts {
            store: site.store.clone(),
            reader,
            plugins: site.host.clone(),
            media: site.media.clone(),
            theme,
            cache: Arc::new(PageCache::new(64 * 1024 * 1024)),
            routes,
            config,
            migrations_ok,
        })
        .expect("app state")
        .with_auth(Self::auth(site))
        .with_comments(Arc::new(CommentService::new(
            site.store.clone(),
            Self::auth(site),
        )))
    }

    /// This app with a fresh state (empty cache) that records `request.served` events
    /// into the returned buffer.
    pub async fn with_served(mut self) -> (Self, Arc<ServedBuffer>) {
        let served = Arc::new(ServedBuffer::new(
            self.site.store.clone(),
            self.site.host.clone(),
        ));
        let state = Self::build_state(
            &self.site,
            self.site.reader.clone(),
            load_theme(fixture("app-theme")),
            true,
            HttpConfig::new(SITE_URL, SITE_TITLE, "en"),
        )
        .await
        .with_served(served.clone());
        self.state = Arc::new(state);
        self.router = build_app(self.state.clone());
        (self, served)
    }

    /// This app with a fresh state (empty cache) built with `config` and passed
    /// through `f` (e.g. `with_hits`).
    pub async fn rebuilt(
        mut self,
        config: HttpConfig,
        f: impl FnOnce(AppState) -> AppState,
    ) -> Self {
        let state = Self::build_state(
            &self.site,
            self.site.reader.clone(),
            load_theme(fixture("app-theme")),
            true,
            config,
        )
        .await;
        self.state = Arc::new(f(state));
        self.router = build_app(self.state.clone());
        self
    }

    /// This app with a fresh state built with `config` that serves `admin` under
    /// `/admin/` (the `with_admin_assets` seam).
    pub async fn with_admin(
        self,
        config: HttpConfig,
        admin: Arc<dyn ferrite_http::admin::AdminAssets>,
    ) -> Self {
        self.rebuilt(config, |s| s.with_admin_assets(admin)).await
    }

    /// The default config of [`App::new`].
    pub fn config() -> HttpConfig {
        HttpConfig::new(SITE_URL, SITE_TITLE, "en")
    }

    /// An auth service over `site`'s store with the site's key (sessions live in the
    /// database, so any instance resolves the cookies another one issued).
    pub fn auth(site: &Site) -> Arc<AuthService> {
        Arc::new(AuthService::new(site.store.clone(), site.key.clone()))
    }

    /// A `Cookie` header value holding a new session of `user`.
    pub async fn session(&self, user: &User) -> String {
        let issued = Self::auth(&self.site)
            .issue_session(user, None, None)
            .await
            .expect("session");
        format!("__Host-ferrite_session={}", issued.token.expose())
    }

    /// GET `path` with the `Cookie` header `cookie`.
    pub async fn get_with_cookie(&self, path: &str, cookie: &str) -> Resp {
        send(&self.router, with_cookie(path, cookie)).await
    }

    pub async fn teardown(self) {
        let Self {
            site,
            router,
            state,
            ..
        } = self;
        drop(router);
        drop(state);
        site.db.teardown().await;
    }

    pub async fn send(&self, req: Request<Body>) -> Resp {
        send(&self.router, req).await
    }

    pub async fn get(&self, path: &str) -> Resp {
        self.send(request(Method::GET, path)).await
    }

    pub async fn cached(&self) -> u64 {
        self.state.cache.len().await
    }

    pub async fn poller(&self) -> InvalidationLoop {
        InvalidationLoop::for_store(self.site.store.clone(), self.state.cache.clone())
            .await
            .expect("invalidation loop")
    }

    pub fn redirects(&self) -> RedirectService {
        RedirectService::new(self.site.store.clone())
    }

    pub async fn category(&self, slug: &str) -> TermId {
        TaxonomyService::new(self.site.store.clone())
            .create_term(&self.admin, "category", slug, Some(slug.into()), None, "")
            .await
            .expect("term")
            .id
    }

    /// A draft post by ada (not published).
    pub async fn draft_post(&self, title: &str, slug: &str, terms: &[TermId]) -> ContentItem {
        self.svc
            .create(&self.ada, draft("post", title, slug, None, terms))
            .await
            .expect("create post")
    }

    /// A published post by ada.
    pub async fn post(&self, title: &str, slug: &str, terms: &[TermId]) -> ContentItem {
        let it = self.draft_post(title, slug, terms).await;
        self.svc
            .publish(&self.ada, it.id, it.version)
            .await
            .expect("publish post")
    }

    /// A published page (by the admin).
    pub async fn page(&self, title: &str, slug: &str, parent: Option<ContentId>) -> ContentItem {
        let it = self
            .svc
            .create(&self.admin, draft("page", title, slug, parent, &[]))
            .await
            .expect("create page");
        self.svc
            .publish(&self.admin, it.id, it.version)
            .await
            .expect("publish page")
    }
}

pub fn draft(
    kind: &str,
    title: &str,
    slug: &str,
    parent: Option<ContentId>,
    terms: &[TermId],
) -> ContentDraft {
    ContentDraft {
        kind: kind.into(),
        title: title.into(),
        slug: Some(slug.into()),
        parent_id: parent,
        excerpt: None,
        body: json!({"type": "doc", "content": [
            {"type": "paragraph", "content": [{"type": "text", "text": format!("Body of {slug}")}]}
        ]}),
        fields: json!({}),
        menu_order: 0,
        terms: terms.to_vec(),
        seo: None,
    }
}

pub fn request(method: Method, path: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(path)
        .header(header::HOST, "example.com")
        .body(Body::empty())
        .expect("request")
}

/// A GET request for `path` with the `Cookie` header `cookie`.
pub fn with_cookie(path: &str, cookie: &str) -> Request<Body> {
    let mut req = request(Method::GET, path);
    req.headers_mut()
        .insert(header::COOKIE, cookie.parse().expect("cookie header"));
    req
}

pub async fn send(router: &Router, req: Request<Body>) -> Resp {
    let res = router.clone().oneshot(req).await.expect("infallible");
    let status = res.status();
    let headers = res.headers().clone();
    let bytes = res.into_body().collect().await.expect("body").to_bytes();
    Resp {
        status,
        headers,
        body: String::from_utf8_lossy(&bytes).into_owned(),
        raw: bytes.to_vec(),
    }
}

/// Runs each named `async fn(App) -> App` on both profiles (solo: SQLite + local
/// media; cluster: Postgres + in-process S3, skipped without `FERRITE_TEST_PG_URL`).
/// Multi-threaded runtime: the renderer blocks on the store from `spawn_blocking`.
#[macro_export]
macro_rules! app_tests {
    ($($name:ident),* $(,)?) => {$(
        mod $name {
            #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
            async fn solo() {
                let db = ferrite_store_tests::TestDb::sqlite().await;
                let site = $crate::common::Site::new(db, $crate::common::local_media()).await;
                let app = $crate::harness::App::new(site).await;
                super::$name(app).await.teardown().await;
            }

            #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
            async fn cluster() {
                let Some(db) = ferrite_store_tests::TestDb::postgres().await else {
                    return;
                };
                let site =
                    $crate::common::Site::new(db, $crate::common::s3_media().await).await;
                let app = $crate::harness::App::new(site).await;
                super::$name(app).await.teardown().await;
            }
        }
    )*};
}
