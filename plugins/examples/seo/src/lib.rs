//! SEO example plugin: the `render.head` filter.
//!
//! Core already emits the title, description, canonical, robots (on items) and the
//! `og:title`, `og:description`, `og:url`, `og:type` and `og:image` tags. This plugin
//! adds what core leaves out, and never repeats a tag that is already in the head it
//! receives (core's, or an earlier plugin's):
//!
//! * `og:site_name`, `og:locale`;
//! * Twitter cards (`twitter:card` follows the `og:image`);
//! * on a `post`: `article:published_time`, `article:author`, `article:section` and
//!   one `article:tag` per tag;
//! * JSON-LD: `WebSite` with a `SearchAction` on `/`, `BlogPosting` on a post;
//! * Google and Bing verification metas, the RSS discovery link.
//!
//! JSON-LD is built with `serde_json`, so a title holding quotes or `</script>` stays
//! valid JSON; the host escapes `<`, `>` and `&` again before it renders the block.

use ferrite_sdk::{Ctx, Head, Post, Result, options};
use serde_json::{Value, json};

const RSS: &str = "application/rss+xml";

fn head(mut head: Head, ctx: &Ctx) -> Result<Head> {
    let request = ctx.request();
    let post = ctx.current_post().filter(|p| p.kind == "post");
    let site_url = site_url(&head, &request.path);
    let site_title = site_title(&head, ctx, post.as_ref());

    if let Some(title) = &site_title
        && !head.has_meta_property("og:site_name")
    {
        head.og("og:site_name", title);
    }
    let locale = request.locale.trim().replace('-', "_");
    if !locale.is_empty() && !head.has_meta_property("og:locale") {
        head.og("og:locale", &locale);
    }

    twitter(&mut head, ctx);
    if let Some(post) = &post {
        article(&mut head, post);
    }

    let org = organization(ctx, site_title.as_deref());
    if let Some(site_url) = &site_url {
        if request.path == "/" && !has_json_ld(&head, "WebSite") {
            head.json_ld(&website(site_url, site_title.as_deref()).to_string());
        }
        if setting_bool(ctx, "feed_link", true) && !has_rss_link(&head) {
            head.link_typed("alternate", &format!("{site_url}/feed/"), RSS);
        }
    }
    if let Some(post) = &post
        && !has_json_ld(&head, "BlogPosting")
    {
        let image = head.meta_property("og:image").map(str::to_owned);
        head.json_ld(&blog_posting(post, image.as_deref(), org).to_string());
    }

    for (name, key) in [
        ("google-site-verification", "google_verification"),
        ("msvalidate.01", "bing_verification"),
    ] {
        if let Some(token) = setting_text(ctx, key)
            && !head.has_meta_name(name)
        {
            head.meta(name, &token);
        }
    }
    Ok(head)
}

/// `twitter:card` (large when core found an image), the title and description copied
/// from Open Graph, and `twitter:site` from the setting.
fn twitter(head: &mut Head, ctx: &Ctx) {
    if !head.has_meta_name("twitter:card") {
        let card = if head.has_meta_property("og:image") {
            "summary_large_image"
        } else {
            "summary"
        };
        head.meta("twitter:card", card);
    }
    for (name, og) in [
        ("twitter:title", "og:title"),
        ("twitter:description", "og:description"),
    ] {
        if !head.has_meta_name(name)
            && let Some(value) = head.meta_property(og).map(str::to_owned)
        {
            head.meta(name, &value);
        }
    }
    if let Some(handle) = setting_text(ctx, "twitter_site") {
        let handle = handle.trim_start_matches('@');
        if !handle.is_empty() && !head.has_meta_name("twitter:site") {
            head.meta("twitter:site", &format!("@{handle}"));
        }
    }
}

/// The publish time (RFC 3339) of a published post.
fn published(post: &Post) -> Option<&str> {
    post.published.as_deref().filter(|p| !p.is_empty())
}

/// The `article:*` tags of a post (a repeated `article:tag` counts as present once).
fn article(head: &mut Head, post: &Post) {
    if let Some(published) = published(post)
        && !head.has_meta_property("article:published_time")
    {
        head.og("article:published_time", published);
    }
    if !post.author.is_empty() && !head.has_meta_property("article:author") {
        head.og("article:author", &post.author);
    }
    if !head.has_meta_property("article:section")
        && let Some(section) = post.terms.iter().find(|t| t.taxonomy == "category")
    {
        head.og("article:section", &section.name);
    }
    if !head.has_meta_property("article:tag") {
        // Core's tag taxonomy is `tag` (the importer maps WordPress `post_tag` to it).
        for tag in post
            .terms
            .iter()
            .filter(|t| matches!(t.taxonomy.as_str(), "tag" | "post_tag"))
        {
            head.og("article:tag", &tag.name);
        }
    }
}

