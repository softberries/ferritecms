//! The embedded `docs` starter theme (M4-3b P59): it loads, every template renders with
//! an empty context, and the sidebar, breadcrumbs, table of contents and pager come
//! from `page_tree()` and `post.toc`. Assertions check key elements, not whole-page
//! snapshots.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use ferrite_core::{
    DepKeys, Listing, PageItem, PublicItem, UserPublic,
    blocks::TocEntry,
    site::{ContentTree, ListOrder, TreeNode},
};
use ferrite_domain::{ContentId, Timestamp, UserId};
use ferrite_theme::{
    ArchiveInfo, Bridged, HelperBridge, ImageRef, MenuNode, MenuTree, NoBridge, RenderContext,
    Renderer, RequestInfo, SiteInfo, TemplateKey, Theme, ThemeSource, error_template,
    resolve_template,
};
use serde_json::json;

fn docs() -> Theme {
    Theme::load(&ThemeSource::Embedded("docs")).expect("docs theme loads")
}

/// `site` with the theme's declared settings at their defaults, plus `overrides`.
fn site(theme: &Theme, overrides: serde_json::Value) -> SiteInfo {
    let stored = overrides.as_object().cloned().unwrap_or_default();
    SiteInfo {
        title: "Ferrite <Docs>".into(),
        url: "https://example.com".into(),
        description: "Manuals".into(),
        locale: "en-GB".into(),
        timezone: "UTC".into(),
        settings: theme.manifest().settings_values(&stored),
    }
}

fn ctx(theme: &Theme, path: &str) -> RenderContext {
    RenderContext {
        site: site(theme, json!({})),
        request: RequestInfo {
            path: path.into(),
            query_s: None,
            page: 1,
            cpage: 1,
        },
        status: 200,
        head_html: "<title>Head title</title>".into(),
        footer_html: "<p class=\"plugin-footer\">Plugin footer</p>".into(),
        ..RenderContext::default()
    }
}

fn toc(level: u8, id: &str, text: &str) -> TocEntry {
    TocEntry {
        level,
        id: id.into(),
        text: text.into(),
    }
}

/// A page item with id `id` and a body with headings at levels 1 to 4.
fn page(id: ContentId, title: &str) -> PageItem {
    let ts = Timestamp::from_micros(1_709_634_030_000_000).unwrap(); // 2024-03-05T10:20:30Z
    PageItem {
        item: PublicItem {
            id,
            kind: "page".into(),
            slug: title.to_lowercase().replace(' ', "-"),
            title: title.into(),
            excerpt: String::new(),
            published_at: Some(ts),
            updated_at: ts,
            menu_order: 0,
            parent_id: None,
            fields: json!({}),
        },
        url: format!("/{}/", title.to_lowercase().replace(' ', "-")),
        html: "<h2 id=\"install-step\">Install step</h2><p>Body <em>html</em></p>\
               <pre tabindex=\"0\"><code>cargo run</code></pre>"
            .into(),
        toc: vec![
            toc(1, "big-title", "Big title"),
            toc(2, "install-step", "Install step"),
            toc(3, "on-linux", "On <Linux>"),
            toc(4, "deep-detail", "Deep detail"),
        ],
        author: Some(UserPublic {
            id: UserId::new(),
            login: "ana".into(),
            display_name: "Ana".into(),
        }),
        terms: Vec::new(),
        featured: None,
    }
}

fn tnode(title: &str, parent: Option<&TreeNode>, menu_order: i32) -> TreeNode {
    TreeNode {
        id: ContentId::new(),
        parent: parent.map(|p| p.id),
        title: title.into(),
        url: format!("/{}/", title.to_lowercase().replace(' ', "-")),
        menu_order,
        excerpt: String::new(),
        featured: None,
    }
}

/// Canned menus and one content tree; records the kinds `tree()` was asked for.
#[derive(Default)]
struct FakeBridge {
    menus: BTreeMap<String, MenuTree>,
    tree: Option<ContentTree>,
    tree_args: Mutex<Vec<String>>,
}

