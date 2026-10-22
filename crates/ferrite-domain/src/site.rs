use serde::{Deserialize, Serialize};

use crate::{RedirectId, Timestamp, string_enum};

/// HTTP status codes a redirect may use.
pub const REDIRECT_CODES: [u16; 5] = [301, 302, 307, 308, 410];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OptionEntry {
    pub key: String,
    pub value: serde_json::Value,
    pub autoload: bool,
    pub updated_at: Timestamp,
}

string_enum!(
    RedirectOrigin {
        Manual => "manual",
        Import => "import",
        SlugChange => "slug-change",
    }
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Redirect {
    pub id: RedirectId,
    pub source_path: String,
    pub target: String,
    /// One of [`REDIRECT_CODES`].
    pub code: u16,
    pub hits: i64,
    pub origin: RedirectOrigin,
    pub created_at: Timestamp,
    /// When `RedirectRepo::hit` last ran for it; `None` if never hit.
    pub last_hit_at: Option<Timestamp>,
}
