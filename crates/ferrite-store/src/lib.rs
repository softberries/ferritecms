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
    Compat, Directives, MigrationStatus, PendingMigration, Phase, SCHEMA_META_MIN_BINARY,
    expand_lint, next_min_binary, parse_directives, plan_down, verdict,
};
pub use paging::{
    ListPart, MAX_EVENT_BATCH, MAX_ID_FILTER, MAX_META_KEYS, MAX_REDIRECTS_BY_TARGET,
    MAX_SLUG_PREFIX_RESULTS, REDIRECT_CURSOR_TAG, SortKey, USER_CURSOR_TAG, check_content_filters,
    check_event_limit, check_id_list, check_len, check_limit, content_cursor, content_list_plan,
    content_sort_key, encode_content_cursor, encode_text_cursor, slug_has_prefix,
    slug_prefix_bounds, text_cursor,
};
pub use repos::{
    ArtifactRepo, Backend, ContentRepo, ContentTypeRepo, EventRepo, MediaRepo, MenuRepo, MetaRepo,
    OptionRepo, PluginKvRepo, PluginRepo, RedirectRepo, Repos, RevisionRepo, SchemaMetaRepo,
    SearchRepo, Store, TaxonomyRepo, TermRepo, ThemeRepo, Tx, UserRepo,
};
pub use search::{
    IndexTerms, IndexedTerm, MAX_INDEX_TERMS, MAX_QUERY_TERMS, MAX_SEARCH_KINDS, MAX_TERM_CHARS,
    SearchPlan, encode_rank_cursor, index_terms, plan_search, rank_cursor, search_terms,
};
pub use text::{email_key, validate_text};
