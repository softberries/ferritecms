//! Keyset-pagination helpers shared by the backends so cursors mean the same thing everywhere.

use std::{collections::BTreeMap, time::Duration};

use ferrite_domain::{
    ArtifactHash, Autosave, Comment, CommentStatus, ContentId, ContentItem, ContentOrder,
    ContentQuery, Cursor, IdempotencyKey, Job, MAX_PAGE, MEDIA_KIND, MediaKind, NewAudit, NewJob,
    NewNode, RateHit, TermId, Timestamp,
};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{StoreError, validate_json, validate_text};

/// The sort-key part of a content cursor, typed by [`ContentOrder`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SortKey {
    /// Unix microseconds, or `None` for a NULL timestamp (`published_at` only).
    Micros(Option<i64>),
    Int(i32),
    Text(String),
}

/// Validates a page size: 1 through [`MAX_PAGE`].
pub fn check_limit(limit: u32) -> Result<u32, StoreError> {
    if limit == 0 || limit > MAX_PAGE {
        return Err(StoreError::Invalid(format!(
            "limit must be between 1 and {MAX_PAGE}"
        )));
    }
    Ok(limit)
}

/// Most ids a content query's `ids`/`exclude` filter (or `get_many`) accepts.
pub const MAX_ID_FILTER: usize = 200;

/// Most keys `MetaRepo::get_many` accepts.
pub const MAX_META_KEYS: usize = 50;

/// Most slugs `slugs_with_prefix` returns.
pub const MAX_SLUG_PREFIX_RESULTS: u32 = 1000;

/// Most redirects `RedirectRepo::list_by_target` returns.
pub const MAX_REDIRECTS_BY_TARGET: u32 = 1000;

/// Validates a batch-loader input length: at most `max` entries.
pub fn check_len(what: &str, len: usize, max: usize) -> Result<(), StoreError> {
    if len > max {
        return Err(StoreError::Invalid(format!(
            "{what} holds more than {max} entries"
        )));
    }
    Ok(())
}

/// Most hashes `ArtifactRepo::existing` accepts.
pub const MAX_EXISTING_HASHES: usize = 1000;

/// SQL predicate on `artifacts` (table name unaliased): no row names the artifact. P10's SQL
/// part of "referenced", the same text on both backends.
pub const ARTIFACT_UNREFERENCED_SQL: &str = "NOT EXISTS (SELECT 1 FROM media WHERE artifact_hash = artifacts.hash) \
     AND NOT EXISTS (SELECT 1 FROM media_variants WHERE hash = artifacts.hash) \
     AND NOT EXISTS (SELECT 1 FROM plugins WHERE artifact_hash = artifacts.hash) \
     AND NOT EXISTS (SELECT 1 FROM themes WHERE artifact_hash = artifacts.hash) \
     AND NOT EXISTS (SELECT 1 FROM import_runs WHERE report_hash = artifacts.hash)";

/// The hashes as one JSON array text, the single parameter the artifact GC statements
/// unpack (`json_each` / `jsonb_array_elements_text`).
pub fn artifact_hash_array(hashes: &[ArtifactHash]) -> Result<String, StoreError> {
    serde_json::to_string(&hashes.iter().map(ArtifactHash::as_str).collect::<Vec<_>>())
        .map_err(|e| StoreError::Invalid(e.to_string()))
}

/// Validates an id list: at most [`MAX_ID_FILTER`] entries.
pub fn check_id_list(what: &str, ids: &[ContentId]) -> Result<(), StoreError> {
    if ids.len() > MAX_ID_FILTER {
        return Err(StoreError::Invalid(format!(
            "{what} holds more than {MAX_ID_FILTER} ids"
        )));
    }
    Ok(())
}

/// The `media` row predicate of a [`MediaKind`] (by the type's top-level part), the
/// same SQL text on both backends.
pub fn media_kind_sql(kind: MediaKind) -> &'static str {
    match kind {
        MediaKind::Image => "substr(mime, 1, 6) = 'image/'",
        MediaKind::Video => "substr(mime, 1, 6) = 'video/'",
        MediaKind::Audio => "substr(mime, 1, 6) = 'audio/'",
        MediaKind::Document => "substr(mime, 1, 6) NOT IN ('image/', 'video/', 'audio/')",
    }
}

/// Most kinds a content query's `kinds` filter accepts.
pub const MAX_CONTENT_KINDS: usize = 20;

/// Validates the filter part of a content query (shared by `list` and `count`).
pub fn check_content_filters(query: &ContentQuery) -> Result<(), StoreError> {
    if let Some(kind) = &query.kind {
        validate_text("content kind", kind)?;
    }
    if query.kind.is_some() && !query.kinds.is_empty() {
        return Err(StoreError::Invalid(
            "content query takes kind or kinds, not both".into(),
        ));
    }
    if query.kinds.len() > MAX_CONTENT_KINDS {
        return Err(StoreError::Invalid(format!(
            "kinds holds more than {MAX_CONTENT_KINDS} entries"
        )));
    }
    for kind in &query.kinds {
        validate_text("content kind", kind)?;
    }
    if query.media_kind.is_some()
        && (query.kind.as_deref().is_some_and(|k| k != MEDIA_KIND)
            || query.kinds.iter().any(|k| k != MEDIA_KIND))
    {
        return Err(StoreError::Invalid(
            "media_kind only applies to kind media".into(),
        ));
    }
    check_id_list("ids", &query.ids)?;
    check_id_list("exclude", &query.exclude)
}

