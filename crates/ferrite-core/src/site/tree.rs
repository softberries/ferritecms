//! The content tree of one type (M4-3b P56): what the theme's `page_tree()` draws.

use ferrite_domain::{ContentId, ContentItem, ContentOrder, MEDIA_KIND};
use serde_json::Value;

use super::{DepKeys, MAX_TREE_NODES, SiteRead, SiteReader, public, published, walk_paths};
use crate::{CoreError, events::archive_key, fields::FEATURED_IMAGE};

/// The public items of one type as flat nodes with parent links, in the store's menu
/// order (`menu_order`, then id). A node's parent may be missing from `nodes` (a
/// draft, trashed or truncated parent); `page_tree()` drops such a node with its
/// subtree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentTree {
    pub kind: String,
    pub nodes: Vec<TreeNode>,
    /// More than [`MAX_TREE_NODES`] public items: `nodes` holds the first ones.
    pub truncated: bool,
}

/// One public item of a [`ContentTree`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeNode {
    pub id: ContentId,
    /// The stored parent, whether or not it is public.
    pub parent: Option<ContentId>,
    pub title: String,
    /// The item's path, exactly as the sitemap lists it.
    pub url: String,
    pub menu_order: i32,
    /// The stored excerpt (empty when none was written; listing cards fall back to the
    /// body, the tree does not render bodies).
    pub excerpt: String,
    /// `fields.featured_image` as stored (a media item id), without a visibility check:
    /// the theme's `image()` returns none for media that is not public.
    pub featured: Option<ContentId>,
}

impl TreeNode {
    fn of(item: ContentItem, url: String) -> Self {
        let featured = item
            .fields
            .get(FEATURED_IMAGE)
            .and_then(Value::as_str)
            .and_then(|s| s.parse().ok());
        TreeNode {
            id: item.id,
            parent: item.parent_id,
            title: item.title,
            url,
            menu_order: item.menu_order,
            excerpt: item.excerpt,
            featured,
        }
    }
}

impl SiteReader {
    /// The public items of `kind` as a tree, with exactly the sitemap's visibility and
    /// paths (M4-3b P56): at most [`MAX_TREE_NODES`] in `menu_order`, read in pages of
    /// 200 rows, `truncated` past that. `None` for media and for a type with no public
    /// items (an unknown type has none).
    ///
    /// Deps are `archive:<kind>` only, as for the sitemap: every content write in the
    /// type emits it, so publishing, moving or reordering an item refreshes every page
    /// that drew the tree.
    pub async fn tree(&self, kind: &str) -> Result<SiteRead<Option<ContentTree>>, CoreError> {
        let now = (self.clock)();
        // One set of rules for the whole read (R10).
        let pinned = self.permalinks.pin();
        let rules = pinned.as_ref();
        let repos = self.store.read();
        let mut deps = DepKeys::default();
        deps.insert(archive_key(kind));
        if kind == MEDIA_KIND {
            return Ok(public(None, deps));
        }
        let mut q = published(now);
        q.kind = Some(kind.to_owned());
        q.order = ContentOrder::MenuOrderAsc;
        let mut nodes = Vec::new();
        walk_paths(repos, rules, &mut q, MAX_TREE_NODES, |item, url| {
            nodes.push(TreeNode::of(item, url));
        })
        .await?;
        if nodes.is_empty() {
            return Ok(public(None, deps));
        }
        Ok(public(
            Some(ContentTree {
                kind: kind.to_owned(),
                nodes,
                truncated: q.cursor.is_some(),
            }),
            deps,
        ))
    }
}
