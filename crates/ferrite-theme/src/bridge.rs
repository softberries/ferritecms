//! The data-backed template helpers (plan ruling P2).

use ferrite_core::PageItem;
use ferrite_domain::{ContentId, TermId};
use serde::Serialize;

/// Most items `query()` may ask for.
pub const MAX_QUERY_LIMIT: u32 = 50;
/// Most items `related()` may ask for (`SiteReader::related` caps at the same).
pub const MAX_RELATED: u32 = ferrite_core::site::MAX_RELATED as u32;

/// Source of the helpers that need the store. Synchronous: MiniJinja renders on a
/// blocking thread, and implementations may block on the async store
/// (`Handle::block_on` inside `spawn_blocking`). Every method must return public
/// (published-only) data. The renderer caps calls per render (see
/// [`Renderer`](crate::Renderer)) and records dependency keys for what is returned.
pub trait HelperBridge: Send + Sync {
    /// Up to `n` (1..=[`MAX_RELATED`]) public items related to `item`.
    fn related(&self, item: ContentId, n: u32) -> Vec<PageItem>;
    /// Up to `limit` (1..=[`MAX_QUERY_LIMIT`]) public items of `kind`, newest first;
    /// `term` narrows to one term, written `"<taxonomy>/<slug>"`.
    fn query(&self, kind: &str, term: Option<&str>, limit: u32) -> Vec<PageItem>;
    /// The menu at location `name`, with links to non-public items removed.
    fn menu(&self, name: &str) -> Option<MenuTree>;
    /// The `size` variant of a public media item (the original for an unknown size).
    fn image(&self, media: ContentId, size: &str) -> Option<ImageRef>;
}

/// A bridge with no data: every helper returns nothing. For error pages and tests.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoBridge;

impl HelperBridge for NoBridge {
    fn related(&self, _: ContentId, _: u32) -> Vec<PageItem> {
        Vec::new()
    }
    fn query(&self, _: &str, _: Option<&str>, _: u32) -> Vec<PageItem> {
        Vec::new()
    }
    fn menu(&self, _: &str) -> Option<MenuTree> {
        None
    }
    fn image(&self, _: ContentId, _: &str) -> Option<ImageRef> {
        None
    }
}

/// A menu as templates see it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MenuTree {
    pub items: Vec<MenuNode>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MenuNode {
    pub label: String,
    pub url: String,
    pub children: Vec<MenuNode>,
    /// What the entry links to, for dependency keys; not shown to templates.
    #[serde(skip)]
    pub target: Option<MenuTarget>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuTarget {
    Item(ContentId),
    Term(TermId),
}

/// An image variant for `<img>`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ImageRef {
    pub url: String,
    pub width: Option<i32>,
    pub height: Option<i32>,
    /// `url 640w, url 1280w`, from the variants with a known width; may be empty.
    pub srcset: String,
}