/// The exclusive upper bound of the slugs starting with `prefix-`: `prefix.` (`.` is the
/// byte after `-`). Together with `>= prefix-` it is an exact, case-sensitive,
/// index-friendly range under binary collation, with no LIKE escaping or ASCII case folding.
pub fn slug_prefix_bounds(prefix: &str) -> (String, String) {
    (format!("{prefix}-"), format!("{prefix}."))
}

/// Defence in depth for `slugs_with_prefix`: is `slug` exactly `prefix` or `prefix-…`?
pub fn slug_has_prefix(slug: &str, prefix: &str) -> bool {
    slug.strip_prefix(prefix)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('-'))
}

/// Largest batch an event reader may request (design 04 §4 polls `LIMIT 1000`).
pub const MAX_EVENT_BATCH: u32 = 1000;

/// Validates an event batch size: 1 through [`MAX_EVENT_BATCH`].
pub fn check_event_limit(limit: u32) -> Result<u32, StoreError> {
    if limit == 0 || limit > MAX_EVENT_BATCH {
        return Err(StoreError::Invalid(format!(
            "limit must be between 1 and {MAX_EVENT_BATCH}"
        )));
    }
    Ok(limit)
}

/// Most jobs one `JobRepo::claim` call returns.
pub const MAX_CLAIM: u32 = 100;

/// Most rows one `prune` call (events, jobs, nodes) deletes (R5).
pub const MAX_PRUNE_BATCH: u32 = 10_000;

/// Validates a failure count added to a `plugin_failures` bucket: at least 1.
pub fn check_failure_count(count: u32) -> Result<(), StoreError> {
    if count == 0 {
        return Err(StoreError::Invalid(
            "plugin failure count must be at least 1".into(),
        ));
    }
    Ok(())
}

/// Validates a prune batch size: 1..=[`MAX_PRUNE_BATCH`].
pub fn check_prune_limit(limit: u32) -> Result<i64, StoreError> {
    if limit == 0 || limit > MAX_PRUNE_BATCH {
        return Err(StoreError::Invalid(format!(
            "prune limit must be between 1 and {MAX_PRUNE_BATCH}"
        )));
    }
    Ok(i64::from(limit))
}

/// Most kinds one `JobRepo::claim` call accepts.
pub const MAX_CLAIM_KINDS: usize = 64;

/// Longest stored `last_error`, in bytes (longer handler text is cut on a char boundary).
pub const MAX_JOB_ERROR: usize = 1024;

/// Longest job `unique_key`, in characters.
pub const MAX_JOB_UNIQUE_KEY: usize = 200;

/// Largest `max_attempts` a job may ask for.
pub const MAX_JOB_ATTEMPTS: i32 = 100;

/// Longest lease or release delay a `JobRepo` call accepts (one year).
pub const MAX_JOB_DURATION: Duration = Duration::from_secs(365 * 24 * 3600);

/// Retry delay after a failed attempt: `min(2^attempts × 10 s, 3600 s)`; `attempts <= 0`
/// counts as 0 (10 s).
pub fn job_backoff(attempts: i32) -> Duration {
    const CAP: u64 = 3600;
    let exp = u32::try_from(attempts.max(0)).unwrap_or(0);
    let secs = 2u64
        .checked_pow(exp)
        .and_then(|p| p.checked_mul(10))
        .map_or(CAP, |s| s.min(CAP));
    Duration::from_secs(secs)
}

/// Validates a job kind: 1 to 64 characters from `[a-z0-9._-]`.
pub fn check_job_kind(kind: &str) -> Result<(), StoreError> {
    let ok = (1..=64).contains(&kind.len())
        && kind
            .bytes()
            .all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'.' | b'_' | b'-'));
    if ok {
        Ok(())
    } else {
        Err(StoreError::Invalid(format!(
            "job kind {kind:?} must be 1 to 64 characters from [a-z0-9._-]"
        )))
    }
}

/// Validates an enqueue input: kind, unique key (at most [`MAX_JOB_UNIQUE_KEY`]
/// characters, no NUL), payload (a JSON object without NUL) and `max_attempts`
/// (1 through [`MAX_JOB_ATTEMPTS`]).
pub fn check_new_job(job: &NewJob) -> Result<(), StoreError> {
    check_job_kind(&job.kind)?;
    if let Some(key) = &job.unique_key {
        validate_text("job unique_key", key)?;
        if key.chars().count() > MAX_JOB_UNIQUE_KEY {
            return Err(StoreError::Invalid(format!(
                "job unique_key is longer than {MAX_JOB_UNIQUE_KEY} characters"
            )));
        }
    }
    if !job.payload.is_object() {
        return Err(StoreError::Invalid(
            "job payload must be a JSON object".into(),
        ));
    }
    validate_json(&job.payload)?;
    if !(1..=MAX_JOB_ATTEMPTS).contains(&job.max_attempts) {
        return Err(StoreError::Invalid(format!(
            "job max_attempts must be between 1 and {MAX_JOB_ATTEMPTS}"
        )));
    }
    Ok(())
}

/// Most rows `NodeRepo::list` returns.
pub const MAX_NODES: u32 = 1000;

