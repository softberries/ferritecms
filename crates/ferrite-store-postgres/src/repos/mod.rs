//! Repository implementations. Each later M1a task replaces one group of `&Unsupported`
//! accessors with a real implementation on [`PgRepos`].

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
use sqlx::{PgPool, Postgres, Transaction};
use stub::Unsupported;
use tokio::sync::Mutex;

use crate::exec::{Exec, SharedTx, finished, map_db_err};

/// Every repository, bound to one executor (the pool or a write transaction).
pub(crate) struct PgRepos {
    exec: Exec,
}

impl PgRepos {
    pub(crate) fn new(pool: PgPool) -> Self {
        Self {
            exec: Exec::Pool(pool),
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

/// A write transaction on a pool connection; dropping it rolls back.
pub(crate) struct PgTx {
    tx: SharedTx,
    repos: PgRepos,
}

impl PgTx {
    pub(crate) fn new(tx: Transaction<'static, Postgres>) -> Self {
        let tx = Arc::new(Mutex::new(Some(tx)));
        Self {
            repos: PgRepos {
                exec: Exec::Tx(tx.clone()),
            },
            tx,
        }
    }

    async fn take(&self) -> Result<Transaction<'static, Postgres>, StoreError> {
        self.tx.lock().await.take().ok_or_else(finished)
    }
}

#[async_trait]
impl Tx for PgTx {
    async fn commit(self: Box<Self>) -> Result<(), StoreError> {
        self.take().await?.commit().await.map_err(map_db_err)
    }

    async fn rollback(self: Box<Self>) -> Result<(), StoreError> {
        self.take().await?.rollback().await.map_err(map_db_err)
    }
}

macro_rules! delegate_repos {
    ($($method:ident: $repo:ident),* $(,)?) => {
        impl Repos for PgTx {
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
