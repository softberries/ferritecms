//! The whole site (`build_site`: public app + `/api/v1`) on either profile (solo:
//! SQLite + local media; cluster: Postgres + in-process S3), with the fixture theme,
//! sent in-process with a client address (F9).
//!
//! Its own small fixture rather than `tests/plugins/common.rs`: this crate needs none
//! of the plugin helpers, and including them would leave them unused here.

use std::{
    net::SocketAddr,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use axum::{
    Extension, Router,
    body::Body,
    extract::ConnectInfo,
    http::{HeaderMap, Method, Request, StatusCode, header, request},
};
use ferrite_cache::InvalidationLoop;
use ferrite_cache::PageCache;
use ferrite_core::{
    Actor, AuthService, ContentService, ContentTypeService, ExtensionAdmin, MenuService,
    NewAccount, OptionService, PermalinkRules, RedirectService, Scopes, SiteReader,
    TaxonomyService, UserService, auth::SiteKey,
};
use ferrite_domain::{ArtifactHash, Role, Secret, Timestamp, User};
use ferrite_http::{
    AppParts, AppState, HttpConfig, PluginReloader, StoreHostServices,
    api::{ApiParts, ApiState},
    build_site,
    extensions::Installer,
    load_route_table,
};
use ferrite_media::{Backend, MediaConfig, MediaStore, PublicUrlMode, S3Config};
use ferrite_plugin_host::{HostConfig, PluginHost};
use ferrite_store::Store;
use ferrite_store_tests::TestDb;
use ferrite_theme::{Theme, ThemeSource};
use http_body_util::BodyExt;
use hyper_util::{
    rt::{TokioExecutor, TokioIo},
    server::conn::auto::Builder as ConnBuilder,
};
use s3s::{auth::SimpleAuth, service::S3ServiceBuilder};
use serde_json::Value;
use tokio::{net::TcpListener, task::JoinHandle};
use tower::ServiceExt;

#[path = "../../../ferrite-plugin-host/src/test_support.rs"]
mod test_support;

pub use test_support::plugin_wasm;

pub const SITE_URL: &str = "https://example.com";
pub const SITE_TITLE: &str = "Test Site";

/// The temp dir (and S3 server) behind the media store; aborts the server on drop.
pub struct Media {
    _dir: tempfile::TempDir,
    server: Option<JoinHandle<()>>,
    config: MediaConfig,
}

impl Drop for Media {
    fn drop(&mut self) {
        if let Some(server) = self.server.take() {
            server.abort();
        }
    }
}

fn public() -> PublicUrlMode {
    PublicUrlMode::ServeViaApp {
        path_prefix: "/_media".into(),
    }
}

pub fn local_media() -> Media {
    let dir = tempfile::tempdir().expect("tempdir");
    let config = MediaConfig {
        backend: Backend::Local {
            root: dir.path().join("media"),
        },
        prefix: String::new(),
        public: public(),
    };
    Media {
        _dir: dir,
        server: None,
        config,
    }
}

pub async fn s3_media() -> Media {
    const BUCKET: &str = "ferrite-test";
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join(BUCKET)).expect("bucket dir");
    let mut builder = S3ServiceBuilder::new(s3s_fs::FileSystem::new(dir.path()).expect("s3s-fs"));
    builder.set_auth(SimpleAuth::from_single("test-access", "test-secret"));
    let service = builder.build();
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let server = tokio::spawn(async move {
        while let Ok((socket, _)) = listener.accept().await {
            let service = service.clone();
            tokio::spawn(async move {
                let http = ConnBuilder::new(TokioExecutor::new());
                let _ = http.serve_connection(TokioIo::new(socket), service).await;
            });
        }
    });
    let config = MediaConfig {
        backend: Backend::S3(S3Config {
            bucket: BUCKET.into(),
            region: "us-east-1".into(),
            endpoint: Some(format!("http://{addr}")),
            access_key_id: Some("test-access".into()),
            secret_access_key: Some(Secret::new("test-secret")),
            allow_http: true,
            virtual_hosted_style: false,
        }),
        prefix: "site/".into(),
        public: public(),
    };
    Media {
        _dir: dir,
        server: Some(server),
        config,
    }
}

/// The address every harness request comes from.
pub const CLIENT: SocketAddr = SocketAddr::new(
    std::net::IpAddr::V4(std::net::Ipv4Addr::new(203, 0, 113, 7)),
    40_000,
);

