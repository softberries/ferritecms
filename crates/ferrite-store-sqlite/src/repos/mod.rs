//! Repository implementations. Each later M1a task replaces one group of `&Unsupported`
//! accessors with a real implementation on [`SqliteRepos`].
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
mod site;
mod stub;
mod taxonomy;
mod user;

use std::sync::Arc;

use async_trait::async_trait;
use ferrite_store::{
    ArtifactRepo, ContentRepo, ContentTypeRepo, EventRepo, MediaRepo, MenuRepo, MetaRepo,
    OptionRepo, PluginKvRepo, PluginRepo, RedirectRepo, Repos, RevisionRepo, SchemaMetaRepo,
    SearchRepo, StoreError, TaxonomyRepo, TermRepo, ThemeRepo, Tx, UserRepo,
};
use sqlx::{Sqlite, SqlitePool, Transaction};
use stub::Unsupported;
use tokio::sync::Mutex;

use crate::exec::{Exec, SharedTx, finished, map_db_err};

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
        &Unsupported
    }
    fn options(&self) -> &dyn OptionRepo {
        self
    }
    fn redirects(&self) -> &dyn RedirectRepo {
        &Unsupported
    }
    fn users(&self) -> &dyn UserRepo {
        self
    }
    fn artifacts(&self) -> &dyn ArtifactRepo {
        &Unsupported
    }
    fn media(&self) -> &dyn MediaRepo {
        &Unsupported
    }
    fn plugins(&self) -> &dyn PluginRepo {
        &Unsupported
    }
    fn plugin_kv(&self) -> &dyn PluginKvRepo {
        &Unsupported
    }
    fn themes(&self) -> &dyn ThemeRepo {
        &Unsupported
    }
    fn events(&self) -> &dyn EventRepo {
        &Unsupported
    }
    fn search(&self) -> &dyn SearchRepo {
        &Unsupported
    }
    fn schema_meta(&self) -> &dyn SchemaMetaRepo {
        self
    }
}

/// A write transaction on the writer connection; dropping it rolls back.
pub(crate) struct SqliteTx {
    tx: SharedTx,
    repos: SqliteRepos,
}

impl SqliteTx {
    pub(crate) fn new(tx: Transaction<'static, Sqlite>) -> Self {
        let tx = Arc::new(Mutex::new(Some(tx)));
        Self {
            repos: SqliteRepos {
                reader: Exec::Tx(tx.clone()),
                writer: Exec::Tx(tx.clone()),
            },
            tx,
        }
    }

    async fn take(&self) -> Result<Transaction<'static, Sqlite>, StoreError> {
        self.tx.lock().await.take().ok_or_else(finished)
    }
}

#[async_trait]
impl Tx for SqliteTx {
    async fn commit(self: Box<Self>) -> Result<(), StoreError> {
        self.take().await?.commit().await.map_err(map_db_err)
    }

    async fn rollback(self: Box<Self>) -> Result<(), StoreError> {
        self.take().await?.rollback().await.map_err(map_db_err)
    }
}

macro_rules! delegate_repos {
    ($($method:ident: $repo:ident),* $(,)?) => {
        impl Repos for SqliteTx {
            $(fn $method(&self) -> &dyn $repo {
                self.repos.$method()
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
    plugin_kv: PluginKvRepo,
    themes: ThemeRepo,
    events: EventRepo,
    search: SearchRepo,
    schema_meta: SchemaMetaRepo,
);