/// Validates node `meta`: a JSON object without NUL.
pub fn check_node_meta(meta: &Value) -> Result<(), StoreError> {
    if !meta.is_object() {
        return Err(StoreError::Invalid(
            "node meta must be a JSON object".into(),
        ));
    }
    validate_json(meta)
}

/// Validates a node registration: no NUL in `name` or `version`, `meta` as
/// [`check_node_meta`].
pub fn check_new_node(node: &NewNode) -> Result<(), StoreError> {
    validate_text("node name", &node.name)?;
    validate_text("node version", &node.version)?;
    check_node_meta(&node.meta)
}

/// Validates claim arguments: 1 through [`MAX_CLAIM_KINDS`] valid kinds and a `limit`
/// of 1 through [`MAX_CLAIM`].
pub fn check_claim(kinds: &[String], limit: u32) -> Result<(), StoreError> {
    if kinds.is_empty() || kinds.len() > MAX_CLAIM_KINDS {
        return Err(StoreError::Invalid(format!(
            "claim needs between 1 and {MAX_CLAIM_KINDS} kinds"
        )));
    }
    kinds.iter().try_for_each(|k| check_job_kind(k))?;
    if limit == 0 || limit > MAX_CLAIM {
        return Err(StoreError::Invalid(format!(
            "claim limit must be between 1 and {MAX_CLAIM}"
        )));
    }
    Ok(())
}

/// Sorts claimed jobs into claim order: `(priority DESC, run_at, id)` (`RETURNING` order
/// is unspecified on both engines).
pub fn sort_claimed(jobs: &mut [Job]) {
    jobs.sort_by(|a, b| {
        b.priority
            .cmp(&a.priority)
            .then_with(|| a.run_at.cmp(&b.run_at))
            .then_with(|| a.id.cmp(&b.id))
    });
}

/// Validates a lease or delay (at most [`MAX_JOB_DURATION`]; a lease must also be
/// positive) and returns it in microseconds.
pub fn check_job_duration(what: &str, d: Duration, positive: bool) -> Result<i64, StoreError> {
    if d > MAX_JOB_DURATION || (positive && d.is_zero()) {
        let min = if positive {
            "more than 0"
        } else {
            "at least 0"
        };
        return Err(StoreError::Invalid(format!(
            "{what} must be {min} and at most {} s",
            MAX_JOB_DURATION.as_secs()
        )));
    }
    i64::try_from(d.as_micros()).map_err(|_| StoreError::Invalid(format!("{what} too large")))
}

/// Handler error text as stored: NUL replaced by U+FFFD, cut to [`MAX_JOB_ERROR`] bytes
/// on a char boundary. Handler text is not caller-validated input, so it is sanitised
/// rather than refused.
pub fn sanitize_job_error(error: &str) -> String {
    let mut text = error.replace('\0', "\u{FFFD}");
    if text.len() > MAX_JOB_ERROR {
        let mut cut = MAX_JOB_ERROR;
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        text.truncate(cut);
    }
    text
}

/// The sort key of `item` under `order`.
pub fn content_sort_key(order: ContentOrder, item: &ContentItem) -> SortKey {
    match order {
        ContentOrder::PublishedDesc => SortKey::Micros(item.published_at.map(|t| t.as_micros())),
        ContentOrder::UpdatedDesc => SortKey::Micros(Some(item.updated_at.as_micros())),
        ContentOrder::MenuOrderAsc => SortKey::Int(item.menu_order),
        ContentOrder::TitleAsc => SortKey::Text(item.title.clone()),
    }
}

/// The cursor kind tag of a content listing in `order`.
fn content_tag(order: ContentOrder) -> &'static str {
    match order {
        ContentOrder::PublishedDesc => "c:published",
        ContentOrder::UpdatedDesc => "c:updated",
        ContentOrder::MenuOrderAsc => "c:menu",
        ContentOrder::TitleAsc => "c:title",
    }
}

/// Cursor tag of the redirect listing (see [`encode_text_cursor`]).
pub const REDIRECT_CURSOR_TAG: &str = "r:source";

/// Cursor tag of the user listing (see [`encode_text_cursor`]).
pub const USER_CURSOR_TAG: &str = "u:login";

/// Cursor tag of the audit-log listing (see [`encode_audit_cursor`]).
pub const AUDIT_CURSOR_TAG: &str = "a:at";

/// Encodes the audit-log cursor pointing just after the row (`at`, `id`).
pub fn encode_audit_cursor(at: Timestamp, id: Uuid) -> String {
    Cursor::encode(AUDIT_CURSOR_TAG, &json!(at.as_micros()), id)
}

/// Decodes a cursor made by [`encode_audit_cursor`] into (`at` micros, `id`).
pub fn audit_cursor(cursor: &str) -> Result<(i64, Uuid), StoreError> {
    micros_cursor(AUDIT_CURSOR_TAG, cursor)
}

/// An [`AuditQuery`](ferrite_domain::AuditQuery) checked by [`check_audit_query`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuditPlan<'a> {
    pub limit: u32,
    /// The decoded cursor (`at` micros, `id`).
    pub after: Option<(i64, Uuid)>,
    /// The `action` filter: the text and whether it is a prefix (it ends in `.`).
    pub action: Option<(&'a str, bool)>,
}

