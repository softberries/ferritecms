//! Backend-neutral storage traits, errors and migration compatibility logic.

mod error;
mod json;
mod menu;
mod migrate;
mod paging;
mod repos;
mod search;
mod text;

pub use error::StoreError;
pub use json::validate_json;
pub use menu::{check_redirect_code, insertion_order, tree_order};
pub use migrate::{
    Compat, Directives, MigrationStatus, Phase, SCHEMA_META_MIN_BINARY, expand_lint,
    next_min_binary, parse_directives, plan_down, verdict,
};
pub use paging::{
    ListPart, MAX_EVENT_BATCH, SortKey, check_event_limit, check_limit, content_cursor,
    content_list_plan, content_sort_key, encode_content_cursor, encode_text_cursor, text_cursor,
};
pub use repos::{
    ArtifactRepo, Backend, ContentRepo, ContentTypeRepo, EventRepo, MediaRepo, MenuRepo, MetaRepo,
    OptionRepo, PluginKvRepo, PluginRepo, RedirectRepo, Repos, RevisionRepo, SchemaMetaRepo,
    SearchRepo, Store, TaxonomyRepo, TermRepo, ThemeRepo, Tx, UserRepo,
};
pub use search::{
    IndexTerms, IndexedTerm, MAX_INDEX_TERMS, MAX_QUERY_TERMS, MAX_TERM_CHARS, index_terms,
    search_terms,
};
pub use text::{email_key, validate_text};
