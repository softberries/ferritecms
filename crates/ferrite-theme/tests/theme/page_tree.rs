//! `page_tree()` (M4-3b P57) over a fixed tree from the bridge, and `query(order=)`
//! (P58).

use std::sync::{Arc, atomic::Ordering};

use ferrite_core::{
    PageItem,
    events::archive_key,
    site::{ContentTree, ListOrder, TreeNode},
};
use ferrite_domain::ContentId;
use ferrite_theme::{MAX_BRIDGE_CALLS, RenderContext, RenderOutput, Renderer, ThemeError};

use crate::common::{FakeBridge, ctx, item, theme};

fn node(title: &str, parent: Option<ContentId>, menu_order: i32) -> TreeNode {
    TreeNode {
        id: ContentId::new(),
        parent,
        title: title.into(),
        url: format!("/{}/", title.to_lowercase()),
        menu_order,
        excerpt: format!("About {title}"),
        featured: None,
    }
}

fn tree(nodes: Vec<TreeNode>) -> ContentTree {
    ContentTree {
        kind: "page".into(),
        nodes,
        truncated: false,
        media: Default::default(),
    }
}

fn bridge(t: ContentTree) -> Arc<FakeBridge> {
    Arc::new(FakeBridge {
        tree: Some(t),
        ..FakeBridge::default()
    })
}

/// A page item with the tree node `n`'s id as `post`.
fn current(n: &TreeNode) -> RenderContext {
    let mut it: PageItem = item("page", &n.title);
    it.item.id = n.id;
    let mut c = ctx();
    c.item = Some(it);
    c
}

fn try_render(
    src: &str,
    c: RenderContext,
    bridge: Arc<FakeBridge>,
) -> Result<RenderOutput, ThemeError> {
    let t = theme(&[("templates/t.html", src)]);
    Renderer::new().render(&t, "t.html", c, bridge)
}

fn render(src: &str, c: RenderContext, bridge: Arc<FakeBridge>) -> String {
    try_render(src, c, bridge).unwrap().html
}

/// Every node of `t.items`, pre-order, as `title(depth)` plus `*` when open and `!`
/// when current.
const WALK: &str = "{% for n in t.items recursive %}{{ n.title }}({{ n.depth }}){% if n.open %}*{% endif %}{% if n.current %}!{% endif %} {{ loop(n.children) }}{% endfor %}";

#[test]
fn nests_and_marks_the_trail() {
    let root = node("Root", None, 0);
    let parent = node("Parent", Some(root.id), 0);
    let sibling = node("Sibling", Some(root.id), 1);
    let cur = node("Current", Some(parent.id), 0);
    let k1 = node("K1", Some(cur.id), 0);
    let k2 = node("K2", Some(cur.id), 1);
    let gk = node("GK", Some(k1.id), 0);
    let other = node("Other", None, 1);
    let c = current(&cur);
    let b = bridge(tree(vec![
        other.clone(),
        gk,
        k2,
        k1,
        cur.clone(),
        sibling,
        parent,
        root,
    ]));
    let html = render(
        &format!(
            "{{% set t = page_tree(post) %}}\
             A:{{% for a in t.ancestors %}}{{{{ a.title }}}}/{{{{ a.depth }}}}/{{{{ a.open }}}};{{% endfor %}}\
             C:{{{{ t.current.title }}}}/{{{{ t.current.depth }}}}/{{{{ t.current.url }}}}\
             K:{{% for k in t.children %}}{{{{ k.title }}}}/{{{{ k.depth }}}}[{{% for g in k.children %}}{{{{ g.title }}}}{{% endfor %}}]{{{{ k.excerpt }}}};{{% endfor %}}\
             T:{{{{ t.truncated }}}}\
             W:{WALK}"
        ),
        c,
        b.clone(),
    );
    assert_eq!(
        html,
        "A:Root/0/True;Parent/1/True;\
         C:Current/2/&#x2f;current&#x2f;\
         K:K1/3[GK]About K1;K2/3[]About K2;\
         T:False\
         W:Root(0)* Parent(1)* Current(2)*! K1(3) GK(4) K2(3) Sibling(1) Other(0) "
    );
    assert_eq!(*b.tree_args.lock().unwrap(), vec!["page".to_owned()]);
}