/// Validates an [`AuditQuery`](ferrite_domain::AuditQuery): `limit` 1..=200, a cursor
/// of the audit listing, no NUL in any filter, and `target_id` only with
/// `target_kind`.
pub fn check_audit_query(q: &ferrite_domain::AuditQuery) -> Result<AuditPlan<'_>, StoreError> {
    let limit = check_limit(q.limit)?;
    let after = q.cursor.as_deref().map(audit_cursor).transpose()?;
    for (field, value) in [
        ("audit action filter", &q.action),
        ("audit target_kind filter", &q.target_kind),
        ("audit target_id filter", &q.target_id),
    ] {
        if let Some(v) = value {
            validate_text(field, v)?;
        }
    }
    if q.target_id.is_some() && q.target_kind.is_none() {
        return Err(StoreError::Invalid(
            "an audit target_id filter needs target_kind".into(),
        ));
    }
    Ok(AuditPlan {
        limit,
        after,
        action: q.action.as_deref().map(|a| (a, a.ends_with('.'))),
    })
}

/// Cursor tag of the comment listing (see [`encode_comment_cursor`]; `c:` belongs to
/// content cursors, F4).
pub const COMMENT_CURSOR_TAG: &str = "cm:created";

/// Encodes the comment-listing cursor pointing just after the row (`created_at`, `id`).
pub fn encode_comment_cursor(created_at: Timestamp, id: Uuid) -> String {
    Cursor::encode(COMMENT_CURSOR_TAG, &json!(created_at.as_micros()), id)
}

/// Decodes a cursor made by [`encode_comment_cursor`] into (`created_at` micros, `id`).
pub fn comment_cursor(cursor: &str) -> Result<(i64, Uuid), StoreError> {
    micros_cursor(COMMENT_CURSOR_TAG, cursor)
}

/// Cursor tag of the job listing (see [`encode_job_cursor`]).
pub const JOB_CURSOR_TAG: &str = "j:created";

/// Encodes the job-listing cursor pointing just after the row (`created_at`, `id`).
pub fn encode_job_cursor(created_at: Timestamp, id: Uuid) -> String {
    Cursor::encode(JOB_CURSOR_TAG, &json!(created_at.as_micros()), id)
}

/// Decodes a cursor made by [`encode_job_cursor`] into (`created_at` micros, `id`).
pub fn job_cursor(cursor: &str) -> Result<(i64, Uuid), StoreError> {
    micros_cursor(JOB_CURSOR_TAG, cursor)
}

/// Validates a [`JobQuery`](ferrite_domain::JobQuery): `limit` 1..=200, `kind` like
/// [`check_job_kind`]; returns the limit and the decoded cursor.
pub fn check_job_query(
    query: &ferrite_domain::JobQuery,
) -> Result<(u32, Option<(i64, Uuid)>), StoreError> {
    let limit = check_limit(query.limit)?;
    if let Some(kind) = &query.kind {
        check_job_kind(kind)?;
    }
    let after = query.cursor.as_deref().map(job_cursor).transpose()?;
    Ok((limit, after))
}

/// Decodes a `tag` cursor whose sort key is a microsecond timestamp.
fn micros_cursor(tag: &str, cursor: &str) -> Result<(i64, Uuid), StoreError> {
    let bad = || StoreError::Invalid("cursor does not match the query order".into());
    match Cursor::decode(cursor, tag)? {
        (Value::Number(n), id) => n
            .as_i64()
            .filter(|at| Timestamp::from_micros(*at).is_ok())
            .map(|at| (at, id))
            .ok_or_else(bad),
        _ => Err(bad()),
    }
}

/// Most roots (top-level threads) `CommentRepo::approved_page` may be asked for.
pub const MAX_PAGE_ROOTS: u32 = 500;

/// The row budget of one public comment page (`CommentRepo::approved_page`, amended
/// P34): a page always holds every one of its roots, then their replies in thread order
/// (depth first, oldest first) until it holds this many rows; later replies are not
/// shown.
pub const MAX_PAGE_ROWS: u32 = 2_000;

/// Validates an `approved_page` request: `roots` 1..=[`MAX_PAGE_ROOTS`] and `max_rows`
/// `roots..=`[`MAX_PAGE_ROWS`] (the roots always fit). Returns both as `i64` binds.
pub fn check_comment_page(roots: u32, max_rows: u32) -> Result<(i64, i64), StoreError> {
    if roots == 0 || roots > MAX_PAGE_ROOTS {
        return Err(StoreError::Invalid(format!(
            "roots must be between 1 and {MAX_PAGE_ROOTS}"
        )));
    }
    if max_rows < roots || max_rows > MAX_PAGE_ROWS {
        return Err(StoreError::Invalid(format!(
            "max_rows must be between roots ({roots}) and {MAX_PAGE_ROWS}"
        )));
    }
    Ok((i64::from(roots), i64::from(max_rows)))
}

/// Validates a comment before any SQL: no NUL in any text.
pub fn check_comment(comment: &Comment) -> Result<(), StoreError> {
    validate_text("comment author_name", &comment.author_name)?;
    validate_text("comment author_email", &comment.author_email)?;
    validate_text("comment author_url", &comment.author_url)?;
    validate_text("comment body", &comment.body)?;
    if let Some(ip) = &comment.ip_hash {
        validate_text("comment ip_hash", ip)?;
    }
    Ok(())
}

