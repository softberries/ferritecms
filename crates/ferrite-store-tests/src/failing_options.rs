//! A `Store` wrapper whose `read().options().get(name)` fails for one option name
//! (`StoreError::Database`), for the paths that must survive an unreadable option
//! (M4-4 P77: the site title in core mail). Every other call, and everything inside a
//! transaction, goes to the wrapped store.

use std::sync::Arc;

use async_trait::async_trait;
use ferrite_domain::OptionEntry;
use ferrite_store::{
    ApiTokenRepo, ArtifactRepo, AuditRepo, AutosaveRepo, Backend, Channel, CommentRepo,
    ContentRepo, ContentTypeRepo, EventRepo, IdempotencyRepo, ImportRepo, JobRepo, LockGuard,
    MediaRepo, MenuRepo, MetaRepo, MigrationStatus, NodeMetricsRepo, NodeRepo, OAuthRepo,
    OptionRepo, PluginFailureRepo, PluginKvRepo, PluginRepo, RateLimitRepo, RedirectRepo, Repos,
    RevisionRepo, SchemaMetaRepo, SearchRepo, SessionRepo, Store, StoreError, Subscription,
    TaxonomyRepo, TermRepo, ThemeRepo, Tx, UserRepo,
};
use serde_json::Value;

/// See the module docs.
pub struct FailingOptionsStore {
    repos: FailingRepos,
}

struct FailingRepos {
    store: Arc<dyn Store>,
    options: FailingOptions,
}

struct FailingOptions {
    store: Arc<dyn Store>,
    name: String,
}

impl FailingOptionsStore {
    /// `store`, with `read().options().get(name)` failing.
    pub fn new(store: Arc<dyn Store>, name: &str) -> Self {
        Self {
            repos: FailingRepos {
                store: store.clone(),
                options: FailingOptions {
                    store,
                    name: name.to_owned(),
                },
            },
        }
    }

    fn inner(&self) -> &dyn Store {
        self.repos.store.as_ref()
    }
}

#[async_trait]
impl OptionRepo for FailingOptions {
    async fn get(&self, name: &str) -> Result<Option<OptionEntry>, StoreError> {
        if name == self.name {
            return Err(StoreError::Database(format!(
                "injected read failure for {name}"
            )));
        }
        self.store.read().options().get(name).await
    }
    async fn set(&self, name: &str, value: &Value, autoload: bool) -> Result<(), StoreError> {
        self.store.read().options().set(name, value, autoload).await
    }
    async fn delete(&self, name: &str) -> Result<(), StoreError> {
        self.store.read().options().delete(name).await
    }
    async fn autoloaded(&self) -> Result<Vec<OptionEntry>, StoreError> {
        self.store.read().options().autoloaded().await
    }
    async fn list_prefix(
        &self,
        prefix: &str,
        after: &str,
        limit: u32,
    ) -> Result<Vec<OptionEntry>, StoreError> {
        self.store
            .read()
            .options()
            .list_prefix(prefix, after, limit)
            .await
    }
}

