//! The data-backed template helpers (plan ruling P2).

use ferrite_core::{DepKeys, MediaPublic, PageItem, comments::CommentThread, site::srcset};
use ferrite_domain::ContentId;
use serde::Serialize;

/// Most items `query()` may ask for (`SiteReader::term_items` caps at the same).
pub const MAX_QUERY_LIMIT: u32 = ferrite_core::site::MAX_QUERY_LIMIT;
/// Most items `related()` may ask for (`SiteReader::related` caps at the same).
pub const MAX_RELATED: u32 = ferrite_core::site::MAX_RELATED as u32;

/// A helper result plus the dependency keys the bridge's reads touched.
pub type Bridged<T> = (T, DepKeys);

/// Source of the helpers that need the store. Synchronous: MiniJinja renders on a
/// blocking thread, and implementations may block on the async store
/// (`Handle::block_on` inside `spawn_blocking`). The renderer caps calls per render
/// (see [`Renderer`](crate::Renderer)) and merges the returned keys into the page's.
///
/// **Published-only.** Implementations must read through `SiteReader` (or apply the
/// same published-and-`published_at <= now` rule) for every item they return or
/// reference.
///
/// **Dependency keys** follow the contract in `ferrite_core::events`: everything the
/// read touched, including what it filtered out, so that publishing a hidden item
/// refreshes the page. The renderer additionally records the keys of every returned
/// item (`post:`, `user:`, `term:`, featured media), so the bridge may leave those out.
pub trait HelperBridge: Send + Sync {
    /// Up to `n` (1..=[`MAX_RELATED`]) public items related to `item`. Empty unless
    /// `item` itself is public (loaded through `SiteReader`, published-only). Keys:
    /// at least `archive:<kind>` and `post:<item>`, even when the result is empty.
    fn related(&self, item: ContentId, n: u32) -> Bridged<Vec<PageItem>>;
    /// Up to `limit` (1..=[`MAX_QUERY_LIMIT`]) public items of `kind`, newest first;
    /// `term` narrows to one term, written `"<taxonomy>/<slug>"`. Keys: at least
    /// `archive:<kind>`, plus `term:<id>` for the term when it exists.
    fn query(&self, kind: &str, term: Option<&str>, limit: u32) -> Bridged<Vec<PageItem>>;
    /// The menu at location `name`, with links to non-public items removed. Keys:
    /// `menu:<name>`, plus `post:<id>`/`term:<id>` for **every** target, including
    /// removed (draft, trashed, missing) ones.
    fn menu(&self, name: &str) -> Bridged<Option<MenuTree>>;
    /// The `size` variant of a public media item (the original for an unknown size).
    /// The renderer records `post:<media>`.
    fn image(&self, media: ContentId, size: &str) -> Option<ImageRef>;
    /// Page `page` (1-based, clamped to the last) of the public comment thread of
    /// `item` (`SiteReader::comments`): `None` unless `item` is a public post. Approved
    /// comments only, never an e-mail or address hash. Keys: `post:<item>`,
    /// `option:comments.enabled` and `option:comments.per_page`. The default has no
    /// data (bridges without comments, such as error pages and tests).
    fn comments(&self, _item: ContentId, _page: u32) -> Bridged<Option<CommentThread>> {
        (None, DepKeys::default())
    }
}

/// A bridge with no data: every helper returns nothing. For error pages and tests.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoBridge;

impl HelperBridge for NoBridge {
    fn related(&self, _: ContentId, _: u32) -> Bridged<Vec<PageItem>> {
        (Vec::new(), DepKeys::default())
    }
    fn query(&self, _: &str, _: Option<&str>, _: u32) -> Bridged<Vec<PageItem>> {
        (Vec::new(), DepKeys::default())
    }
    fn menu(&self, _: &str) -> Bridged<Option<MenuTree>> {
        (None, DepKeys::default())
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

impl ImageRef {
    /// The `size` variant of `m`, or the original for an unknown size. `srcset` lists
    /// one candidate per known width, ascending: of several files with the same width
    /// (a `hero` crop and the `large` resize can both be 1280 wide), the requested
    /// `size` wins, then the original, then the variant first by name.
    pub fn from_media(m: &MediaPublic, size: &str) -> Self {
        let srcset = srcset(&m.srcset_candidates(Some(size)));
        let (url, width, height) = m
            .variants
            .iter()
            .find(|v| v.name == size)
            .map_or((&m.url, m.width, m.height), |v| (&v.url, v.width, v.height));
        ImageRef {
            url: url.clone(),
            width,
            height,
            srcset,
        }
    }
}

#[cfg(test)]
mod tests {
    use ferrite_core::MediaVariant;

    use super::*;

    fn variant(name: &str, width: i32) -> MediaVariant {
        MediaVariant {
            name: name.into(),
            url: format!("/_media/{name}/a.png"),
            width: Some(width),
            height: None,
        }
    }

    #[test]
    fn srcset_has_one_candidate_per_width() {
        let m = MediaPublic {
            id: ContentId::new(),
            url: "/_media/orig/a.png".into(),
            alt: String::new(),
            width: Some(1280),
            height: Some(720),
            variants: vec![
                variant("card", 640),
                variant("hero", 1280),
                variant("large", 1280),
                variant("medium", 768),
                variant("other", 640),
            ],
        };
        // The requested size wins its width.
        let r = ImageRef::from_media(&m, "hero");
        assert_eq!(r.url, "/_media/hero/a.png");
        assert_eq!(
            r.srcset,
            "/_media/card/a.png 640w, /_media/medium/a.png 768w, /_media/hero/a.png 1280w"
        );
        // Otherwise the original wins over variants of its width.
        let r = ImageRef::from_media(&m, "card");
        assert_eq!(
            r.srcset,
            "/_media/card/a.png 640w, /_media/medium/a.png 768w, /_media/orig/a.png 1280w"
        );
        // Among variants only, the first by name.
        let r = ImageRef::from_media(&m, "nope");
        assert!(
            r.srcset.starts_with("/_media/card/a.png 640w, "),
            "{}",
            r.srcset
        );
    }
}
