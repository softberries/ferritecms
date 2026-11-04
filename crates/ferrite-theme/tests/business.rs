//! The embedded `business` starter theme (M4-3b P60): it loads, every template renders
//! with an empty context, the front page shows the hero, the CTA, the feature cards and
//! the news strip, and the contact page embeds the contact-form plugin's form as its
//! README says. Assertions check key elements, not whole-page snapshots.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use ferrite_core::{
    DepKeys, MediaPublic, MediaVariant, PageItem, PublicItem,
    site::{ContentTree, ListOrder, TreeNode},
};
use ferrite_domain::{ContentId, Timestamp};
use ferrite_theme::{
    Bridged, HelperBridge, ImageRef, MenuNode, MenuTree, NoBridge, RenderContext, Renderer,
    RequestInfo, SiteInfo, TemplateKey, Theme, ThemeSource, error_template, resolve_template,
};
use serde_json::json;

fn business() -> Theme {
    Theme::load(&ThemeSource::Embedded("business")).expect("business theme loads")
}

/// `site` with the theme's declared settings at their defaults, plus `overrides`.
fn site(theme: &Theme, overrides: serde_json::Value) -> SiteInfo {
    let stored = overrides.as_object().cloned().unwrap_or_default();
    SiteInfo {
        title: "Larkspur <Studio>".into(),
        url: "https://example.com".into(),
        description: "Gardens & <courtyards>".into(),
        locale: "en-GB".into(),
        timezone: "UTC".into(),
        settings: theme.manifest().settings_values(&stored),
        locales: Vec::new(),
        default_locale: String::new(),
    }
}

