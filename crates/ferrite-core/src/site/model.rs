//! What [`SiteReader`](super::SiteReader) returns.

use std::collections::BTreeSet;

use ferrite_domain::{ContentId, TermId, Timestamp, UserId};
use serde::Serialize;

/// A read result plus the dependency keys it touched (renderer side of the contract in
/// [`crate::events`]).
#[derive(Debug, Clone, PartialEq)]
pub struct SiteRead<T> {
    pub value: T,
    pub deps: DepKeys,
    /// `false` for previews: their response must never be cached.
    pub cacheable: bool,
}

/// A set of dependency keys (`post:<id>`, `term:<id>`, `archive:<kind>`,
/// `option:<key>`), kept sorted and deduplicated.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DepKeys(BTreeSet<String>);

impl DepKeys {
    pub fn insert(&mut self, key: String) {
        self.0.insert(key);
    }

    pub fn contains(&self, key: &str) -> bool {
        self.0.contains(key)
    }

    pub fn iter(&self) -> impl Iterator<Item = &str> {
        self.0.iter().map(String::as_str)
    }

    pub fn extend(&mut self, other: DepKeys) {
        self.0.extend(other.0);
    }

    /// The keys in sorted order.
    pub fn into_vec(self) -> Vec<String> {
        self.0.into_iter().collect()
    }
}

/// The public view of a content item: everything a template may see of the row.
/// No body (the rendered HTML is [`PageItem::html`]), status, version, scheduling or
/// author id.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PublicItem {
    pub id: ContentId,
    pub kind: String,
    pub slug: String,
    pub title: String,
    /// The stored excerpt, or one derived from the body when it is empty.
    pub excerpt: String,
    pub published_at: Option<Timestamp>,
    pub updated_at: Timestamp,
    pub menu_order: i32,
    pub parent_id: Option<ContentId>,
    /// Custom fields, filtered: `featured_image` is kept only if it resolved to public
    /// media, and UUID strings naming non-public content items are removed.
    pub fields: serde_json::Value,
}

/// A content item ready for a template.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PageItem {
    pub item: PublicItem,
    /// Public path (canonical, trailing slash).
    pub url: String,
    /// The rendered block body; `ferrite:media/<id>` images point at published media,
    /// other media references are dropped.
    ///
    /// **Invariant:** sanitised HTML (the block renderer escapes text and imported
    /// `html` blocks are sanitised). Themes print it unescaped (`post.body` is marked
    /// safe), so any later transformation (plugin `content.body` filters included)
    /// must re-sanitise before the result is stored here.
    pub html: String,
    /// `None` only if the author row is gone.
    pub author: Option<UserPublic>,
    /// In stored position order.
    pub terms: Vec<TermPublic>,
    /// From `fields.featured_image` (a media item id); `None` when unset, invalid or
    /// not a published media item.
    pub featured: Option<MediaPublic>,
}

/// The public face of a user: nothing beyond these three fields leaves core.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UserPublic {
    pub id: UserId,
    pub login: String,
    pub display_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TermPublic {
    pub id: TermId,
    pub taxonomy: String,
    pub slug: String,
    pub name: String,
    /// Term archive path, `/{taxonomy}/{slug}/`.
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MediaPublic {
    /// The media item's id.
    pub id: ContentId,
    pub url: String,
    pub alt: String,
    pub width: Option<i32>,
    pub height: Option<i32>,
    /// Generated sizes, sorted by name.
    pub variants: Vec<MediaVariant>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MediaVariant {
    pub name: String,
    pub url: String,
    pub width: Option<i32>,
    pub height: Option<i32>,
}

/// One page of a public listing.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Listing {
    pub items: Vec<PageItem>,
    /// 1-based.
    pub page: u32,
    pub per_page: u32,
    /// Matching items over all pages (for search: matches found, see
    /// [`SiteReader::search`](super::SiteReader::search)).
    pub total: u64,
}

/// What the site root shows.
#[derive(Debug, Clone, PartialEq)]
pub enum FrontPage {
    /// `reading.front_page` names a published item.
    Page(Box<PageItem>),
    /// The post listing.
    Posts(Listing),
}

/// Which items a feed lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FeedSource {
    /// Latest items of a content type.
    Kind(String),
    /// Latest posts with a term.
    Term { taxonomy: String, slug: String },
}

/// One sitemap file.
#[derive(Debug, Clone, PartialEq)]
pub struct SitemapPage {
    pub entries: Vec<SitemapEntry>,
    /// Pass back to get the next file; `None` on the last one.
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SitemapEntry {
    pub url: String,
    /// The item's `updated_at`.
    pub lastmod: Timestamp,
}