/// Largest stored idempotent response, in bytes of its JSON text (M2a-2b: 64 KiB).
pub const MAX_IDEMPOTENCY_RESPONSE: usize = 64 * 1024;

/// Longest `stale_after` `IdempotencyRepo::begin` accepts.
const MAX_IDEMPOTENCY_STALE: Duration = Duration::from_secs(24 * 3600);

/// Validates an idempotency key hash: 64 lowercase hex characters.
pub fn check_key_hash(key_hash: &str) -> Result<(), StoreError> {
    let ok = key_hash.len() == 64
        && key_hash
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
    if ok {
        Ok(())
    } else {
        Err(StoreError::Invalid(
            "idempotency key_hash must be 64 lowercase hex characters".into(),
        ))
    }
}

/// Validates `AutosaveRepo::put` input (NUL in its texts or JSON, as for a content
/// row) and returns its `terms` as a JSON array (`None` stays `None`).
pub fn check_autosave(a: &Autosave) -> Result<Option<Value>, StoreError> {
    validate_text("autosave title", &a.title)?;
    if let Some(excerpt) = &a.excerpt {
        validate_text("autosave excerpt", excerpt)?;
    }
    if let Some(slug) = &a.slug {
        validate_text("autosave slug", slug)?;
    }
    validate_json(&a.body)?;
    validate_json(&a.fields)?;
    if let Some(seo) = &a.seo {
        validate_json(seo)?;
    }
    Ok(a.terms.as_ref().map(|terms| {
        Value::Array(
            terms
                .iter()
                .map(|t| Value::String(t.as_uuid().to_string()))
                .collect(),
        )
    }))
}

/// Decodes the `terms` of a stored autosave (a JSON array of term ids).
pub fn autosave_terms(value: Option<Value>) -> Result<Option<Vec<TermId>>, StoreError> {
    value
        .map(|v| {
            serde_json::from_value::<Vec<Uuid>>(v)
                .map(|ids| ids.into_iter().map(TermId::from_uuid).collect())
                .map_err(|e| StoreError::Database(format!("corrupt autosave terms: {e}")))
        })
        .transpose()
}

/// Validates `IdempotencyRepo::begin` input and returns `stale_after` in microseconds.
pub fn check_idempotency(key: &IdempotencyKey, stale_after: Duration) -> Result<i64, StoreError> {
    check_key_hash(&key.key_hash)?;
    validate_text("idempotency method", &key.method)?;
    validate_text("idempotency path", &key.path)?;
    if stale_after.is_zero() || stale_after > MAX_IDEMPOTENCY_STALE {
        return Err(StoreError::Invalid(
            "idempotency stale_after must be more than zero and at most one day".into(),
        ));
    }
    i64::try_from(stale_after.as_micros())
        .map_err(|_| StoreError::Invalid("idempotency stale_after out of range".into()))
}

/// Validates an `IdempotencyRepo::finish` outcome and returns the response as JSON text
/// (at most [`MAX_IDEMPOTENCY_RESPONSE`] bytes, no NUL) and the status as `i32`.
pub fn check_idempotency_outcome(
    key_hash: &str,
    status: u16,
    response: &Value,
) -> Result<(String, i32), StoreError> {
    check_key_hash(key_hash)?;
    if !(100..=599).contains(&status) {
        return Err(StoreError::Invalid(
            "idempotency status must be between 100 and 599".into(),
        ));
    }
    validate_json(response)?;
    let text = serde_json::to_string(response).map_err(|e| StoreError::Invalid(e.to_string()))?;
    if text.len() > MAX_IDEMPOTENCY_RESPONSE {
        return Err(StoreError::Invalid(format!(
            "idempotency response is larger than {MAX_IDEMPOTENCY_RESPONSE} bytes"
        )));
    }
    Ok((text, i32::from(status)))
}

/// Decodes a stored idempotency status code.
pub fn idempotency_status(code: i64) -> Result<u16, StoreError> {
    u16::try_from(code)
        .map_err(|_| StoreError::Database(format!("corrupt idempotency status {code}")))
}

/// Most sessions or API tokens one `list_for_user` returns, and the largest
/// `UserRepo::list_active_admins` limit.
pub const MAX_AUTH_LIST: u32 = 100;

/// Validates a `UserRepo::list_active_admins` limit: 1..=[`MAX_AUTH_LIST`].
pub fn check_admin_limit(limit: u32) -> Result<u32, StoreError> {
    if limit == 0 || limit > MAX_AUTH_LIST {
        return Err(StoreError::Invalid(format!(
            "limit must be between 1 and {MAX_AUTH_LIST}"
        )));
    }
    Ok(limit)
}

/// Longest rate-limit bucket name, in characters.
pub const MAX_RATE_BUCKET: usize = 200;

/// Longest rate-limit window.
pub const MAX_RATE_WINDOW: Duration = Duration::from_secs(24 * 3600);

/// Validates a rate-limit bucket: 1..=[`MAX_RATE_BUCKET`] characters, no NUL.
pub fn check_rate_bucket(bucket: &str) -> Result<(), StoreError> {
    validate_text("rate limit bucket", bucket)?;
    if bucket.is_empty() || bucket.chars().count() > MAX_RATE_BUCKET {
        return Err(StoreError::Invalid(format!(
            "rate limit bucket must be 1 to {MAX_RATE_BUCKET} characters"
        )));
    }
    Ok(())
}