/// The site name: the `site.title` option, else what core's own head says. Core's
/// `og:title` is the site title on a page without an item; on an item the head title
/// is `<item> – <site>` (unless the item has its own SEO title, which names no site).
fn site_title(head: &Head, ctx: &Ctx, post: Option<&Post>) -> Option<String> {
    if let Some(title) = options::get("site.title")
        && !title.trim().is_empty()
    {
        return Some(title.trim().to_owned());
    }
    if ctx.request().post.is_none() {
        return head.meta_property("og:title").map(str::to_owned);
    }
    let prefix = format!("{} – ", post?.title.trim());
    let title = head.as_wit().title.as_deref()?.strip_prefix(&prefix)?;
    Some(title.to_owned()).filter(|t| !t.is_empty())
}

/// The site URL without a trailing slash. The `site.url` option when the host gives it,
/// else core's canonical link without the page path, else its origin (a canonical
/// replaced by the item's own SEO canonical may name another site).
fn site_url(head: &Head, path: &str) -> Option<String> {
    if let Some(url) = options::get("site.url").map(|u| u.trim().trim_end_matches('/').to_owned())
        && url.starts_with("http")
    {
        return Some(url);
    }
    let canonical = head
        .as_wit()
        .links
        .iter()
        .find(|l| l.rel == "canonical")?
        .href
        .as_str();
    if let Some(base) = canonical.strip_suffix(path)
        && base.starts_with("http")
    {
        return Some(base.trim_end_matches('/').to_owned());
    }
    let after_scheme = canonical.find("://")? + 3;
    let end = canonical[after_scheme..]
        .find('/')
        .map_or(canonical.len(), |i| after_scheme + i);
    Some(canonical[..end].to_owned())
}

fn website(site_url: &str, name: Option<&str>) -> Value {
    let mut site = json!({
        "@context": "https://schema.org",
        "@type": "WebSite",
        "url": format!("{site_url}/"),
        "potentialAction": {
            "@type": "SearchAction",
            "target": format!("{site_url}/?s={{search_term_string}}"),
            "query-input": "required name=search_term_string",
        },
    });
    if let Some(name) = name {
        site["name"] = json!(name);
    }
    site
}

fn blog_posting(post: &Post, image: Option<&str>, publisher: Option<Value>) -> Value {
    let mut ld = json!({
        "@context": "https://schema.org",
        "@type": "BlogPosting",
        "headline": post.title,
        "url": post.url,
    });
    if let Some(published) = published(post) {
        ld["datePublished"] = json!(published);
    }
    if !post.author.is_empty() {
        ld["author"] = json!({"@type": "Person", "name": post.author});
    }
    if let Some(image) = image {
        ld["image"] = json!(image);
    }
    if let Some(publisher) = publisher {
        ld["publisher"] = publisher;
    }
    ld
}

/// The publisher: `organization_name`, else the site title; the logo only from an
/// `https` URL. No name, no publisher.
fn organization(ctx: &Ctx, site_title: Option<&str>) -> Option<Value> {
    let name = setting_text(ctx, "organization_name").or_else(|| site_title.map(str::to_owned))?;
    let mut org = json!({"@type": "Organization", "name": name});
    if let Some(logo) = setting_text(ctx, "organization_logo")
        && logo.starts_with("https://")
    {
        org["logo"] = json!({"@type": "ImageObject", "url": logo});
    }
    Some(org)
}

/// True when a JSON-LD block of `kind` is already in the head.
fn has_json_ld(head: &Head, kind: &str) -> bool {
    head.as_wit()
        .scripts
        .iter()
        .filter_map(|s| s.json_ld.as_deref())
        .filter_map(|json| serde_json::from_str::<Value>(json).ok())
        .any(|v| v["@type"] == kind)
}

fn has_rss_link(head: &Head) -> bool {
    head.as_wit().links.iter().any(|l| {
        l.rel
            .split_ascii_whitespace()
            .any(|t| t.eq_ignore_ascii_case("alternate"))
            && l.kind
                .as_deref()
                .is_some_and(|k| k.eq_ignore_ascii_case(RSS))
    })
}

/// A text setting, trimmed; `None` when unset or blank.
fn setting_text(ctx: &Ctx, key: &str) -> Option<String> {
    let value: Value = serde_json::from_str(&ctx.setting(key)?).ok()?;
    let text = value.as_str()?.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

fn setting_bool(ctx: &Ctx, key: &str, default: bool) -> bool {
    ctx.setting(key)
        .and_then(|json| serde_json::from_str::<Value>(&json).ok())
        .and_then(|v| v.as_bool())
        .unwrap_or(default)
}

ferrite_sdk::export_plugin! {
    filters: { "render.head" => head },
}