impl HelperBridge for FakeBridge {
    fn related(&self, _: ContentId, _: u32) -> Bridged<Vec<PageItem>> {
        (Vec::new(), DepKeys::default())
    }
    fn query(&self, _: &str, _: Option<&str>, _: u32, _: ListOrder) -> Bridged<Vec<PageItem>> {
        (Vec::new(), DepKeys::default())
    }
    fn menu(&self, name: &str) -> Bridged<Option<MenuTree>> {
        (self.menus.get(name).cloned(), DepKeys::default())
    }
    fn image(&self, _: ContentId, _: &str) -> Option<ImageRef> {
        None
    }
    fn tree(&self, kind: &str) -> Bridged<Option<ContentTree>> {
        self.tree_args.lock().unwrap().push(kind.to_owned());
        (self.tree.clone(), DepKeys::default())
    }
}

fn menu_node(label: &str, url: &str) -> MenuNode {
    MenuNode {
        label: label.into(),
        url: url.into(),
        children: Vec::new(),
    }
}

/// The docs fixture: Getting started (Install, Configure (Database, Mail)), Guides
/// (Themes), Reference.
struct Fixture {
    nodes: Vec<TreeNode>,
    database: TreeNode,
}

fn fixture() -> Fixture {
    let start = tnode("Getting started", None, 0);
    let install = tnode("Install", Some(&start), 0);
    let configure = tnode("Configure", Some(&start), 1);
    let database = tnode("Database", Some(&configure), 0);
    let mail = tnode("Mail", Some(&configure), 1);
    let guides = tnode("Guides", None, 1);
    let themes = tnode("Themes", Some(&guides), 0);
    let reference = tnode("Reference", None, 2);
    Fixture {
        nodes: vec![
            reference,
            themes,
            guides,
            mail,
            database.clone(),
            configure,
            install,
            start,
        ],
        database,
    }
}

fn bridge_with(nodes: Vec<TreeNode>, truncated: bool) -> Arc<FakeBridge> {
    let mut menus = BTreeMap::new();
    menus.insert(
        "primary".to_owned(),
        MenuTree {
            items: vec![menu_node("Docs", "/"), menu_node("Blog", "/blog/")],
        },
    );
    menus.insert(
        "footer".to_owned(),
        MenuTree {
            items: vec![menu_node("Privacy", "/privacy/")],
        },
    );
    Arc::new(FakeBridge {
        menus,
        tree: Some(ContentTree {
            kind: "page".into(),
            nodes,
            truncated,
        }),
        tree_args: Mutex::new(Vec::new()),
    })
}

fn render(theme: &Theme, template: &str, ctx: RenderContext, bridge: Arc<FakeBridge>) -> String {
    Renderer::new()
        .render(theme, template, ctx, bridge)
        .unwrap_or_else(|e| panic!("{template}: {e}"))
        .html
}

/// The page of `node` rendered with `page.html` over the fixture's tree.
fn render_page(theme: &Theme, c: RenderContext, f: &Fixture) -> String {
    let mut c = c;
    c.item = Some(page(f.database.id, &f.database.title));
    render(theme, "page.html", c, bridge_with(f.nodes.clone(), false))
}

/// Language, skip link, head, one `<main>`, one `<h1>`, landmarks, accent and assets
/// every base page has.
fn assert_frame(theme: &Theme, html: &str) {
    for needle in [
        "<html lang=\"en-GB\">",
        "<title>Head title</title>",
        "<a class=\"skip-link\" href=\"#content\">Skip to content</a>",
        "<header class=\"site-header\">",
        "<main id=\"content\"",
        "<footer class=\"site-footer\"",
        "Powered by FerriteCMS",
        "<nav class=\"site-nav\" aria-label=\"Primary\">",
        "<nav class=\"footer-nav\" aria-label=\"Footer\">",
        "Ferrite &lt;Docs&gt;",
        "<style>:root{--accent:#2563eb}</style>",
        "</footer>\n<p class=\"plugin-footer\">Plugin footer</p>\n</body>",
    ] {
        assert!(html.contains(needle), "missing {needle:?} in:\n{html}");
    }
    let asset = |p: &str| format!("&#x2f;_theme&#x2f;{}&#x2f;{p}", theme.hash());
    assert!(
        html.contains(&format!(
            "<link rel=\"stylesheet\" href=\"{}\">",
            asset("style.css")
        )),
        "{html}"
    );
    assert!(
        html.contains(&format!(
            "<script src=\"{}\" defer></script>",
            asset("docs.js")
        )),
        "{html}"
    );
    assert_eq!(html.matches("<main").count(), 1, "{html}");
    assert_eq!(html.matches("<h1").count(), 1, "one h1:\n{html}");
    // Every <nav> is labelled.
    for (i, _) in html.match_indices("<nav") {
        let tag = &html[i..i + html[i..].find('>').unwrap()];
        assert!(tag.contains("aria-label=\""), "unlabelled {tag}");
    }
    // No boolean or none printed anywhere, text or attribute (no fixture string holds
    // these words).
    for word in ["True", "False", "None"] {
        assert!(!html.contains(word), "{word} in:\n{html}");
    }
}

