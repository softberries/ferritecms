//! The `seo` example plugin through a real host (M4-3a Task 2, plan P43): what it adds
//! to core's head on a post, the home page and an archive, that it never repeats a tag
//! core emitted, and that a hostile title keeps the JSON-LD valid (Review Focus 3).

use ferrite_domain::{
    Artifact, ArtifactHash, ArtifactKind, ContentId, ContentStatus, Media, NewContent, Role, Term,
    TermId, Timestamp, User,
};
use ferrite_http::{HeadPage, PluginRequest, RenderTrace, StoreHostServices, page_head};
use ferrite_plugin_host::{FilterValue, Head, LinkTag, MetaTag, ScriptTag};
use serde_json::{Value, json};

use crate::common::{HOUR, SITE_URL, Site, plugin_manifest, plugin_wasm};
use crate::profile_tests;

profile_tests!(
    post_page_gets_article_tags_and_blog_posting,
    hostile_title_keeps_json_ld_valid,
    home_gets_website_search_action,
    archive_gets_site_level_tags_only,
    pages_get_no_article_markup,
    card_follows_the_image,
    settings_drive_feed_link_verification_and_twitter_site,
    organization_settings_reach_the_publisher,
    nothing_core_emitted_is_repeated,
    options_grant_decides_which_dependent_tags_appear,
    tags_already_in_the_head_are_not_repeated,
);

async fn term(site: &Site, taxonomy: &str, slug: &str, name: &str) -> TermId {
    let id = TermId::new();
    site.store
        .read()
        .terms()
        .insert(&Term {
            id,
            taxonomy: taxonomy.into(),
            slug: slug.into(),
            name: name.into(),
            description: String::new(),
            parent_id: None,
        })
        .await
        .unwrap();
    id
}

/// A published item of `kind` with `fields` and `terms`.
async fn item(
    site: &Site,
    author: &User,
    kind: &str,
    slug: &str,
    title: &str,
    fields: Value,
    terms: &[TermId],
) -> ContentId {
    let id = ContentId::new();
    let now = Timestamp::now().as_micros();
    let tx = site.store.begin().await.unwrap();
    tx.content()
        .insert(&NewContent {
            id,
            kind: kind.into(),
            status: ContentStatus::Published,
            slug: slug.into(),
            parent_id: None,
            author_id: author.id,
            title: title.into(),
            excerpt: format!("Excerpt of {slug}."),
            body: json!({"type": "doc", "content": []}),
            body_html: None,
            fields,
            menu_order: 0,
            published_at: Some(Timestamp::from_micros(now - HOUR).unwrap()),
            scheduled_at: None,
            created_at: None,
            updated_at: None,
        })
        .await
        .unwrap();
    tx.terms().set_for_item(id, terms).await.unwrap();
    tx.commit().await.unwrap();
    id
}

/// A published media item (one PNG artifact), the featured image of `post`.
async fn picture(site: &Site, author: &User, digit: char) -> ContentId {
    let hash = ArtifactHash::parse(&digit.to_string().repeat(64)).unwrap();
    let id = item(
        site,
        author,
        "media",
        &format!("pic-{digit}"),
        "Picture",
        json!({}),
        &[],
    )
    .await;
    let repos = site.store.read();
    repos
        .artifacts()
        .insert_if_absent(&Artifact {
            hash: hash.clone(),
            kind: ArtifactKind::Media,
            bytes: 10,
            mime: "image/png".into(),
            created_at: Timestamp::now(),
            orphaned_at: None,
        })
        .await
        .unwrap();
    repos
        .media()
        .insert(&Media {
            item_id: id,
            artifact_hash: hash,
            mime: "image/png".into(),
            bytes: 10,
            width: Some(800),
            height: Some(600),
            alt: "a cat".into(),
            original_name: "cat.png".into(),
            variants: json!({}),
            sanitized: false,
        })
        .await
        .unwrap();
    id
}

async fn option(site: &Site, key: &str, value: Value) {
    site.store
        .read()
        .options()
        .set(key, &value, false)
        .await
        .unwrap();
}

/// A setting of the `seo` plugin (a plain option under the plugin's namespace).
async fn setting(site: &Site, key: &str, value: Value) {
    option(site, &format!("plugin.seo.settings.{key}"), value).await;
}

