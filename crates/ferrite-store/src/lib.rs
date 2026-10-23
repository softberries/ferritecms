//! Backend-neutral storage traits, errors and migration compatibility logic.

mod error;
mod json;
mod lock;
mod menu;
mod migrate;
mod notify;
mod paging;
mod repos;
mod search;
mod text;

pub use error::StoreError;
pub use json::validate_json;
pub use lock::{
    LOCK_EXTENSIONS, LOCK_IMPORT, LOCK_MIGRATE, LOCK_POLL, LockGuard, check_lock_name, lock,
};
pub use menu::{check_redirect_code, insertion_order, tree_order};
pub use migrate::{
    Compat, Directives, MigrationStatus, PendingMigration, Phase, SCHEMA_META_MIN_BINARY,
    expand_lint, next_min_binary, parse_directives, plan_down, verdict,
};
pub use notify::{Channel, Notice, Subscription};
pub use paging::{
    ListPart, MAX_CLAIM, MAX_CLAIM_KINDS, MAX_EVENT_BATCH, MAX_ID_FILTER, MAX_JOB_ATTEMPTS,
    MAX_JOB_DURATION, MAX_JOB_ERROR, MAX_JOB_UNIQUE_KEY, MAX_META_KEYS, MAX_NODES, MAX_PRUNE_BATCH,
    MAX_REDIRECTS_BY_TARGET, MAX_SLUG_PREFIX_RESULTS, REDIRECT_CURSOR_TAG, SortKey,
    USER_CURSOR_TAG, check_claim, check_content_filters, check_event_limit, check_id_list,
    check_job_duration, check_job_kind, check_len, check_limit, check_new_job, check_new_node,
    check_node_meta, check_prune_limit, content_cursor, content_list_plan, content_sort_key,
    encode_content_cursor, encode_text_cursor, job_backoff, sanitize_job_error, slug_has_prefix,
    slug_prefix_bounds, sort_claimed, text_cursor,
};
pub use repos::{
    ArtifactRepo, Backend, ContentRepo, ContentTypeRepo, EventRepo, JobRepo, MediaRepo, MenuRepo,
    MetaRepo, NodeRepo, OptionRepo, PluginKvRepo, PluginRepo, RedirectRepo, Repos, RevisionRepo,
    SchemaMetaRepo, SearchRepo, Store, TaxonomyRepo, TermRepo, ThemeRepo, Tx, UserRepo,
};
pub use search::{
    IndexTerms, IndexedTerm, MAX_INDEX_TERMS, MAX_QUERY_TERMS, MAX_SEARCH_KINDS, MAX_TERM_CHARS,
    SearchPlan, encode_rank_cursor, index_terms, plan_search, rank_cursor, search_terms,
};
pub use text::{email_key, validate_text};

/// Attempts a job gets by default.
pub use ferrite_domain::DEFAULT_MAX_ATTEMPTS;