/// The `id` attribute values in `html`.
fn ids(html: &str) -> Vec<String> {
    html.match_indices(" id=\"")
        .map(|(i, m)| {
            let rest = &html[i + m.len()..];
            rest[..rest.find('"').unwrap()].to_owned()
        })
        .collect()
}

/// The theme's own ids are reserved ones (06 §4) or start with `docs-`, so no heading
/// id from a body can collide with them.
#[test]
fn fixed_ids_are_reserved_or_prefixed() {
    let t = docs();
    let f = fixture();
    let ok = |id: &str| {
        ferrite_domain::html::RESERVED_FRAGMENT_IDS.contains(&id) || id.starts_with("docs-")
    };
    let mut pages = Vec::new();
    let mut c = ctx(&t, "/database/");
    let mut it = page(f.database.id, "Database");
    it.html = String::new();
    c.item = Some(it);
    pages.push(render(
        &t,
        "page.html",
        c,
        bridge_with(f.nodes.clone(), false),
    ));
    let mut c = ctx(&t, "/");
    c.request.query_s = Some("x".into());
    pages.push(render(
        &t,
        "search.html",
        c,
        bridge_with(f.nodes.clone(), false),
    ));
    let mut c = ctx(&t, "");
    c.status = 404;
    pages.push(render(
        &t,
        "404.html",
        c,
        bridge_with(f.nodes.clone(), false),
    ));
    for html in pages {
        let found = ids(&html);
        assert!(found.contains(&"content".to_owned()), "{found:?}");
        for id in found {
            assert!(
                ok(&id),
                "theme id {id:?} is neither reserved nor docs-prefixed"
            );
        }
    }
}

#[test]
fn loads_cleanly() {
    let t = docs();
    assert_eq!(t.name(), "docs");
    assert!(Theme::embedded_names().contains(&"docs".to_owned()));
    let m = t.manifest();
    assert_eq!(m.license.as_deref(), Some("MIT"));
    assert_eq!(m.menus["primary"], "Primary navigation");
    assert_eq!(m.menus["footer"], "Footer");
    assert_eq!(m.settings["accent_color"].default, Some(json!("#2563eb")));
    assert_eq!(m.settings["nav_type"].default, Some(json!("page")));
    assert_eq!(m.settings["show_toc"].default, Some(json!(true)));
    assert_eq!(m.settings["show_updated"].default, Some(json!(true)));
    for name in [
        "base.html",
        "index.html",
        "page.html",
        "single.html",
        "archive.html",
        "search.html",
        "404.html",
        "500.html",
        "partials/sidebar.html",
        "partials/toc.html",
        "partials/breadcrumbs.html",
        "partials/pager.html",
        "partials/search-form.html",
        "partials/accent.html",
        "partials/menu.html",
    ] {
        assert!(t.has_template(name), "{name}");
    }
    let (css, mime) = t.asset("style.css").unwrap();
    let css = std::str::from_utf8(&css).unwrap();
    assert_eq!(mime, "text/css; charset=utf-8");
    for needle in [
        "prefers-color-scheme: dark",
        ":focus-visible",
        "prefers-reduced-motion",
        "forced-colors",
        "var(--accent)",
    ] {
        assert!(css.contains(needle), "style.css lacks {needle}");
    }
    let (js, mime) = t.asset("docs.js").unwrap();
    assert_eq!(mime, "text/javascript; charset=utf-8");
    assert!(js.len() < 3 * 1024, "docs.js is {} bytes", js.len());
    let js = std::str::from_utf8(&js).unwrap();
    for needle in [
        "heading-anchor",
        "Link to this section",
        "navigator.clipboard",
        // Runs once: a second run adds nothing.
        "if (root.dataset.docsJs) return;",
        // The anchor goes after the heading, outside its accessible name.
        "row.append(h, a);",
        "Copy",
    ] {
        assert!(js.contains(needle), "docs.js lacks {needle}");
    }
    // No external requests from the script.
    for absent in ["http:", "https:", "fetch(", "XMLHttpRequest", "import("] {
        assert!(!js.contains(absent), "docs.js has {absent}");
    }
}