/// A site with `seo` installed and loaded, and the site title option set.
async fn site_with_seo(site: Site) -> Site {
    let site = crate::with_generous_deadlines(site);
    option(&site, "site.title", json!("Blog Title")).await;
    site.install("seo").await;
    let summary = site.load().await;
    assert_eq!(summary.loaded, ["seo"], "{:?}", summary.skipped);
    site
}

/// The head of the page at `path`, showing `item` (if any) in `locale`.
async fn render(site: &Site, item: Option<ContentId>, path: &str, locale: &str) -> String {
    let page = match item {
        Some(id) => Some(site.reader.by_id(id).await.unwrap().value.unwrap()),
        None => None,
    };
    let path = page.as_ref().map_or(path, |p| p.url.as_str());
    let mut trace = RenderTrace::default();
    let html = page_head(
        &site.host,
        site.store.as_ref(),
        crate::SITE,
        HeadPage {
            item: page.as_ref(),
            path,
        },
        &PluginRequest::public(path, locale, item),
        &mut trace,
    )
    .await;
    assert!(
        site.host.failures().is_empty(),
        "{:?}",
        site.host.failures()
    );
    html
}

/// The `content` of every `<meta {attr}="{key}">`, in page order.
fn metas(html: &str, attr: &str, key: &str) -> Vec<String> {
    let start = format!("<meta {attr}=\"{key}\" content=\"");
    html.match_indices(&start)
        .filter_map(|(at, _)| html[at + start.len()..].split('"').next())
        .map(str::to_owned)
        .collect()
}

fn prop(html: &str, key: &str) -> Vec<String> {
    metas(html, "property", key)
}

fn name(html: &str, key: &str) -> Vec<String> {
    metas(html, "name", key)
}

/// Every JSON-LD block, parsed; a block that is not JSON or holds a `<` fails the test.
fn json_ld(html: &str) -> Vec<Value> {
    const OPEN: &str = "<script type=\"application/ld+json\">";
    html.match_indices(OPEN)
        .map(|(at, _)| {
            let body = html[at + OPEN.len()..].split("</script>").next().unwrap();
            assert!(!body.contains('<'), "raw `<` in JSON-LD: {body}");
            serde_json::from_str(body).unwrap_or_else(|e| panic!("{e}: {body}"))
        })
        .collect()
}

fn of_type<'a>(blocks: &'a [Value], ty: &str) -> Option<&'a Value> {
    blocks.iter().find(|b| b["@type"] == ty)
}

const RSS: &str =
    "<link rel=\"alternate\" href=\"https://example.com/feed/\" type=\"application/rss+xml\">";

async fn post_page_gets_article_tags_and_blog_posting(site: Site) -> Site {
    let site = site_with_seo(site).await;
    let author = site.user("ada", Role::Author).await;
    let news = term(&site, "category", "news", "News").await;
    let rust = term(&site, "tag", "rust", "Rust").await;
    let wasm = term(&site, "tag", "wasm", "WASM").await;
    let pic = picture(&site, &author, 'a').await;
    let id = item(
        &site,
        &author,
        "post",
        "hello",
        "Hello World",
        json!({"featured_image": pic.to_string()}),
        &[news, rust, wasm],
    )
    .await;

    let html = render(&site, Some(id), "/", "pl-PL").await;

    assert_eq!(prop(&html, "og:site_name"), ["Blog Title"], "{html}");
    assert_eq!(prop(&html, "og:locale"), ["pl_PL"]);
    assert_eq!(name(&html, "twitter:card"), ["summary_large_image"]);
    // Core's tags, once; the card copies them.
    assert_eq!(prop(&html, "og:title").len(), 1, "{html}");
    assert_eq!(prop(&html, "og:description").len(), 1);
    assert_eq!(prop(&html, "og:image").len(), 1);
    assert_eq!(
        name(&html, "twitter:title"),
        prop(&html, "og:title"),
        "{html}"
    );
    assert_eq!(
        name(&html, "twitter:description"),
        prop(&html, "og:description")
    );
    // Article tags.
    let published = prop(&html, "article:published_time");
    assert_eq!(published.len(), 1, "{html}");
    assert!(published[0].starts_with("20") && published[0].contains('T'));
    assert_eq!(prop(&html, "article:author"), ["ada display"]);
    assert_eq!(prop(&html, "article:section"), ["News"]);
    assert_eq!(prop(&html, "article:tag"), ["Rust", "WASM"]);
    assert!(html.contains(RSS), "{html}");

    // JSON-LD: a BlogPosting, no WebSite.
    let blocks = json_ld(&html);
    assert_eq!(blocks.len(), 1, "{html}");
    let post = of_type(&blocks, "BlogPosting").unwrap();
    assert_eq!(post["@context"], "https://schema.org");
    assert_eq!(post["headline"], "Hello World");
    assert_eq!(post["datePublished"], published[0].as_str());
    assert_eq!(post["url"], "https://example.com/hello/");
    assert_eq!(post["author"]["name"], "ada display");
    assert_eq!(post["image"], prop(&html, "og:image")[0].as_str());
    assert_eq!(post["publisher"]["@type"], "Organization");
    assert_eq!(post["publisher"]["name"], "Blog Title");
    assert!(post["publisher"].get("logo").is_none());
    assert!(of_type(&blocks, "WebSite").is_none());
    site
}

