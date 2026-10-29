//! A site on one deployment profile: solo (SQLite + local media) or cluster
//! (Postgres + in-process S3), with a plugin host over [`StoreHostServices`].

use std::sync::Arc;

use bytes::Bytes;
use ferrite_core::{Actor, PermalinkRules, SiteReader, auth::SiteKey};
use ferrite_domain::{
    Artifact, ArtifactHash, ArtifactKind, ContentId, ContentStatus, InstalledPlugin, NewContent,
    NewUser, Role, Secret, Timestamp, User, UserId, UserStatus,
};
use ferrite_http::{LoadSummary, PluginReloader, StoreHostServices};
use ferrite_media::{Backend, MediaConfig, MediaStore, PublicUrlMode, S3Config};
use ferrite_plugin_host::{HostConfig, HostServices, KindOk, PluginHost, Post, SettingSpec};
use ferrite_store::Store;
use ferrite_store_tests::TestDb;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto::Builder as ConnBuilder;
use s3s::auth::SimpleAuth;
use s3s::service::S3ServiceBuilder;
use serde_json::json;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

#[path = "../../../ferrite-plugin-host/src/test_support.rs"]
mod test_support;

pub use test_support::plugin_wasm;

pub const SITE_URL: &str = "https://example.com";
pub const HOUR: i64 = 3_600_000_000;

/// The temp dir (and S3 server) behind a site's media store; aborts the server on drop.
pub struct MediaBacking {
    _dir: tempfile::TempDir,
    server: Option<JoinHandle<()>>,
    config: MediaConfig,
    /// Directory where object keys appear as plain files (including the prefix).
    objects_root: std::path::PathBuf,
}

impl Drop for MediaBacking {
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

pub fn local_media() -> MediaBacking {
    let dir = tempfile::tempdir().expect("tempdir");
    let config = MediaConfig {
        backend: Backend::Local {
            root: dir.path().join("media"),
        },
        prefix: String::new(),
        public: public(),
    };
    MediaBacking {
        objects_root: dir.path().join("media"),
        _dir: dir,
        server: None,
        config,
    }
}

pub async fn s3_media() -> MediaBacking {
    const BUCKET: &str = "ferrite-test";
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join(BUCKET)).expect("bucket dir");
    let fs = s3s_fs::FileSystem::new(dir.path()).expect("s3s-fs");
    let mut builder = S3ServiceBuilder::new(fs);
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
            copy_if_not_exists: None,
            conditional_put: None,
        }),
        prefix: "site/".into(),
        public: public(),
    };
    MediaBacking {
        objects_root: dir.path().join(BUCKET).join("site"),
        _dir: dir,
        server: Some(server),
        config,
    }
}

pub struct Site {
    pub db: TestDb,
    _media_backing: MediaBacking,
    pub store: Arc<dyn Store>,
    pub media: Arc<MediaStore>,
    pub reader: Arc<SiteReader>,
    pub services: Arc<StoreHostServices>,
    pub host: Arc<PluginHost>,
    /// The site key the services open sealed settings with.
    pub key: SiteKey,
}

impl Site {
    pub async fn new(db: TestDb, backing: MediaBacking) -> Self {
        let store = db.shared_store();
        let media = Arc::new(MediaStore::open(backing.config.clone()).expect("open media"));
        let reader = Arc::new(SiteReader::new(
            store.clone(),
            Arc::new(PermalinkRules::default()),
            Arc::new(|hash: &ArtifactHash, name: &str| format!("/_media/{hash}/{name}")),
            Arc::new(Timestamp::now),
        ));
        let key = SiteKey::generate().expect("site key");
        let services = Arc::new(
            StoreHostServices::new(store.clone(), reader.clone(), SITE_URL)
                .with_site_key(key.clone()),
        );
        // The production default (200 ms render deadline, R18): stable on both profiles.
        let host =
            Arc::new(PluginHost::new(HostConfig::default(), services.clone()).expect("host"));
        Self {
            db,
            _media_backing: backing,
            store,
            media,
            reader,
            services,
            host,
            key,
        }
    }

    /// Store `wasm` as a plugin artifact (blob + `artifacts` row); returns its hash.
    pub async fn put_artifact(&self, wasm: Vec<u8>) -> ArtifactHash {
        let r = self
            .media
            .put_artifact(Bytes::from(wasm))
            .await
            .expect("put artifact");
        self.store
            .read()
            .artifacts()
            .insert_if_absent(&Artifact {
                hash: r.hash.clone(),
                kind: ArtifactKind::Plugin,
                bytes: i64::try_from(r.bytes).expect("size"),
                mime: "application/wasm".into(),
                created_at: Timestamp::now(),
                orphaned_at: None,
            })
            .await
            .expect("artifact row");
        r.hash
    }