#[test]
fn hierarchy_resolves_the_real_files() {
    let t = docs();
    let s = |v: &str| v.to_owned();
    let cases = [
        (
            TemplateKey::Single {
                kind: s("post"),
                slug: s("hello"),
            },
            "single.html",
        ),
        (
            TemplateKey::Page {
                slug: s("install"),
                id: s("0190"),
            },
            "page.html",
        ),
        (TemplateKey::Archive { kind: s("post") }, "archive.html"),
        (
            TemplateKey::Term {
                tax: s("category"),
                slug: s("news"),
            },
            "archive.html",
        ),
        (TemplateKey::Author { login: s("ana") }, "archive.html"),
        (TemplateKey::Date, "archive.html"),
        (TemplateKey::Search, "search.html"),
        (
            TemplateKey::FrontPage {
                slug: s("welcome"),
                id: s("0190"),
            },
            "page.html",
        ),
        (TemplateKey::FrontPosts, "index.html"),
        (TemplateKey::NotFound, "404.html"),
    ];
    for (key, expected) in cases {
        assert_eq!(
            resolve_template(&key, |n| t.has_template(n)),
            expected,
            "{key:?}"
        );
    }
    assert_eq!(error_template(|n| t.has_template(n)), Some("500.html"));
}

#[test]
fn every_page_template_renders_with_an_empty_context() {
    let t = docs();
    let pages: Vec<String> = t
        .templates()
        .filter(|n| !n.starts_with("partials/"))
        .map(str::to_owned)
        .collect();
    assert!(pages.len() >= 8, "{pages:?}");
    for name in pages {
        let html = Renderer::new()
            .render(&t, &name, RenderContext::default(), Arc::new(NoBridge))
            .unwrap_or_else(|e| panic!("{name}: {e}"))
            .html;
        assert!(html.contains("<main id=\"content\""), "{name}");
        assert!(
            !html.contains("True") && !html.contains("None"),
            "{name}: {html}"
        );
    }
}

#[test]
fn page_shows_sidebar_breadcrumbs_toc_and_pager() {
    let t = docs();
    let f = fixture();
    let html = render_page(&t, ctx(&t, "/database/"), &f);
    assert_frame(&t, &html);
    for needle in [
        // The sidebar: a "Pages" disclosure right before the nav, not around it, so the
        // nav never depends on a closed <details> (or a script) to show.
        "<div class=\"docs-sidebar\">\n<details class=\"docs-pages-toggle\"><summary>Pages</summary></details>\n<nav class=\"docs-nav\" id=\"nav\" aria-label=\"Pages\">",
        "<a class=\"tree__link\" href=\"&#x2f;database&#x2f;\" aria-current=\"page\">Database</a>",
        "<a class=\"tree__link\" href=\"&#x2f;guides&#x2f;\">Guides</a>",
        "<a class=\"tree__link\" href=\"&#x2f;reference&#x2f;\">Reference</a>",
        // Breadcrumbs: the ancestors, root first, then the current title unlinked.
        "<nav class=\"breadcrumbs\" aria-label=\"Breadcrumb\">",
        "<li><a href=\"&#x2f;getting-started&#x2f;\">Getting started</a></li>",
        "<li><a href=\"&#x2f;configure&#x2f;\">Configure</a></li>",
        "<li><span aria-current=\"page\">Database</span></li>\n  </ol>\n</nav>",
        // The page.
        "<h1 class=\"docs-title\">Database</h1>",
        "<h2 id=\"install-step\">Install step</h2>",
        "<time datetime=\"2024-03-05T10:20:30+00:00\">5 March 2024</time>",
        // The table of contents: levels 2 and 3 only.
        "<nav class=\"docs-toc\" id=\"toc\" aria-label=\"On this page\">",
        "<a href=\"#install-step\">Install step</a>",
        "<a href=\"#on-linux\">On &lt;Linux&gt;</a>",
        // The pager: pre-order neighbours.
        "<nav class=\"docs-pager\" aria-label=\"Previous and next page\">",
        "rel=\"prev\" href=\"&#x2f;configure&#x2f;\"",
        "rel=\"next\" href=\"&#x2f;mail&#x2f;\"",
    ] {
        assert!(html.contains(needle), "missing {needle:?} in:\n{html}");
    }
    assert!(!html.contains("#big-title"), "level 1 in the TOC: {html}");
    assert!(!html.contains("#deep-detail"), "level 4 in the TOC: {html}");
    // Only the current page is marked in the sidebar.
    assert_eq!(
        html.matches("class=\"tree__link\" href=\"&#x2f;database&#x2f;\" aria-current")
            .count(),
        1
    );
    assert_eq!(html.matches("tree__link\" href").count(), 8, "{html}");
    // The trail's sections are open (Getting started, Configure); Guides is closed.
    assert_eq!(
        html.matches("<details class=\"tree__section\" open>")
            .count(),
        2,
        "{html}"
    );
    assert_eq!(
        html.matches("<details class=\"tree__section\">").count(),
        1,
        "{html}"
    );
    let guides = html.find(">Guides</a>").unwrap();
    let after = &html[guides..];
    assert!(
        after.find("<details class=\"tree__section\">").unwrap()
            < after.find(">Themes</a>").unwrap(),
        "{html}"
    );
    // No truncation note on a whole tree.
    assert!(!html.contains("docs-nav__note"), "{html}");
}