/// Validates a rate-limit window (more than zero, at most [`MAX_RATE_WINDOW`]) and
/// returns it in microseconds.
pub fn check_rate_window(window: Duration) -> Result<i64, StoreError> {
    if window.is_zero() || window > MAX_RATE_WINDOW {
        return Err(StoreError::Invalid(
            "rate limit window must be more than zero and at most one day".into(),
        ));
    }
    i64::try_from(window.as_micros())
        .map_err(|_| StoreError::Invalid("rate limit window out of range".into()))
}

/// Builds the [`RateHit`] a backend returns from the stored `count` and `window_start`
/// (micros), the clock the hit used and the window length in micros.
pub fn rate_hit(
    count: i64,
    window_start: i64,
    now: Timestamp,
    window: i64,
) -> Result<RateHit, StoreError> {
    let count = u32::try_from(count)
        .map_err(|_| StoreError::Database(format!("corrupt rate limit count {count}")))?;
    let reset_at = Timestamp::from_micros(window_start.saturating_add(window))
        .map_err(|e| StoreError::Database(e.to_string()))?;
    Ok(RateHit {
        count,
        reset_at,
        now,
    })
}

/// Validates an audit row before any SQL: NUL-free text, `details` a NUL-free object.
pub fn check_new_audit(entry: &NewAudit) -> Result<(), StoreError> {
    validate_text("audit action", &entry.action)?;
    validate_text("audit target_kind", &entry.target_kind)?;
    if let Some(id) = &entry.target_id {
        validate_text("audit target_id", id)?;
    }
    if !entry.details.is_object() {
        return Err(StoreError::Invalid(
            "audit details must be a JSON object".into(),
        ));
    }
    validate_json(&entry.details)
}

/// Encodes the cursor pointing just after `item`.
pub fn encode_content_cursor(order: ContentOrder, item: &ContentItem) -> String {
    let key = match content_sort_key(order, item) {
        SortKey::Micros(m) => json!(m),
        SortKey::Int(i) => json!(i),
        SortKey::Text(t) => json!(t),
    };
    Cursor::encode(content_tag(order), &key, item.id.as_uuid())
}

/// Decodes a cursor for `order`; a malformed cursor or one of another order is `Invalid`.
pub fn content_cursor(order: ContentOrder, cursor: &str) -> Result<(SortKey, Uuid), StoreError> {
    let (key, id) = Cursor::decode(cursor, content_tag(order))?;
    let bad = || StoreError::Invalid("cursor does not match the query order".into());
    let key = match (order, key) {
        (ContentOrder::PublishedDesc, Value::Null) => SortKey::Micros(None),
        (ContentOrder::PublishedDesc | ContentOrder::UpdatedDesc, Value::Number(n)) => {
            SortKey::Micros(Some(n.as_i64().ok_or_else(bad)?))
        }
        (ContentOrder::MenuOrderAsc, Value::Number(n)) => SortKey::Int(
            n.as_i64()
                .and_then(|v| i32::try_from(v).ok())
                .ok_or_else(bad)?,
        ),
        (ContentOrder::TitleAsc, Value::String(s)) => {
            validate_text("cursor", &s)?;
            SortKey::Text(s)
        }
        _ => return Err(bad()),
    };
    Ok((key, id))
}

/// One index-friendly part of a content listing (I3). A listing runs its parts in order,
/// each as its own query with `LIMIT` = rows still wanted, until the page is full. Every
/// part's predicate is a row comparison with the same column order as its ORDER BY and
/// its index, so both engines walk the index from the cursor instead of sorting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListPart {
    /// `published_at IS NOT NULL [AND (published_at, id) < (k, id)]`, newest first.
    Published(Option<(i64, Uuid)>),
    /// `published_at IS NULL [AND id < id]`, by id descending: the NULLS LAST group.
    Unpublished(Option<Uuid>),
    /// `[(updated_at, id) < (k, id)]`, newest first.
    Updated(Option<(i64, Uuid)>),
    /// `[(menu_order, id) > (k, id)]`, ascending.
    MenuOrder(Option<(i32, Uuid)>),
    /// `[(title, id) > (k, id)]`, ascending, byte-wise.
    Title(Option<(String, Uuid)>),
}

/// The parts of a listing in `order` that starts after `after` (a decoded cursor).
/// `PublishedDesc` sorts NULL `published_at` last, so it is split into the non-NULL part
/// and the NULL group; a cursor inside the NULL group skips the first part.
pub fn content_list_plan(
    order: ContentOrder,
    after: Option<(SortKey, Uuid)>,
) -> Result<Vec<ListPart>, StoreError> {
    let bad = || StoreError::Invalid("cursor does not match order".into());
    Ok(match (order, after) {
        (ContentOrder::PublishedDesc, None) => {
            vec![ListPart::Published(None), ListPart::Unpublished(None)]
        }
        (ContentOrder::PublishedDesc, Some((SortKey::Micros(Some(k)), id))) => {
            vec![
                ListPart::Published(Some((k, id))),
                ListPart::Unpublished(None),
            ]
        }
        (ContentOrder::PublishedDesc, Some((SortKey::Micros(None), id))) => {
            vec![ListPart::Unpublished(Some(id))]
        }
        (ContentOrder::UpdatedDesc, None) => vec![ListPart::Updated(None)],
        (ContentOrder::UpdatedDesc, Some((SortKey::Micros(Some(k)), id))) => {
            vec![ListPart::Updated(Some((k, id)))]
        }
        (ContentOrder::MenuOrderAsc, None) => vec![ListPart::MenuOrder(None)],
        (ContentOrder::MenuOrderAsc, Some((SortKey::Int(k), id))) => {
            vec![ListPart::MenuOrder(Some((k, id)))]
        }
        (ContentOrder::TitleAsc, None) => vec![ListPart::Title(None)],
        (ContentOrder::TitleAsc, Some((SortKey::Text(k), id))) => {
            vec![ListPart::Title(Some((k, id)))]
        }
        _ => return Err(bad()),
    })
}