    /// Write the `plugins` row for example plugin `name` with `manifest` (TOML text) and
    /// a null `granted` (manifest honoured, R21).
    pub async fn row(&self, name: &str, manifest: &str, hash: ArtifactHash, enabled: bool) {
        self.row_granted(name, manifest, hash, enabled, serde_json::Value::Null)
            .await;
    }

    pub async fn row_granted(
        &self,
        name: &str,
        manifest: &str,
        hash: ArtifactHash,
        enabled: bool,
        granted: serde_json::Value,
    ) {
        let table: toml::Table = toml::from_str(manifest).expect("manifest toml");
        let now = Timestamp::now();
        self.store
            .read()
            .plugins()
            .upsert(&InstalledPlugin {
                name: name.into(),
                version: "0.1.0".into(),
                artifact_hash: hash,
                manifest: serde_json::to_value(table).expect("manifest json"),
                granted,
                enabled,
                disabled_reason: None,
                installed_at: now,
                updated_at: now,
            })
            .await
            .expect("plugin row");
    }

    /// Install example plugin `name` the way M1c-2 does: artifact + enabled row.
    pub async fn install(&self, name: &str) {
        let hash = self.put_artifact(plugin_wasm(name)).await;
        self.row(name, &plugin_manifest(name), hash, true).await;
    }

    /// Overwrite the stored blob of `hash` (tampering), on either backend.
    pub fn tamper(&self, hash: &ArtifactHash, bytes: &[u8]) {
        let h = hash.as_str();
        let path = self
            ._media_backing
            .objects_root
            .join(format!("artifacts/{}/{h}", &h[..2]));
        assert!(path.is_file(), "no blob at {}", path.display());
        std::fs::write(path, bytes).expect("tamper");
    }

    /// The startup load: the first reload of a fresh reloader, as `serve` does.
    pub async fn load(&self) -> LoadSummary {
        let report = PluginReloader::new(self.store.clone(), self.media.clone(), self.host.clone())
            .reload()
            .await
            .expect("load plugins");
        LoadSummary::from(&report)
    }

    pub async fn user(&self, login: &str, role: Role) -> User {
        let tx = self.store.begin().await.unwrap();
        let u = tx
            .users()
            .insert(&NewUser {
                id: UserId::new(),
                login: login.into(),
                email: format!("{login}@example.com"),
                display_name: format!("{login} display"),
                password_hash: None,
                role,
                status: UserStatus::Active,
                must_reset: false,
            })
            .await
            .unwrap();
        tx.commit().await.unwrap();
        u
    }

    /// A post `slug` titled `title` with `status`, published `published` µs from now.
    pub async fn post(
        &self,
        author: &User,
        slug: &str,
        title: &str,
        status: ContentStatus,
        published: Option<i64>,
    ) -> ContentId {
        let id = ContentId::new();
        let now = Timestamp::now().as_micros();
        let tx = self.store.begin().await.unwrap();
        tx.content()
            .insert(&NewContent {
                id,
                kind: "post".into(),
                status,
                slug: slug.into(),
                parent_id: None,
                author_id: author.id,
                title: title.into(),
                excerpt: format!("Excerpt of {slug}."),
                body: json!({"type": "doc", "content": [
                    {"type": "paragraph", "content": [{"type": "text", "text": format!("Body of {slug}")}]}
                ]}),
                body_html: None,
                fields: json!({}),
                menu_order: 0,
                published_at: published.map(|o| Timestamp::from_micros(now + o).unwrap()),
                scheduled_at: (status == ContentStatus::Scheduled)
                    .then(|| Timestamp::from_micros(now + HOUR).unwrap()),
                    created_at: None,
                    updated_at: None,
            })
            .await
            .unwrap();
        tx.commit().await.unwrap();
        id
    }

    pub fn actor(u: &User) -> Actor {
        Actor::new(u.id, u.role)
    }
}

