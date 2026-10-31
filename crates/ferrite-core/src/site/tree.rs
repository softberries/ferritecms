//! The content tree of one type (M4-3b P56): what the theme's `page_tree()` draws.

use std::collections::{HashMap, HashSet};

use ferrite_domain::{ContentId, ContentItem, ContentOrder, MEDIA_KIND};
use ferrite_store::MAX_ID_FILTER;
use serde_json::Value;

use super::{
    DepKeys, FRONT_PAGE_OPTION, MAX_TREE_NODES, MediaPublic, SiteRead, SiteReader, front_page_id,
    hydrate::{Hydrator, unique},
    is_public, public, published, walk_paths,
};
use crate::{
    CoreError,
    events::{archive_key, option_key, post_key},
    fields::FEATURED_IMAGE,
};

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
    /// The public media the nodes' `featured` ids name, as
    /// [`SiteReader::media`] gives it, so the theme's `image()` on a node costs no
    /// bridge call (M4-4). A featured id without a media row has no entry.
    pub media: HashMap<ContentId, MediaPublic>,
}

/// One public item of a [`ContentTree`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeNode {
    pub id: ContentId,
    /// The stored parent, whether or not it is public.
    pub parent: Option<ContentId>,
    pub title: String,
    /// The item's path, exactly as the sitemap lists it; `/` for the static front page
    /// (`reading.front_page`, M4-4 P67), whose own permalink redirects there.
    pub url: String,
    pub menu_order: i32,
    /// The stored excerpt only: empty when none was written, with no fallback to the
    /// body (the tree does not render bodies) and no plugin `content.excerpt` filter.
    /// Items from `query()` or `related()` carry the stored excerpt or, without one, a
    /// text taken from the body; that filter runs only on the page's own item and the
    /// items of its listing (06 §5), never on these.
    pub excerpt: String,
    /// `fields.featured_image` (a media item id) when it names a public media item
    /// (published, `published_at <= now`, of kind `media`), else `None`: the public
    /// view never shows the id of a draft or trashed item (R8). The theme's `image()`
    /// resolves it.
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
    /// The static front page's node (`reading.front_page`) has the URL `/` (M4-4 P67);
    /// its children keep their paths.
    ///
    /// Deps are `archive:<kind>`, as for the sitemap: every content write in the type
    /// emits it, so publishing, moving or reordering an item refreshes every page that
    /// drew the tree. Plus `option:reading.front_page`, and `post:<media>` for every
    /// featured image id checked, public or not, so publishing or unpublishing that
    /// media refreshes the page too. The featured ids are checked in one `get_many` per
    /// 200 ids, and the media rows of the public ones are read the same way into
    /// [`ContentTree::media`].
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
        deps.insert(option_key(FRONT_PAGE_OPTION));
        let front = front_page_id(repos).await?;
        let mut q = published(now);
        q.kind = Some(kind.to_owned());
        q.order = ContentOrder::MenuOrderAsc;
        let mut nodes = Vec::new();
        walk_paths(repos, rules, &mut q, MAX_TREE_NODES, |item, url| {
            let url = if front == Some(item.id) {
                "/".to_owned()
            } else {
                url
            };
            nodes.push(TreeNode::of(item, url));
        })
        .await?;
        if nodes.is_empty() {
            return Ok(public(None, deps));
        }
        let hydrator = self.hydrator(repos, now, rules);
        let media = public_featured(&hydrator, &mut nodes, &mut deps).await?;
        Ok(public(
            Some(ContentTree {
                kind: kind.to_owned(),
                nodes,
                truncated: q.cursor.is_some(),
                media,
            }),
            deps,
        ))
    }
}

/// Keeps each node's `featured` only when it names a public media item (as hydration
/// keeps `featured_image`, R8), checked in one `get_many` per [`MAX_ID_FILTER`] ids, and
/// records `post:<id>` for every id checked. Returns the public ones' media, read in
/// one `media().get_many` per [`MAX_ID_FILTER`] ids.
async fn public_featured(
    hydrator: &Hydrator<'_>,
    nodes: &mut [TreeNode],
    deps: &mut DepKeys,
) -> Result<HashMap<ContentId, MediaPublic>, CoreError> {
    let repos = hydrator.repos;
    let ids = unique(nodes.iter().filter_map(|n| n.featured));
    let mut shown = HashSet::new();
    for chunk in ids.chunks(MAX_ID_FILTER) {
        for item in repos.content().get_many(chunk).await? {
            if item.kind == MEDIA_KIND && is_public(&item, hydrator.now) {
                shown.insert(item.id);
            }
        }
    }
    for id in ids {
        deps.insert(post_key(id));
    }
    for n in nodes {
        n.featured = n.featured.filter(|id| shown.contains(id));
    }
    let public_ids: Vec<ContentId> = shown.into_iter().collect();
    let mut media = HashMap::new();
    for chunk in public_ids.chunks(MAX_ID_FILTER) {
        for m in repos.media().get_many(chunk).await? {
            media.insert(m.item_id, hydrator.media_public(&m));
        }
    }
    Ok(media)
}