pub struct Api {
    pub db: TestDb,
    _media: Media,
    pub store: Arc<dyn Store>,
    pub app: Arc<AppState>,
    pub api: Arc<ApiState>,
    pub router: Router,
    /// Component compiles the API's installer has started (inspections and installs).
    pub compiles: Arc<AtomicUsize>,
    /// Reloads [`Api::app`]'s plugin host from the rows, as a node's reload task does
    /// on `plugins.changed` (pre-flight F18: the harness has no reload task).
    pub reloader: PluginReloader,
}

/// A response with its body read.
#[derive(Debug)]
pub struct Resp {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: String,
}

impl Resp {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|v| v.to_str().ok())
    }

    pub fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or_else(|e| panic!("not JSON ({e}): {self:?}"))
    }

    /// A problem document with `status` and type `/api/v1/problems/<slug>`, sent with
    /// `Cache-Control: no-store`; returns the body.
    pub fn assert_problem(&self, status: u16, slug: &str) -> Value {
        assert_eq!(self.status.as_u16(), status, "{self:?}");
        assert_eq!(
            self.header("content-type"),
            Some("application/problem+json"),
            "{self:?}"
        );
        assert_eq!(self.header("cache-control"), Some("no-store"), "{self:?}");
        let body = self.json();
        assert_eq!(body["type"], format!("/api/v1/problems/{slug}"), "{body}");
        assert_eq!(body["status"], status, "{body}");
        assert!(body["title"].is_string(), "{body}");
        assert!(body["detail"].is_string(), "{body}");
        body
    }
}

impl Api {
    pub async fn new(db: TestDb, media: Media) -> Self {
        let store = db.shared_store();
        let media_store = Arc::new(MediaStore::open(media.config.clone()).expect("open media"));
        let reader = Arc::new(SiteReader::new(
            store.clone(),
            Arc::new(PermalinkRules::default()),
            Arc::new(|hash: &ArtifactHash, name: &str| format!("/_media/{hash}/{name}")),
            Arc::new(Timestamp::now),
        ));
        let services = Arc::new(StoreHostServices::new(
            store.clone(),
            reader.clone(),
            SITE_URL,
        ));
        let host = Arc::new(PluginHost::new(HostConfig::default(), services).expect("host"));
        let reloader = PluginReloader::new(store.clone(), media_store.clone(), host.clone());
        let compiles = Arc::new(AtomicUsize::new(0));
        let counter = compiles.clone();
        let installer = Arc::new(
            Installer::new(store.clone(), media_store.clone(), HostConfig::default())
                .expect("installer")
                .with_prepare(move |host, manifest, wasm, approved| {
                    counter.fetch_add(1, Ordering::SeqCst);
                    host.prepare_with(manifest, wasm, approved)
                }),
        );
        let theme = Arc::new(
            Theme::load(&ThemeSource::Dir(
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/app-theme"),
            ))
            .expect("theme loads"),
        );
        let routes = load_route_table(store.as_ref(), &PermalinkRules::default())
            .await
            .expect("route table");
        let app = Arc::new(AppState::new(AppParts {
            store: store.clone(),
            reader,
            plugins: host,
            media: media_store.clone(),
            theme,
            cache: Arc::new(PageCache::new(64 * 1024 * 1024)),
            routes,
            config: HttpConfig::new(SITE_URL, SITE_TITLE, "en"),
            migrations_ok: true,
        }));
        let api = Arc::new(
            ApiState::new(ApiParts {
                store: store.clone(),
                auth: Arc::new(
                    AuthService::new(store.clone(), SiteKey::generate().expect("site key"))
                        .with_issuer_name(SITE_TITLE.into()),
                ),
                users: Arc::new(UserService::new(store.clone())),
                content: Arc::new(ContentService::new(
                    store.clone(),
                    Arc::new(PermalinkRules::default()),
                )),
                types: Arc::new(ContentTypeService::new(store.clone())),
                taxonomy: Arc::new(TaxonomyService::new(store.clone())),
                menus: Arc::new(MenuService::new(store.clone())),
                options: Arc::new(OptionService::new(store.clone())),
                redirects: Arc::new(RedirectService::new(store.clone())),
                extensions: Arc::new(ExtensionAdmin::new(store.clone())),
                installer,
                media: media_store,
                site_url: SITE_URL.into(),
            })
            .expect("api state"),
        );
        let router = with_client(build_site(app.clone(), api.clone()));
        Self {
            db,
            _media: media,
            store,
            app,
            api,
            router,
            compiles,
            reloader,
        }
    }

    pub async fn teardown(self) {
        let Self {
            db,
            _media,
            store,
            app,
            api,
            router,
            compiles: _,
            reloader,
        } = self;
        drop((router, reloader, app, api, store, _media));
        db.teardown().await;
    }

