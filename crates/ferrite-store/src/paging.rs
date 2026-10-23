//! Keyset-pagination helpers shared by the backends so cursors mean the same thing everywhere.

use std::time::Duration;

use ferrite_domain::{
    ContentId, ContentItem, ContentOrder, ContentQuery, Cursor, Job, MAX_PAGE, NewJob,
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

/// Validates an id list: at most [`MAX_ID_FILTER`] entries.
pub fn check_id_list(what: &str, ids: &[ContentId]) -> Result<(), StoreError> {
    if ids.len() > MAX_ID_FILTER {
        return Err(StoreError::Invalid(format!(
            "{what} holds more than {MAX_ID_FILTER} ids"
        )));
    }
    Ok(())
}

/// Validates the filter part of a content query (shared by `list` and `count`).
pub fn check_content_filters(query: &ContentQuery) -> Result<(), StoreError> {
    if let Some(kind) = &query.kind {
        validate_text("content kind", kind)?;
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

#[cfg(test)]
mod tests {
    use super::*;

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