/// The example plugin's `ferrite-plugin.toml`.
pub fn plugin_manifest(name: &str) -> String {
    let examples = test_support::repo_root().join("plugins/examples");
    let dir = [examples.join(name), examples.join("hostile").join(name)]
        .into_iter()
        .find(|d| d.join("ferrite-plugin.toml").is_file())
        .unwrap_or_else(|| panic!("no ferrite-plugin.toml for plugin {name}"));
    std::fs::read_to_string(dir.join("ferrite-plugin.toml")).expect("read manifest")
}

/// Runs each named `async fn(Site)` on both profiles.
#[macro_export]
macro_rules! profile_tests {
    ($($name:ident),* $(,)?) => {$(
        mod $name {
            #[tokio::test]
            async fn solo() {
                let db = ferrite_store_tests::TestDb::sqlite().await;
                let site = $crate::common::Site::new(db, $crate::common::local_media()).await;
                let site = super::$name(site).await;
                site.db.teardown().await;
            }

            #[tokio::test]
            async fn cluster() {
                let Some(db) = ferrite_store_tests::TestDb::postgres().await else {
                    return;
                };
                let site =
                    $crate::common::Site::new(db, $crate::common::s3_media().await).await;
                let site = super::$name(site).await;
                site.db.teardown().await;
            }
        }
    )*};
}

/// [`StoreHostServices`] whose `get_post` first sleeps `delay` (and counts its callers):
/// makes `slow-import` slow (F34) on a real site.
pub struct SlowPosts {
    inner: Arc<StoreHostServices>,
    delay: std::time::Duration,
    calls: std::sync::atomic::AtomicUsize,
}

impl SlowPosts {
    pub fn new(inner: Arc<StoreHostServices>, delay: std::time::Duration) -> Self {
        Self {
            inner,
            delay,
            calls: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    /// `get_post` calls so far.
    pub fn calls(&self) -> usize {
        self.calls.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// A host over these services with `slow-import` installed under each of `names`
    /// (one component, several manifests).
    pub fn host(self: &Arc<Self>, cfg: HostConfig, names: &[&str]) -> Arc<PluginHost> {
        let host = PluginHost::new(cfg, self.clone()).expect("host");
        for name in names {
            let manifest = plugin_manifest("slow-import")
                .replace("name      = \"slow-import\"", &format!("name = \"{name}\""));
            let manifest = ferrite_plugin_host::Manifest::parse(&manifest).expect("manifest");
            host.install(manifest, &plugin_wasm("slow-import"))
                .expect("install");
        }
        Arc::new(host)
    }
}

#[async_trait::async_trait]
impl HostServices for SlowPosts {
    async fn get_post(&self, id: &str, kind_ok: &KindOk) -> Option<Post> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        tokio::time::sleep(self.delay).await;
        self.inner.get_post(id, kind_ok).await
    }
    async fn post_kind(&self, id: &str) -> Option<String> {
        self.inner.post_kind(id).await
    }
    async fn get_post_by_slug(&self, kind: &str, slug: &str) -> Option<Post> {
        self.inner.get_post_by_slug(kind, slug).await
    }
    async fn list_posts(&self, kind: &str, limit: u32, offset: u32) -> Vec<Post> {
        self.inner.list_posts(kind, limit, offset).await
    }
    async fn meta_get(&self, post: &str, key: &str) -> Option<String> {
        self.inner.meta_get(post, key).await
    }
    async fn meta_set(&self, post: &str, key: &str, value: &str) -> Result<(), String> {
        self.inner.meta_set(post, key, value).await
    }
    async fn option_get(&self, key: &str) -> Option<String> {
        self.inner.option_get(key).await
    }
    async fn kv_get(&self, plugin: &str, key: &str) -> Option<Vec<u8>> {
        self.inner.kv_get(plugin, key).await
    }
    async fn kv_set(&self, plugin: &str, key: &str, value: Vec<u8>) -> Result<(), String> {
        self.inner.kv_set(plugin, key, value).await
    }
    async fn kv_delete(&self, plugin: &str, key: &str) {
        self.inner.kv_delete(plugin, key).await;
    }
    async fn kv_list(&self, plugin: &str, prefix: &str, limit: u32) -> Vec<String> {
        self.inner.kv_list(plugin, prefix, limit).await
    }
    async fn setting_get(&self, plugin: &str, key: &str, spec: &SettingSpec) -> Option<String> {
        self.inner.setting_get(plugin, key, spec).await
    }
    async fn notify_admins(&self, plugin: &str, subject: &str, text: &str) -> Result<(), String> {
        self.inner.notify_admins(plugin, subject, text).await
    }
}
