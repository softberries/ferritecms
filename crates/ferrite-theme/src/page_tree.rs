//! The value `page_tree()` returns (M4-3b P57), built in Rust from a flat
//! [`ContentTree`].
//!
//! * **Shape:** `{items, current, ancestors, children, prev, next, truncated}`. Every
//!   node is `{id, title, url, depth, current, open, excerpt, featured, children}`
//!   (`depth` from 0, `open` on the current node and its ancestors, `featured` the
//!   id of a public media item or none, as `SiteReader::tree` checked it).
//! * **Nesting:** `items` holds the roots, each with all its descendants. The other
//!   entries hold nodes with an empty `children`, except `children` (and
//!   `current.children`, the same list), whose nodes keep one level of their own
//!   children. Nested nodes are shared values, built once, so the cost is linear in the
//!   tree, never one copy per path.
//! * **Order:** siblings by [`menu_order_key`] (`menu_order`, title case-insensitively,
//!   id); `prev`/`next` in depth-first pre-order over the whole tree.
//! * **Hidden parents:** a node whose parent is not in the tree is dropped with its
//!   subtree, so it has no place: with it as current, `current`, `prev` and `next` are
//!   none and nothing is open.
//! * **Depth:** a node deeper than [`MAX_PARENT_DEPTH`] (64, the longest parent chain
//!   core writes or walks; its URL would already be cut short) is dropped the same way.
//!   This keeps the nested value shallow enough to build, render and drop on a render
//!   thread's stack whatever the stored rows say.

use std::collections::{BTreeMap, HashMap};

use ferrite_core::{
    MAX_PARENT_DEPTH,
    site::{ContentTree, TreeNode, menu_order_key},
};
use ferrite_domain::ContentId;
use minijinja::value::Value;

/// A node's place in the pre-order walk.
struct Placed {
    /// Index into `ContentTree::nodes`.
    node: usize,
    depth: usize,
    /// Pre-order position of the parent.
    parent: Option<usize>,
    /// Pre-order positions of the children, in sibling order.
    children: Vec<usize>,
}

/// The template value of `tree` with `current` (if it has a place) marked.
pub(crate) fn tree_value(tree: &ContentTree, current: Option<ContentId>) -> Value {
    let nodes = &tree.nodes;
    let pre = preorder(nodes);
    let pos_of: HashMap<ContentId, usize> = pre
        .iter()
        .enumerate()
        .map(|(pos, p)| (nodes[p.node].id, pos))
        .collect();
    let cur = current.and_then(|id| pos_of.get(&id).copied());
    let mut open = vec![false; pre.len()];
    let mut trail = Vec::new();
    let mut up = cur;
    while let Some(pos) = up {
        open[pos] = true;
        trail.push(pos);
        up = pre[pos].parent;
    }
    trail.reverse();

    let node = |pos: usize, children: Value| -> Value {
        let p = &pre[pos];
        let n: &TreeNode = &nodes[p.node];
        let mut m: BTreeMap<&'static str, Value> = BTreeMap::new();
        m.insert("id", Value::from(n.id.to_string()));
        m.insert("title", Value::from(n.title.as_str()));
        m.insert("url", Value::from(n.url.as_str()));
        m.insert("depth", Value::from(p.depth));
        m.insert("current", Value::from(cur == Some(pos)));
        m.insert("open", Value::from(open[pos]));
        m.insert("excerpt", Value::from(n.excerpt.as_str()));
        m.insert(
            "featured",
            n.featured
                .map_or(Value::from(()), |id| Value::from(id.to_string())),
        );
        m.insert("children", children);
        Value::from(m)
    };
    let empty = || Value::from(Vec::<Value>::new());
    let flat = |pos: usize| node(pos, empty());

    // Children come after their parent in pre-order, so a reverse walk builds each
    // subtree once, before its parent needs it.
    let mut nested: Vec<Value> = vec![Value::from(()); pre.len()];
    for pos in (0..pre.len()).rev() {
        let kids: Vec<Value> = pre[pos]
            .children
            .iter()
            .map(|&c| nested[c].clone())
            .collect();
        nested[pos] = node(pos, Value::from(kids));
    }
    let items: Vec<Value> = pre
        .iter()
        .enumerate()
        .filter(|(_, p)| p.parent.is_none())
        .map(|(pos, _)| nested[pos].clone())
        .collect();

    let none = Value::from(());
    let (current_v, ancestors, children, prev, next) = match cur {
        Some(pos) => {
            let children: Vec<Value> = pre[pos]
                .children
                .iter()
                .map(|&c| {
                    let grand: Vec<Value> = pre[c].children.iter().map(|&g| flat(g)).collect();
                    node(c, Value::from(grand))
                })
                .collect();
            let children = Value::from(children);
            let ancestors: Vec<Value> = trail[..trail.len() - 1].iter().map(|&a| flat(a)).collect();
            let prev = pos.checked_sub(1).map_or(none.clone(), flat);
            let next = if pos + 1 < pre.len() {
                flat(pos + 1)
            } else {
                none.clone()
            };
            (
                node(pos, children.clone()),
                Value::from(ancestors),
                children,
                prev,
                next,
            )
        }
        None => (none.clone(), empty(), empty(), none.clone(), none),
    };
    minijinja::context! {
        items => items,
        current => current_v,
        ancestors => ancestors,
        children => children,
        prev => prev,
        next => next,
        truncated => tree.truncated,
    }
}

