use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::DomainError;

/// One page of results; `next_cursor` is `None` on the last page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_cursor: Option<String>,
}

/// Opaque pagination cursor: URL-safe base64 (no padding) of JSON `[tag, sort_key, id]`.
/// The `tag` names the kind of listing and sort order the cursor belongs to (for example
/// `c:published` or `s:rank`), so a cursor from one listing is rejected by another.
pub struct Cursor;

impl Cursor {
    pub fn encode(tag: &str, sort_key: &serde_json::Value, id: Uuid) -> String {
        let json = serde_json::json!([tag, sort_key, id]);
        URL_SAFE_NO_PAD.encode(json.to_string())
    }

    /// Decodes a cursor made by [`Cursor::encode`] with the same `tag`; any other shape
    /// or tag is [`DomainError::InvalidCursor`].
    pub fn decode(s: &str, tag: &str) -> Result<(serde_json::Value, Uuid), DomainError> {
        let bytes = URL_SAFE_NO_PAD
            .decode(s)
            .map_err(|_| DomainError::InvalidCursor)?;
        let (found, key, id): (String, serde_json::Value, Uuid) =
            serde_json::from_slice(&bytes).map_err(|_| DomainError::InvalidCursor)?;
        if found != tag {
            return Err(DomainError::InvalidCursor);
        }
        Ok((key, id))
    }
}
