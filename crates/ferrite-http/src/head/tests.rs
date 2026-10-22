use ferrite_core::{MediaPublic, PublicItem};
use ferrite_domain::{ContentId, Timestamp};

use super::*;

const SITE: HeadSite<'static> = HeadSite {
    title: "My <Site>",
    url: "https://example.com/",
    description: "A tagline",
};

fn item(title: &str, excerpt: &str) -> PageItem {
    PageItem {
        item: PublicItem {
            id: ContentId::new(),
            kind: "post".into(),
            slug: "hello".into(),
            title: title.into(),
            excerpt: excerpt.into(),
            published_at: None,
            updated_at: Timestamp::from_micros(0).unwrap(),
            menu_order: 0,
            parent_id: None,
            fields: serde_json::json!({}),
        },
        url: "/hello/".into(),
        html: String::new(),
        author: None,
        terms: Vec::new(),
        featured: None,
    }
}

fn page<'a>(item: Option<&'a PageItem>, path: &'a str) -> HeadPage<'a> {
    HeadPage { item, path }
}

fn render(h: &Head) -> String {
    render_head(h, SITE)
}

#[test]
fn core_head_for_an_item() {
    let mut it = item("Hello \"world\"", "An excerpt.");
    it.featured = Some(MediaPublic {
        id: ContentId::new(),
        url: "/_media/abc/cat.png".into(),
        alt: String::new(),
        width: None,
        height: None,
        variants: Vec::new(),
    });
    let html = render(&core_head(SITE, page(Some(&it), "/hello/"), None));
    assert_eq!(
        html,
        "<title>Hello &quot;world&quot; – My &lt;Site&gt;</title>\n\
         <meta name=\"description\" content=\"An excerpt.\">\n\
         <meta property=\"og:title\" content=\"Hello &quot;world&quot;\">\n\
         <meta property=\"og:description\" content=\"An excerpt.\">\n\
         <meta property=\"og:url\" content=\"https://example.com/hello/\">\n\
         <meta property=\"og:type\" content=\"article\">\n\
         <meta property=\"og:image\" content=\"https://example.com/_media/abc/cat.png\">\n\
         <link rel=\"canonical\" href=\"https://example.com/hello/\">\n"
    );
}

#[test]
fn core_head_without_item_uses_site() {
    let h = core_head(SITE, page(None, "/category/news/"), None);
    assert_eq!(h.title.as_deref(), Some("My <Site>"));
    let html = render(&h);
    assert!(html.contains("<meta name=\"description\" content=\"A tagline\">"));
    assert!(html.contains("<meta property=\"og:type\" content=\"website\">"));
    assert!(html.contains("<link rel=\"canonical\" href=\"https://example.com/category/news/\">"));
}

#[test]
fn untitled_item_gets_site_title() {
    let it = item("  ", "");
    let h = core_head(SITE, page(Some(&it), "/hello/"), None);
    assert_eq!(h.title.as_deref(), Some("My <Site>"));
    assert!(
        !render(&h).contains("name=\"description\""),
        "empty description omitted"
    );
}

#[test]
fn seo_description_wins_over_excerpt() {
    let it = item("T", "the excerpt");
    let h = core_head(SITE, page(Some(&it), "/t/"), Some("from seo"));
    assert!(render(&h).contains("<meta name=\"description\" content=\"from seo\">"));
}

#[test]
fn excerpt_is_cut_to_160_chars() {
    let long = "é".repeat(200);
    let it = item("T", &format!("  {long}  "));
    let h = core_head(SITE, page(Some(&it), "/t/"), None);
    let d = &h.metas[0].content;
    assert_eq!(d.chars().count(), DESCRIPTION_CHARS);
    assert!(d.ends_with('…'));
    assert_eq!(truncate("a \n\t b"), "a b");
    assert_eq!(truncate(&"x".repeat(160)), "x".repeat(160));
}

