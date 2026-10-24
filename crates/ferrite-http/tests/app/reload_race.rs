//! Task 9 review Minor 3: a route-table swap and its flush (a node reload) that land
//! between the handler's first route lookup and the render must not leave a page built
//! from the old table in the cache.
//!
//! Deterministic: a store wrapper runs a hook inside the handler's redirect lookup, the
//! one await between the route lookup and the cache. The hook swaps in the new table and
//! flushes, exactly as `ferrite::reload` does. No production seam is needed.

use std::sync::{
    Arc, Weak,
    atomic::{AtomicBool, Ordering},
};

use async_trait::async_trait;
use axum::http::StatusCode;
use ferrite_cache::PageCache;
use ferrite_core::PermalinkRules;
use ferrite_domain::{ContentType, Page, Redirect, RedirectId, Timestamp};
use ferrite_http::{AppParts, AppState, HttpConfig, RouteTable, build_app, load_route_table};
use ferrite_store::{
    ApiTokenRepo, ArtifactRepo, AuditRepo, Backend, Channel, CommentRepo, ContentRepo,
    ContentTypeRepo, EventRepo, IdempotencyRepo, JobRepo, LockGuard, MediaRepo, MenuRepo, MetaRepo,
    MigrationStatus, NodeRepo, OptionRepo, PluginFailureRepo, PluginKvRepo, PluginRepo,
    RateLimitRepo, RedirectRepo, Repos, RevisionRepo, SchemaMetaRepo, SearchRepo, SessionRepo,
    Store, StoreError, Subscription, TaxonomyRepo, TermRepo, ThemeRepo, Tx, UserRepo,
};
use serde_json::json;

use crate::{
    app_tests,
    common::SITE_URL,
    harness::{App, SITE_TITLE, load_theme, send},
};

app_tests!(route_swap_during_lookup_is_not_cached);

/// Runs once, on the first redirect lookup: swaps `table` in and flushes the cache.
struct Hook {
    state: std::sync::OnceLock<Weak<AppState>>,
    table: std::sync::Mutex<Option<RouteTable>>,
    fired: AtomicBool,
}

impl Hook {
    fn fire(&self) {
        if self.fired.swap(true, Ordering::SeqCst) {
            return;
        }
        let state = self.state.get().and_then(Weak::upgrade).expect("state");
        let table = self.table.lock().unwrap().take().expect("table");
        state.swap_routes(table);
        state.cache.flush();
    }
}

/// `inner` with a [`Hook`] in `redirects().get_by_source`.
struct HookStore {
    inner: Arc<dyn Store>,
    repos: HookRepos,
}

struct HookRepos {
    inner: Arc<dyn Store>,
    redirects: HookRedirects,
}

struct HookRedirects {
    inner: Arc<dyn Store>,
    hook: Arc<Hook>,
}

#[async_trait]
impl RedirectRepo for HookRedirects {
    async fn get(&self, id: RedirectId) -> Result<Option<Redirect>, StoreError> {
        self.inner.read().redirects().get(id).await
    }
    async fn get_by_source(&self, source: &str) -> Result<Option<Redirect>, StoreError> {
        self.hook.fire();
        self.inner.read().redirects().get_by_source(source).await
    }
    async fn upsert(&self, redirect: &Redirect) -> Result<Redirect, StoreError> {
        self.inner.read().redirects().upsert(redirect).await
    }
    async fn delete(&self, id: RedirectId) -> Result<(), StoreError> {
        self.inner.read().redirects().delete(id).await
    }
    async fn list(&self, limit: u32, cursor: Option<&str>) -> Result<Page<Redirect>, StoreError> {
        self.inner.read().redirects().list(limit, cursor).await
    }
    async fn list_by_target(&self, target: &str) -> Result<Vec<Redirect>, StoreError> {
        self.inner.read().redirects().list_by_target(target).await
    }
    async fn hit(&self, id: RedirectId) -> Result<(), StoreError> {
        self.inner.read().redirects().hit(id).await
    }
}