fn ctx(theme: &Theme, path: &str, settings: serde_json::Value) -> RenderContext {
    RenderContext {
        site: site(theme, settings),
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

fn media(name: &str) -> MediaPublic {
    MediaPublic {
        id: ContentId::new(),
        url: format!("/_media/h/{name}.png"),
        alt: format!("Photo of {name}"),
        width: Some(2400),
        height: Some(1350),
        variants: vec![
            MediaVariant {
                name: "card".into(),
                url: format!("/_media/h1/{name}-card.png"),
                width: Some(640),
                height: Some(400),
            },
            MediaVariant {
                name: "hero".into(),
                url: format!("/_media/h2/{name}-hero.png"),
                width: Some(1600),
                height: Some(700),
            },
        ],
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
            locale: "en".into(),
            translation_group: ContentId::new(),
        },
        url: format!("/{slug}/"),
        html: format!("<p>Body of {slug}</p>"),
        toc: Vec::new(),
        author: None,
        terms: Vec::new(),
        featured: None,
    }
}

fn tree_node(
    id: ContentId,
    title: &str,
    parent: Option<ContentId>,
    order: i32,
    featured: Option<ContentId>,
) -> TreeNode {
    TreeNode {
        id,
        parent,
        title: title.into(),
        url: format!("/home/{}/", title.to_lowercase()),
        menu_order: order,
        excerpt: format!("What {title} is"),
        featured,
    }
}

fn menu_node(label: &str, url: &str) -> MenuNode {
    MenuNode {
        label: label.into(),
        url: url.into(),
        children: Vec::new(),
    }
}

/// Canned menus, a page tree, news posts and card images; records `query` calls.
#[derive(Default)]
struct FakeBridge {
    menus: BTreeMap<String, MenuTree>,
    tree: Option<ContentTree>,
    news: Vec<PageItem>,
    images: BTreeMap<ContentId, MediaPublic>,
    queries: Mutex<Vec<(String, u32)>>,
    /// `image` calls that reached the bridge.
    image_calls: Mutex<u32>,
}

impl HelperBridge for FakeBridge {
    fn related(&self, _: ContentId, _: u32) -> Bridged<Vec<PageItem>> {
        (Vec::new(), DepKeys::default())
    }
    fn query(
        &self,
        kind: &str,
        _: Option<&str>,
        limit: u32,
        _: ListOrder,
    ) -> Bridged<Vec<PageItem>> {
        self.queries.lock().unwrap().push((kind.to_owned(), limit));
        let items = if kind == "post" {
            self.news.clone()
        } else {
            Vec::new()
        };
        (items, DepKeys::default())
    }
    fn menu(&self, name: &str) -> Bridged<Option<MenuTree>> {
        (self.menus.get(name).cloned(), DepKeys::default())
    }
    fn image(&self, id: ContentId, size: &str) -> Option<ImageRef> {
        *self.image_calls.lock().unwrap() += 1;
        self.images.get(&id).map(|m| ImageRef::from_media(m, size))
    }
    fn tree(&self, kind: &str) -> Bridged<Option<ContentTree>> {
        let t = self.tree.clone().filter(|t| t.kind == kind);
        (t, DepKeys::default())
    }
}

fn menus() -> BTreeMap<String, MenuTree> {
    let mut menus = BTreeMap::new();
    menus.insert(
        "primary".to_owned(),
        MenuTree {
            items: vec![
                menu_node("Home", "/"),
                MenuNode {
                    children: vec![menu_node("Team", "/about/team/")],
                    ..menu_node("About", "/about/")
                },
                menu_node("News", "/post/"),
                menu_node("Contact", "/contact/"),
            ],
        },
    );
    menus.insert(
        "footer".to_owned(),
        MenuTree {
            items: vec![menu_node("Privacy", "/privacy/")],
        },
    );
    menus
}

fn render(theme: &Theme, template: &str, ctx: RenderContext, bridge: Arc<FakeBridge>) -> String {
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
        "<nav class=\"site-nav\" aria-label=\"Primary\">",
        "<nav class=\"footer-nav\" aria-label=\"Footer\">",
        // Lists styled without markers keep their list semantics in Safari (C-M4).
        "<ul class=\"menu\" role=\"list\">",
        "<ul class=\"menu__sub\" role=\"list\">",
        "Larkspur &lt;Studio&gt;",
        "<style>:root{--accent:#0f766e}</style>",
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
    for bare in ["<ul class=\"menu\">", "<ul class=\"menu__sub\">"] {
        assert!(!html.contains(bare), "{bare} without role=list:\n{html}");
    }
    for raw in ["True", "False", "None", "undefined"] {
        assert!(!html.contains(raw), "raw {raw} printed:\n{html}");
    }
}

/// The opening tag of the first element that contains `needle`.
fn tag_with<'a>(html: &'a str, needle: &str) -> &'a str {
    let at = html
        .find(needle)
        .unwrap_or_else(|| panic!("{needle:?} not in:\n{html}"));
    let start = html[..=at].rfind('<').expect("tag start");
    let end = at + html[at..].find('>').expect("tag end");
    &html[start..=end]
}

#[test]
fn loads_cleanly() {
    let t = business();
    assert_eq!(t.name(), "business");
    assert!(Theme::embedded_names().contains(&"business".to_owned()));
    let m = t.manifest();
    assert_eq!(m.menus["primary"], "Primary navigation");
    assert_eq!(m.menus["footer"], "Footer");
    let hero = m.images["hero"];
    assert_eq!(
        (hero.width, hero.height, hero.crop),
        (1600, Some(700), true)
    );
    let card = m.images["card"];
    assert_eq!((card.width, card.height, card.crop), (640, Some(400), true));
    assert_eq!(m.settings["accent_color"].default, Some(json!("#0f766e")));
    assert_eq!(m.settings["cta_label"].default, Some(json!("Get in touch")));
    assert_eq!(m.settings["cta_url"].default, Some(json!("/contact/")));
    assert_eq!(m.settings["show_news"].default, Some(json!(true)));
    assert_eq!(m.settings["contact_form"].default, Some(json!(false)));
    for name in [
        "base.html",
        "index.html",
        "front-page.html",
        "page.html",
        "page-contact.html",
        "single.html",
        "archive.html",
        "search.html",
        "404.html",
        "500.html",
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
        "var(--accent)",
        "@media (forced-colors: active)",
        "@supports not (color: color-mix(",
        "@supports selector(:has(a))",
    ] {
        assert!(css.contains(needle), "style.css lacks {needle}");
    }
    // No script or image files (P60): templates, the manifest, CSS, and the self-hosted
    // font with its licence under `assets/fonts/` (ruling T1, superseding P65's "no font
    // files").
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../themes/business");
    let mut stack = vec![root];
    let mut files = 0;
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            files += 1;
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            let font = path.parent().is_some_and(|p| p.ends_with("assets/fonts"))
                && ["woff2", "txt"].contains(&ext);
            assert!(
                font || ["html", "css", "toml"].contains(&ext),
                "unexpected file {}",
                path.display()
            );
        }
    }
    assert!(files > 10, "{files} files");
}