async fn hostile_title_keeps_json_ld_valid(site: Site) -> Site {
    let site = site_with_seo(site).await;
    let author = site.user("ada", Role::Author).await;
    let title = "Evil </script><script>alert(1)</script>\"x\" & <!-- y";
    let id = item(&site, &author, "post", "evil", title, json!({}), &[]).await;

    let html = render(&site, Some(id), "/", "en").await;

    let blocks = json_ld(&html);
    let post = of_type(&blocks, "BlogPosting").unwrap();
    assert_eq!(post["headline"], title);
    // Only the plugin's one script element: the `</script>` of the title is escaped.
    assert_eq!(html.matches("<script").count(), 1, "{html}");
    assert_eq!(html.matches("</script").count(), 1, "{html}");
    assert!(!html.contains("alert(1)</script>"), "{html}");
    site
}

async fn home_gets_website_search_action(site: Site) -> Site {
    let site = site_with_seo(site).await;

    let html = render(&site, None, "/", "en").await;

    let blocks = json_ld(&html);
    assert_eq!(blocks.len(), 1, "{html}");
    let web = of_type(&blocks, "WebSite").unwrap();
    assert_eq!(web["@context"], "https://schema.org");
    assert_eq!(web["url"], "https://example.com/");
    assert_eq!(web["name"], "Blog Title");
    let action = &web["potentialAction"];
    assert_eq!(action["@type"], "SearchAction");
    assert_eq!(
        action["target"],
        "https://example.com/?s={search_term_string}"
    );
    assert_eq!(action["query-input"], "required name=search_term_string");
    assert_eq!(prop(&html, "og:site_name"), ["Blog Title"]);
    assert_eq!(prop(&html, "og:locale"), ["en"]);
    assert_eq!(name(&html, "twitter:card"), ["summary"]);
    assert!(html.contains(RSS), "{html}");
    assert!(!html.contains("article:"), "{html}");
    site
}

async fn archive_gets_site_level_tags_only(site: Site) -> Site {
    let site = site_with_seo(site).await;

    let html = render(&site, None, "/category/news/", "").await;

    assert_eq!(prop(&html, "og:site_name"), ["Blog Title"], "{html}");
    // No locale in the request: no og:locale.
    assert!(prop(&html, "og:locale").is_empty());
    assert_eq!(name(&html, "twitter:card"), ["summary"]);
    assert!(!html.contains("article:"), "{html}");
    // Neither the WebSite nor a BlogPosting block: they belong to `/` and posts.
    assert!(json_ld(&html).is_empty(), "{html}");
    assert!(html.contains(RSS), "{html}");
    site
}

/// The plugin reads `post` only (`content.read`): a page gets no article markup.
async fn pages_get_no_article_markup(site: Site) -> Site {
    let site = site_with_seo(site).await;
    let author = site.user("ada", Role::Author).await;
    let id = item(&site, &author, "page", "about", "About", json!({}), &[]).await;

    let html = render(&site, Some(id), "/", "en").await;

    assert_eq!(prop(&html, "og:site_name"), ["Blog Title"], "{html}");
    assert!(!html.contains("article:"), "{html}");
    assert!(json_ld(&html).is_empty(), "{html}");
    site
}

