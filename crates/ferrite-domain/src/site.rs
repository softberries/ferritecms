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
    /// Time of the last counted hit (`RedirectRepo::add_hits`, never moved back); `None` if never hit.
    pub last_hit_at: Option<Timestamp>,
}

/// A redirect listing: by source (binary order), optionally searched.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RedirectQuery {
    /// A case-insensitive substring of `source_path` or `target`; `%`, `_` and `\`
    /// match only themselves. Core trims it and allows 1..=200 characters.
    pub q: Option<String>,
    /// 1..=[`MAX_PAGE`](crate::MAX_PAGE).
    pub limit: u32,
    pub cursor: Option<String>,
}
