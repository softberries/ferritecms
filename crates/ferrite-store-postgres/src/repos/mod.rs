//! Repository implementations, all on [`PgRepos`].

mod auth;
mod autosave;
mod comment;
mod content;
mod event;
mod idempotency;
mod import;
mod job;
mod menu;
mod node;
mod node_metrics;
mod oauth;
mod plugin;
mod plugin_failure;

pub use comment::comment_list_sql;
pub use idempotency::PRUNE_IDEMPOTENCY_SQL;
pub use plugin::FIND_MEDIA_BY_ARTIFACT_SQL;
mod search;
mod site;
mod taxonomy;
mod user;
mod webhook;

use std::sync::Arc;

use async_trait::async_trait;
use ferrite_store::{
    ApiTokenRepo, ArtifactRepo, AuditRepo, AutosaveRepo, CommentRepo, ContentRepo, ContentTypeRepo,
    EventRepo, IdempotencyRepo, ImportRepo, JobRepo, MediaRepo, MenuRepo, MetaRepo,
    NodeMetricsRepo, NodeRepo, OAuthRepo, OptionRepo, PluginFailureRepo, PluginKvRepo, PluginRepo,
    RateLimitRepo, RedirectRepo, Repos, RevisionRepo, SchemaMetaRepo, SearchRepo, SessionRepo,
    StoreError, TaxonomyRepo, TermRepo, ThemeRepo, Tx, UserRepo, WebhookRepo,
};
use sqlx::PgPool;

use crate::{
    exec::{Exec, TxHandle},
    listen::Channels,
};

/// Every repository, bound to one executor (the pool or a write transaction).
pub(crate) struct PgRepos {
    exec: Exec,
    /// Where `events.append` and `jobs.enqueue` notify (P2).
    channels: Arc<Channels>,
}

impl PgRepos {
    pub(crate) fn new(pool: PgPool, channels: Arc<Channels>) -> Self {
        Self {
            exec: Exec::Pool(pool),
            channels,
        }
    }
}

impl Repos for PgRepos {
    fn content(&self) -> &dyn ContentRepo {
        self
    }
    fn meta(&self) -> &dyn MetaRepo {
        self
    }
    fn revisions(&self) -> &dyn RevisionRepo {
        self
    }
    fn types(&self) -> &dyn ContentTypeRepo {
        self
    }
    fn taxonomies(&self) -> &dyn TaxonomyRepo {
        self
    }
    fn terms(&self) -> &dyn TermRepo {
        self
    }
    fn menus(&self) -> &dyn MenuRepo {
        self
    }
    fn options(&self) -> &dyn OptionRepo {
        self
    }
    fn redirects(&self) -> &dyn RedirectRepo {
        self
    }
    fn users(&self) -> &dyn UserRepo {
        self
    }
    fn artifacts(&self) -> &dyn ArtifactRepo {
        self
    }
    fn media(&self) -> &dyn MediaRepo {
        self
    }
    fn plugins(&self) -> &dyn PluginRepo {
        self
    }
    fn plugin_failures(&self) -> &dyn PluginFailureRepo {
        self
    }
    fn plugin_kv(&self) -> &dyn PluginKvRepo {
        self
    }
    fn themes(&self) -> &dyn ThemeRepo {
        self
    }
    fn events(&self) -> &dyn EventRepo {
        self
    }
    fn jobs(&self) -> &dyn JobRepo {
        self
    }
    fn nodes(&self) -> &dyn NodeRepo {
        self
    }
    fn search(&self) -> &dyn SearchRepo {
        self
    }
    fn schema_meta(&self) -> &dyn SchemaMetaRepo {
        self
    }
    fn sessions(&self) -> &dyn SessionRepo {
        self
    }
    fn api_tokens(&self) -> &dyn ApiTokenRepo {
        self
    }
    fn rate_limits(&self) -> &dyn RateLimitRepo {
        self
    }
    fn audit(&self) -> &dyn AuditRepo {
        self
    }
    fn comments(&self) -> &dyn CommentRepo {
        self
    }
    fn idempotency(&self) -> &dyn IdempotencyRepo {
        self
    }
    fn autosaves(&self) -> &dyn AutosaveRepo {
        self
    }
    fn imports(&self) -> &dyn ImportRepo {
        self
    }
    fn node_metrics(&self) -> &dyn NodeMetricsRepo {
        self
    }
    fn oauth(&self) -> &dyn OAuthRepo {
        self
    }
    fn webhooks(&self) -> &dyn WebhookRepo {
        self
    }
}

/// A write transaction (or a nested savepoint) on a pool connection; dropping it rolls
/// back.
pub(crate) struct PgTx {
    handle: TxHandle,
    repos: PgRepos,
}

impl PgTx {
    pub(crate) fn new(handle: TxHandle, channels: Arc<Channels>) -> Self {
        Self {
            repos: PgRepos {
                exec: Exec::Tx(handle.clone()),
                channels,
            },
            handle,
        }
    }
}

impl Drop for PgTx {
    fn drop(&mut self) {
        self.handle.abandon();
    }
}

#[async_trait]
impl Tx for PgTx {
    async fn commit(self: Box<Self>) -> Result<(), StoreError> {
        self.handle.finish(true).await
    }

    async fn rollback(self: Box<Self>) -> Result<(), StoreError> {
        self.handle.finish(false).await
    }

    async fn begin_nested(&self) -> Result<Box<dyn Tx>, StoreError> {
        Ok(Box::new(PgTx::new(
            self.handle.nested().await?,
            self.repos.channels.clone(),
        )))
    }
}

macro_rules! delegate_repos {
    ($($method:ident: $repo:ident),* $(,)?) => {
        impl Repos for PgTx {
            $(fn $method(&self) -> &dyn $repo {
                Repos::$method(&self.repos)
            })*
        }
    };
}

delegate_repos!(
    content: ContentRepo,
    meta: MetaRepo,
    revisions: RevisionRepo,
    types: ContentTypeRepo,
    taxonomies: TaxonomyRepo,
    terms: TermRepo,
    menus: MenuRepo,
    options: OptionRepo,
    redirects: RedirectRepo,
    users: UserRepo,
    artifacts: ArtifactRepo,
    media: MediaRepo,
    plugins: PluginRepo,
    plugin_failures: PluginFailureRepo,
    plugin_kv: PluginKvRepo,
    themes: ThemeRepo,
    events: EventRepo,
    jobs: JobRepo,
    nodes: NodeRepo,
    search: SearchRepo,
    schema_meta: SchemaMetaRepo,
    sessions: SessionRepo,
    api_tokens: ApiTokenRepo,
    rate_limits: RateLimitRepo,
    audit: AuditRepo,
    comments: CommentRepo,
    idempotency: IdempotencyRepo,
    autosaves: AutosaveRepo,
    imports: ImportRepo,
    node_metrics: NodeMetricsRepo,
    oauth: OAuthRepo,
    webhooks: WebhookRepo,
);
