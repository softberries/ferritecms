//! Backend-neutral storage traits, errors and migration compatibility logic.

pub mod catalogue;
mod error;
mod export;
mod import;
mod import_writer;
mod json;
mod lock;
mod menu;
mod migrate;
mod notify;
mod paging;
mod repos;
mod search;
mod session;
mod text;

pub use error::StoreError;
pub use export::{ExportReader, MAX_EXPORT_PAGE, check_export_limit};
pub use import::{check_import_map, check_import_run, token_from_db, token_to_db};
pub use import_writer::{
    ImportWriter, InsertBatch, LoadTarget, Loader, MAX_LOAD_BATCH, delete_sql, insert_sql,
};
pub use json::validate_json;
pub use lock::{
    LOCK_BACKUP, LOCK_EXTENSIONS, LOCK_GC, LOCK_IMPORT, LOCK_MIGRATE, LOCK_POLL, LOCK_TYPES,
    LockGuard, check_lock_name, lock,
};
pub use menu::{check_redirect_code, insertion_order, tree_order};
pub use migrate::{
    Compat, Directives, MigrateReport, MigrationStatus, PendingMigration, Phase,
    SCHEMA_META_INSTALL_ID, SCHEMA_META_LAST_BINARY, SCHEMA_META_MIN_BINARY, expand_lint,
    next_min_binary, parse_directives, plan_down, verdict,
};
pub use notify::{Channel, Notice, Subscription};
pub use paging::{
    ARTIFACT_UNREFERENCED_SQL, AUDIT_CURSOR_TAG, AuditPlan, COMMENT_CURSOR_TAG, JOB_CURSOR_TAG,
    ListPart, MAX_AUTH_LIST, MAX_CLAIM, MAX_CLAIM_KINDS, MAX_CONTENT_KINDS, MAX_EVENT_BATCH,
    MAX_EXISTING_HASHES, MAX_ID_FILTER, MAX_IDEMPOTENCY_RESPONSE, MAX_JOB_ATTEMPTS,
    MAX_JOB_DURATION, MAX_JOB_ERROR, MAX_JOB_UNIQUE_KEY, MAX_META_KEYS, MAX_NODES, MAX_PAGE_ROOTS,
    MAX_PAGE_ROWS, MAX_PRUNE_BATCH, MAX_RATE_BUCKET, MAX_RATE_WINDOW, MAX_REDIRECTS_BY_TARGET,
    MAX_SLUG_PREFIX_RESULTS, REDIRECT_CURSOR_TAG, SortKey, USER_CURSOR_TAG, artifact_hash_array,
    audit_cursor, autosave_terms, check_admin_limit, check_audit_query, check_autosave,
    check_claim, check_comment, check_comment_page, check_content_filters, check_event_limit,
    check_failure_count, check_id_list, check_idempotency, check_idempotency_outcome,
    check_job_duration, check_job_kind, check_job_query, check_key_hash, check_len, check_limit,
    check_new_audit, check_new_job, check_new_node, check_node_meta, check_prune_limit,
    check_rate_bucket, check_rate_window, comment_counts, comment_cursor, content_cursor,
    content_list_plan, content_sort_key, encode_audit_cursor, encode_comment_cursor,
    encode_content_cursor, encode_job_cursor, encode_text_cursor, idempotency_status, job_backoff,
    job_cursor, like_contains, media_kind_sql, rate_hit, sanitize_job_error, slug_has_prefix,
    slug_prefix_bounds, sort_claimed, text_cursor,
};
pub use repos::{
    ApiTokenRepo, ApprovedTotals, ArtifactRepo, AuditRepo, AutosaveRepo, Backend, CommentRepo,
    ContentRepo, ContentTypeRepo, EventRepo, IdempotencyRepo, ImportRepo, JobRepo, MarkCounts,
    MediaRepo, MenuRepo, MetaRepo, NodeRepo, OptionRepo, PluginFailureRepo, PluginKvRepo,
    PluginRepo, PoolStats, RateLimitRepo, RedirectRepo, Repos, RetainedPlugin, RevisionRepo,
    SchemaMetaRepo, SearchRepo, SessionRepo, Store, TaxonomyRepo, TermRepo, ThemeRepo, Tx,
    UserRepo, retained_marker,
};
pub use search::{
    IndexTerms, IndexedTerm, MAX_INDEX_TERMS, MAX_QUERY_TERMS, MAX_SEARCH_KINDS, MAX_TERM_CHARS,
    SearchPlan, encode_rank_cursor, index_terms, plan_search, rank_cursor, search_terms,
};
pub use session::session_public_id;
pub use text::{email_key, validate_text};

/// Attempts a job gets by default.
pub use ferrite_domain::DEFAULT_MAX_ATTEMPTS;