#[test]
fn a_root_page_has_no_breadcrumbs_and_one_sided_pager() {
    let t = docs();
    let f = fixture();
    let start = f
        .nodes
        .iter()
        .find(|n| n.title == "Getting started")
        .unwrap();
    let mut c = ctx(&t, "/getting-started/");
    c.item = Some(page(start.id, &start.title));
    let html = render(&t, "page.html", c, bridge_with(f.nodes.clone(), false));
    assert_frame(&t, &html);
    assert!(!html.contains("Breadcrumb"), "{html}");
    assert!(!html.contains("rel=\"prev\""), "{html}");
    assert!(
        html.contains("rel=\"next\" href=\"&#x2f;install&#x2f;\""),
        "{html}"
    );
}

#[test]
fn settings_toggle_toc_and_updated() {
    let t = docs();
    let f = fixture();
    let mut c = ctx(&t, "/database/");
    c.site = site(&t, json!({ "show_toc": false, "show_updated": false }));
    let html = render_page(&t, c, &f);
    assert_frame(&t, &html);
    assert!(!html.contains("docs-toc"), "{html}");
    assert!(!html.contains("<time"), "{html}");
    assert!(!html.contains("Updated"), "{html}");

    // One heading at levels 2–3 is not worth a table of contents.
    let mut c = ctx(&t, "/database/");
    let mut it = page(f.database.id, "Database");
    it.toc = vec![toc(2, "only", "Only"), toc(4, "deep", "Deep")];
    c.item = Some(it);
    let html = render(&t, "page.html", c, bridge_with(f.nodes.clone(), false));
    assert!(!html.contains("docs-toc"), "{html}");
}

#[test]
fn nav_type_setting_selects_the_tree() {
    let t = docs();
    let f = fixture();
    let b = bridge_with(f.nodes.clone(), false);
    let mut c = ctx(&t, "/database/");
    c.site = site(&t, json!({ "nav_type": "guide" }));
    c.item = Some(page(f.database.id, "Database"));
    render(&t, "page.html", c, b.clone());
    assert_eq!(*b.tree_args.lock().unwrap(), vec!["guide".to_owned()]);

    // The default reads the `page` tree, once per render.
    let b = bridge_with(f.nodes.clone(), false);
    let mut c = ctx(&t, "/database/");
    c.item = Some(page(f.database.id, "Database"));
    render(&t, "page.html", c, b.clone());
    assert_eq!(*b.tree_args.lock().unwrap(), vec!["page".to_owned()]);

    // A single item of another type still shows the docs tree, with nothing current.
    let b = bridge_with(f.nodes.clone(), false);
    let mut c = ctx(&t, "/hello/");
    let mut post = page(ContentId::new(), "Hello");
    post.item.kind = "post".into();
    c.item = Some(post);
    let html = render(&t, "single.html", c, b.clone());
    assert_frame(&t, &html);
    assert_eq!(*b.tree_args.lock().unwrap(), vec!["page".to_owned()]);
    assert!(html.contains(">Getting started</a>"), "{html}");
    assert!(!html.contains("tree__link\" href=\"&#x2f;database&#x2f;\" aria-current"));
}