/// R-b1: every node exposes `excerpt` and `featured` (the media id, or none).
#[test]
fn nodes_expose_excerpt_and_featured() {
    let root = node("Root", None, 0);
    let mut kid = node("Kid", Some(root.id), 0);
    let media = ContentId::new();
    kid.featured = Some(media);
    let plain = node("Plain", Some(root.id), 1);
    let html = render(
        "{% set t = page_tree(post) %}{% for k in t.children %}{{ k.title }}={{ k.excerpt }}|{{ k.featured }};{% endfor %}\
         {{ t.items[0].children[0].featured }}|{{ t.current.excerpt }}",
        current(&root),
        bridge(tree(vec![root.clone(), kid, plain])),
    );
    assert_eq!(
        html,
        format!("Kid=About Kid|{media};Plain=About Plain|None;{media}|About Root")
    );
}

#[test]
fn prev_next_are_preorder() {
    let r1 = node("R1", None, 0);
    let c1 = node("C1", Some(r1.id), 0);
    let c2 = node("C2", Some(r1.id), 1);
    let g1 = node("G1", Some(c2.id), 0);
    let r2 = node("R2", None, 1);
    let all = vec![r2.clone(), g1.clone(), c2.clone(), c1.clone(), r1.clone()];
    let src = "{% set t = page_tree(post) %}{{ t.prev.title or '-' }}<{{ t.current.title }}>{{ t.next.title or '-' }}|{{ t.next.depth }}";
    let at = |n: &TreeNode| render(src, current(n), bridge(tree(all.clone())));
    assert_eq!(at(&r1), "-<R1>C1|1");
    assert_eq!(at(&c1), "R1<C1>C2|1");
    assert_eq!(at(&c2), "C1<C2>G1|2");
    assert_eq!(at(&g1), "C2<G1>R2|0");
    assert_eq!(at(&r2), "G1<R2>-|");
}

#[test]
fn siblings_sort_by_order_then_title_then_id() {
    let b = node("b", None, 0);
    let a = node("A", None, 0);
    let first = node("Z", None, -1);
    let mut twin1 = node("same", None, 5);
    let mut twin2 = node("Same", None, 5);
    // Same title (case-insensitively): the smaller id first.
    if twin2.id < twin1.id {
        std::mem::swap(&mut twin1.id, &mut twin2.id);
    }
    let html = render(
        "{% for n in page_tree(kind='page').items %}{{ n.title }} {% endfor %}",
        ctx(),
        bridge(tree(vec![twin2, b, twin1, a, first])),
    );
    assert_eq!(html, "Z A b same Same ");
}

#[test]
fn orphans_are_dropped() {
    let root = node("Root", None, 0);
    let orphan = node("Orphan", Some(ContentId::new()), 0);
    let under = node("Under", Some(orphan.id), 0);
    let kid = node("Kid", Some(root.id), 0);
    let t = tree(vec![root.clone(), orphan, under.clone(), kid]);
    let html = render(
        &format!("{{% set t = page_tree(kind='page') %}}{WALK}"),
        ctx(),
        bridge(t.clone()),
    );
    assert_eq!(html, "Root(0) Kid(1) ");
    // The page of a hidden node still renders, without a place in the tree.
    let html = render(
        "{% set t = page_tree(post) %}{{ t.current is none }} {{ t.prev is none }} {{ t.next is none }} {{ t.items | length }}",
        current(&under),
        bridge(t),
    );
    assert_eq!(html, "True True True 1");
}

#[test]
fn current_absent_gives_no_trail() {
    let root = node("Root", None, 0);
    let kid = node("Kid", Some(root.id), 0);
    let stranger = node("Stranger", None, 0);
    let html = render(
        &format!(
            "{{% set t = page_tree(post) %}}{{{{ t.current is none }}}} {{{{ t.prev is none }}}} {{{{ t.next is none }}}} \
             {{{{ t.ancestors | length }}}} {{{{ t.children | length }}}} {WALK}"
        ),
        current(&stranger),
        bridge(tree(vec![root, kid])),
    );
    assert_eq!(html, "True True True 0 0 Root(0) Kid(1) ");
}

#[test]
fn kind_only_has_no_current() {
    let root = node("Root", None, 0);
    let b = bridge(tree(vec![root.clone()]));
    // A `post` in the context does not make `kind=` mark it.
    let html = render(
        &format!("{{% set t = page_tree(kind='page') %}}{{{{ t.current is none }}}} {WALK}"),
        current(&root),
        b.clone(),
    );
    assert_eq!(html, "True Root(0) ");
    assert_eq!(*b.tree_args.lock().unwrap(), vec!["page".to_owned()]);
}

#[test]
fn no_args_is_none_and_costs_no_call() {
    let b = bridge(tree(vec![node("Root", None, 0)]));
    let html = render(
        "{{ page_tree() is none }} {{ page_tree(post=none) is none }} {{ page_tree(kind='Bad Kind') is none }} \
         {{ page_tree(kind='') is none }} {{ page_tree(kind=3) is none }} {{ page_tree('junk') is none }}",
        ctx(),
        b.clone(),
    );
    assert_eq!(html, "True True True True True True");
    assert_eq!(b.calls.load(Ordering::SeqCst), 0);
}