/// Encodes a cursor whose sort key is text (redirect source path, user login, content
/// title) under the listing's `tag`.
pub fn encode_text_cursor(tag: &str, key: &str, id: Uuid) -> String {
    Cursor::encode(tag, &json!(key), id)
}

/// Decodes a cursor made by [`encode_text_cursor`] with the same `tag`.
pub fn text_cursor(tag: &str, cursor: &str) -> Result<(String, Uuid), StoreError> {
    match Cursor::decode(cursor, tag)? {
        (Value::String(key), id) => {
            validate_text("cursor", &key)?;
            Ok((key, id))
        }
        _ => Err(StoreError::Invalid(
            "cursor does not match the query order".into(),
        )),
    }
}

/// `CommentRepo::counts` from `(status, count)` rows: every [`CommentStatus`] a key,
/// zero when it has no row; an unknown status is a `Database` error (as when decoding a
/// comment).
pub fn comment_counts(
    rows: impl IntoIterator<Item = (String, i64)>,
) -> Result<BTreeMap<CommentStatus, u64>, StoreError> {
    let mut out: BTreeMap<CommentStatus, u64> =
        CommentStatus::ALL.iter().map(|s| (*s, 0)).collect();
    for (status, n) in rows {
        let status: CommentStatus = status
            .parse()
            .map_err(|e| StoreError::Database(format!("corrupt comment status: {e}")))?;
        let n = u64::try_from(n)
            .map_err(|_| StoreError::Database(format!("negative comment count {n}")))?;
        out.insert(status, n);
    }
    Ok(out)
}