#[test]
fn every_attribute_is_escaped() {
    let h = Head {
        title: Some("</title><script>".into()),
        metas: vec![MetaTag {
            name: Some("x\" onload=\"y".into()),
            property: Some("p'q".into()),
            content: "<&>".into(),
        }],
        links: vec![LinkTag {
            rel: "alternate\"".into(),
            href: "/feed/?a=1&b=\"2\"".into(),
            hreflang: Some("en\"".into()),
            kind: Some("application/rss+xml\"".into()),
        }],
        scripts: Vec::new(),
    };
    assert_eq!(
        render(&h),
        "<title>&lt;/title&gt;&lt;script&gt;</title>\n\
         <meta name=\"x&quot; onload=&quot;y\" property=\"p&#x27;q\" content=\"&lt;&amp;&gt;\">\n\
         <link rel=\"alternate&quot;\" href=\"/feed/?a=1&amp;b=&quot;2&quot;\" \
         hreflang=\"en&quot;\" type=\"application/rss+xml&quot;\">\n"
    );
}

#[test]
fn missing_title_falls_back_and_bad_tags_drop() {
    let h = Head {
        title: None,
        metas: vec![MetaTag {
            name: None,
            property: None,
            content: "orphan".into(),
        }],
        links: [
            "javascript:alert(1)",
            "//evil.example/x.css",
            "/\\evil",
            "data:x",
            "/ok.css",
        ]
        .into_iter()
        .map(|href| LinkTag {
            rel: "stylesheet".into(),
            href: href.into(),
            hreflang: None,
            kind: None,
        })
        .collect(),
        scripts: Vec::new(),
    };
    assert_eq!(
        render(&h),
        "<title>My &lt;Site&gt;</title>\n<link rel=\"stylesheet\" href=\"/ok.css\">\n"
    );
}

fn src(s: &str) -> ScriptTag {
    ScriptTag {
        src: Some(s.into()),
        json_ld: None,
    }
}

#[test]
fn script_src_must_be_https_or_same_origin() {
    for ok in [
        "https://cdn.example.org/a.js",
        "/_theme/abc/a.js",
        "https://example.com/a.js",
    ] {
        let html = render(&Head {
            scripts: vec![src(ok)],
            ..Head::default()
        });
        assert!(
            html.contains(&format!("<script src=\"{ok}\"></script>")),
            "{ok}: {html}"
        );
    }
    for bad in [
        "http://cdn.example.org/a.js",
        "//cdn.example.org/a.js",
        "/\\cdn.example.org/a.js",
        "javascript:alert(1)",
        "data:text/javascript,alert(1)",
        "a.js",
        "https://cdn.example.org/a.js\n",
        "",
    ] {
        let html = render(&Head {
            scripts: vec![src(bad)],
            ..Head::default()
        });
        assert!(!html.contains("<script"), "{bad:?} kept: {html}");
    }
}

#[test]
fn json_ld_is_validated_and_escaped() {
    let ld = |s: &str| ScriptTag {
        src: None,
        json_ld: Some(s.into()),
    };
    let html = render(&Head {
        scripts: vec![
            ld(r#"{ "name": "</script><script>alert(1)</script>", "x": "<!-- & -->" }"#),
            ld("{not json"),
        ],
        ..Head::default()
    });
    assert_eq!(
        html,
        "<title>My &lt;Site&gt;</title>\n\
         <script type=\"application/ld+json\">\
         {\"name\":\"\\u003c/script\\u003e\\u003cscript\\u003ealert(1)\\u003c/script\\u003e\",\
         \"x\":\"\\u003c!-- \\u0026 --\\u003e\"}</script>\n"
    );
    // Still the same JSON.
    let body = html
        .split("ld+json\">")
        .nth(1)
        .and_then(|s| s.split("</script>").next())
        .unwrap();
    let v: serde_json::Value = serde_json::from_str(body).unwrap();
    assert_eq!(v["name"], "</script><script>alert(1)</script>");
}
