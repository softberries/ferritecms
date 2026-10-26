//! The embedded `blog` starter theme: it loads, every template renders with a fixture
//! context, and the markup has the accessibility and safety properties it promises.
//! Assertions check key elements, not whole-page snapshots.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{collections::BTreeMap, sync::Arc};

use ferrite_core::{
    DepKeys, Listing, MediaPublic, MediaVariant, PageItem, PublicItem, TermPublic, UserPublic,
};
use ferrite_domain::{ContentId, TermId, Timestamp, UserId};
use ferrite_theme::{
    ArchiveInfo, Bridged, HelperBridge, ImageRef, MenuNode, MenuTree, NoBridge, Pagination,
    RenderContext, Renderer, RequestInfo, SiteInfo, TemplateKey, Theme, ThemeSource,
    error_template, resolve_template,
};
use serde_json::json;

fn blog() -> Theme {
    Theme::load(&ThemeSource::Embedded("blog")).expect("blog theme loads")
}

/// `site` with the theme's declared settings at their defaults, plus `overrides`.
fn site(theme: &Theme, overrides: serde_json::Value) -> SiteInfo {
    let stored = overrides.as_object().cloned().unwrap_or_default();
    SiteInfo {
        title: "Ferrite <Notes>".into(),
        url: "https://example.com".into(),
        description: "Writing about <rust>".into(),
        locale: "en-GB".into(),
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

fn term(tax: &str, slug: &str, name: &str) -> TermPublic {
    TermPublic {
        id: TermId::new(),
        taxonomy: tax.into(),
        slug: slug.into(),
        name: name.into(),
        url: format!("/{tax}/{slug}/"),
    }
}

fn item(kind: &str, slug: &str, title: &str) -> PageItem {
    let ts = Timestamp::from_micros(1_709_634_030_000_000).unwrap(); // 2024-03-05T10:20:30Z
    PageItem {
        item: PublicItem {
            id: ContentId::new(),
            kind: kind.into(),
            slug: slug.into(),
            title: title.into(),
            excerpt: format!("Excerpt of {slug}"),
            published_at: Some(ts),
            updated_at: ts,
            menu_order: 0,
            parent_id: None,
            fields: json!({}),
        },
        url: format!("/{slug}/"),
        html: "<p>Body <em>html</em></p>".into(),
        author: Some(UserPublic {
            id: UserId::new(),
            login: "ana".into(),
            display_name: "Ana Author".into(),
        }),
        terms: vec![
            term("category", "news", "News"),
            term("tag", "rust", "Rust"),
        ],
        featured: Some(MediaPublic {
            id: ContentId::new(),
            url: "/_media/h/photo.jpg".into(),
            alt: "A ferrite magnet on a desk".into(),
            width: Some(2400),
            height: Some(1600),
            variants: vec![
                MediaVariant {
                    name: "card".into(),
                    url: "/_media/h1/photo-card.jpg".into(),
                    width: Some(640),
                    height: Some(360),
                },
                MediaVariant {
                    name: "hero".into(),
                    url: "/_media/h2/photo-hero.jpg".into(),
                    width: Some(1280),
                    height: Some(640),
                },
            ],
        }),
    }
}

fn listing(items: Vec<PageItem>, page: u32, total: u64) -> Listing {
    Listing {
        items,
        page,
        per_page: 2,
        total,
    }
}

/// An item with nothing optional: no author, featured image, terms or date.
fn bare_item(kind: &str) -> PageItem {
    let mut it = item(kind, "bare", "Bare item");
    it.author = None;
    it.featured = None;
    it.terms = Vec::new();
    it.item.published_at = None;
    it.item.excerpt = String::new();
    it.html = String::new();
    it
}

fn archive(kind: &str) -> ArchiveInfo {
    ArchiveInfo {
        kind: kind.into(),
        content_type: Some("post".into()),
        ..ArchiveInfo::default()
    }
}

fn pagination(page: u32, pages: u32, prev: Option<&str>, next: Option<&str>) -> Option<Pagination> {
    Some(Pagination {
        page,
        pages,
        prev_url: prev.map(Into::into),
        next_url: next.map(Into::into),
    })
}

/// Canned menus (primary and footer) and related posts.
#[derive(Default)]
struct FakeBridge {
    menus: BTreeMap<String, MenuTree>,
    related: Vec<PageItem>,
}

impl HelperBridge for FakeBridge {
    fn related(&self, _: ContentId, _: u32) -> Bridged<Vec<PageItem>> {
        (self.related.clone(), DepKeys::default())
    }
    fn query(&self, _: &str, _: Option<&str>, _: u32) -> Bridged<Vec<PageItem>> {
        (Vec::new(), DepKeys::default())
    }
    fn menu(&self, name: &str) -> Bridged<Option<MenuTree>> {
        (self.menus.get(name).cloned(), DepKeys::default())
    }
    fn image(&self, _: ContentId, _: &str) -> Option<ImageRef> {
        None
    }
}

fn node(label: &str, url: &str, children: Vec<MenuNode>) -> MenuNode {
    MenuNode {
        label: label.into(),
        url: url.into(),
        children,
    }
}

fn bridge() -> Arc<FakeBridge> {
    let mut menus = BTreeMap::new();
    menus.insert(
        "primary".to_owned(),
        MenuTree {
            items: vec![
                node("Home", "/", vec![]),
                node(
                    "About & co",
                    "/about/",
                    vec![node("Team", "/about/team/", vec![])],
                ),
            ],
        },
    );
    menus.insert(
        "footer".to_owned(),
        MenuTree {
            items: vec![node("Privacy", "/privacy/", vec![])],
        },
    );
    Arc::new(FakeBridge {
        menus,
        related: vec![item("post", "other", "Other post")],
    })
}

fn render(theme: &Theme, template: &str, ctx: RenderContext) -> String {
    render_with(theme, template, ctx, bridge())
}

fn render_with(
    theme: &Theme,
    template: &str,
    ctx: RenderContext,
    bridge: Arc<dyn HelperBridge>,
) -> String {
    Renderer::new()
        .render(theme, template, ctx, bridge)
        .unwrap_or_else(|e| panic!("{template}: {e}"))
        .html
}

/// Landmarks, skip link, head, stylesheet, accent and `lang` every base page has.
fn assert_frame(theme: &Theme, html: &str) {
    for needle in [
        "<html lang=\"en-GB\">",
        "<title>Head title</title>",
        "<a class=\"skip-link\" href=\"#content\">Skip to content</a>",
        "<header class=\"site-header\">",
        "<main id=\"content\"",
        "<footer class=\"site-footer\">",
        "Powered by FerriteCMS",
        "<nav class=\"site-nav\" aria-label=\"Primary\">",
        "<nav class=\"footer-nav\" aria-label=\"Footer\">",
        "Ferrite &lt;Notes&gt;",
        "<style>:root{--accent:#c2410c}</style>",
        // `footer()` after the site footer, just before `</body>`.
        "</footer>\n<p class=\"plugin-footer\">Plugin footer</p>\n</body>",
    ] {
        assert!(html.contains(needle), "missing {needle:?} in:\n{html}");
    }
    let css = format!("&#x2f;_theme&#x2f;{}&#x2f;style.css", theme.hash());
    assert!(
        html.contains(&format!("<link rel=\"stylesheet\" href=\"{css}\">")),
        "{html}"
    );
    assert_eq!(html.matches("<main").count(), 1, "{html}");
    assert_eq!(html.matches("<h1").count(), 1, "one h1:\n{html}");
}

#[test]
fn loads_cleanly_with_declared_manifest() {
    let t = blog();
    assert_eq!(t.name(), "blog");
    assert!(Theme::embedded_names().contains(&"blog".to_owned()));
    let m = t.manifest();
    assert_eq!(m.menus["primary"], "Primary navigation");
    assert_eq!(m.menus["footer"], "Footer");
    let card = m.images["card"];
    assert_eq!((card.width, card.height, card.crop), (640, Some(360), true));
    let hero = m.images["hero"];
    assert_eq!(
        (hero.width, hero.height, hero.crop),
        (1280, Some(640), true)
    );
    assert_eq!(m.settings["accent_color"].default, Some(json!("#c2410c")));
    assert_eq!(m.settings["show_author"].default, Some(json!(true)));
    assert_eq!(m.settings["show_excerpts"].default, Some(json!(true)));
    for name in [
        "base.html",
        "index.html",
        "single.html",
        "page.html",
        "archive.html",
        "taxonomy.html",
        "author.html",
        "date.html",
        "search.html",
        "404.html",
        "500.html",
        "partials/card.html",
        "partials/pagination.html",
        "partials/menu.html",
    ] {
        assert!(t.has_template(name), "{name}");
    }
    let (css, mime) = t.asset("style.css").unwrap();
    let css = std::str::from_utf8(&css).unwrap();
    assert_eq!(mime, "text/css; charset=utf-8");
    for needle in [
        "prefers-color-scheme: dark",
        "prefers-reduced-motion",
        ":focus-visible",
        "aspect-ratio",
        "65ch",
        "var(--accent)",
    ] {
        assert!(css.contains(needle), "style.css lacks {needle}");
    }
}

#[test]
fn hierarchy_resolves_the_real_files() {
    let t = blog();
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
                slug: s("about"),
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
            "taxonomy.html",
        ),
        (
            TemplateKey::Term {
                tax: s("tag"),
                slug: s("rust"),
            },
            "taxonomy.html",
        ),
        (TemplateKey::Author { login: s("ana") }, "author.html"),
        (TemplateKey::Date, "date.html"),
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
fn every_template_renders_with_an_empty_context() {
    let t = blog();
    let pages: Vec<String> = t
        .templates()
        .filter(|n| !n.starts_with("partials/"))
        .map(str::to_owned)
        .collect();
    for name in pages {
        Renderer::new()
            .render(&t, &name, RenderContext::default(), Arc::new(NoBridge))
            .unwrap_or_else(|e| panic!("{name}: {e}"));
    }
}

#[test]
fn single_post() {
    let t = blog();
    let mut c = ctx(&t, "/hello/");
    c.item = Some(item("post", "hello", "Hello <script>alert(1)</script>"));
    let html = render(&t, "single.html", c);
    assert_frame(&t, &html);
    for needle in [
        "<h1 class=\"entry-title\">Hello &lt;script&gt;alert(1)&lt;&#x2f;script&gt;</h1>",
        "<time datetime=\"2024-03-05T10:20:30+00:00\">5 March 2024</time>",
        "by <a class=\"author\" href=\"/author/ana/\" rel=\"author\">Ana Author</a>",
        "src=\"&#x2f;_media&#x2f;h2&#x2f;photo-hero.jpg\" alt=\"A ferrite magnet on a desk\" width=\"1280\" height=\"640\"",
        "<p>Body <em>html</em></p>",
        "<a href=\"&#x2f;category&#x2f;news&#x2f;\" rel=\"tag\">News</a>",
        "<a href=\"&#x2f;tag&#x2f;rust&#x2f;\" rel=\"tag\">Rust</a>",
        "aria-labelledby=\"related-title\"",
        "<ul class=\"card-grid\" role=\"list\">",
        "<h3 class=\"card__title\"><a href=\"&#x2f;other&#x2f;\">Other post</a></h3>",
    ] {
        assert!(html.contains(needle), "missing {needle:?} in:\n{html}");
    }
    assert!(!html.contains("<script>alert"), "{html}");
}

#[test]
fn author_links_are_url_encoded() {
    let t = blog();
    let mut c = ctx(&t, "/hello/");
    let mut it = item("post", "hello", "Hello");
    if let Some(a) = it.author.as_mut() {
        a.login = "a b\"<&?#:".into();
    }
    c.item = Some(it);
    let html = render(&t, "single.html", c);
    assert!(
        html.contains("href=\"/author/a%20b%22%3C%26%3F%23%3A/\" rel=\"author\""),
        "{html}"
    );
}

#[test]
fn show_author_false_hides_the_author() {
    let t = blog();
    let mut c = ctx(&t, "/hello/");
    c.site = site(&t, json!({ "show_author": false }));
    c.item = Some(item("post", "hello", "Hello"));
    let html = render(&t, "single.html", c);
    assert!(!html.contains("Ana Author"), "{html}");
    assert!(!html.contains("rel=\"author\""), "{html}");
    assert!(html.contains("5 March 2024"), "{html}");
}

#[test]
fn show_excerpts_false_hides_card_excerpts() {
    let t = blog();
    let mut c = ctx(&t, "/");
    c.listing = Some(listing(vec![item("post", "a", "A")], 1, 1));
    assert!(render(&t, "index.html", c.clone()).contains("Excerpt of a"));
    c.site = site(&t, json!({ "show_excerpts": false }));
    assert!(!render(&t, "index.html", c).contains("Excerpt of a"));
}

#[test]
fn accent_colour_setting_is_applied() {
    let t = blog();
    let style = |accent: serde_json::Value| {
        let mut c = ctx(&t, "/");
        c.site.settings.insert("accent_color".into(), accent);
        render(&t, "index.html", c)
    };
    for ok in ["#0f766e", "#ABC", "#c2410C"] {
        let html = style(json!(ok));
        assert!(
            html.contains(&format!("<style>:root{{--accent:{ok}}}</style>")),
            "{ok}: {html}"
        );
    }
    // Even if a bad value got past core, the template does not emit it: `#` plus 3
    // or 6 hex digits only.
    for bad in [
        json!("#};a{b:"),
        json!("#12345g"),
        json!("#abcd"),
        json!("red;}x"),
        json!("red}</style><script>x()</script>"),
        json!(""),
        json!(7),
        json!(null),
    ] {
        let html = style(bad.clone());
        assert!(!html.contains("--accent:"), "{bad}: {html}");
        assert!(!html.contains("<script>x()"), "{html}");
    }
    // An invalid stored value falls back to the default through core's validation.
    let mut c = ctx(&t, "/");
    c.site = site(&t, json!({ "accent_color": "#};a{b:" }));
    let html = render(&t, "index.html", c);
    assert!(
        html.contains("<style>:root{--accent:#c2410c}</style>"),
        "{html}"
    );
}

#[test]
fn page_has_title_body_and_hero_without_post_meta() {
    let t = blog();
    let mut c = ctx(&t, "/about/");
    c.item = Some(item("page", "about", "About us"));
    let html = render(&t, "page.html", c);
    assert_frame(&t, &html);
    assert!(
        html.contains("<h1 class=\"entry-title\">About us</h1>"),
        "{html}"
    );
    assert!(
        html.contains("alt=\"A ferrite magnet on a desk\""),
        "{html}"
    );
    assert!(!html.contains("<time"), "{html}");
    assert!(!html.contains("Related posts"), "{html}");
}

#[test]
fn menus_nest_and_mark_the_current_page() {
    let t = blog();
    let html = render(&t, "page.html", ctx(&t, "/about/team/"));
    let nested = "<a href=\"&#x2f;about&#x2f;\">About &amp; co</a>\n    \
                  <ul class=\"menu__sub\">";
    assert!(html.contains(nested), "{html}");
    assert!(
        html.contains("<a href=\"&#x2f;about&#x2f;team&#x2f;\" aria-current=\"page\">Team</a>"),
        "{html}"
    );
    // `loop()` output is not escaped a second time.
    assert!(!html.contains("&lt;li"), "{html}");
    // The nested list sits inside its parent's <li>.
    let parent = html.find("About &amp; co").unwrap();
    let child = html.find(">Team<").unwrap();
    let close = html[parent..].find("</li>").unwrap() + parent;
    assert!(child < close, "{html}");
}

#[test]
fn pagination_comes_from_the_context() {
    let t = blog();
    let items = || vec![item("post", "a", "Post A"), item("post", "b", "Post B")];

    // Middle page of a term archive.
    let mut c = ctx(&t, "/category/news/page/2/");
    c.listing = Some(listing(items(), 2, 6));
    c.archive = Some(ArchiveInfo {
        term: Some(term("category", "news", "News")),
        ..archive("term")
    });
    c.pagination = pagination(
        2,
        3,
        Some("/category/news/"),
        Some("/category/news/page/3/"),
    );
    let html = render(&t, "taxonomy.html", c);
    assert_frame(&t, &html);
    for needle in [
        "<p class=\"page-eyebrow\">Category</p>",
        "<h1 class=\"page-title\">News</h1>",
        "<nav class=\"pagination\" aria-label=\"Pagination\">",
        "rel=\"prev\" href=\"&#x2f;category&#x2f;news&#x2f;\"",
        "rel=\"next\" href=\"&#x2f;category&#x2f;news&#x2f;page&#x2f;3&#x2f;\"",
        "Page 2 of 3",
        "<ul class=\"card-grid\" role=\"list\">",
        "<li><article class=\"card\">",
        "<h2 class=\"card__title\"><a href=\"&#x2f;a&#x2f;\">Post A</a></h2>",
        "src=\"&#x2f;_media&#x2f;h1&#x2f;photo-card.jpg\" alt=\"\"",
    ] {
        assert!(html.contains(needle), "missing {needle:?} in:\n{html}");
    }

    // The template prints what it is given; it does not derive URLs from the path.
    let mut c = ctx(&t, "/");
    c.listing = Some(listing(items(), 1, 6));
    c.pagination = pagination(1, 3, None, Some("/elsewhere/2/"));
    let html = render(&t, "index.html", c);
    assert!(
        html.contains("rel=\"next\" href=\"&#x2f;elsewhere&#x2f;2&#x2f;\""),
        "{html}"
    );
    assert!(!html.contains("rel=\"prev\""), "{html}");

    // Search uses plain rel links too, with the server-encoded query.
    let mut c = ctx(&t, "/page/2/");
    c.request.query_s = Some("a&b c".into());
    c.listing = Some(listing(items(), 2, 6));
    c.archive = Some(archive("search"));
    c.pagination = pagination(2, 3, Some("/?s=a%26b%20c"), Some("/page/3/?s=a%26b%20c"));
    let html = render(&t, "search.html", c);
    for needle in [
        "rel=\"prev\" href=\"&#x2f;?s=a%26b%20c\"",
        "rel=\"next\" href=\"&#x2f;page&#x2f;3&#x2f;?s=a%26b%20c\"",
    ] {
        assert!(html.contains(needle), "missing {needle:?} in:\n{html}");
    }
    assert!(!html.contains("<form class=\"pagination"), "{html}");

    // One page, or no pagination in the context: no nav.
    let mut c = ctx(&t, "/");
    c.listing = Some(listing(items(), 1, 2));
    c.pagination = pagination(1, 1, None, None);
    assert!(!render(&t, "index.html", c.clone()).contains("class=\"pagination\""));
    c.pagination = None;
    assert!(!render(&t, "index.html", c).contains("class=\"pagination\""));
}

#[test]
fn archive_author_and_date_headings_come_from_the_context() {
    let t = blog();
    let one = || Some(listing(vec![item("post", "a", "Post A")], 1, 1));
    let heading = |template: &str, a: Option<ArchiveInfo>, items: bool| {
        let mut c = ctx(&t, "/whatever/");
        c.listing = if items {
            one()
        } else {
            Some(listing(vec![], 1, 0))
        };
        c.archive = a;
        let html = render(&t, template, c);
        assert_frame(&t, &html);
        let start = html.find("<h1 class=\"page-title\">").unwrap() + 23;
        let end = html[start..].find("</h1>").unwrap() + start;
        html[start..end].trim().to_owned()
    };

    let event = ArchiveInfo {
        content_type: Some("event".into()),
        ..archive("type")
    };
    assert_eq!(heading("archive.html", Some(event), true), "Event archive");
    assert_eq!(heading("archive.html", None, true), "Archive");

    let ana = UserPublic {
        id: UserId::new(),
        login: "ana".into(),
        display_name: "Ana <A>".into(),
    };
    let by_ana = ArchiveInfo {
        author: Some(ana),
        ..archive("author")
    };
    // From the context, not from the listed items (an empty archive still knows).
    assert_eq!(
        heading("author.html", Some(by_ana), false),
        "Posts by Ana &lt;A&gt;"
    );
    assert_eq!(heading("author.html", None, true), "Posts by this author");

    let genre = ArchiveInfo {
        term: Some(term("genre", "jazz", "Jazz")),
        ..archive("term")
    };
    assert_eq!(heading("taxonomy.html", Some(genre), false), "Jazz");
    assert_eq!(heading("taxonomy.html", None, true), "Topic archive");

    for (year, month, expected) in [
        (Some(2024), Some(3), "March 2024"),
        (Some(2024), Some(12), "December 2024"),
        (Some(2024), None, "2024"),
        (Some(2024), Some(13), "2024"),
        (Some(99), Some(3), "Posts by date"),
        (None, None, "Posts by date"),
    ] {
        let a = ArchiveInfo {
            year,
            month,
            ..archive("date")
        };
        assert_eq!(
            heading("date.html", Some(a), true),
            expected,
            "{year:?} {month:?}"
        );
    }

    // An empty archive says so.
    let mut c = ctx(&t, "/post/");
    c.listing = Some(listing(vec![], 1, 0));
    let html = render(&t, "archive.html", c);
    assert!(html.contains("<p class=\"empty\">"), "{html}");
}

#[test]
fn search_results_form_and_empty_state() {
    let t = blog();
    let mut c = ctx(&t, "/");
    c.request.query_s = Some("ferrite \"magnets\" & <b>".into());
    c.listing = Some(listing(
        vec![item("post", "a", "Post A"), item("post", "b", "Post B")],
        1,
        3,
    ));
    c.archive = Some(archive("search"));
    let html = render(&t, "search.html", c.clone());
    assert_frame(&t, &html);
    let q = "ferrite &quot;magnets&quot; &amp; &lt;b&gt;";
    for needle in [
        &format!("Search results for “{q}”</h1>") as &str,
        "<form class=\"search-form\" role=\"search\" method=\"get\" action=\"/\">",
        "<label for=\"search-main\">Search this site</label>",
        &format!("name=\"s\" value=\"{q}\""),
        "3 results",
        "Post A",
    ] {
        assert!(html.contains(needle), "missing {needle:?} in:\n{html}");
    }
    assert!(!html.contains("<b>"), "{html}");

    c.listing = Some(listing(vec![], 1, 0));
    let html = render(&t, "search.html", c);
    assert!(html.contains("No results</h2>"), "{html}");
    assert!(html.contains("role=\"search\""), "{html}");
}

#[test]
fn bare_items_and_empty_listings_render_without_data_helpers() {
    let t = blog();
    let none = || -> Arc<dyn HelperBridge> { Arc::new(NoBridge) };
    for (template, kind) in [
        ("single.html", "post"),
        ("page.html", "page"),
        ("index.html", "post"),
    ] {
        let mut c = ctx(&t, "/bare/");
        c.item = Some(bare_item(kind));
        let html = render_with(&t, template, c, none());
        assert!(html.contains("Bare item</h1>"), "{template}: {html}");
        for absent in [
            "<time",
            "rel=\"author\"",
            "<img",
            "class=\"terms\"",
            "Related posts",
            "<nav",
        ] {
            assert!(!html.contains(absent), "{template} has {absent:?}: {html}");
        }
        assert!(html.contains("<main id=\"content\""), "{template}");
    }
    // A listing of bare items: cards without image, date or excerpt.
    let mut c = ctx(&t, "/");
    c.listing = Some(listing(vec![bare_item("post")], 1, 1));
    let html = render_with(&t, "index.html", c, none());
    assert!(
        html.contains("<a href=\"&#x2f;bare&#x2f;\">Bare item</a>"),
        "{html}"
    );
    assert!(
        !html.contains("card__excerpt") && !html.contains("<img"),
        "{html}"
    );
    // Empty listings, with and without archive info.
    for template in [
        "index.html",
        "archive.html",
        "taxonomy.html",
        "author.html",
        "date.html",
        "search.html",
    ] {
        for a in [None, Some(archive("term"))] {
            let mut c = ctx(&t, "/");
            c.listing = Some(listing(vec![], 1, 0));
            c.archive = a;
            let html = render_with(&t, template, c, none());
            assert!(html.contains("<h1"), "{template}: {html}");
            assert!(!html.contains("class=\"pagination\""), "{template}");
        }
    }
}

#[test]
fn no_menus_means_no_navs() {
    let t = blog();
    let mut c = ctx(&t, "/hello/");
    c.item = Some(item("post", "hello", "Hello"));
    let html = render_with(&t, "single.html", c, Arc::new(FakeBridge::default()));
    assert!(
        !html.contains("site-nav") && !html.contains("footer-nav"),
        "{html}"
    );
    for needle in [
        "<header class=\"site-header\">",
        "<footer class=\"site-footer\">",
        "Powered by FerriteCMS",
    ] {
        assert!(html.contains(needle), "{needle}");
    }
}

#[test]
fn not_found_body_does_not_depend_on_the_path() {
    let t = blog();
    let mut c = ctx(&t, "");
    c.status = 404;
    let html = render(&t, "404.html", c.clone());
    assert_frame(&t, &html);
    assert!(html.contains("Page not found</h1>"), "{html}");
    assert!(html.contains("role=\"search\""), "{html}");
    assert!(!html.contains("aria-current"), "{html}");
    // Shared body (R26): the server passes an empty path, and even a path that equals
    // a menu URL (which marks `aria-current` on a 200 page) renders the same body.
    let src = include_str!("../../../themes/blog/templates/404.html");
    assert!(!src.contains("request.path"), "404.html reads request.path");
    for path in ["/about/", "/about/team/", "/some/missing/url/"] {
        let mut other = c.clone();
        other.request.path = path.into();
        assert_eq!(render(&t, "404.html", other), html, "{path}");
    }

    c.status = 410;
    let html = render(&t, "404.html", c);
    assert!(html.contains("Page gone</h1>"), "{html}");
}

#[test]
fn server_error_renders_with_an_empty_context() {
    let t = blog();
    let html = render_with(&t, "500.html", RenderContext::default(), Arc::new(NoBridge));
    for needle in [
        "<html lang=\"en\">",
        "<main id=\"content\"",
        "Something went wrong</h1>",
        "Powered by FerriteCMS",
        ">Home</a>",
    ] {
        assert!(html.contains(needle), "missing {needle:?} in:\n{html}");
    }
    assert!(!html.contains("--accent:"), "{html}");

    // With the minimal context the server gives it (site defaults, a head).
    let mut c = ctx(&t, "");
    c.status = 500;
    let html = render_with(&t, "500.html", c, Arc::new(NoBridge));
    assert!(html.contains("Ferrite &lt;Notes&gt;"), "{html}");
    assert!(html.contains("<title>Head title</title>"), "{html}");
    assert!(html.contains("--accent:#c2410c"), "{html}");
}

#[test]
fn archive_term_and_author_are_recorded() {
    let t = blog();
    let news = term("category", "news", "News");
    let ana = UserPublic {
        id: UserId::new(),
        login: "ana".into(),
        display_name: "Ana".into(),
    };
    let mut c = ctx(&t, "/category/news/");
    c.listing = Some(listing(vec![], 1, 0));
    c.archive = Some(ArchiveInfo {
        term: Some(news.clone()),
        author: Some(ana.clone()),
        ..archive("term")
    });
    let out = Renderer::new()
        .render(&t, "taxonomy.html", c, Arc::new(NoBridge))
        .unwrap();
    assert!(out.deps.contains(&ferrite_core::events::term_key(news.id)));
    assert!(out.deps.contains(&ferrite_core::events::user_key(ana.id)));
}

/// A bridge with one comment thread.
struct ThreadBridge(Option<ferrite_core::comments::CommentThread>);

impl HelperBridge for ThreadBridge {
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
    fn comments(
        &self,
        _: ContentId,
        _: u32,
    ) -> Bridged<Option<ferrite_core::comments::CommentThread>> {
        (self.0.clone(), DepKeys::default())
    }
}

/// The comment section: the pending notice, labelled fields, the honeypot out of reach
/// of people and assistive technology; closed threads keep their comments and lose the
/// form; pages never get one.
#[test]
fn comments_section_form_and_states() {
    use ferrite_core::comments::{CommentPublic, CommentThread};
    use ferrite_domain::CommentId;

    let t = blog();
    let one = CommentPublic {
        id: CommentId::new(),
        parent_id: None,
        depth: 0,
        author_name: "Ann".into(),
        author_url: None,
        html: "<p>Nice</p>".into(),
        date: Timestamp::from_micros(1_709_634_030_000_000).unwrap(),
        can_reply: true,
    };
    let thread = |open: bool, items: Vec<CommentPublic>| {
        Arc::new(ThreadBridge(Some(CommentThread {
            open,
            total: items.len() as u64,
            items,
            page: 1,
            pages: 1,
            per_page: 50,
            prev_url: None,
            next_url: None,
        })))
    };
    let single = |bridge: Arc<ThreadBridge>, kind: &str| {
        let mut c = ctx(&t, "/hello/");
        c.item = Some(item(kind, "hello", "Hello"));
        render_with(
            &t,
            if kind == "page" {
                "page.html"
            } else {
                "single.html"
            },
            c,
            bridge,
        )
    };

    let html = single(thread(true, Vec::new()), "post");
    for needle in [
        "<h2 id=\"comments-title\" class=\"section-title\">No comments yet</h2>",
        "<p id=\"comment-pending\" class=\"comment-notice\" role=\"status\">",
        "awaiting moderation",
        "method=\"post\" action=\"&#x2f;_comment\"",
        "<div class=\"comment-form__trap\" aria-hidden=\"true\">",
        "name=\"website\" tabindex=\"-1\" autocomplete=\"off\"",
        "<button type=\"submit\">Post comment</button>",
    ] {
        assert!(html.contains(needle), "missing {needle:?} in:\n{html}");
    }
    // Every visible field has its label.
    for field in ["author", "email", "url", "trap", "comment"] {
        let id = format!("comment-form-{field}");
        assert!(html.contains(&format!("<label for=\"{id}\"")), "{field}");
        assert!(html.contains(&format!("id=\"{id}\"")), "{field}");
    }
    assert!(!html.contains("<script"), "{html}");

    let html = single(thread(true, vec![one.clone()]), "post");
    assert!(html.contains("1 comment</h2>"), "{html}");
    assert!(
        !html.contains("comment-pages"),
        "one page, no links: {html}"
    );
    assert!(html.contains("<summary>Reply</summary>"));
    assert!(html.contains(&format!("name=\"parent\" value=\"{}\"", one.id)));

    // No reply form where core would refuse the reply.
    let full = CommentPublic {
        can_reply: false,
        ..one.clone()
    };
    let html = single(thread(true, vec![full]), "post");
    assert!(!html.contains("<summary>Reply</summary>"), "{html}");
    assert!(html.contains("<button type=\"submit\">Post comment</button>"));

    let html = single(thread(false, vec![one.clone()]), "post");
    assert!(html.contains("<p>Nice</p>"), "{html}");
    assert!(html.contains("Comments are closed."));
    for gone in [
        "<form class=\"comment-form\"",
        "comment-pending",
        "<summary>Reply",
    ] {
        assert!(!html.contains(gone), "{gone}: {html}");
    }

    // A middle page of a long thread: the heading counts every approved comment, and
    // the page links point both ways.
    let paged = Arc::new(ThreadBridge(Some(CommentThread {
        open: true,
        items: vec![one.clone()],
        total: 120,
        page: 2,
        pages: 3,
        per_page: 50,
        prev_url: Some("/hello/".into()),
        next_url: Some("/hello/?cpage=3".into()),
    })));
    let html = single(paged, "post");
    for needle in [
        "120 comments</h2>",
        "<nav class=\"pagination comment-pages\" aria-label=\"Comment pages\">",
        "<a class=\"pagination__prev\" rel=\"prev\" href=\"&#x2f;hello&#x2f;#comments\"><span aria-hidden=\"true\">←</span> Older comments</a>",
        "<p class=\"pagination__status\">Page 2 of 3</p>",
        "<a class=\"pagination__next\" rel=\"next\" href=\"&#x2f;hello&#x2f;?cpage=3#comments\">Newer comments <span aria-hidden=\"true\">→</span></a>",
    ] {
        assert!(html.contains(needle), "missing {needle:?} in:\n{html}");
    }
    // The first page has no link back.
    let first = Arc::new(ThreadBridge(Some(CommentThread {
        open: true,
        items: vec![one.clone()],
        total: 120,
        page: 1,
        pages: 3,
        per_page: 50,
        prev_url: None,
        next_url: Some("/hello/?cpage=2".into()),
    })));
    let html = single(first, "post");
    assert!(!html.contains("rel=\"prev\""), "{html}");
    assert!(html.contains("rel=\"next\""), "{html}");

    let html = single(thread(true, vec![one]), "page");
    assert!(!html.contains("class=\"comments"), "{html}");
}