/// `twitter:card` is `summary` without an `og:image` and `summary_large_image` with one.
async fn card_follows_the_image(site: Site) -> Site {
    let site = site_with_seo(site).await;
    let author = site.user("ada", Role::Author).await;
    let bare = item(&site, &author, "post", "bare", "Bare", json!({}), &[]).await;
    let pic = picture(&site, &author, 'b').await;
    let framed = item(
        &site,
        &author,
        "post",
        "framed",
        "Framed",
        json!({"featured_image": pic.to_string()}),
        &[],
    )
    .await;

    let html = render(&site, Some(bare), "/", "en").await;
    assert!(prop(&html, "og:image").is_empty(), "{html}");
    assert_eq!(name(&html, "twitter:card"), ["summary"]);
    // No image, no `image` in the JSON-LD.
    assert!(json_ld(&html)[0].get("image").is_none());

    let html = render(&site, Some(framed), "/", "en").await;
    assert_eq!(prop(&html, "og:image").len(), 1, "{html}");
    assert_eq!(name(&html, "twitter:card"), ["summary_large_image"]);
    site
}

async fn settings_drive_feed_link_verification_and_twitter_site(site: Site) -> Site {
    let site = site_with_seo(site).await;

    // Unset: the feed link (default true), no verification metas, no twitter:site.
    let html = render(&site, None, "/", "en").await;
    assert!(html.contains(RSS), "{html}");
    assert!(name(&html, "google-site-verification").is_empty());
    assert!(name(&html, "msvalidate.01").is_empty());
    assert!(name(&html, "twitter:site").is_empty());

    setting(&site, "feed_link", json!(false)).await;
    setting(&site, "google_verification", json!(" g-token_1 ")).await;
    setting(&site, "bing_verification", json!("B1NG")).await;
    setting(&site, "twitter_site", json!("ferritecms")).await;
    let html = render(&site, None, "/", "en").await;
    assert!(!html.contains("application/rss+xml"), "{html}");
    assert_eq!(name(&html, "google-site-verification"), ["g-token_1"]);
    assert_eq!(name(&html, "msvalidate.01"), ["B1NG"]);
    assert_eq!(name(&html, "twitter:site"), ["@ferritecms"]);

    // A handle that already has its `@` keeps one; blank values add nothing.
    setting(&site, "twitter_site", json!("@ferritecms")).await;
    setting(&site, "google_verification", json!("  ")).await;
    let html = render(&site, None, "/", "en").await;
    assert_eq!(name(&html, "twitter:site"), ["@ferritecms"]);
    assert!(name(&html, "google-site-verification").is_empty());
    site
}

async fn organization_settings_reach_the_publisher(site: Site) -> Site {
    let site = site_with_seo(site).await;
    let author = site.user("ada", Role::Author).await;
    let id = item(&site, &author, "post", "org", "Org", json!({}), &[]).await;
    setting(&site, "organization_name", json!("Acme Inc.")).await;
    setting(
        &site,
        "organization_logo",
        json!("https://cdn.example.com/logo.png"),
    )
    .await;

    let html = render(&site, Some(id), "/", "en").await;
    let blocks = json_ld(&html);
    let publisher = &of_type(&blocks, "BlogPosting").unwrap()["publisher"];
    assert_eq!(publisher["name"], "Acme Inc.");
    assert_eq!(publisher["logo"]["@type"], "ImageObject");
    assert_eq!(publisher["logo"]["url"], "https://cdn.example.com/logo.png");

    // Only an https logo is used.
    setting(&site, "organization_logo", json!("javascript:alert(1)")).await;
    let html = render(&site, Some(id), "/", "en").await;
    let blocks = json_ld(&html);
    let publisher = &of_type(&blocks, "BlogPosting").unwrap()["publisher"];
    assert_eq!(publisher["name"], "Acme Inc.");
    assert!(publisher.get("logo").is_none());
    site
}

/// `site` whose services know the configured site title `title` (`ferrite.toml`), the
/// fallback of `options.get("site.title")` (ruling X1). Call it before installing.
fn configured(mut site: Site, title: &str) -> Site {
    site.services = std::sync::Arc::new(
        StoreHostServices::new(site.store.clone(), site.reader.clone(), SITE_URL)
            .with_site_key(site.key.clone())
            .with_site_title(title.to_owned()),
    );
    crate::with_generous_deadlines(site)
}

