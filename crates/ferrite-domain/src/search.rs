use serde::{Deserialize, Serialize};

use crate::{ContentStatus, ReadableBy};

/// A filtered, paginated full-text search.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchQuery {
    /// Free text; it has no syntax (see `SearchRepo`).
    pub q: String,
    /// Only items of these content types; empty means all types.
    pub kinds: Vec<String>,
    /// Only items in these statuses; empty means all statuses.
    pub statuses: Vec<ContentStatus>,
    /// Only items publicly visible at `now` or authored by the user (see
    /// [`ContentQuery::readable_by`](crate::ContentQuery::readable_by)); `None`: no such
    /// filter.
    pub readable_by: Option<ReadableBy>,
    /// Only items in this locale; `None`: any.
    pub locale: Option<String>,
    /// Page size, 1 through [`crate::MAX_PAGE`].
    pub limit: u32,
    /// Opaque cursor from a previous page's `next_cursor`.
    pub cursor: Option<String>,
}
