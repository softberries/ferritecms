//! A `Store` wrapper that counts every repository call made through `read()`.
//!
//! The repositories `SiteReader` reads from are wrapped method by method (each method
//! call counts once). The others are counted per accessor call, which can only
//! over-count. `begin()` counts as one call; calls inside the transaction do not count
//! there, except the content-type reads: [`CountingStore::type_lists`] counts
//! `types().list()` and [`CountingStore::type_reads`] `types().get()` and `list()`,
//! through `read()` and every transaction (nested ones too).

use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use async_trait::async_trait;
use ferrite_domain::{
    ContentId, ContentItem, ContentQuery, ContentType, Media, NewContent, NewUser, OptionEntry,
    Page, SearchQuery, Secret, Taxonomy, Term, TermId, Timestamp, User, UserId, UserQuery,
};
use ferrite_store::{
    ApiTokenRepo, ArtifactRepo, AuditRepo, AutosaveRepo, Backend, Channel, CommentRepo,
    ContentRepo, ContentTypeRepo, EventRepo, IdempotencyRepo, ImportRepo, JobRepo, LockGuard,
    MediaRepo, MenuRepo, MetaRepo, MigrationStatus, NodeRepo, OptionRepo, PluginFailureRepo,
    PluginKvRepo, PluginRepo, RateLimitRepo, RedirectRepo, Repos, RevisionRepo, SchemaMetaRepo,
    SearchRepo, SessionRepo, Store, StoreError, Subscription, TaxonomyRepo, TermRepo, ThemeRepo,
    Tx, UserRepo,
};
use serde_json::Value;

#[derive(Clone)]
struct Inner {
    store: Arc<dyn Store>,
    calls: Arc<AtomicUsize>,
    types: TypeCounters,
}

/// Content-type read counters, shared by the store and its transactions.
#[derive(Clone, Default)]
struct TypeCounters {
    lists: Arc<AtomicUsize>,
    reads: Arc<AtomicUsize>,
}

impl TypeCounters {
    fn list(&self) {
        self.lists.fetch_add(1, Ordering::SeqCst);
        self.reads.fetch_add(1, Ordering::SeqCst);
    }

    fn get(&self) {
        self.reads.fetch_add(1, Ordering::SeqCst);
    }
}

impl Inner {
    fn read(&self) -> &dyn Repos {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.store.read()
    }
}

pub struct CountingStore {
    inner: Inner,
    repos: CountingRepos,
}

impl CountingStore {
    pub fn new(store: Arc<dyn Store>) -> Self {
        let inner = Inner {
            store,
            calls: Arc::new(AtomicUsize::new(0)),
            types: TypeCounters::default(),
        };
        Self {
            repos: CountingRepos {
                content: Counted(inner.clone()),
                inner: inner.clone(),
            },
            inner,
        }
    }

    pub fn calls(&self) -> usize {
        self.inner.calls.load(Ordering::SeqCst)
    }

    /// `ContentTypeRepo::list` calls, through `read()` and inside transactions.
    pub fn type_lists(&self) -> usize {
        self.inner.types.lists.load(Ordering::SeqCst)
    }

    /// `ContentTypeRepo::get` and `list` calls, through `read()` and inside
    /// transactions.
    pub fn type_reads(&self) -> usize {
        self.inner.types.reads.load(Ordering::SeqCst)
    }

    pub fn reset(&self) {
        self.inner.calls.store(0, Ordering::SeqCst);
        self.inner.types.lists.store(0, Ordering::SeqCst);
        self.inner.types.reads.store(0, Ordering::SeqCst);
    }
}

/// One wrapper type implements every counted repository trait.
struct Counted(Inner);

struct CountingRepos {
    inner: Inner,
    content: Counted,
}

