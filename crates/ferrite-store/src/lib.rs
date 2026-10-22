//! Backend-neutral storage traits, errors and migration compatibility logic.

mod error;
mod json;
mod migrate;
mod repos;

pub use error::StoreError;
pub use json::validate_json;
pub use migrate::{
    Compat, Directives, MigrationStatus, Phase, SCHEMA_META_MIN_BINARY, expand_lint,
    next_min_binary, parse_directives, plan_down, verdict,
};
pub use repos::{
    ArtifactRepo, Backend, ContentRepo, ContentTypeRepo, EventRepo, MediaRepo, MenuRepo, MetaRepo,
    OptionRepo, PluginKvRepo, PluginRepo, RedirectRepo, Repos, RevisionRepo, SchemaMetaRepo,
    SearchRepo, Store, TaxonomyRepo, TermRepo, ThemeRepo, Tx, UserRepo,
};