/// The nodes reachable from the roots within [`MAX_PARENT_DEPTH`] levels, in
/// depth-first pre-order with siblings sorted. A node whose parent is not among `nodes`
/// is unreachable, and so is everything under it (cycles included). Each node is placed
/// at most once even if ids repeat.
fn preorder(nodes: &[TreeNode]) -> Vec<Placed> {
    let mut kids: HashMap<Option<ContentId>, Vec<usize>> = HashMap::new();
    for (i, n) in nodes.iter().enumerate() {
        kids.entry(n.parent).or_default().push(i);
    }
    for list in kids.values_mut() {
        list.sort_by_cached_key(|&i| {
            let n = &nodes[i];
            menu_order_key(n.menu_order, &n.title, n.id)
        });
    }
    let mut placed = vec![false; nodes.len()];
    let mut pre: Vec<Placed> = Vec::with_capacity(nodes.len());
    let mut stack: Vec<(usize, usize, Option<usize>)> = kids
        .get(&None)
        .map(|roots| roots.iter().rev().map(|&r| (r, 0, None)).collect())
        .unwrap_or_default();
    while let Some((i, depth, parent)) = stack.pop() {
        if std::mem::replace(&mut placed[i], true) {
            continue;
        }
        let pos = pre.len();
        pre.push(Placed {
            node: i,
            depth,
            parent,
            children: Vec::new(),
        });
        if let Some(p) = parent {
            pre[p].children.push(pos);
        }
        if depth < MAX_PARENT_DEPTH
            && let Some(list) = kids.get(&Some(nodes[i].id))
        {
            stack.extend(list.iter().rev().map(|&c| (c, depth + 1, Some(pos))));
        }
    }
    pre
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: ContentId, parent: Option<ContentId>, title: &str) -> TreeNode {
        TreeNode {
            id,
            parent,
            title: title.into(),
            url: String::new(),
            menu_order: 0,
            excerpt: String::new(),
            featured: None,
        }
    }

    fn titles(nodes: &[TreeNode]) -> Vec<(String, usize)> {
        preorder(nodes)
            .iter()
            .map(|p| (nodes[p.node].title.clone(), p.depth))
            .collect()
    }

    #[test]
    fn cycles_and_self_parents_are_unreachable() {
        let (a, b, c) = (ContentId::new(), ContentId::new(), ContentId::new());
        let nodes = vec![
            node(a, Some(b), "a"),
            node(b, Some(a), "b"),
            node(c, Some(c), "c"),
            node(ContentId::new(), None, "root"),
        ];
        assert_eq!(titles(&nodes), [("root".to_owned(), 0)]);
    }

    #[test]
    fn repeated_ids_are_placed_once() {
        let x = ContentId::new();
        let nodes = vec![node(x, None, "x"), node(x, Some(x), "y")];
        assert_eq!(titles(&nodes), [("x".to_owned(), 0), ("y".to_owned(), 1)]);
    }

    /// A chain of 5000 (rows no core write makes) keeps its first 65 levels.
    #[test]
    fn a_deep_chain_stops_at_the_parent_depth() {
        let mut nodes = Vec::new();
        let mut ids = Vec::new();
        let mut parent = None;
        for i in 0..5000 {
            let id = ContentId::new();
            nodes.push(node(id, parent, &format!("n{i}")));
            ids.push(id);
            parent = Some(id);
        }
        let pre = preorder(&nodes);
        assert_eq!(pre.len(), MAX_PARENT_DEPTH + 1);
        assert_eq!(pre[MAX_PARENT_DEPTH].depth, MAX_PARENT_DEPTH);
        let tree = ContentTree {
            kind: "page".into(),
            nodes,
            truncated: false,
            media: Default::default(),
        };
        let deepest = tree_value(&tree, Some(ids[MAX_PARENT_DEPTH]));
        assert_eq!(
            deepest.get_attr("ancestors").unwrap().len(),
            Some(MAX_PARENT_DEPTH)
        );
        assert!(deepest.get_attr("next").unwrap().is_none());
        let below = tree_value(&tree, Some(ids[MAX_PARENT_DEPTH + 1]));
        assert!(below.get_attr("current").unwrap().is_none());
    }
}