#[test]
fn hierarchy_resolves_the_real_files() {
    let t = business();
    let s = |v: &str| v.to_owned();
    let cases = [
        (
            TemplateKey::FrontPage {
                slug: s("home"),
                id: s("0190"),
            },
            "front-page.html",
        ),
        (TemplateKey::FrontPosts, "front-page.html"),
        (
            TemplateKey::Page {
                slug: s("contact"),
                id: s("0190"),
            },
            "page-contact.html",
        ),
        (
            TemplateKey::Page {
                slug: s("about"),
                id: s("0190"),
            },
            "page.html",
        ),
        (
            TemplateKey::Single {
                kind: s("post"),
                slug: s("hello"),
            },
            "single.html",
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

/// The seeded shape: Home with a featured image and three children (given to the
/// bridge out of order), plus a page that is not a child.
fn front_fixture(t: &Theme, settings: serde_json::Value) -> (RenderContext, Arc<FakeBridge>) {
    let mut home = item("page", "home", "Gardens that <grow>");
    home.item.excerpt = "Design & planting for small spaces".into();
    home.featured = Some(media("hero"));
    let home_id = home.item.id;
    let mut images = BTreeMap::new();
    let mut nodes = vec![tree_node(home_id, "Home", None, 0, None)];
    for (title, order) in [("Support", 3), ("Services", 1), ("Consulting", 2)] {
        let m = media(&title.to_lowercase());
        nodes.push(tree_node(
            ContentId::new(),
            title,
            Some(home_id),
            order,
            Some(m.id),
        ));
        images.insert(m.id, m);
    }
    nodes.push(tree_node(ContentId::new(), "About", None, 1, None));
    let mut news = Vec::new();
    for (slug, title) in [
        ("spring", "Spring <planting> notes"),
        ("award", "We won an award"),
        ("hiring", "We are hiring"),
    ] {
        news.push(item("post", slug, title));
    }
    let bridge = Arc::new(FakeBridge {
        menus: menus(),
        tree: Some(ContentTree {
            kind: "page".into(),
            nodes,
            truncated: false,
            media: Default::default(),
        }),
        news,
        images,
        ..FakeBridge::default()
    });
    let mut c = ctx(t, "/", settings);
    c.item = Some(home);
    (c, bridge)
}

#[test]
fn front_page_has_hero_cta_cards_and_news() {
    let t = business();
    let (c, b) = front_fixture(
        &t,
        json!({ "cta_label": "Book a <visit>", "cta_url": "/book/" }),
    );
    let html = render(&t, "front-page.html", c, b.clone());
    assert_frame(&t, &html);

    // The hero: the featured image at `hero`, with srcset, sizes and high priority.
    let hero = tag_with(&html, "h2&#x2f;hero-hero.png");
    for needle in [
        "srcset=\"",
        "sizes=\"",
        "fetchpriority=\"high\"",
        "alt=\"Photo of hero\"",
    ] {
        assert!(hero.contains(needle), "{needle} in {hero}");
    }
    assert!(!hero.contains("loading=\"lazy\""), "{hero}");
    assert!(
        html.contains("<h1 class=\"hero__title\">Gardens that &lt;grow&gt;</h1>"),
        "{html}"
    );
    assert!(
        html.contains("Design &amp; planting for small spaces"),
        "{html}"
    );

    // The CTA from the settings, escaped.
    let cta = tag_with(&html, "hero__cta\"");
    assert!(cta.contains("href=\"&#x2f;book&#x2f;\""), "{cta}");
    assert!(html.contains("Book a &lt;visit&gt;</a>"), "{html}");

    // The children as cards, in `menu_order`, each with its `card` image and excerpt;
    // About (not a child) is no card.
    let positions: Vec<usize> = ["Services", "Consulting", "Support"]
        .iter()
        .map(|t| {
            html.find(&format!(
                "class=\"feature__title\"><a href=\"&#x2f;home&#x2f;{}&#x2f;\">{t}</a>",
                t.to_lowercase()
            ))
            .unwrap_or_else(|| panic!("card {t} in:\n{html}"))
        })
        .collect();
    assert!(positions.windows(2).all(|w| w[0] < w[1]), "{positions:?}");
    assert!(html.contains("What Consulting is"), "{html}");
    assert!(!html.contains("What About is"), "{html}");
    for name in ["services", "consulting", "support"] {
        let img = tag_with(&html, &format!("{name}-card.png"));
        assert!(img.contains("loading=\"lazy\""), "{img}");
        assert!(img.contains("alt=\"\""), "decorative: {img}");
    }

    // The news strip: three posts from one `query(type="post", limit=3)`.
    assert!(html.contains("Latest news"), "{html}");
    assert!(html.contains("Spring &lt;planting&gt; notes"), "{html}");
    assert!(html.contains("We are hiring"), "{html}");
    assert_eq!(*b.queries.lock().unwrap(), vec![("post".to_owned(), 3)]);

    // Off, there is no strip and no query.
    let (c, b) = front_fixture(&t, json!({ "show_news": false }));
    let html = render(&t, "front-page.html", c, b.clone());
    assert_frame(&t, &html);
    assert!(!html.contains("Latest news"), "{html}");
    assert!(!html.contains("We are hiring"), "{html}");
    assert!(b.queries.lock().unwrap().is_empty());
    // The default CTA.
    let cta = tag_with(&html, "hero__cta\"");
    assert!(cta.contains("href=\"&#x2f;contact&#x2f;\""), "{cta}");
    assert!(html.contains("Get in touch</a>"), "{html}");
}

/// M4-4: a front page with more child cards than the bridge budget
/// (`MAX_BRIDGE_CALLS`) shows every card's image: the tree carries the media it
/// checked, so `image()` on a node's `featured` costs no bridge call.
#[test]
fn cards_render_images_past_the_bridge_budget() {
    let t = business();
    let (mut c, _) = front_fixture(&t, json!({}));
    let home = c.item.as_mut().unwrap();
    let home_id = home.item.id;
    let mut nodes = vec![tree_node(home_id, "Home", None, 0, None)];
    let mut images = BTreeMap::new();
    let n = usize::try_from(ferrite_theme::MAX_BRIDGE_CALLS).unwrap();
    for i in 0..n {
        let title = format!("Card{i:02}");
        let m = media(&title.to_lowercase());
        let order = i32::try_from(i).unwrap();
        nodes.push(tree_node(
            ContentId::new(),
            &title,
            Some(home_id),
            order,
            Some(m.id),
        ));
        images.insert(m.id, m);
    }
    // The bridge could answer every image too; it must not be asked.
    let tree = ContentTree {
        kind: "page".into(),
        nodes,
        truncated: false,
        media: images.iter().map(|(id, m)| (*id, m.clone())).collect(),
    };
    let b = Arc::new(FakeBridge {
        menus: menus(),
        tree: Some(tree),
        images,
        ..FakeBridge::default()
    });
    let html = render(&t, "front-page.html", c, b.clone());
    for i in 0..n {
        let img = tag_with(&html, &format!("card{i:02}-card.png"));
        assert!(img.contains("width=\"640\" height=\"400\""), "{img}");
    }
    assert_eq!(
        html.matches("class=\"feature__media\"").count(),
        n,
        "{html}"
    );
    assert_eq!(
        *b.image_calls.lock().unwrap(),
        0,
        "no bridge call for a card"
    );
}

#[test]
fn front_page_without_image_children_or_news_still_renders() {
    let t = business();
    let mut c = ctx(&t, "/", json!({}));
    let mut home = item("page", "home", "Welcome");
    home.item.excerpt = String::new();
    c.item = Some(home);
    let html = render(&t, "front-page.html", c, Arc::new(FakeBridge::default()));
    assert!(
        html.contains("<h1 class=\"hero__title\">Welcome</h1>"),
        "{html}"
    );
    assert!(!html.contains("<img"), "{html}");
    assert!(!html.contains("class=\"features\""), "{html}");
    assert!(!html.contains("Latest news"), "{html}");
    for raw in ["True", "False", "None"] {
        assert!(!html.contains(raw), "{raw}:\n{html}");
    }
}

#[test]
fn contact_page_form_follows_the_plugin_readme() {
    let t = business();
    let mut contact = item("page", "contact", "Contact us");
    contact.html = "<p>Call us on weekdays.</p>".into();
    let bridge = || {
        Arc::new(FakeBridge {
            menus: menus(),
            ..FakeBridge::default()
        })
    };

    let mut c = ctx(&t, "/contact/", json!({ "contact_form": true }));
    c.item = Some(contact.clone());
    let html = render(&t, "page-contact.html", c, bridge());
    assert_frame(&t, &html);
    assert!(html.contains("<p>Call us on weekdays.</p>"), "{html}");
    let body = html.find("Call us on weekdays").unwrap();
    let form_at = html.find("<form").expect("a form");
    assert!(body < form_at, "the body comes first");

    let form = tag_with(&html, "<form");
    assert!(form.contains("method=\"post\""), "{form}");
    assert!(
        form.contains("action=\"&#x2f;_p&#x2f;contact-form&#x2f;submit\""),
        "{form}"
    );
    let end = html[form_at..].find("</form>").expect("form end") + form_at;
    let inner = &html[form_at..end];

    let ret = tag_with(inner, "name=\"return\"");
    assert!(ret.contains("type=\"hidden\""), "{ret}");
    assert!(ret.contains("value=\"&#x2f;contact&#x2f;\""), "{ret}");
    for (field, tag) in [
        ("name", "input"),
        ("email", "input"),
        ("subject", "input"),
        ("message", "textarea"),
        ("website", "input"),
    ] {
        let el = tag_with(inner, &format!("name=\"{field}\""));
        assert!(el.starts_with(&format!("<{tag} ")), "{el}");
        let id = el
            .split("id=\"")
            .nth(1)
            .and_then(|r| r.split('"').next())
            .unwrap_or_else(|| panic!("no id on {el}"));
        assert!(
            inner.contains(&format!("<label for=\"{id}\">")),
            "a label for {id}:\n{inner}"
        );
    }
    assert!(tag_with(inner, "name=\"email\"").contains("type=\"email\""));
    for required in ["name", "email", "message"] {
        assert!(
            tag_with(inner, &format!("name=\"{required}\"")).contains("required"),
            "{required}"
        );
    }
    assert!(!tag_with(inner, "name=\"subject\"").contains("required"));
    // The plugin's limits (`plugins/examples/contact-form/src/form.rs`).
    for (field, attrs) in [
        ("name", &["maxlength=\"100\""][..]),
        ("email", &["maxlength=\"254\""]),
        ("subject", &["maxlength=\"150\""]),
        ("message", &["minlength=\"10\"", "maxlength=\"5000\""]),
    ] {
        let el = tag_with(inner, &format!("name=\"{field}\""));
        for a in attrs {
            assert!(el.contains(a), "{a} on {el}");
        }
    }

    // The honeypot: out of tab order, no autofill, inside an aria-hidden wrapper,
    // hidden with CSS rather than `type="hidden"`.
    let trap = tag_with(inner, "name=\"website\"");
    assert!(trap.contains("tabindex=\"-1\""), "{trap}");
    assert!(trap.contains("autocomplete=\"off\""), "{trap}");
    assert!(!trap.contains("type=\"hidden\""), "{trap}");
    let trap_at = inner.find("name=\"website\"").unwrap();
    let wrapper_at = inner[..trap_at].rfind("<div").expect("a wrapper");
    let wrapper = tag_with(&inner[wrapper_at..], "<div");
    assert!(wrapper.contains("aria-hidden=\"true\""), "{wrapper}");
    assert!(!inner[wrapper_at..trap_at].contains("</div>"), "{inner}");
    assert!(inner.contains("<button type=\"submit\""), "{inner}");

    // Off (the default), no form at all.
    let mut c = ctx(&t, "/contact/", json!({}));
    c.item = Some(contact);
    let html = render(&t, "page-contact.html", c, bridge());
    assert_frame(&t, &html);
    assert!(html.contains("<p>Call us on weekdays.</p>"), "{html}");
    assert!(!html.contains("<form"), "{html}");
    assert!(!html.contains("contact-form"), "{html}");
}

#[test]
fn every_page_template_renders_with_an_empty_context() {
    let t = business();
    let pages: Vec<String> = t
        .templates()
        .filter(|n| !n.starts_with("partials/"))
        .map(str::to_owned)
        .collect();
    assert!(pages.len() >= 10, "{pages:?}");
    for name in pages {
        let html = Renderer::new()
            .render(&t, &name, RenderContext::default(), Arc::new(NoBridge))
            .unwrap_or_else(|e| panic!("{name}: {e}"))
            .html;
        for raw in ["True", "False", "None"] {
            assert!(!html.contains(raw), "{name} prints {raw}:\n{html}");
        }
    }
}

#[test]
fn cta_url_is_escaped() {
    let t = business();
    let (c, b) = front_fixture(&t, json!({ "cta_url": "\" onclick=x" }));
    let html = render(&t, "front-page.html", c, b);
    assert!(!html.contains("\" onclick=x"), "{html}");
    let cta = tag_with(&html, "hero__cta\"");
    assert!(cta.contains("href=\"&quot; onclick=x\""), "{cta}");

    // A script URL gives no button at all; a full https URL is kept.
    let (c, b) = front_fixture(&t, json!({ "cta_url": " JavaScript:alert(1)" }));
    let html = render(&t, "front-page.html", c, b);
    assert!(!html.contains("hero__cta"), "{html}");
    assert!(!html.contains("alert(1)"), "{html}");
    let (c, b) = front_fixture(&t, json!({ "cta_url": "https://example.org/book" }));
    let html = render(&t, "front-page.html", c, b);
    let cta = tag_with(&html, "hero__cta\"");
    assert!(
        cta.contains("href=\"https:&#x2f;&#x2f;example.org&#x2f;book\""),
        "{cta}"
    );

    // A colon after a `/`, `?` or `#` names no scheme: the button stays.
    for url in [
        "/book/?at=10:00",
        "/a/b:c",
        "#top:1",
        "?at=10:00",
        "mailto:hi@example.org",
        "TEL:+441234",
    ] {
        let (c, b) = front_fixture(&t, json!({ "cta_url": url }));
        let html = render(&t, "front-page.html", c, b);
        assert!(html.contains("hero__cta\""), "{url} kept:\n{html}");
    }
    // Any other scheme is dropped.
    for url in [
        "data:text/html,x",
        "vbscript:x",
        "java\tscript:x",
        "ftp://example.org/",
    ] {
        let (c, b) = front_fixture(&t, json!({ "cta_url": url }));
        let html = render(&t, "front-page.html", c, b);
        assert!(!html.contains("hero__cta"), "{url:?} dropped:\n{html}");
    }
}

#[test]
fn pages_listings_and_errors_render() {
    let t = business();
    let b = || {
        Arc::new(FakeBridge {
            menus: menus(),
            ..FakeBridge::default()
        })
    };
    // A page and a post.
    let mut c = ctx(&t, "/about/", json!({}));
    c.item = Some(item("page", "about", "About <us>"));
    let html = render(&t, "page.html", c, b());
    assert_frame(&t, &html);
    assert!(html.contains("About &lt;us&gt;</h1>"), "{html}");
    assert!(html.contains("<p>Body of about</p>"), "{html}");
    let current = tag_with(&html, "aria-current=\"page\"");
    assert!(current.contains("href=\"&#x2f;about&#x2f;\""), "{current}");

    let mut c = ctx(&t, "/spring/", json!({}));
    c.item = Some(item("post", "spring", "Spring notes"));
    let html = render(&t, "single.html", c, b());
    assert_frame(&t, &html);
    assert!(
        html.contains("<time datetime=\"2024-03-05T10:20:30+00:00\">5 March 2024</time>"),
        "{html}"
    );

    // A listing with pagination.
    let mut c = ctx(&t, "/post/", json!({}));
    c.listing = Some(ferrite_core::Listing {
        items: vec![item("post", "a", "Post A"), item("post", "b", "Post B")],
        page: 1,
        per_page: 2,
        total: 3,
    });
    c.archive = Some(ferrite_theme::ArchiveInfo {
        kind: "type".into(),
        content_type: Some("post".into()),
        ..Default::default()
    });
    c.pagination = Some(ferrite_theme::Pagination {
        page: 1,
        pages: 2,
        prev_url: None,
        next_url: Some("/post/page/2/".into()),
    });
    let html = render(&t, "archive.html", c, b());
    assert_frame(&t, &html);
    assert!(html.contains("Post A") && html.contains("Post B"), "{html}");
    assert!(html.contains("rel=\"next\""), "{html}");

    // Search with no results, and the 404 body.
    let mut c = ctx(&t, "/", json!({}));
    c.request.query_s = Some("<x>".into());
    c.archive = Some(ferrite_theme::ArchiveInfo {
        kind: "search".into(),
        ..Default::default()
    });
    let html = render(&t, "search.html", c, b());
    assert_frame(&t, &html);
    assert!(html.contains("&lt;x&gt;"), "{html}");
    assert!(html.contains("role=\"search\""), "{html}");

    let mut c = ctx(&t, "/missing/", json!({}));
    c.status = 404;
    let html = render(&t, "404.html", c, b());
    assert_frame(&t, &html);
    assert!(
        !html.contains("missing"),
        "the 404 body is path-independent:\n{html}"
    );
    // Also at the CTA's own path (a missing contact page): the header CTA is shown
    // either way, so the shared body does not depend on which path rendered it.
    assert!(html.contains("site-header__cta"), "{html}");
    let mut c = ctx(&t, "/contact/", json!({}));
    c.status = 404;
    assert_eq!(render(&t, "404.html", c, b()), html);
    // On a 200 page at that path the CTA hides.
    let mut c = ctx(&t, "/contact/", json!({}));
    c.item = Some(item("page", "contact", "Contact"));
    let page = render(&t, "page-contact.html", c, b());
    assert!(!page.contains("site-header__cta"), "{page}");
}

/// The closing band repeats the CTA above the footer with the site's tagline, except on
/// the page the CTA links to (a 200 page only) and when the CTA would not show.
#[test]
fn cta_band_follows_the_cta_settings() {
    let t = business();
    let (c, b) = front_fixture(&t, json!({}));
    let html = render(&t, "front-page.html", c, b);
    let band = &html[html.find("<section class=\"cta-band\">").expect("band")..];
    assert!(band.contains("<p class=\"cta-band__text\">"), "{band}");
    let button = tag_with(band, "cta-band__button");
    assert!(button.contains("href=\"&#x2f;contact&#x2f;\""), "{button}");
    assert!(html.find("cta-band").unwrap() < html.find("<footer").unwrap());

    let mut c = ctx(&t, "/contact/", json!({}));
    c.item = Some(item("page", "contact", "Contact"));
    let html = render(&t, "page-contact.html", c, Arc::new(FakeBridge::default()));
    assert!(!html.contains("cta-band"), "{html}");

    let (c, b) = front_fixture(&t, json!({ "cta_url": "javascript:x" }));
    let html = render(&t, "front-page.html", c, b);
    assert!(!html.contains("cta-band"), "{html}");
}