#[test]
fn search_and_404_show_the_sidebar_without_current() {
    let t = docs();
    let f = fixture();
    let mut c = ctx(&t, "/");
    c.request.query_s = Some("data\"base".into());
    c.archive = Some(ArchiveInfo {
        kind: "search".into(),
        ..ArchiveInfo::default()
    });
    let mut hit = page(f.database.id, "Database");
    hit.item.excerpt = "Pick a database".into();
    c.listing = Some(Listing {
        items: vec![hit],
        page: 1,
        per_page: 10,
        total: 1,
    });
    let html = render(&t, "search.html", c, bridge_with(f.nodes.clone(), false));
    assert_frame(&t, &html);
    for needle in [
        "<form class=\"search-form",
        "role=\"search\"",
        "Search results for “data&quot;base”",
        "<a class=\"tree__link\" href=\"&#x2f;database&#x2f;\">Database</a>",
        "Pick a database",
    ] {
        assert!(html.contains(needle), "missing {needle:?} in:\n{html}");
    }
    assert!(!html.contains("aria-current"), "{html}");
    assert!(!html.contains("tree__section\" open"), "{html}");

    let mut c = ctx(&t, "");
    c.status = 404;
    let html = render(
        &t,
        "404.html",
        c.clone(),
        bridge_with(f.nodes.clone(), false),
    );
    assert_frame(&t, &html);
    assert!(html.contains("Page not found</h1>"), "{html}");
    assert!(html.contains(">Reference</a>"), "{html}");
    assert!(!html.contains("aria-current"), "{html}");
    // Shared body (R26): the same for every path.
    let src = include_str!("../../../themes/docs/templates/404.html");
    assert!(!src.contains("request.path"), "404.html reads request.path");
    for path in ["/database/", "/", "/missing/"] {
        let mut other = c.clone();
        other.request.path = path.into();
        assert_eq!(
            render(&t, "404.html", other, bridge_with(f.nodes.clone(), false)),
            html,
            "{path}"
        );
    }
    c.status = 410;
    let html = render(&t, "404.html", c, bridge_with(f.nodes.clone(), false));
    assert!(html.contains("Page gone</h1>"), "{html}");
}

#[test]
fn truncated_tree_says_so() {
    let t = docs();
    let f = fixture();
    let mut c = ctx(&t, "/database/");
    c.item = Some(page(f.database.id, "Database"));
    let html = render(&t, "page.html", c, bridge_with(f.nodes.clone(), true));
    assert!(
        html.contains("<p class=\"docs-nav__note\">Only the first 1000 pages are listed.</p>"),
        "{html}"
    );
}

#[test]
fn a_thousand_node_tree_renders_within_the_default_fuel() {
    let t = docs();
    let mut nodes = Vec::new();
    let mut deepest = None;
    for a in 0..10 {
        let root = tnode(&format!("R{a}"), None, a);
        for b in 0..10 {
            let mid = tnode(&format!("R{a} M{b}"), Some(&root), b);
            for c in 0..10 {
                let leaf = tnode(&format!("R{a} M{b} L{c}"), Some(&mid), c);
                if (a, b, c) == (9, 9, 9) {
                    deepest = Some(leaf.clone());
                }
                nodes.push(leaf);
            }
            nodes.push(mid);
        }
        nodes.push(root);
    }
    assert_eq!(nodes.len(), 1110);
    let cur = deepest.unwrap();
    let mut c = ctx(&t, &cur.url);
    c.item = Some(page(cur.id, &cur.title));
    let html = render(&t, "page.html", c, bridge_with(nodes, false));
    assert_frame(&t, &html);
    assert_eq!(html.matches("class=\"tree__link\"").count(), 1110);
    assert_eq!(html.matches("aria-current=\"page\">R9 M9 L9<").count(), 2);
}