impl Repos for CountingRepos {
    fn content(&self) -> &dyn ContentRepo {
        &self.content
    }
    fn meta(&self) -> &dyn MetaRepo {
        &self.content
    }
    fn revisions(&self) -> &dyn RevisionRepo {
        self.inner.read().revisions()
    }
    fn types(&self) -> &dyn ContentTypeRepo {
        &self.content
    }
    fn taxonomies(&self) -> &dyn TaxonomyRepo {
        &self.content
    }
    fn terms(&self) -> &dyn TermRepo {
        &self.content
    }
    fn menus(&self) -> &dyn MenuRepo {
        self.inner.read().menus()
    }
    fn options(&self) -> &dyn OptionRepo {
        &self.content
    }
    fn redirects(&self) -> &dyn RedirectRepo {
        self.inner.read().redirects()
    }
    fn users(&self) -> &dyn UserRepo {
        &self.content
    }
    fn artifacts(&self) -> &dyn ArtifactRepo {
        self.inner.read().artifacts()
    }
    fn media(&self) -> &dyn MediaRepo {
        &self.content
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
        &self.content
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
    fn autosaves(&self) -> &dyn AutosaveRepo {
        self.inner.read().autosaves()
    }
    fn imports(&self) -> &dyn ImportRepo {
        self.inner.read().imports()
    }
}

#[async_trait]
impl Store for CountingStore {
    fn backend(&self) -> Backend {
        self.inner.store.backend()
    }
    fn read(&self) -> &dyn Repos {
        &self.repos
    }
    async fn begin(&self) -> Result<Box<dyn Tx>, StoreError> {
        self.inner.calls.fetch_add(1, Ordering::SeqCst);
        let tx = self.inner.store.begin().await?;
        Ok(Box::new(CountingTx {
            tx,
            types: self.inner.types.clone(),
        }))
    }
    async fn subscribe(
        &self,
        channel: Channel,
    ) -> Result<Option<Box<dyn Subscription>>, StoreError> {
        self.inner.store.subscribe(channel).await
    }
    async fn try_lock(&self, name: &str) -> Result<Option<Box<dyn LockGuard>>, StoreError> {
        self.inner.store.try_lock(name).await
    }
    async fn migration_status(
        &self,
        binary: &semver::Version,
    ) -> Result<MigrationStatus, StoreError> {
        self.inner.store.migration_status(binary).await
    }
    async fn migrate_up(&self, to: Option<i64>) -> Result<Vec<i64>, StoreError> {
        self.inner.store.migrate_up(to).await
    }
    async fn migrate_down(&self, to: i64, allow_lossy: bool) -> Result<Vec<i64>, StoreError> {
        self.inner.store.migrate_down(to, allow_lossy).await
    }
    async fn verify_migrations(&self) -> Result<(), StoreError> {
        self.inner.store.verify_migrations().await
    }
    async fn snapshot(&self, dest: &std::path::Path) -> Result<(), StoreError> {
        self.inner.store.snapshot(dest).await
    }
    async fn close(&self) {
        self.inner.store.close().await;
    }
}

#[async_trait]
impl ContentRepo for Counted {
    async fn insert(&self, item: &NewContent) -> Result<ContentItem, StoreError> {
        self.0.read().content().insert(item).await
    }
    async fn get(&self, id: ContentId) -> Result<Option<ContentItem>, StoreError> {
        self.0.read().content().get(id).await
    }
    async fn get_by_slug(
        &self,
        kind: &str,
        parent: Option<ContentId>,
        slug: &str,
    ) -> Result<Option<ContentItem>, StoreError> {
        self.0
            .read()
            .content()
            .get_by_slug(kind, parent, slug)
            .await
    }
    async fn update(&self, item: &ContentItem) -> Result<ContentItem, StoreError> {
        self.0.read().content().update(item).await
    }
    async fn set_updated_at(&self, id: ContentId, at: Timestamp) -> Result<(), StoreError> {
        self.0.read().content().set_updated_at(id, at).await
    }
    async fn list(&self, query: &ContentQuery) -> Result<Page<ContentItem>, StoreError> {
        self.0.read().content().list(query).await
    }
    async fn count(&self, query: &ContentQuery) -> Result<u64, StoreError> {
        self.0.read().content().count(query).await
    }
    async fn get_many(&self, ids: &[ContentId]) -> Result<Vec<ContentItem>, StoreError> {
        self.0.read().content().get_many(ids).await
    }
    async fn slugs_with_prefix(
        &self,
        kind: &str,
        parent: Option<ContentId>,
        prefix: &str,
    ) -> Result<Vec<String>, StoreError> {
        self.0
            .read()
            .content()
            .slugs_with_prefix(kind, parent, prefix)
            .await
    }
    async fn delete(&self, id: ContentId) -> Result<(), StoreError> {
        self.0.read().content().delete(id).await
    }
    async fn reassign_author(&self, from: UserId, to: UserId) -> Result<u64, StoreError> {
        self.0.read().content().reassign_author(from, to).await
    }
    async fn due_scheduled(
        &self,
        now: Timestamp,
        limit: u32,
    ) -> Result<Vec<ContentItem>, StoreError> {
        self.0.read().content().due_scheduled(now, limit).await
    }
}

#[async_trait]
impl MetaRepo for Counted {
    async fn get(&self, id: ContentId, key: &str) -> Result<Option<Value>, StoreError> {
        self.0.read().meta().get(id, key).await
    }
    async fn set(&self, id: ContentId, key: &str, value: &Value) -> Result<(), StoreError> {
        self.0.read().meta().set(id, key, value).await
    }
    async fn delete(&self, id: ContentId, key: &str) -> Result<(), StoreError> {
        self.0.read().meta().delete(id, key).await
    }
    async fn list(&self, id: ContentId) -> Result<Vec<(String, Value)>, StoreError> {
        self.0.read().meta().list(id).await
    }
    async fn get_many(
        &self,
        ids: &[ContentId],
        keys: &[&str],
    ) -> Result<Vec<(ContentId, String, Value)>, StoreError> {
        self.0.read().meta().get_many(ids, keys).await
    }
}

#[async_trait]
impl ContentTypeRepo for Counted {
    async fn upsert(&self, ty: &ContentType) -> Result<(), StoreError> {
        self.0.read().types().upsert(ty).await
    }
    async fn insert(&self, ty: &ContentType) -> Result<(), StoreError> {
        self.0.read().types().insert(ty).await
    }
    async fn get(&self, name: &str) -> Result<Option<ContentType>, StoreError> {
        self.0.types.get();
        self.0.read().types().get(name).await
    }
    async fn list(&self) -> Result<Vec<ContentType>, StoreError> {
        self.0.types.list();
        self.0.read().types().list().await
    }
    async fn delete(&self, name: &str) -> Result<(), StoreError> {
        self.0.read().types().delete(name).await
    }
}

#[async_trait]
impl TaxonomyRepo for Counted {
    async fn upsert(&self, taxonomy: &Taxonomy) -> Result<(), StoreError> {
        self.0.read().taxonomies().upsert(taxonomy).await
    }
    async fn insert(&self, taxonomy: &Taxonomy) -> Result<(), StoreError> {
        self.0.read().taxonomies().insert(taxonomy).await
    }
    async fn get(&self, name: &str) -> Result<Option<Taxonomy>, StoreError> {
        self.0.read().taxonomies().get(name).await
    }
    async fn list(&self) -> Result<Vec<Taxonomy>, StoreError> {
        self.0.read().taxonomies().list().await
    }
    async fn delete(&self, name: &str) -> Result<(), StoreError> {
        self.0.read().taxonomies().delete(name).await
    }
}

#[async_trait]
impl TermRepo for Counted {
    async fn insert(&self, term: &Term) -> Result<(), StoreError> {
        self.0.read().terms().insert(term).await
    }
    async fn update(&self, term: &Term) -> Result<(), StoreError> {
        self.0.read().terms().update(term).await
    }
    async fn get(&self, id: TermId) -> Result<Option<Term>, StoreError> {
        self.0.read().terms().get(id).await
    }
    async fn get_by_slug(&self, tax: &str, slug: &str) -> Result<Option<Term>, StoreError> {
        self.0.read().terms().get_by_slug(tax, slug).await
    }
    async fn list(&self, tax: &str) -> Result<Vec<Term>, StoreError> {
        self.0.read().terms().list(tax).await
    }
    async fn delete(&self, id: TermId) -> Result<(), StoreError> {
        self.0.read().terms().delete(id).await
    }
    async fn set_for_item(&self, item: ContentId, terms: &[TermId]) -> Result<(), StoreError> {
        self.0.read().terms().set_for_item(item, terms).await
    }
    async fn for_item(&self, item: ContentId) -> Result<Vec<Term>, StoreError> {
        self.0.read().terms().for_item(item).await
    }
    async fn for_items(
        &self,
        items: &[ContentId],
    ) -> Result<HashMap<ContentId, Vec<Term>>, StoreError> {
        self.0.read().terms().for_items(items).await
    }
}

#[async_trait]
impl OptionRepo for Counted {
    async fn get(&self, name: &str) -> Result<Option<OptionEntry>, StoreError> {
        self.0.read().options().get(name).await
    }
    async fn set(&self, name: &str, value: &Value, autoload: bool) -> Result<(), StoreError> {
        self.0.read().options().set(name, value, autoload).await
    }
    async fn delete(&self, name: &str) -> Result<(), StoreError> {
        self.0.read().options().delete(name).await
    }
    async fn autoloaded(&self) -> Result<Vec<OptionEntry>, StoreError> {
        self.0.read().options().autoloaded().await
    }
    async fn list_prefix(
        &self,
        prefix: &str,
        after: &str,
        limit: u32,
    ) -> Result<Vec<OptionEntry>, StoreError> {
        self.0
            .read()
            .options()
            .list_prefix(prefix, after, limit)
            .await
    }
}

#[async_trait]
impl UserRepo for Counted {
    async fn insert(&self, user: &NewUser) -> Result<User, StoreError> {
        self.0.read().users().insert(user).await
    }
    async fn get(&self, id: UserId) -> Result<Option<User>, StoreError> {
        self.0.read().users().get(id).await
    }
    async fn get_by_login(&self, login: &str) -> Result<Option<User>, StoreError> {
        self.0.read().users().get_by_login(login).await
    }
    async fn get_by_email(&self, email: &str) -> Result<Option<User>, StoreError> {
        self.0.read().users().get_by_email(email).await
    }
    async fn get_for_update(&self, id: UserId) -> Result<Option<User>, StoreError> {
        self.0.read().users().get_for_update(id).await
    }
    async fn list_active_admins(&self, limit: u32) -> Result<Vec<User>, StoreError> {
        self.0.read().users().list_active_admins(limit).await
    }
    async fn update(&self, user: &User) -> Result<(), StoreError> {
        self.0.read().users().update(user).await
    }
    async fn set_totp(&self, id: UserId, secret: Option<&Secret>) -> Result<(), StoreError> {
        self.0.read().users().set_totp(id, secret).await
    }
    async fn advance_totp_step(&self, id: UserId, step: i64) -> Result<bool, StoreError> {
        self.0.read().users().advance_totp_step(id, step).await
    }
    async fn delete(&self, id: UserId) -> Result<(), StoreError> {
        self.0.read().users().delete(id).await
    }
    async fn count_active_admins(&self) -> Result<u64, StoreError> {
        self.0.read().users().count_active_admins().await
    }
    async fn list(&self, query: &UserQuery) -> Result<Page<User>, StoreError> {
        self.0.read().users().list(query).await
    }
    async fn get_many(&self, ids: &[UserId]) -> Result<Vec<User>, StoreError> {
        self.0.read().users().get_many(ids).await
    }
}

#[async_trait]
impl MediaRepo for Counted {
    async fn insert(&self, media: &Media) -> Result<(), StoreError> {
        self.0.read().media().insert(media).await
    }
    async fn get(&self, id: ContentId) -> Result<Option<Media>, StoreError> {
        self.0.read().media().get(id).await
    }
    async fn get_for_update(&self, id: ContentId) -> Result<Option<Media>, StoreError> {
        self.0.read().media().get_for_update(id).await
    }
    async fn update(&self, media: &Media) -> Result<(), StoreError> {
        self.0.read().media().update(media).await
    }
    async fn get_many(&self, ids: &[ContentId]) -> Result<Vec<Media>, StoreError> {
        self.0.read().media().get_many(ids).await
    }
    async fn find_by_artifact(
        &self,
        hash: &ferrite_domain::ArtifactHash,
        limit: u32,
    ) -> Result<Vec<Media>, StoreError> {
        self.0.read().media().find_by_artifact(hash, limit).await
    }
}

#[async_trait]
impl SearchRepo for Counted {
    async fn index(&self, id: ContentId, title: &str, body_text: &str) -> Result<(), StoreError> {
        self.0.read().search().index(id, title, body_text).await
    }
    async fn remove(&self, id: ContentId) -> Result<(), StoreError> {
        self.0.read().search().remove(id).await
    }
    async fn search(&self, query: &SearchQuery) -> Result<Page<ContentId>, StoreError> {
        self.0.read().search().search(query).await
    }
}

/// A transaction that passes every call through, counting the content-type reads.
struct CountingTx {
    tx: Box<dyn Tx>,
    types: TypeCounters,
}

impl Repos for CountingTx {
    fn content(&self) -> &dyn ContentRepo {
        self.tx.content()
    }
    fn meta(&self) -> &dyn MetaRepo {
        self.tx.meta()
    }
    fn revisions(&self) -> &dyn RevisionRepo {
        self.tx.revisions()
    }
    fn taxonomies(&self) -> &dyn TaxonomyRepo {
        self.tx.taxonomies()
    }
    fn terms(&self) -> &dyn TermRepo {
        self.tx.terms()
    }
    fn menus(&self) -> &dyn MenuRepo {
        self.tx.menus()
    }
    fn options(&self) -> &dyn OptionRepo {
        self.tx.options()
    }
    fn redirects(&self) -> &dyn RedirectRepo {
        self.tx.redirects()
    }
    fn users(&self) -> &dyn UserRepo {
        self.tx.users()
    }
    fn artifacts(&self) -> &dyn ArtifactRepo {
        self.tx.artifacts()
    }
    fn media(&self) -> &dyn MediaRepo {
        self.tx.media()
    }
    fn plugins(&self) -> &dyn PluginRepo {
        self.tx.plugins()
    }
    fn plugin_failures(&self) -> &dyn PluginFailureRepo {
        self.tx.plugin_failures()
    }
    fn plugin_kv(&self) -> &dyn PluginKvRepo {
        self.tx.plugin_kv()
    }
    fn themes(&self) -> &dyn ThemeRepo {
        self.tx.themes()
    }
    fn events(&self) -> &dyn EventRepo {
        self.tx.events()
    }
    fn jobs(&self) -> &dyn JobRepo {
        self.tx.jobs()
    }
    fn nodes(&self) -> &dyn NodeRepo {
        self.tx.nodes()
    }
    fn search(&self) -> &dyn SearchRepo {
        self.tx.search()
    }
    fn schema_meta(&self) -> &dyn SchemaMetaRepo {
        self.tx.schema_meta()
    }
    fn sessions(&self) -> &dyn SessionRepo {
        self.tx.sessions()
    }
    fn api_tokens(&self) -> &dyn ApiTokenRepo {
        self.tx.api_tokens()
    }
    fn rate_limits(&self) -> &dyn RateLimitRepo {
        self.tx.rate_limits()
    }
    fn audit(&self) -> &dyn AuditRepo {
        self.tx.audit()
    }
    fn comments(&self) -> &dyn CommentRepo {
        self.tx.comments()
    }
    fn idempotency(&self) -> &dyn IdempotencyRepo {
        self.tx.idempotency()
    }
    fn autosaves(&self) -> &dyn AutosaveRepo {
        self.tx.autosaves()
    }
    fn imports(&self) -> &dyn ImportRepo {
        self.tx.imports()
    }
    fn types(&self) -> &dyn ContentTypeRepo {
        self
    }
}

#[async_trait]
impl Tx for CountingTx {
    async fn commit(self: Box<Self>) -> Result<(), StoreError> {
        self.tx.commit().await
    }
    async fn rollback(self: Box<Self>) -> Result<(), StoreError> {
        self.tx.rollback().await
    }
    async fn begin_nested(&self) -> Result<Box<dyn Tx>, StoreError> {
        let tx = self.tx.begin_nested().await?;
        Ok(Box::new(CountingTx {
            tx,
            types: self.types.clone(),
        }))
    }
}

#[async_trait]
impl ContentTypeRepo for CountingTx {
    async fn upsert(&self, ty: &ContentType) -> Result<(), StoreError> {
        self.tx.types().upsert(ty).await
    }
    async fn insert(&self, ty: &ContentType) -> Result<(), StoreError> {
        self.tx.types().insert(ty).await
    }
    async fn get(&self, name: &str) -> Result<Option<ContentType>, StoreError> {
        self.types.get();
        self.tx.types().get(name).await
    }
    async fn list(&self) -> Result<Vec<ContentType>, StoreError> {
        self.types.list();
        self.tx.types().list().await
    }
    async fn delete(&self, name: &str) -> Result<(), StoreError> {
        self.tx.types().delete(name).await
    }
}