/// The plugin repeats no tag of core's. With no `site.title` option the site name is the
/// configured title, which the host answers through `options.get`.
async fn nothing_core_emitted_is_repeated(site: Site) -> Site {
    let site = configured(site, "Configured Title");
    site.install("seo").await;
    site.load().await;
    let author = site.user("ada", Role::Author).await;
    let id = item(&site, &author, "post", "once", "Once", json!({}), &[]).await;

    let html = render(&site, Some(id), "/", "en").await;
    for key in [
        "og:title",
        "og:description",
        "og:url",
        "og:type",
        "og:site_name",
    ] {
        assert_eq!(prop(&html, key).len(), 1, "{key}: {html}");
    }
    assert_eq!(html.matches("rel=\"canonical\"").count(), 1, "{html}");
    assert_eq!(name(&html, "description").len(), 1, "{html}");
    assert_eq!(prop(&html, "og:site_name"), ["Configured Title"], "{html}");
    let blocks = json_ld(&html);
    assert_eq!(
        of_type(&blocks, "BlogPosting").unwrap()["publisher"]["name"],
        "Configured Title"
    );

    let home = render(&site, None, "/", "en").await;
    assert_eq!(prop(&home, "og:site_name"), ["Configured Title"], "{home}");
    assert_eq!(
        of_type(&json_ld(&home), "WebSite").unwrap()["name"],
        "Configured Title"
    );

    // A stored option wins over the configured title.
    option(&site, "site.title", json!("Stored Title")).await;
    let home = render(&site, None, "/", "en").await;
    assert_eq!(prop(&home, "og:site_name"), ["Stored Title"], "{home}");
    site
}

/// `options.read` names the keys a plugin may read (ruling X1). A plugin granted only
/// `site.title` reads no URL, and one granted only `site.url` reads no title; the tags
/// that depend on the missing option are left out and the rest is still added. (A
/// component that imports `options` is not loaded at all without any `options.read`.)
async fn options_grant_decides_which_dependent_tags_appear(site: Site) -> Site {
    let site = configured(site, "Configured Title");
    let author = site.user("ada", Role::Author).await;
    let id = item(&site, &author, "post", "bare", "Bare", json!({}), &[]).await;
    let hash = site.put_artifact(plugin_wasm("seo")).await;
    let from_manifest = |reads: &str| {
        plugin_manifest("seo").replace(
            "options.read = [\"site.title\", \"site.url\"]",
            &format!("options.read = [{reads}]"),
        )
    };

    // Title only: a site name and a publisher, but no URL: no `WebSite`, no feed link.
    site.row("seo", &from_manifest("\"site.title\""), hash.clone(), true)
        .await;
    assert_eq!(site.load().await.loaded, ["seo"]);
    let home = render(&site, None, "/", "en").await;
    assert_eq!(prop(&home, "og:site_name"), ["Configured Title"], "{home}");
    assert!(!home.contains("application/rss+xml"), "{home}");
    assert!(json_ld(&home).is_empty(), "{home}");
    let html = render(&site, Some(id), "/", "en").await;
    let blocks = json_ld(&html);
    let publisher = &of_type(&blocks, "BlogPosting").unwrap()["publisher"];
    assert_eq!(publisher["name"], "Configured Title");

    // URL only: no site name, so a `WebSite` without a name, the feed link, and a
    // `BlogPosting` without a publisher.
    site.row("seo", &from_manifest("\"site.url\""), hash, true)
        .await;
    assert_eq!(site.load().await.loaded, ["seo"]);
    let home = render(&site, None, "/", "en").await;
    assert!(prop(&home, "og:site_name").is_empty(), "{home}");
    assert!(home.contains(RSS), "{home}");
    let blocks = json_ld(&home);
    let web = of_type(&blocks, "WebSite").unwrap();
    assert_eq!(web["url"], "https://example.com/");
    assert!(web.get("name").is_none());
    let html = render(&site, Some(id), "/", "en").await;
    assert!(prop(&html, "og:site_name").is_empty(), "{html}");
    assert_eq!(prop(&html, "article:author"), ["ada display"]);
    let blocks = json_ld(&html);
    let post = of_type(&blocks, "BlogPosting").unwrap();
    assert_eq!(post["headline"], "Bare");
    assert!(post.get("publisher").is_none());

    // The same enforcement for a key outside the grant: `reader` is granted `site.url`
    // only, and its `options.get("site.title")` is unset.
    let reader = plugin_manifest("reader").replace(
        "options.read      = [\"site.title\"]",
        "options.read      = [\"site.url\"]",
    );
    let hash = site.put_artifact(plugin_wasm("reader")).await;
    site.row("reader", &reader, hash, true).await;
    assert_eq!(site.load().await.loaded.len(), 2);
    let home = render(&site, None, "/", "en").await;
    assert_eq!(name(&home, "reader-option"), ["-"], "{home}");
    site
}

