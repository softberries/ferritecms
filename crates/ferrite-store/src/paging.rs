//! Keyset-pagination helpers shared by the backends so cursors mean the same thing everywhere.

use ferrite_domain::{ContentItem, ContentOrder, Cursor, MAX_PAGE};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::StoreError;

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
        (ContentOrder::TitleAsc, Value::String(s)) => SortKey::Text(s),
        _ => return Err(bad()),
    };
    Ok((key, id))
}

/// Encodes a cursor whose sort key is text (redirect source path, user login).
pub fn encode_text_cursor(key: &str, id: Uuid) -> String {
    Cursor::encode(&json!(key), id)
}

/// Decodes a cursor made by [`encode_text_cursor`].
pub fn text_cursor(cursor: &str) -> Result<(String, Uuid), StoreError> {
    match Cursor::decode(cursor)? {
        (Value::String(key), id) => Ok((key, id)),
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
}