    pub async fn send(&self, req: Request<Body>) -> Resp {
        send(&self.router, req).await
    }

    pub async fn get(&self, path: &str) -> Resp {
        self.send(request(Method::GET, path)).await
    }

    /// The whole site again, with the same store and services but another `site.url`
    /// for the API (origin and the cookie's `Secure` follow it).
    pub fn router_for_site(&self, site_url: &str) -> Router {
        let a = &self.api;
        let api = Arc::new(
            ApiState::new(ApiParts {
                store: a.store.clone(),
                auth: a.auth.clone(),
                users: a.users.clone(),
                content: a.content.clone(),
                types: a.types.clone(),
                taxonomy: a.taxonomy.clone(),
                menus: a.menus.clone(),
                options: a.options.clone(),
                redirects: a.redirects.clone(),
                extensions: a.extensions.clone(),
                installer: a.installer.clone(),
                media: a.media.clone(),
                site_url: site_url.into(),
            })
            .expect("api state"),
        );
        with_client(build_site(self.app.clone(), api))
    }

    /// The API of another node on `store` (a [`TestDb::peer`]): fresh core services and
    /// site key over that store, the same `site.url`. The public side stays this node's;
    /// tests use it for API requests only.
    pub async fn router_on(&self, store: Arc<dyn Store>) -> Router {
        let api = Arc::new(
            ApiState::new(ApiParts {
                store: store.clone(),
                auth: Arc::new(
                    AuthService::new(store.clone(), SiteKey::generate().expect("site key"))
                        .with_issuer_name(SITE_TITLE.into()),
                ),
                users: Arc::new(UserService::new(store.clone())),
                content: Arc::new(ContentService::new(
                    store.clone(),
                    Arc::new(PermalinkRules::default()),
                )),
                types: Arc::new(ContentTypeService::new(store.clone())),
                taxonomy: Arc::new(TaxonomyService::new(store.clone())),
                menus: Arc::new(MenuService::new(store.clone())),
                options: Arc::new(OptionService::new(store.clone())),
                redirects: Arc::new(RedirectService::new(store.clone())),
                extensions: Arc::new(ExtensionAdmin::new(store.clone())),
                installer: Arc::new(
                    Installer::new(store.clone(), self.api.media.clone(), HostConfig::default())
                        .expect("installer"),
                ),
                media: self.api.media.clone(),
                site_url: SITE_URL.into(),
            })
            .expect("api state"),
        );
        with_client(build_site(self.app.clone(), api))
    }

    /// Waits for the next rate-limit window (database clock) unless at least `need` of
    /// the current one is left (F10).
    pub async fn align_to_window(&self, need: Duration) {
        let w = i64::try_from(WINDOW.as_micros()).unwrap();
        let now = self
            .store
            .read()
            .nodes()
            .db_now()
            .await
            .unwrap()
            .as_micros();
        let left = w - now.rem_euclid(w);
        if left < i64::try_from(need.as_micros()).unwrap() {
            tokio::time::sleep(
                Duration::from_micros(left.unsigned_abs()) + Duration::from_millis(50),
            )
            .await;
        }
    }
}

/// The password of every account [`Api::user`] creates.
pub const PASSWORD: &str = "correct horse battery";

impl Api {
    /// An active account with [`PASSWORD`] (created through the operator path).
    pub async fn user(&self, login: &str, role: Role, must_reset: bool) -> User {
        self.api
            .users
            .operator()
            .create(NewAccount {
                login: login.into(),
                email: format!("{login}@example.com"),
                display_name: login.into(),
                role,
                password: Some(PASSWORD.into()),
                must_reset,
            })
            .await
            .unwrap()
    }

    /// A browser client logged in as `login` (with [`PASSWORD`]).
    pub async fn logged_in(&self, login: &str) -> Client {
        let mut c = Client::new();
        let r = c.login(self, login, PASSWORD).await;
        assert_eq!(r.status, StatusCode::OK, "{r:?}");
        assert!(c.cookie.is_some() && c.csrf.is_some(), "{r:?}");
        c
    }

    /// An API token of `user` holding `scopes` (`["*"]`: every scope).
    pub async fn bearer(&self, user: &User, scopes: &[&str]) -> Bearer {
        let scopes = if scopes == ["*"] {
            Scopes::ALL
        } else {
            let names: Vec<String> = scopes.iter().map(|s| (*s).to_owned()).collect();
            Scopes::from_strs(&names).unwrap()
        };
        let (token, _) = self
            .api
            .auth
            .create_token(
                &Actor::new(user.id, user.role),
                user.id,
                "test",
                scopes,
                None,
            )
            .await
            .unwrap();
        Bearer(token.expose().to_owned())
    }