/// An earlier-priority plugin (or core) already put these in the head: the plugin adds
/// none of them again and leaves the earlier values alone. Filter level, with an
/// incoming head that already holds every tag the plugin would add.
async fn tags_already_in_the_head_are_not_repeated(site: Site) -> Site {
    let site = site_with_seo(site).await;
    setting(&site, "twitter_site", json!("ferritecms")).await;
    setting(&site, "google_verification", json!("g")).await;
    setting(&site, "bing_verification", json!("b")).await;
    let author = site.user("ada", Role::Author).await;
    let news = term(&site, "category", "news", "News").await;
    let rust = term(&site, "tag", "rust", "Rust").await;
    let id = item(
        &site,
        &author,
        "post",
        "dup",
        "Dup",
        json!({}),
        &[news, rust],
    )
    .await;

    let og = |p: &str, c: &str| MetaTag {
        name: None,
        property: Some(p.into()),
        content: c.into(),
    };
    let named = |n: &str, c: &str| MetaTag {
        name: Some(n.into()),
        property: None,
        content: c.into(),
    };
    let earlier = Head {
        title: Some("Dup".into()),
        metas: vec![
            og("og:title", "Dup"),
            og("og:site_name", "Earlier"),
            og("og:locale", "xx_XX"),
            og("article:published_time", "2000-01-01T00:00:00Z"),
            og("article:author", "Earlier Author"),
            og("article:section", "Earlier Section"),
            og("article:tag", "Earlier Tag"),
            named("twitter:card", "app"),
            named("twitter:title", "Earlier"),
            named("twitter:description", "Earlier"),
            named("twitter:site", "@earlier"),
            named("google-site-verification", "earlier"),
            named("msvalidate.01", "earlier"),
        ],
        links: vec![LinkTag {
            rel: "alternate".into(),
            href: "https://example.com/other-feed/".into(),
            hreflang: None,
            kind: Some("application/rss+xml".into()),
        }],
        scripts: vec![
            ScriptTag {
                src: None,
                json_ld: Some(r#"{"@type":"BlogPosting","headline":"Earlier"}"#.into()),
            },
            ScriptTag {
                src: None,
                json_ld: Some(r#"{"@type":"WebSite","name":"Earlier"}"#.into()),
            },
        ],
    };

    // A post page (`BlogPosting` present) and the home page (`WebSite` present).
    for (item, path) in [(Some(id), "/dup/"), (None, "/")] {
        let req = PluginRequest::public(path, "en", item);
        let mut trace = RenderTrace::default();
        let FilterValue::Head(out) = ferrite_http::run_filter(
            &site.host,
            "render.head",
            FilterValue::Head(earlier.clone()),
            &req,
            &mut trace,
        )
        .await
        else {
            panic!("render.head returned another variant");
        };
        assert!(
            site.host.failures().is_empty(),
            "{:?}",
            site.host.failures()
        );
        let html = ferrite_http::render_head(&out, crate::SITE);
        for (attr, key, earlier_value) in [
            ("property", "og:site_name", "Earlier"),
            ("property", "og:locale", "xx_XX"),
            ("property", "article:published_time", "2000-01-01T00:00:00Z"),
            ("property", "article:author", "Earlier Author"),
            ("property", "article:section", "Earlier Section"),
            ("property", "article:tag", "Earlier Tag"),
            ("name", "twitter:card", "app"),
            ("name", "twitter:title", "Earlier"),
            ("name", "twitter:description", "Earlier"),
            ("name", "twitter:site", "@earlier"),
            ("name", "google-site-verification", "earlier"),
            ("name", "msvalidate.01", "earlier"),
        ] {
            assert_eq!(metas(&html, attr, key), [earlier_value], "{key}: {html}");
        }
        assert_eq!(html.matches("application/rss+xml").count(), 1, "{html}");
        assert!(html.contains("other-feed"), "{html}");
        // Still the two earlier blocks, none added.
        let blocks = json_ld(&html);
        assert_eq!(blocks.len(), 2, "{html}");
        assert_eq!(
            of_type(&blocks, "BlogPosting").unwrap()["headline"],
            "Earlier"
        );
        assert_eq!(of_type(&blocks, "WebSite").unwrap()["name"], "Earlier");
    }
    site
}
