//! The whole site (`build_site`: public app + `/api/v1`) on either profile (solo:
//! SQLite + local media; cluster: Postgres + in-process S3), with the fixture theme,
//! sent in-process with a client address (F9).
//!
//! Its own small fixture rather than `tests/plugins/common.rs`: this crate needs none
//! of the plugin helpers, and including them would leave them unused here.

use std::{net::SocketAddr, path::PathBuf, sync::Arc};

use axum::{
    Router,
    body::Body,
    extract::connect_info::MockConnectInfo,
    http::{HeaderMap, Method, Request, StatusCode, header},
};
use ferrite_cache::PageCache;
use ferrite_core::{
    AuthService, ContentService, MenuService, OptionService, PermalinkRules, RedirectService,
    SiteReader, TaxonomyService, UserService, auth::SiteKey,
};
use ferrite_domain::{ArtifactHash, Secret, Timestamp};
use ferrite_http::{
    AppParts, AppState, HttpConfig, StoreHostServices,
    api::{ApiParts, ApiState},
    build_site, load_route_table,
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
            media: media_store,
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
                taxonomy: Arc::new(TaxonomyService::new(store.clone())),
                menus: Arc::new(MenuService::new(store.clone())),
                options: Arc::new(OptionService::load(store.clone()).await.expect("options")),
                redirects: Arc::new(RedirectService::new(store.clone())),
                site_url: SITE_URL.into(),
            })
            .expect("api state"),
        );
        let router = build_site(app.clone(), api.clone()).layer(MockConnectInfo(CLIENT));
        Self {
            db,
            _media: media,
            store,
            app,
            api,
            router,
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
        } = self;
        drop((router, app, api, store, _media));
        db.teardown().await;
    }

    pub async fn send(&self, req: Request<Body>) -> Resp {
        send(&self.router, req).await
    }

    pub async fn get(&self, path: &str) -> Resp {
        self.send(request(Method::GET, path)).await
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