    /// An invalidation loop over the page cache of [`Api::app`], positioned at the end
    /// of the outbox (call `poll_once` to apply what was written since).
    pub async fn poller(&self) -> InvalidationLoop {
        InvalidationLoop::for_store(self.store.clone(), self.app.cache.clone())
            .await
            .expect("invalidation loop")
    }
}

/// An API client authenticating with a bearer token (no `Origin`, no CSRF token).
pub struct Bearer(pub String);

impl Bearer {
    /// A request carrying the token.
    pub fn builder(&self, method: Method, path: &str) -> request::Builder {
        builder(method, path).header(header::AUTHORIZATION, format!("Bearer {}", self.0))
    }

    pub async fn get(&self, api: &Api, path: &str) -> Resp {
        api.send(self.builder(Method::GET, path).body(Body::empty()).unwrap())
            .await
    }

    /// `method path` with an optional JSON body and `If-Match: "<if_match>"`.
    pub async fn send(
        &self,
        api: &Api,
        method: Method,
        path: &str,
        body: Option<&Value>,
        if_match: Option<i64>,
    ) -> Resp {
        let mut b = self.builder(method, path);
        if let Some(v) = if_match {
            b = b.header(header::IF_MATCH, format!("\"{v}\""));
        }
        let req = match body {
            Some(body) => json_body(b, body),
            None => b.body(Body::empty()).unwrap(),
        };
        api.send(req).await
    }
}

/// `router` with every request coming from [`CLIENT`]. The `ConnectInfo` extension is
/// inserted directly: `record_client` reads that extension, and axum's
/// `MockConnectInfo` would only feed the `ConnectInfo` extractor (F9).
fn with_client(router: Router) -> Router {
    router.layer(Extension(ConnectInfo(CLIENT)))
}

/// The rate-limit window of every auth limit.
pub const WINDOW: Duration = Duration::from_secs(60);

/// A request builder with the harness `Host`.
pub fn builder(method: Method, path: &str) -> request::Builder {
    Request::builder()
        .method(method)
        .uri(path)
        .header(header::HOST, "example.com")
}

/// `b` with a JSON body.
pub fn json_body(b: request::Builder, body: &Value) -> Request<Body> {
    b.header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .expect("request")
}

/// A browser-like API client: sends `Origin`, keeps the session cookie from
/// `Set-Cookie` and the CSRF token from login and `me` responses, and sends the token on
/// every request but GET/HEAD.
pub struct Client {
    pub origin: String,
    pub cookie: Option<String>,
    pub csrf: Option<String>,
}

impl Default for Client {
    fn default() -> Self {
        Self::new()
    }
}

impl Client {
    /// A client of the default site (`Origin` = [`SITE_URL`]).
    pub fn new() -> Self {
        Self::with_origin(SITE_URL)
    }

    pub fn with_origin(origin: &str) -> Self {
        Self {
            origin: origin.to_owned(),
            cookie: None,
            csrf: None,
        }
    }

    /// A request carrying the client's `Origin`, cookie and (for unsafe methods) CSRF
    /// token.
    pub fn builder(&self, method: Method, path: &str) -> request::Builder {
        let unsafe_method = method != Method::GET && method != Method::HEAD;
        let mut b = builder(method, path).header(header::ORIGIN, &self.origin);
        if let Some(c) = &self.cookie {
            b = b.header(header::COOKIE, format!("ferrite_session={c}"));
        }
        if let (true, Some(t)) = (unsafe_method, &self.csrf) {
            b = b.header("x-csrf-token", t);
        }
        b
    }

    /// Takes the cookie from `Set-Cookie` (dropped on `Max-Age=0`; a new cookie drops
    /// the old CSRF token) and the CSRF token from a JSON body's `csrf_token`.
    pub fn absorb(&mut self, r: &Resp) {
        if let Some(set) = r.header("set-cookie") {
            let value = set
                .split(';')
                .next()
                .and_then(|kv| kv.strip_prefix("ferrite_session="))
                .expect("session cookie")
                .to_owned();
            self.csrf = None;
            self.cookie = if set.contains("Max-Age=0") {
                None
            } else {
                Some(value)
            };
        }
        if r.header("content-type") == Some("application/json")
            && let Some(t) = r.json()["csrf_token"].as_str()
        {
            self.csrf = Some(t.to_owned());
        }
    }