impl Repos for HookRepos {
    fn content(&self) -> &dyn ContentRepo {
        self.inner.read().content()
    }
    fn meta(&self) -> &dyn MetaRepo {
        self.inner.read().meta()
    }
    fn revisions(&self) -> &dyn RevisionRepo {
        self.inner.read().revisions()
    }
    fn types(&self) -> &dyn ContentTypeRepo {
        self.inner.read().types()
    }
    fn taxonomies(&self) -> &dyn TaxonomyRepo {
        self.inner.read().taxonomies()
    }
    fn terms(&self) -> &dyn TermRepo {
        self.inner.read().terms()
    }
    fn menus(&self) -> &dyn MenuRepo {
        self.inner.read().menus()
    }
    fn options(&self) -> &dyn OptionRepo {
        self.inner.read().options()
    }
    fn redirects(&self) -> &dyn RedirectRepo {
        &self.redirects
    }
    fn users(&self) -> &dyn UserRepo {
        self.inner.read().users()
    }
    fn artifacts(&self) -> &dyn ArtifactRepo {
        self.inner.read().artifacts()
    }
    fn media(&self) -> &dyn MediaRepo {
        self.inner.read().media()
    }
    fn plugins(&self) -> &dyn PluginRepo {
        self.inner.read().plugins()
    }
    fn plugin_failures(&self) -> &dyn PluginFailureRepo {
        self.inner.read().plugin_failures()
    }
    fn plugin_kv(&self) -> &dyn PluginKvRepo {
        self.inner.read().plugin_kv()
    }
    fn themes(&self) -> &dyn ThemeRepo {
        self.inner.read().themes()
    }
    fn events(&self) -> &dyn EventRepo {
        self.inner.read().events()
    }
    fn jobs(&self) -> &dyn JobRepo {
        self.inner.read().jobs()
    }
    fn nodes(&self) -> &dyn NodeRepo {
        self.inner.read().nodes()
    }
    fn search(&self) -> &dyn SearchRepo {
        self.inner.read().search()
    }
    fn schema_meta(&self) -> &dyn SchemaMetaRepo {
        self.inner.read().schema_meta()
    }
    fn sessions(&self) -> &dyn SessionRepo {
        self.inner.read().sessions()
    }
    fn api_tokens(&self) -> &dyn ApiTokenRepo {
        self.inner.read().api_tokens()
    }
    fn rate_limits(&self) -> &dyn RateLimitRepo {
        self.inner.read().rate_limits()
    }
    fn audit(&self) -> &dyn AuditRepo {
        self.inner.read().audit()
    }
    fn comments(&self) -> &dyn CommentRepo {
        self.inner.read().comments()
    }
    fn idempotency(&self) -> &dyn IdempotencyRepo {
        self.inner.read().idempotency()
    }
}

#[async_trait]
impl Store for HookStore {
    fn backend(&self) -> Backend {
        self.inner.backend()
    }
    fn read(&self) -> &dyn Repos {
        &self.repos
    }
    async fn begin(&self) -> Result<Box<dyn Tx>, StoreError> {
        self.inner.begin().await
    }
    async fn subscribe(
        &self,
        channel: Channel,
    ) -> Result<Option<Box<dyn Subscription>>, StoreError> {
        self.inner.subscribe(channel).await
    }
    async fn try_lock(&self, name: &str) -> Result<Option<Box<dyn LockGuard>>, StoreError> {
        self.inner.try_lock(name).await
    }
    async fn migration_status(
        &self,
        binary: &semver::Version,
    ) -> Result<MigrationStatus, StoreError> {
        self.inner.migration_status(binary).await
    }
    async fn migrate_up(&self, to: Option<i64>) -> Result<Vec<i64>, StoreError> {
        self.inner.migrate_up(to).await
    }
    async fn migrate_down(&self, to: i64, allow_lossy: bool) -> Result<Vec<i64>, StoreError> {
        self.inner.migrate_down(to, allow_lossy).await
    }
    async fn verify_migrations(&self) -> Result<(), StoreError> {
        self.inner.verify_migrations().await
    }
    async fn snapshot(&self, dest: &std::path::Path) -> Result<(), StoreError> {
        self.inner.snapshot(dest).await
    }
    async fn close(&self) {}
}

async fn route_swap_during_lookup_is_not_cached(app: App) -> App {
    let store = app.site.store.clone();
    let rules = PermalinkRules::default();
    // The table after the reload: no `event` type yet.
    let without = load_route_table(store.as_ref(), &rules).await.unwrap();
    store
        .read()
        .types()
        .upsert(&ContentType {
            name: "event".into(),
            label: "Events".into(),
            hierarchical: false,
            has_archive: true,
            fields: json!({}),
            built_in: false,
            created_at: Timestamp::now(),
        })
        .await
        .unwrap();
    // The table before it: `/event/` is an archive.
    let with = load_route_table(store.as_ref(), &rules).await.unwrap();

    let hook = Arc::new(Hook {
        state: std::sync::OnceLock::new(),
        table: std::sync::Mutex::new(Some(without)),
        fired: AtomicBool::new(false),
    });
    let hooked: Arc<dyn Store> = Arc::new(HookStore {
        inner: store.clone(),
        repos: HookRepos {
            inner: store.clone(),
            redirects: HookRedirects {
                inner: store.clone(),
                hook: hook.clone(),
            },
        },
    });
    let state = Arc::new(AppState::new(AppParts {
        store: hooked,
        reader: app.site.reader.clone(),
        plugins: app.site.host.clone(),
        media: app.site.media.clone(),
        theme: load_theme(crate::harness::fixture("app-theme")),
        cache: Arc::new(PageCache::new(64 * 1024 * 1024)),
        routes: with,
        config: HttpConfig::new(SITE_URL, SITE_TITLE, "en"),
        migrations_ok: true,
    }));
    hook.state.set(Arc::downgrade(&state)).unwrap();
    let router = build_app(state.clone());

    // The handler resolves `/event/` as the archive, then the reload lands in its
    // redirect lookup. The render re-resolves inside the cache's flush guard and finds
    // no route: 404, and nothing stale is stored.
    let first = send(
        &router,
        crate::harness::request(axum::http::Method::GET, "/event/"),
    )
    .await;
    assert!(hook.fired.load(Ordering::SeqCst), "the hook ran");
    assert_eq!(first.status, StatusCode::NOT_FOUND, "{first:?}");
    let again = send(
        &router,
        crate::harness::request(axum::http::Method::GET, "/event/"),
    )
    .await;
    assert_eq!(
        again.status,
        StatusCode::NOT_FOUND,
        "no stale archive cached"
    );
    drop(router);
    drop(state);
    app
}
