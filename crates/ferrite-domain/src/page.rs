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

/// Opaque pagination cursor: URL-safe base64 (no padding) of JSON `[sort_key, id]`.
pub struct Cursor;

impl Cursor {
    pub fn encode(sort_key: &serde_json::Value, id: Uuid) -> String {
        let json = serde_json::json!([sort_key, id]);
        URL_SAFE_NO_PAD.encode(json.to_string())
    }

    pub fn decode(s: &str) -> Result<(serde_json::Value, Uuid), DomainError> {
        let bytes = URL_SAFE_NO_PAD
            .decode(s)
            .map_err(|_| DomainError::InvalidCursor)?;
        serde_json::from_slice(&bytes).map_err(|_| DomainError::InvalidCursor)
    }
}