#[test]
fn a_64_deep_chain_renders() {
    let t = docs();
    // Depths 0 to 64 are kept; 65 and below are dropped by `page_tree`.
    let mut chain: Vec<TreeNode> = Vec::new();
    for i in 0..70 {
        let n = tnode(&format!("Level {i}"), chain.last(), 0);
        chain.push(n);
    }
    let cur = chain[64].clone();
    let mut c = ctx(&t, &cur.url);
    c.item = Some(page(cur.id, &cur.title));
    let html = render(&t, "page.html", c, bridge_with(chain, false));
    assert_frame(&t, &html);
    assert_eq!(html.matches("class=\"tree__link\"").count(), 65, "{html}");
    assert!(
        html.contains("aria-current=\"page\">Level 64</a>"),
        "{html}"
    );
    assert_eq!(
        html.matches("<details class=\"tree__section\" open>")
            .count(),
        64
    );
    assert!(!html.contains(">Level 65<"), "{html}");
}

#[test]
fn docs_js_is_progressive() {
    let t = docs();
    let f = fixture();
    let html = render_page(&t, ctx(&t, "/database/"), &f);
    // The only script is the deferred asset; the page needs none to work.
    assert_eq!(html.matches("<script").count(), 1, "{html}");
    assert!(html.contains(" defer></script>"), "{html}");
    for absent in [" onclick=", " onload=", "javascript:"] {
        assert!(!html.contains(absent), "{absent}: {html}");
    }
    // The 500 page is standalone and has no script at all.
    let html = Renderer::new()
        .render(&t, "500.html", RenderContext::default(), Arc::new(NoBridge))
        .unwrap()
        .html;
    assert!(!html.contains("<script"), "{html}");
}

#[test]
fn accent_colour_setting_is_applied_and_checked() {
    let t = docs();
    let style = |accent: serde_json::Value| {
        let mut c = ctx(&t, "/");
        c.site.settings.insert("accent_color".into(), accent);
        render(&t, "index.html", c, Arc::new(FakeBridge::default()))
    };
    let html = style(json!("#0f766e"));
    assert!(
        html.contains("<style>:root{--accent:#0f766e}</style>"),
        "{html}"
    );
    for bad in [
        json!("#};a{b:"),
        json!("red}</style><script>x()</script>"),
        json!(7),
    ] {
        let html = style(bad.clone());
        assert!(!html.contains("--accent:"), "{bad}: {html}");
        assert!(!html.contains("<script>x()"), "{html}");
    }
}

#[test]
fn listings_render_titles_excerpts_and_pagination() {
    let t = docs();
    let mut c = ctx(&t, "/blog/");
    let mut it = page(ContentId::new(), "Release notes");
    it.item.kind = "post".into();
    it.item.excerpt = "What changed".into();
    c.listing = Some(Listing {
        items: vec![it],
        page: 2,
        per_page: 1,
        total: 3,
    });
    c.archive = Some(ArchiveInfo {
        kind: "type".into(),
        content_type: Some("post".into()),
        ..ArchiveInfo::default()
    });
    c.pagination = Some(ferrite_theme::Pagination {
        page: 2,
        pages: 3,
        prev_url: Some("/blog/".into()),
        next_url: Some("/blog/page/3/".into()),
    });
    let mut menus_only = bridge_with(Vec::new(), false);
    Arc::get_mut(&mut menus_only).unwrap().tree = None;
    let html = render(&t, "archive.html", c, menus_only);
    assert_frame(&t, &html);
    for needle in [
        "<a href=\"&#x2f;release-notes&#x2f;\">Release notes</a>",
        "What changed",
        "<nav class=\"pagination\" aria-label=\"Pagination\">",
        "rel=\"prev\" href=\"&#x2f;blog&#x2f;\"",
        "rel=\"next\" href=\"&#x2f;blog&#x2f;page&#x2f;3&#x2f;\"",
        "Page 2 of 3",
    ] {
        assert!(html.contains(needle), "missing {needle:?} in:\n{html}");
    }
    // Without a tree (no public pages), no sidebar.
    assert!(!html.contains("docs-sidebar"), "{html}");
}
