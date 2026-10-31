//! What [`SiteReader`](super::SiteReader) returns.

use std::collections::BTreeSet;

use ferrite_domain::{ContentId, ContentStatus, TermId, Timestamp, UserId};
use serde::Serialize;

use crate::blocks::TocEntry;

/// A read result plus the dependency keys it touched (renderer side of the contract in
/// [`crate::events`]).
#[derive(Debug, Clone, PartialEq)]
pub struct SiteRead<T> {
    pub value: T,
    pub deps: DepKeys,
    /// `false` for previews: their response must never be cached.
    pub cacheable: bool,
}

impl<T> SiteRead<T> {
    /// The same read with its value mapped; keys and cacheability are kept.
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> SiteRead<U> {
        SiteRead {
            value: f(self.value),
            deps: self.deps,
            cacheable: self.cacheable,
        }
    }
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
    /// The body's headings in document order, as rendered into [`html`](Self::html)
    /// (`post.toc` in templates; M4-3b P55). Computed with the body: a plugin
    /// `content.body` filter that changes the HTML leaves it as it was, so a filter that
    /// removes a heading leaves a dead entry.
    pub toc: Vec<TocEntry>,
    /// `None` only if the author row is gone.
    pub author: Option<UserPublic>,
    /// In stored position order.
    pub terms: Vec<TermPublic>,
    /// From `fields.featured_image` (a media item id); `None` when unset, invalid or
    /// not a published media item.
    pub featured: Option<MediaPublic>,
}

/// An item loaded by [`SiteReader::preview`](super::SiteReader::preview): the only way
/// to get one (private fields, no public constructor), so code that requires a
/// `Previewed` (the plugin preview scope in `ferrite-http`) can only be handed an item
/// whose `can_read` check passed (R17).
#[derive(Debug, Clone, PartialEq)]
pub struct Previewed {
    item: PageItem,
    status: ContentStatus,
}

impl Previewed {
    pub(crate) fn new(item: PageItem, status: ContentStatus) -> Self {
        Self { item, status }
    }

    pub fn item(&self) -> &PageItem {
        &self.item
    }

    /// The item's real status (`PageItem` has none, ruling R8).
    pub fn status(&self) -> ContentStatus {
        self.status
    }

    pub fn into_item(self) -> PageItem {
        self.item
    }
}

/// Title and URL of a public item, for links (menus) that show nothing else.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ItemLink {
    pub title: String,
    pub url: String,
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

impl MediaPublic {
    /// The `srcset` candidates of the item: one `(width, url)` per known width,
    /// ascending. With `shape` (a `(width, height)`), only the files of that aspect
    /// ratio (up to the rounding of a resize) are candidates, the original included
    /// only when it has that shape too, so a browser never swaps a crop for a different
    /// picture; a file whose height is unknown is then left out. Without `shape` every
    /// file with a known width is. Of several files with the same width (a `hero` crop and the
    /// `large` resize can both be 1280 wide), the variant named `preferred` wins, then
    /// the original, then the variant first by name. The theme's `image()` helper and
    /// inline body images (06 §4) both use it.
    pub fn srcset_candidates(
        &self,
        shape: Option<(i32, i32)>,
        preferred: Option<&str>,
    ) -> Vec<(i32, &str)> {
        let fits = |w: Option<i32>, h: Option<i32>| shape.is_none_or(|s| same_shape(s, w.zip(h)));
        // (width, rank, url): lower rank wins a width.
        let mut candidates: Vec<(i32, u8, &str)> = self
            .variants
            .iter()
            .filter(|v| fits(v.width, v.height))
            .filter_map(|v| {
                let rank = if Some(v.name.as_str()) == preferred {
                    0
                } else {
                    2
                };
                v.width.map(|w| (w, rank, v.url.as_str()))
            })
            .collect();
        if let Some(w) = self.width
            && fits(self.width, self.height)
        {
            candidates.push((w, 1, self.url.as_str()));
        }
        // Stable: equal (width, rank) keep the variants' name order.
        candidates.sort_by_key(|&(w, rank, _)| (w, rank));
        candidates.dedup_by_key(|c| c.0);
        candidates.into_iter().map(|(w, _, url)| (w, url)).collect()
    }
}

/// Whether a file of `size` has the aspect ratio of `shape`, up to the rounding of a
/// resize (`|w·sh − h·sw| ≤ max(sw, sh)`); a file whose size is unknown does not.
fn same_shape(shape: (i32, i32), size: Option<(i32, i32)>) -> bool {
    let Some((w, h)) = size else {
        return false;
    };
    let (sw, sh) = (i64::from(shape.0), i64::from(shape.1));
    let (w, h) = (i64::from(w), i64::from(h));
    w > 0 && h > 0 && (w * sh - h * sw).abs() <= sw.max(sh)
}

/// `srcset` text for `candidates`: `url 640w, url 1280w`.
pub fn srcset(candidates: &[(i32, &str)]) -> String {
    candidates
        .iter()
        .map(|(w, url)| format!("{url} {w}w"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// A stored file that `/_media/<hash>/<name>` may serve: the original or a variant of
/// a public media item ([`SiteReader::media_file`](super::SiteReader::media_file)).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaFile {
    /// The media item.
    pub item: ContentId,
    /// The variant's name, `None` for the original.
    pub variant: Option<String>,
    /// MIME type sniffed at upload (the variant's own when it records one).
    pub mime: String,
    /// The item's `original_name`: the file name in every URL of the item, variants
    /// included.
    pub name: String,
    /// The original is the SVG sanitiser's output (`media.sanitized`); always `false`
    /// for a variant (variants are raster images).
    pub sanitized: bool,
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

#[cfg(test)]
mod tests {
    use super::same_shape;

    #[test]
    fn same_shape_allows_rounding_only() {
        // A 4:3 original: resizes (rounded either way) match, crops do not.
        assert!(same_shape((2000, 1500), Some((768, 576))));
        assert!(same_shape((2000, 1500), Some((1001, 751))));
        assert!(same_shape((1999, 1333), Some((768, 512))));
        assert!(!same_shape((2000, 1500), Some((1280, 640))));
        assert!(!same_shape((2000, 1500), Some((150, 150))));
        assert!(!same_shape((2000, 1500), Some((640, 360))));
        assert!(!same_shape((2000, 1500), None));
        assert!(!same_shape((2000, 1500), Some((0, 0))));
    }
}
