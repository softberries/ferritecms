use ferrite_core::{MediaPublic, PublicItem, content::Seo};
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
    let html = render(&core_head(
        SITE,
        page(Some(&it), "/hello/"),
        &Seo::default(),
    ));
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
    let h = core_head(SITE, page(None, "/category/news/"), &Seo::default());
    assert_eq!(h.title.as_deref(), Some("My <Site>"));
    let html = render(&h);
    assert!(html.contains("<meta name=\"description\" content=\"A tagline\">"));
    assert!(html.contains("<meta property=\"og:type\" content=\"website\">"));
    assert!(html.contains("<link rel=\"canonical\" href=\"https://example.com/category/news/\">"));
}

#[test]
fn untitled_item_gets_site_title() {
    let it = item("  ", "");
    let h = core_head(SITE, page(Some(&it), "/hello/"), &Seo::default());
    assert_eq!(h.title.as_deref(), Some("My <Site>"));
    assert!(
        !render(&h).contains("name=\"description\""),
        "empty description omitted"
    );
}

fn seo(title: Option<&str>, description: Option<&str>) -> Seo {
    Seo {
        title: title.map(str::to_owned),
        description: description.map(str::to_owned),
        ..Seo::default()
    }
}

#[test]
fn seo_description_wins_over_excerpt() {
    let it = item("T", "the excerpt");
    let h = core_head(SITE, page(Some(&it), "/t/"), &seo(None, Some("from seo")));
    let html = render(&h);
    assert!(html.contains("<meta name=\"description\" content=\"from seo\">"));
    assert!(html.contains("<meta property=\"og:description\" content=\"from seo\">"));
    assert!(!html.contains("the excerpt"), "{html}");
    // A blank one counts as unset: the excerpt again.
    let h = core_head(SITE, page(Some(&it), "/t/"), &seo(None, Some("  ")));
    assert!(render(&h).contains("<meta name=\"description\" content=\"the excerpt\">"));
    // Without an item the tagline stays, whatever is passed.
    let h = core_head(SITE, page(None, "/"), &seo(Some("X"), Some("from seo")));
    assert_eq!(h.title.as_deref(), Some("My <Site>"));
    assert!(render(&h).contains("<meta name=\"description\" content=\"A tagline\">"));
}

#[test]
fn seo_title_replaces_title_and_og_title() {
    let it = item("Item title", "e");
    let h = core_head(
        SITE,
        page(Some(&it), "/t/"),
        &seo(Some(" Search <title> "), None),
    );
    // The whole title: no site suffix.
    assert_eq!(h.title.as_deref(), Some("Search <title>"));
    let html = render(&h);
    assert!(
        html.starts_with("<title>Search &lt;title&gt;</title>\n"),
        "{html}"
    );
    assert!(
        html.contains("<meta property=\"og:title\" content=\"Search &lt;title&gt;\">"),
        "{html}"
    );
    // The description is unaffected: the excerpt.
    assert!(
        html.contains("<meta name=\"description\" content=\"e\">"),
        "{html}"
    );
    // Blank: the item title with the suffix again.
    let h = core_head(SITE, page(Some(&it), "/t/"), &seo(Some(" "), None));
    assert_eq!(h.title.as_deref(), Some("Item title – My <Site>"));
}

