//! Repository implementations, all on [`SqliteRepos`].
//!
//! **Executor rule (copy it in every repo):** run SELECTs on `self.reader` and every
//! mutation (INSERT/UPDATE/DELETE, including upserts) on `self.writer`:
//!
//! ```text
//! run!(&self.reader, |e| sqlx::query_scalar("SELECT ...").fetch_optional(e))
//! run!(&self.writer, |e| sqlx::query("UPDATE ...").execute(e))
//! ```
//!
//! For `Store::read()` the reader is the read-only reader pool (a write sent there fails
//! loudly) and the writer is the single writer connection. Inside a `Tx` both point at
//! the transaction, so reads see the transaction's own writes.

mod content;
mod event;
mod job;
mod menu;
mod node;
mod plugin;
mod plugin_failure;

pub use plugin::FIND_MEDIA_BY_ARTIFACT_SQL;
mod search;
mod site;
mod taxonomy;
mod user;

use async_trait::async_trait;
use ferrite_store::{
    ArtifactRepo, ContentRepo, ContentTypeRepo, EventRepo, JobRepo, MediaRepo, MenuRepo, MetaRepo,
    NodeRepo, OptionRepo, PluginFailureRepo, PluginKvRepo, PluginRepo, RedirectRepo, Repos,
    RevisionRepo, SchemaMetaRepo, SearchRepo, StoreError, TaxonomyRepo, TermRepo, ThemeRepo, Tx,
    UserRepo,
};
use sqlx::SqlitePool;

use crate::exec::{Exec, TxHandle};

/// Every repository, bound to a read and a write executor (see the module docs).
pub(crate) struct SqliteRepos {
    reader: Exec,
    writer: Exec,
}

impl SqliteRepos {
    /// Autocommit repositories: reads on `reader`, writes on `writer`.
    pub(crate) fn new(reader: SqlitePool, writer: SqlitePool) -> Self {
        Self {
            reader: Exec::Pool(reader),
            writer: Exec::Pool(writer),
        }
    }
}

impl Repos for SqliteRepos {
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
}

/// A write transaction (or a nested savepoint) on the writer connection; dropping it
/// rolls back.
pub(crate) struct SqliteTx {
    handle: TxHandle,
    repos: SqliteRepos,
}

impl SqliteTx {
    pub(crate) fn new(handle: TxHandle) -> Self {
        Self {
            repos: SqliteRepos {
                reader: Exec::Tx(handle.clone()),
                writer: Exec::Tx(handle.clone()),
            },
            handle,
        }
    }
}

impl Drop for SqliteTx {
    fn drop(&mut self) {
        self.handle.abandon();
    }
}

#[async_trait]
impl Tx for SqliteTx {
    async fn commit(self: Box<Self>) -> Result<(), StoreError> {
        self.handle.finish(true).await
    }

    async fn rollback(self: Box<Self>) -> Result<(), StoreError> {
        self.handle.finish(false).await
    }

    async fn begin_nested(&self) -> Result<Box<dyn Tx>, StoreError> {
        Ok(Box::new(SqliteTx::new(self.handle.nested().await?)))
    }
}

macro_rules! delegate_repos {
    ($($method:ident: $repo:ident),* $(,)?) => {
        impl Repos for SqliteTx {
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
);
