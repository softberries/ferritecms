//! Backend-neutral storage traits, errors and migration compatibility logic.

mod error;
mod migrate;
mod repos;

pub use error::StoreError;
pub use migrate::{
    Compat, Directives, MigrationStatus, Phase, SCHEMA_META_MIN_BINARY, expand_lint,
    parse_directives, verdict,
};
pub use repos::{
    ArtifactRepo, Backend, ContentRepo, ContentTypeRepo, EventRepo, MediaRepo, MenuRepo, MetaRepo,
    OptionRepo, PluginKvRepo, PluginRepo, RedirectRepo, Repos, RevisionRepo, SchemaMetaRepo,
    SearchRepo, Store, TaxonomyRepo, TermRepo, ThemeRepo, Tx, UserRepo,
};
