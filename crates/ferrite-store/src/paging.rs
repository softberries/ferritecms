//! Keyset-pagination helpers shared by the backends so cursors mean the same thing everywhere.

use ferrite_domain::{ContentId, ContentItem, ContentOrder, ContentQuery, Cursor, MAX_PAGE};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::{StoreError, validate_text};

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

/// The sort key of `item` under `order`.
pub fn content_sort_key(order: ContentOrder, item: &ContentItem) -> SortKey {
    match order {
        ContentOrder::PublishedDesc => SortKey::Micros(item.published_at.map(|t| t.as_micros())),
        ContentOrder::UpdatedDesc => SortKey::Micros(Some(item.updated_at.as_micros())),
        ContentOrder::MenuOrderAsc => SortKey::Int(item.menu_order),
        ContentOrder::TitleAsc => SortKey::Text(item.title.clone()),
    }
}

/// Encodes the cursor pointing just after `item`.
pub fn encode_content_cursor(order: ContentOrder, item: &ContentItem) -> String {
    let key = match content_sort_key(order, item) {
        SortKey::Micros(m) => json!(m),
        SortKey::Int(i) => json!(i),
        SortKey::Text(t) => json!(t),
    };
    Cursor::encode(&key, item.id.as_uuid())
}

/// Decodes a cursor for `order`; a malformed cursor or one of another order is `Invalid`.
pub fn content_cursor(order: ContentOrder, cursor: &str) -> Result<(SortKey, Uuid), StoreError> {
    let (key, id) = Cursor::decode(cursor)?;
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

/// Encodes a cursor whose sort key is text (redirect source path, user login).
pub fn encode_text_cursor(key: &str, id: Uuid) -> String {
    Cursor::encode(&json!(key), id)
}

/// Decodes a cursor made by [`encode_text_cursor`].
pub fn text_cursor(cursor: &str) -> Result<(String, Uuid), StoreError> {
    match Cursor::decode(cursor)? {
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
    fn limit_bounds() {
        assert!(check_limit(0).is_err());
        assert_eq!(check_limit(1).unwrap(), 1);
        assert_eq!(check_limit(MAX_PAGE).unwrap(), MAX_PAGE);
        assert!(check_limit(MAX_PAGE + 1).is_err());
    }

    #[test]
    fn cursor_key_must_match_order() {
        let id = Uuid::now_v7();
        let c = Cursor::encode(&json!("title"), id);
        assert_eq!(
            content_cursor(ContentOrder::TitleAsc, &c).unwrap(),
            (SortKey::Text("title".into()), id)
        );
        assert!(content_cursor(ContentOrder::UpdatedDesc, &c).is_err());
        let null = Cursor::encode(&Value::Null, id);
        assert_eq!(
            content_cursor(ContentOrder::PublishedDesc, &null)
                .unwrap()
                .0,
            SortKey::Micros(None)
        );
        assert!(content_cursor(ContentOrder::UpdatedDesc, &null).is_err());
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