#[test]
fn memoised_per_kind() {
    let root = node("Root", None, 0);
    let b = bridge(tree(vec![root.clone()]));
    let out = try_render(
        "{{ page_tree(post).current.title }} {{ page_tree(post=post).current.title }} \
         {{ page_tree(kind='page').current is none }} {{ page_tree(kind='doc') is none }}",
        current(&root),
        b.clone(),
    )
    .unwrap();
    assert_eq!(out.html, "Root Root True True");
    assert_eq!(
        *b.tree_args.lock().unwrap(),
        vec!["page".to_owned(), "doc".to_owned()]
    );
    assert!(out.deps.contains(&archive_key("page")));
    assert!(out.deps.contains(&archive_key("doc")));
}

#[test]
fn counts_against_the_call_cap() {
    let b = bridge(tree(vec![node("Root", None, 0)]));
    let out = try_render(
        &format!(
            "{{% for i in range({MAX_BRIDGE_CALLS}) %}}{{{{ query(type='post') | length }}}}{{% endfor %}}\
             |{{{{ page_tree(kind='page') is none }}}}"
        ),
        ctx(),
        b.clone(),
    )
    .unwrap();
    assert!(out.html.ends_with("|True"), "{}", out.html);
    assert_eq!(b.calls.load(Ordering::SeqCst), MAX_BRIDGE_CALLS);
    assert!(b.tree_args.lock().unwrap().is_empty());
    // The key is still recorded, so the page refreshes with the type.
    assert!(out.deps.contains(&archive_key("page")));
}

#[test]
fn truncated_is_exposed() {
    let mut t = tree(vec![node("Root", None, 0)]);
    t.truncated = true;
    let html = render("{{ page_tree(kind='page').truncated }}", ctx(), bridge(t));
    assert_eq!(html, "True");
}

#[test]
fn query_rejects_unknown_order() {
    let b = Arc::new(FakeBridge::default());
    let html = render(
        "{{ query(type='service', order='menu') | length }}{{ query(type='post', order='date') | length }}",
        ctx(),
        b.clone(),
    );
    assert_eq!(html, "11");
    assert_eq!(
        *b.queries.lock().unwrap(),
        vec![
            ("service".into(), None, 10, ListOrder::Menu),
            ("post".into(), None, 10, ListOrder::Date),
        ]
    );
    let b = Arc::new(FakeBridge::default());
    let err = try_render("{{ query(type='post', order='x') }}", ctx(), b.clone()).unwrap_err();
    assert!(matches!(err, ThemeError::Render(_)), "{err:?}");
    assert!(err.to_string().contains("order"), "{err}");
    assert_eq!(b.calls.load(Ordering::SeqCst), 0);
}

/// A 1000-node tree drawn 2000 times in a loop: one bridge call, and the value is built
/// once per current node, so the render stays cheap.
#[test]
fn repeated_calls_reuse_the_value() {
    let nodes: Vec<TreeNode> = (0..1000).map(|i| node(&format!("n{i}"), None, i)).collect();
    let first = nodes[0].clone();
    let b = bridge(tree(nodes));
    let out = try_render(
        "{% for i in range(2000) %}{{ page_tree(post).items | length }}{% endfor %}",
        current(&first),
        b.clone(),
    )
    .unwrap();
    assert_eq!(out.html, "1000".repeat(2000));
    assert_eq!(b.calls.load(Ordering::SeqCst), 1);
}

/// More distinct current nodes than the per-render value memo holds: each still
/// renders its own value, from one bridge call.
#[test]
fn many_currents_render_correctly() {
    let nodes: Vec<TreeNode> = (0..100)
        .map(|i| node(&format!("n{i:03}"), None, i))
        .collect();
    let b = bridge(tree(nodes));
    let out = try_render(
        "{% for n in page_tree(kind='page').items %}\
         {% set t = page_tree(post=n, kind='page') %}{{ t.current.title }}<{{ t.prev.title }}>,\
         {% endfor %}",
        ctx(),
        b.clone(),
    )
    .unwrap();
    let want: String = (0..100)
        .map(|i| {
            let prev = if i == 0 {
                String::new()
            } else {
                format!("n{:03}", i - 1)
            };
            format!("n{i:03}<{prev}>,")
        })
        .collect();
    assert_eq!(out.html, want);
    assert_eq!(b.calls.load(Ordering::SeqCst), 1);
}