impl Repos for FailingRepos {
    fn content(&self) -> &dyn ContentRepo {
        self.store.read().content()
    }
    fn meta(&self) -> &dyn MetaRepo {
        self.store.read().meta()
    }
    fn revisions(&self) -> &dyn RevisionRepo {
        self.store.read().revisions()
    }
    fn types(&self) -> &dyn ContentTypeRepo {
        self.store.read().types()
    }
    fn taxonomies(&self) -> &dyn TaxonomyRepo {
        self.store.read().taxonomies()
    }
    fn terms(&self) -> &dyn TermRepo {
        self.store.read().terms()
    }
    fn menus(&self) -> &dyn MenuRepo {
        self.store.read().menus()
    }
    fn options(&self) -> &dyn OptionRepo {
        &self.options
    }
    fn redirects(&self) -> &dyn RedirectRepo {
        self.store.read().redirects()
    }
    fn users(&self) -> &dyn UserRepo {
        self.store.read().users()
    }
    fn artifacts(&self) -> &dyn ArtifactRepo {
        self.store.read().artifacts()
    }
    fn media(&self) -> &dyn MediaRepo {
        self.store.read().media()
    }
    fn plugins(&self) -> &dyn PluginRepo {
        self.store.read().plugins()
    }
    fn plugin_failures(&self) -> &dyn PluginFailureRepo {
        self.store.read().plugin_failures()
    }
    fn plugin_kv(&self) -> &dyn PluginKvRepo {
        self.store.read().plugin_kv()
    }
    fn themes(&self) -> &dyn ThemeRepo {
        self.store.read().themes()
    }
    fn events(&self) -> &dyn EventRepo {
        self.store.read().events()
    }
    fn jobs(&self) -> &dyn JobRepo {
        self.store.read().jobs()
    }
    fn nodes(&self) -> &dyn NodeRepo {
        self.store.read().nodes()
    }
    fn search(&self) -> &dyn SearchRepo {
        self.store.read().search()
    }
    fn schema_meta(&self) -> &dyn SchemaMetaRepo {
        self.store.read().schema_meta()
    }
    fn sessions(&self) -> &dyn SessionRepo {
        self.store.read().sessions()
    }
    fn api_tokens(&self) -> &dyn ApiTokenRepo {
        self.store.read().api_tokens()
    }
    fn rate_limits(&self) -> &dyn RateLimitRepo {
        self.store.read().rate_limits()
    }
    fn audit(&self) -> &dyn AuditRepo {
        self.store.read().audit()
    }
    fn comments(&self) -> &dyn CommentRepo {
        self.store.read().comments()
    }
    fn idempotency(&self) -> &dyn IdempotencyRepo {
        self.store.read().idempotency()
    }
    fn autosaves(&self) -> &dyn AutosaveRepo {
        self.store.read().autosaves()
    }
    fn imports(&self) -> &dyn ImportRepo {
        self.store.read().imports()
    }
    fn node_metrics(&self) -> &dyn NodeMetricsRepo {
        self.store.read().node_metrics()
    }
    fn oauth(&self) -> &dyn OAuthRepo {
        self.store.read().oauth()
    }
}

#[async_trait]
impl Store for FailingOptionsStore {
    fn backend(&self) -> Backend {
        self.inner().backend()
    }
    fn pool_stats(&self) -> ferrite_store::PoolStats {
        self.inner().pool_stats()
    }
    fn read(&self) -> &dyn Repos {
        &self.repos
    }
    async fn begin(&self) -> Result<Box<dyn Tx>, StoreError> {
        self.inner().begin().await
    }
    async fn subscribe(
        &self,
        channel: Channel,
    ) -> Result<Option<Box<dyn Subscription>>, StoreError> {
        self.inner().subscribe(channel).await
    }
    async fn try_lock(&self, name: &str) -> Result<Option<Box<dyn LockGuard>>, StoreError> {
        self.inner().try_lock(name).await
    }
    async fn lock_wait(
        &self,
        name: &str,
        wait: std::time::Duration,
    ) -> Result<Option<Box<dyn LockGuard>>, StoreError> {
        self.inner().lock_wait(name, wait).await
    }
    async fn migration_status(
        &self,
        binary: &semver::Version,
    ) -> Result<MigrationStatus, StoreError> {
        self.inner().migration_status(binary).await
    }
    async fn migrate_up(
        &self,
        to: Option<i64>,
    ) -> Result<ferrite_store::MigrateReport, StoreError> {
        self.inner().migrate_up(to).await
    }
    async fn migrate_down(&self, to: i64, allow_lossy: bool) -> Result<Vec<i64>, StoreError> {
        self.inner().migrate_down(to, allow_lossy).await
    }
    async fn verify_migrations(&self) -> Result<(), StoreError> {
        self.inner().verify_migrations().await
    }
    async fn export_reader(&self) -> Result<Box<dyn ferrite_store::ExportReader>, StoreError> {
        self.inner().export_reader().await
    }
    async fn import_writer(&self) -> Result<Box<dyn ferrite_store::ImportWriter>, StoreError> {
        self.inner().import_writer().await
    }
    async fn snapshot(&self, dest: &std::path::Path) -> Result<(), StoreError> {
        self.inner().snapshot(dest).await
    }
    async fn close(&self) {
        self.inner().close().await;
    }
}
