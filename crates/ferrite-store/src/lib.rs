//! Backend-neutral storage traits, errors and migration compatibility logic.

pub mod catalogue;
mod change_set;
mod editorial;
mod error;
mod export;
mod import;
mod import_writer;
mod json;
mod locale;
mod lock;
mod menu;
mod migrate;
mod node_metrics;
mod notify;
mod paging;
mod plugin_trust;
mod quality;
mod repos;
mod search;
mod session;
mod text;
mod webhook;

pub use change_set::{
    ChangeSetRepo, MAX_CHANGES_READ, change_set_cursor, check_new_change, check_new_change_set,
    encode_change_set_cursor,
};
pub use editorial::{
    MAX_RELEASES_OF, PreviewLinkRepo, RELEASE_CURSOR_TAG, REVIEW_CURSOR_TAG, ReleaseRepo,
    ReviewRepo, WorkflowRepo, check_release, check_review, check_steps, encode_release_cursor,
    encode_review_cursor, micros_cursor,
};
pub use error::StoreError;
pub use export::{ExportReader, MAX_EXPORT_PAGE, check_export_limit};
pub use import::{check_import_map, check_import_run, token_from_db, token_to_db};
pub use import_writer::{
    ImportWriter, InsertBatch, LoadTarget, Loader, MAX_LOAD_BATCH, delete_sql, insert_sql,
};
pub use json::validate_json;
pub use locale::{LocaleFilter, check_groups, kinds_locale_filter, locale_filter};
pub use lock::{
    LOCK_BACKUP, LOCK_EXTENSIONS, LOCK_GC, LOCK_HEALTH, LOCK_IMPORT, LOCK_MIGRATE, LOCK_POLL,
    LOCK_TYPES, LockGuard, check_lock_name, lock,
};
pub use menu::{check_redirect_code, insertion_order, tree_order};
pub use migrate::{
    Compat, Directives, MigrateReport, MigrationStatus, PendingMigration, Phase,
    SCHEMA_META_INSTALL_ID, SCHEMA_META_LAST_BINARY, SCHEMA_META_MIN_BINARY, expand_lint,
    next_min_binary, parse_directives, plan_down, verdict,
};
pub use node_metrics::{check_node_metrics, check_series_range, series_bucket_from_db};
pub use notify::{Channel, Notice, Subscription};
pub use paging::{
    ARTIFACT_UNREFERENCED_SQL, AUDIT_CURSOR_TAG, AuditPlan, COMMENT_CURSOR_TAG, GRANT_CURSOR_TAG,
    JOB_CURSOR_TAG, ListPart, MAX_AUTH_LIST, MAX_CLAIM, MAX_CLAIM_KINDS, MAX_CONTENT_KINDS,
    MAX_EVENT_BATCH, MAX_EXISTING_HASHES, MAX_ID_FILTER, MAX_IDEMPOTENCY_RESPONSE,
    MAX_JOB_ATTEMPTS, MAX_JOB_DURATION, MAX_JOB_ERROR, MAX_JOB_UNIQUE_KEY, MAX_META_KEYS,
    MAX_NODES, MAX_PAGE_ROOTS, MAX_PAGE_ROWS, MAX_PRUNE_BATCH, MAX_RATE_BUCKET, MAX_RATE_WINDOW,
    MAX_REDIRECTS_BY_TARGET, MAX_SLUG_PREFIX_RESULTS, REDIRECT_CURSOR_TAG, SortKey,
    USER_CURSOR_TAG, artifact_hash_array, audit_cursor, autosave_terms, check_admin_limit,
    check_audit_query, check_autosave, check_claim, check_comment, check_comment_page,
    check_content_filters, check_event_limit, check_failure_count, check_file_bounds,
    check_id_list, check_idempotency, check_idempotency_outcome, check_job_duration,
    check_job_kind, check_job_query, check_key_hash, check_len, check_limit, check_new_audit,
    check_new_job, check_new_node, check_node_meta, check_prune_limit, check_rate_bucket,
    check_rate_window, comment_counts, comment_cursor, content_cursor, content_list_plan,
    content_sort_key, encode_audit_cursor, encode_comment_cursor, encode_content_cursor,
    encode_grant_cursor, encode_job_cursor, encode_text_cursor, file_bounds_from_rows,
    grant_cursor, idempotency_status, job_backoff, job_cursor, like_contains, media_kind_sql,
    rate_hit, sanitize_job_error, slug_has_prefix, slug_prefix_bounds, sort_claimed, text_cursor,
};
pub use plugin_trust::{
    PluginTrustRepo, check_plugin_owner, check_registry_row, check_trusted_key,
};
pub use quality::{
    FINDING_CURSOR_TAG, MAX_FINDING_BATCH, MAX_LINK_BATCH, QualityRepo, RunEnd, check_finding,
    check_finding_query, check_link, check_run, check_suppression, encode_finding_cursor,
    finding_cursor, link_url_hash,
};
pub use repos::{
    ApiTokenRepo, ApprovedTotals, ArtifactRepo, AuditRepo, AutosaveRepo, Backend, CommentRepo,
    ContentRepo, ContentTypeRepo, EventRepo, FileBound, IdempotencyRepo, ImportRepo, JobRepo,
    LocaleRepo, LocaleUsage, MAX_GROUPS, MarkCounts, MediaRepo, MenuRepo, MenuSummary, MetaRepo,
    NodeMetricsRepo, NodeRepo, OAuthRepo, OptionRepo, PluginFailureRepo, PluginKvRepo, PluginRepo,
    PoolStats, RateLimitRepo, RedirectRepo, Repos, RetainedPlugin, RevisionRepo, SchemaMetaRepo,
    SearchRepo, SessionRepo, Store, TaxonomyRepo, TermRepo, ThemeRepo, TranslationRef, Tx,
    UserRepo, retained_marker,
};
pub use search::{
    IndexTerms, IndexedTerm, MAX_INDEX_TERMS, MAX_QUERY_TERMS, MAX_SEARCH_KINDS, MAX_TERM_CHARS,
    SearchPlan, encode_rank_cursor, index_terms, plan_search, rank_cursor, search_terms,
};
pub use session::session_public_id;
pub use text::{email_key, validate_text};
pub use webhook::{
    MAX_WEBHOOK_ENDPOINTS_READ, WEBHOOK_DELIVERY_CURSOR_TAG, WebhookRepo, check_webhook_attempt,
    check_webhook_endpoint, encode_webhook_delivery_cursor, webhook_delivery_cursor,
};

/// Attempts a job gets by default.
pub use ferrite_domain::DEFAULT_MAX_ATTEMPTS;