    pub async fn send_on(&mut self, router: &Router, req: Request<Body>) -> Resp {
        let r = send(router, req).await;
        self.absorb(&r);
        r
    }

    pub async fn get(&mut self, api: &Api, path: &str) -> Resp {
        let req = self.builder(Method::GET, path).body(Body::empty()).unwrap();
        self.send_on(&api.router, req).await
    }

    /// POST without a body.
    pub async fn post_empty(&mut self, api: &Api, path: &str) -> Resp {
        let req = self
            .builder(Method::POST, path)
            .body(Body::empty())
            .unwrap();
        self.send_on(&api.router, req).await
    }

    pub async fn post(&mut self, api: &Api, path: &str, body: Value) -> Resp {
        let req = json_body(self.builder(Method::POST, path), &body);
        self.send_on(&api.router, req).await
    }

    /// `method path` with an optional JSON body and `If-Match: "<if_match>"`.
    pub async fn send(
        &mut self,
        api: &Api,
        method: Method,
        path: &str,
        body: Option<&Value>,
        if_match: Option<i64>,
    ) -> Resp {
        let mut b = self.builder(method, path);
        if let Some(v) = if_match {
            b = b.header(header::IF_MATCH, format!("\"{v}\""));
        }
        let req = match body {
            Some(body) => json_body(b, body),
            None => b.body(Body::empty()).unwrap(),
        };
        self.send_on(&api.router, req).await
    }

    /// `POST path` with a `multipart/form-data` body of `parts` (name, bytes), in order.
    pub async fn multipart(&mut self, api: &Api, path: &str, parts: &[(&str, &[u8])]) -> Resp {
        let (content_type, body) = multipart_body(parts);
        let req = self
            .builder(Method::POST, path)
            .header(header::CONTENT_TYPE, content_type)
            .body(Body::from(body))
            .unwrap();
        self.send_on(&api.router, req).await
    }

    /// `POST /api/v1/auth/login`.
    pub async fn login(&mut self, api: &Api, login: &str, password: &str) -> Resp {
        self.post(
            api,
            "/api/v1/auth/login",
            serde_json::json!({"login": login, "password": password}),
        )
        .await
    }
}

/// The boundary [`multipart_body`] uses (never inside the test payloads).
pub const BOUNDARY: &str = "ferrite-test-boundary-7d1f";

/// A `multipart/form-data` body of `parts` (name, bytes) and its `Content-Type`.
pub fn multipart_body(parts: &[(&str, &[u8])]) -> (String, Vec<u8>) {
    let mut body = Vec::new();
    for (name, bytes) in parts {
        body.extend_from_slice(
            format!(
                "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"{name}\"; \
                 filename=\"{name}\"\r\nContent-Type: application/octet-stream\r\n\r\n"
            )
            .as_bytes(),
        );
        body.extend_from_slice(bytes);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{BOUNDARY}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={BOUNDARY}"), body)
}

/// The `ferrite-plugin.toml` of the example plugin `name` (`plugins/examples/<name>`,
/// or `…/hostile/<name>`).
pub fn plugin_manifest(name: &str) -> String {
    let examples = test_support::repo_root().join("plugins/examples");
    let dir = [examples.join(name), examples.join("hostile").join(name)]
        .into_iter()
        .find(|d| d.join("ferrite-plugin.toml").is_file())
        .unwrap_or_else(|| panic!("no ferrite-plugin.toml for plugin {name}"));
    std::fs::read_to_string(dir.join("ferrite-plugin.toml")).expect("read manifest")
}

pub fn request(method: Method, path: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(path)
        .header(header::HOST, "example.com")
        .body(Body::empty())
        .expect("request")
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
    }
}

/// Runs each named `async fn(Api) -> Api` on both profiles (solo: SQLite + local
/// media; cluster: Postgres + in-process S3, skipped without `FERRITE_TEST_PG_URL`).
#[macro_export]
macro_rules! api_tests {
    ($($name:ident),* $(,)?) => {$(
        mod $name {
            #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
            async fn solo() {
                let db = ferrite_store_tests::TestDb::sqlite().await;
                let api = $crate::harness::Api::new(db, $crate::harness::local_media()).await;
                super::$name(api).await.teardown().await;
            }

            #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
            async fn cluster() {
                let Some(db) = ferrite_store_tests::TestDb::postgres().await else {
                    return;
                };
                let api =
                    $crate::harness::Api::new(db, $crate::harness::s3_media().await).await;
                super::$name(api).await.teardown().await;
            }
        }
    )*};
}