/// The `LIKE … ESCAPE '\'` pattern that matches `q` anywhere in a value: `\`, `%` and
/// `_` in `q` are escaped, so they match only themselves (both backends use the same
/// escape character).
pub fn like_contains(q: &str) -> String {
    let mut out = String::with_capacity(q.len() + 2);
    out.push('%');
    for c in q.chars() {
        if matches!(c, '\\' | '%' | '_') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('%');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn like_contains_escapes_wildcards() {
        assert_eq!(like_contains("50%_off"), "%50\\%\\_off%");
        assert_eq!(like_contains("a\\b"), "%a\\\\b%");
        assert_eq!(like_contains("é"), "%é%");
    }

    #[test]
    fn job_backoff_values() {
        let secs = |a| job_backoff(a).as_secs();
        assert_eq!(secs(1), 20);
        assert_eq!(secs(2), 40);
        assert_eq!(secs(5), 320);
        assert_eq!(secs(9), 3600);
        assert_eq!(secs(30), 3600);
        assert_eq!(secs(0), 10);
        assert_eq!(secs(-3), 10);
        assert_eq!(secs(i32::MAX), 3600);
    }

    #[test]
    fn job_error_is_sanitised() {
        assert_eq!(sanitize_job_error("a\0b"), "a\u{FFFD}b");
        let long = "é".repeat(MAX_JOB_ERROR); // 2 bytes each
        let cut = sanitize_job_error(&format!("x{long}"));
        assert_eq!(cut.len(), MAX_JOB_ERROR - 1);
        assert!(cut.starts_with('x') && !cut.contains('\0'));
        assert_eq!(sanitize_job_error("short"), "short");
    }

    #[test]
    fn job_inputs_are_checked() {
        let ok = NewJob::new("a.b-c_1", json!({}));
        check_new_job(&ok).unwrap();
        for kind in ["", "A", "a b", &"a".repeat(65), "a\0"] {
            assert!(
                check_new_job(&NewJob::new(kind, json!({}))).is_err(),
                "{kind:?}"
            );
        }
        assert!(check_new_job(&NewJob::new("k", json!([]))).is_err());
        assert!(check_new_job(&NewJob::new("k", json!({"a": "\0"}))).is_err());
        let with = |f: fn(&mut NewJob)| {
            let mut j = ok.clone();
            f(&mut j);
            check_new_job(&j)
        };
        assert!(with(|j| j.max_attempts = 0).is_err());
        assert!(with(|j| j.max_attempts = 101).is_err());
        assert!(with(|j| j.unique_key = Some("é".repeat(200))).is_ok());
        assert!(with(|j| j.unique_key = Some("é".repeat(201))).is_err());
        assert!(with(|j| j.unique_key = Some("a\0".into())).is_err());
        let kinds = |n: usize| vec!["k".to_string(); n];
        assert!(check_claim(&kinds(1), 1).is_ok());
        assert!(check_claim(&kinds(0), 1).is_err());
        assert!(check_claim(&kinds(65), 1).is_err());
        assert!(check_claim(&kinds(64), MAX_CLAIM).is_ok());
        assert!(check_claim(&kinds(1), 0).is_err());
        assert!(check_claim(&kinds(1), MAX_CLAIM + 1).is_err());
        assert!(check_claim(&["Bad".to_string()], 1).is_err());
        assert_eq!(
            check_job_duration("lease", Duration::from_secs(1), true).unwrap(),
            1_000_000
        );
        assert!(check_job_duration("lease", Duration::ZERO, true).is_err());
        assert_eq!(
            check_job_duration("delay", Duration::ZERO, false).unwrap(),
            0
        );
        assert!(check_job_duration("delay", MAX_JOB_DURATION * 2, false).is_err());
    }

    #[test]
    fn comment_cursor_and_idempotency_checks() {
        let id = Uuid::now_v7();
        let at = Timestamp::from_micros(1_000).unwrap();
        let c = encode_comment_cursor(at, id);
        assert_eq!(comment_cursor(&c).unwrap(), (1_000, id));
        assert!(audit_cursor(&c).is_err());
        assert!(comment_cursor(&encode_audit_cursor(at, id)).is_err());
        assert!(comment_cursor(&Cursor::encode(COMMENT_CURSOR_TAG, &json!(i64::MIN), id)).is_err());
        assert!(check_comment_page(0, 10).is_err());
        assert_eq!(check_comment_page(500, 2_000).unwrap(), (500, 2_000));
        assert!(check_comment_page(501, 2_000).is_err());
        assert_eq!(check_comment_page(3, 3).unwrap(), (3, 3));
        assert!(check_comment_page(3, 2).is_err());
        assert!(check_comment_page(3, 2_001).is_err());

        let hash = "a".repeat(64);
        check_key_hash(&hash).unwrap();
        for bad in [
            "A".repeat(64),
            "a".repeat(63),
            "g".repeat(64),
            String::new(),
        ] {
            assert!(check_key_hash(&bad).is_err(), "{bad}");
        }
        let fits = json!("x".repeat(MAX_IDEMPOTENCY_RESPONSE - 2));
        assert!(check_idempotency_outcome(&hash, 201, &fits).is_ok());
        let over = json!("x".repeat(MAX_IDEMPOTENCY_RESPONSE - 1));
        assert!(check_idempotency_outcome(&hash, 201, &over).is_err());
        assert!(check_idempotency_outcome(&hash, 99, &json!({})).is_err());
        assert!(check_idempotency_outcome(&hash, 600, &json!({})).is_err());
        assert!(check_idempotency_outcome(&hash, 200, &json!({"a": "\0"})).is_err());
    }

    #[test]
    fn limit_bounds() {
        assert!(check_limit(0).is_err());
        assert_eq!(check_limit(1).unwrap(), 1);
        assert_eq!(check_limit(MAX_PAGE).unwrap(), MAX_PAGE);
        assert!(check_limit(MAX_PAGE + 1).is_err());
    }

    #[test]
    fn cursor_key_must_match_order() {
        let id = Uuid::now_v7();
        let c = Cursor::encode("c:title", &json!("title"), id);
        assert_eq!(
            content_cursor(ContentOrder::TitleAsc, &c).unwrap(),
            (SortKey::Text("title".into()), id)
        );
        assert!(content_cursor(ContentOrder::UpdatedDesc, &c).is_err());
        let null = Cursor::encode("c:published", &Value::Null, id);
        assert_eq!(
            content_cursor(ContentOrder::PublishedDesc, &null)
                .unwrap()
                .0,
            SortKey::Micros(None)
        );
        assert!(content_cursor(ContentOrder::UpdatedDesc, &null).is_err());
        // Same JSON key, other kind of listing.
        let text = encode_text_cursor(REDIRECT_CURSOR_TAG, "title", id);
        assert!(content_cursor(ContentOrder::TitleAsc, &text).is_err());
        assert!(text_cursor(USER_CURSOR_TAG, &text).is_err());
        assert_eq!(
            text_cursor(REDIRECT_CURSOR_TAG, &text).unwrap(),
            ("title".to_owned(), id)
        );
        assert!(content_cursor(ContentOrder::MenuOrderAsc, "garbage!").is_err());
    }

    #[test]
    fn published_plan_splits_off_the_null_group() {
        let id = Uuid::now_v7();
        assert_eq!(
            content_list_plan(ContentOrder::PublishedDesc, None).unwrap(),
            [ListPart::Published(None), ListPart::Unpublished(None)]
        );
        assert_eq!(
            content_list_plan(
                ContentOrder::PublishedDesc,
                Some((SortKey::Micros(Some(5)), id))
            )
            .unwrap(),
            [
                ListPart::Published(Some((5, id))),
                ListPart::Unpublished(None)
            ]
        );
        assert_eq!(
            content_list_plan(
                ContentOrder::PublishedDesc,
                Some((SortKey::Micros(None), id))
            )
            .unwrap(),
            [ListPart::Unpublished(Some(id))]
        );
        assert!(
            content_list_plan(ContentOrder::UpdatedDesc, Some((SortKey::Micros(None), id)))
                .is_err()
        );
        assert!(content_list_plan(ContentOrder::TitleAsc, Some((SortKey::Int(1), id))).is_err());
    }
}