#[test]
fn excerpt_is_cut_to_160_chars() {
    let long = "é".repeat(200);
    let it = item("T", &format!("  {long}  "));
    let h = core_head(SITE, page(Some(&it), "/t/"), &Seo::default());
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
fn resource_links_need_https_or_root_relative() {
    let link = |rel: &str, href: &str| LinkTag {
        rel: rel.into(),
        href: href.into(),
        hreflang: None,
        kind: None,
    };
    let h = Head {
        links: vec![
            link("stylesheet", "http://cdn.example.org/a.css"),
            link("Shortcut ICON", "http://cdn.example.org/f.ico"),
            link("preload", "http://cdn.example.org/f.woff2"),
            link("modulepreload", "http://cdn.example.org/m.js"),
            link("prefetch", "http://cdn.example.org/p"),
            link("stylesheet", "https://cdn.example.org/a.css"),
            link("icon", "/favicon.ico"),
            link("alternate", "http://other.example.org/feed"),
        ],
        ..Head::default()
    };
    assert_eq!(
        render(&h),
        "<title>My &lt;Site&gt;</title>\n\
         <link rel=\"stylesheet\" href=\"https://cdn.example.org/a.css\">\n\
         <link rel=\"icon\" href=\"/favicon.ico\">\n\
         <link rel=\"alternate\" href=\"http://other.example.org/feed\">\n"
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

/// Ruling R22: a head script may not load plugin-route or media bytes from the site,
/// however the path is spelled; `/_theme/` and external hosts stay allowed.
#[test]
fn script_src_refuses_same_site_plugin_and_media_paths() {
    let kept = |site: HeadSite<'_>, s: &str| {
        render_head(
            &Head {
                scripts: vec![src(s)],
                ..Head::default()
            },
            site,
        )
        .contains("<script src=")
    };
    for bad in [
        "/_p/seo/x.js",
        "/_p/seo/",
        "/_p",
        "/_media/abc/x.js",
        "/_media",
        // Percent-encoding, once and twice, of the underscore, the letters and the slash.
        "/%5Fp/seo/x.js",
        "/%5fp/seo/x.js",
        "/%255Fp/seo/x.js",
        "/_%70/seo/x.js",
        "/_p%2Fseo/x.js",
        "/%5F%6D%65%64%69%61/abc/x.js",
        // Case, doubled slashes, dot segments and an encoded backslash.
        "/_P/seo/x.js",
        "/_MEDIA/abc/x.js",
        "/_p//seo/x.js",
        "/./_p/seo/x.js",
        "/x/../_p/seo/x.js",
        "/_theme/../_p/seo/x.js",
        "/_theme/%2e%2e/_media/abc/x.js",
        "/x/..%2F_p/seo/x.js",
        "/x%5C..%5C_p/seo/x.js",
        // The site's own origin, spelled differently.
        "https://example.com/_p/seo/x.js",
        "https://EXAMPLE.com/_p/seo/x.js",
        "https://example.com:443/_p/seo/x.js",
        "https://example.com./_media/abc/x.js",
        "https://user@example.com/_p/seo/x.js",
        "https://%65xample.com/_p/seo/x.js",
        "https://example.com//_p/seo/x.js",
        "https://example.com/%5Fp/seo/x.js",
        "http://example.com/_p/seo/x.js",
    ] {
        assert!(!kept(SITE, bad), "{bad:?} kept");
    }
    for ok in [
        "/_theme/abc/a.js",
        "/_theme/abc/_p.js",
        "/assets/_p/x.js",
        "https://cdn.example.org/_p/x.js",
        "https://example.com.evil.org/_p/x.js",
        "https://example.com/assets/a.js",
        "/_preview.js",
        "/_pages/a.js",
    ] {
        assert!(kept(SITE, ok), "{ok:?} dropped");
    }
    // A site under a base path: the prefixes count from the root and from the base.
    let based = HeadSite {
        url: "http://localhost:8080/blog/",
        ..SITE
    };
    for bad in [
        "/blog/_p/seo/x.js",
        "/BLOG/%5Fmedia/abc/x.js",
        "/_p/seo/x.js",
        "http://localhost:8080/blog/_p/seo/x.js",
        "https://localhost/_media/abc/x.js",
    ] {
        assert!(!kept(based, bad), "{bad:?} kept under a base path");
    }
    assert!(kept(based, "/blog/_theme/abc/a.js"));
    assert!(kept(based, "http://localhost:8080/blog/a.js"));
}

/// R24 N1: a head script src is resolved against `site.url` like a browser resolves
/// it before the `/_p/` and `/_media/` refusal, so other spellings of the site's host
/// (case, default port, IDNA mapping, numeric IPv4 forms) and dot segments cannot reach
/// them.
#[test]
fn head_src_resolved_like_a_browser() {
    let kept = |site_url: &str, s: &str| {
        render_head(
            &Head {
                scripts: vec![src(s)],
                ..Head::default()
            },
            HeadSite {
                url: site_url,
                ..SITE
            },
        )
        .contains("<script src=")
    };
    let site = "https://site.example/";
    for bad in [
        "//site.example/%2e%2e/_p/x",
        "/blog/../_media/a",
        "https://SITE.example:443/_p/x",
        "https://site.example/blog/%2e%2e/_p/x",
        "https://site.example/%2E%2E/_media/a",
        // IDNA: a fullwidth letter and an ideographic full stop map to the site's host.
        "https://\u{ff53}ite.example/_p/x",
        "https://site\u{3002}example/_p/x",
        "https://site.example./_p/x",
    ] {
        assert!(!kept(site, bad), "{bad:?} kept");
    }
    for ok in [
        "https://cdn.example/x.js",
        "/_theme/x.js",
        "https://site.example/x.js",
    ] {
        assert!(kept(site, ok), "{ok:?} dropped");
    }
    // Numeric IPv4 forms of an IP-addressed site.
    let ip_site = "http://127.0.0.1:8080/";
    for bad in [
        "http://2130706433:8080/_p/x",
        "http://0x7f.0.0.1:8080/_p/x",
        "http://127.1:8080/_media/a",
        "https://127.0.0.1/_p/x",
    ] {
        assert!(!kept(ip_site, bad), "{bad:?} kept");
    }
    assert!(kept(ip_site, "http://127.1:8080/_theme/x.js"));
    assert!(kept(ip_site, "/x.js"));
    // A site URL that does not parse refuses every src.
    assert!(!kept("not a url", "/_theme/x.js"));
    assert!(!kept("not a url", "https://cdn.example/x.js"));
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

#[test]
fn seo_canonical_replaces_default_link_and_og_url() {
    let it = item("T", "");
    let s = Seo {
        canonical: Some("https://elsewhere.example/orig/".into()),
        ..Seo::default()
    };
    let h = core_head(SITE, page(Some(&it), "/t/"), &s);
    let html = render(&h);
    assert!(
        html.contains("<link rel=\"canonical\" href=\"https://elsewhere.example/orig/\">"),
        "{html}"
    );
    assert!(
        html.contains("<meta property=\"og:url\" content=\"https://elsewhere.example/orig/\">")
    );
    assert!(!html.contains("example.com/t/"), "{html}");
    // Ignored without an item.
    let h = core_head(SITE, page(None, "/"), &s);
    assert!(render(&h).contains("href=\"https://example.com/\""));
}

#[test]
fn seo_robots_is_one_merged_tag() {
    use ferrite_core::content::Robots;
    let it = item("T", "");
    let s = Seo {
        robots: vec![Robots::NoFollow, Robots::NoIndex],
        ..Seo::default()
    };
    let html = render(&core_head(SITE, page(Some(&it), "/t/"), &s));
    assert_eq!(html.matches("name=\"robots\"").count(), 1, "{html}");
    assert!(
        html.contains("<meta name=\"robots\" content=\"nofollow, noindex\">"),
        "{html}"
    );
    // None set: no tag.
    let html = render(&core_head(SITE, page(Some(&it), "/t/"), &Seo::default()));
    assert!(!html.contains("robots"), "{html}");
}
