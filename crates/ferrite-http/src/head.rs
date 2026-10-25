//! The page `<head>`: core SEO tags, then the `render.head` plugin filter, then HTML
//! rendered by the host with full escaping (design 05 §7). Plugins return a structured
//! [`Head`]; they cannot put arbitrary markup into `<head>`.
//!
//! Rendering rules:
//!
//! * Every attribute value and the title are escaped (`& < > " '`).
//! * A meta needs a `name` or a `property`; a link needs an `http(s)://` or
//!   root-relative (`/…`, not `//…`) `href`, and `https://` or root-relative when its
//!   `rel` loads a resource (stylesheet, preload, modulepreload, prefetch, icon,
//!   apple-touch-icon, manifest). Others are dropped (logged at `debug`).
//! * `script-tag.src` must be `https://…`, a URL on the site's own origin, or
//!   root-relative (`/…`, not `//…`); no backslashes, whitespace or control characters.
//!   The src is resolved against `site.url` the way a browser does (`Url::join`: host
//!   IDNA mapping, numeric IPv4 forms, default ports, dot segments; R24 N1), and a
//!   src that resolves onto the site's host (any scheme or port, a trailing dot
//!   ignored) may not point under `/_p/` (plugin routes) or `/_media/` (uploads): both
//!   serve bytes a plugin or an author controls, and a head script runs with the
//!   site's origin (ruling R22). The resolved path is checked after percent-decoding
//!   (repeatedly), lowercasing, collapsing `//` and resolving `.`/`..`, from the root
//!   and from the site URL's base path.
//!   `/_theme/…` and external `https://` hosts stay allowed. Others are dropped and
//!   logged (`warn`).
//! * `script-tag.json-ld` must parse as JSON. It is re-serialised compactly and every
//!   `<`, `>` and `&` is written as a `\u00XX` escape, so neither `</script>` nor
//!   `<!--` can occur inside the element. Invalid JSON is dropped and logged.
//! * Size and count caps (64 KiB, 256 metas, 64 links, 16 scripts) are enforced by the
//!   plugin host on every plugin's output (05 §7), so the rendered head is bounded.

use ferrite_core::PageItem;
use ferrite_plugin_host::{FilterValue, Head, LinkTag, MetaTag, PluginHost, ScriptTag};
use ferrite_store::Store;

use crate::plugins::{PluginRequest, RenderTrace, run_filter};

/// Longest meta description derived from an excerpt, in characters.
pub const DESCRIPTION_CHARS: usize = 160;
/// Meta key holding an item's SEO description (JSON string), set by editors or an SEO
/// plugin (`content.meta.write = ["seo.*"]`).
pub const SEO_DESCRIPTION_META: &str = "seo.description";

/// Site-wide inputs of the head.
#[derive(Debug, Clone, Copy)]
pub struct HeadSite<'a> {
    pub title: &'a str,
    /// Absolute site URL (`https://example.com`), trailing slash optional.
    pub url: &'a str,
    /// Tagline, used as the description of pages that show no single item.
    pub description: &'a str,
}

/// What the page shows.
#[derive(Debug, Clone, Copy)]
pub struct HeadPage<'a> {
    /// The single item, if the page shows one.
    pub item: Option<&'a PageItem>,
    /// Canonical path of the page (`/about/`, `/category/news/page/2/`).
    pub path: &'a str,
}

/// Core head, plugin filter and rendering for one page. Reads the item's
/// `seo.description` meta (falling back to its excerpt). Never fails: a store error or
/// a failing plugin leaves the core tags, and either marks `trace` volatile.
pub async fn page_head(
    host: &PluginHost,
    store: &dyn Store,
    site: HeadSite<'_>,
    page: HeadPage<'_>,
    req: &PluginRequest,
    trace: &mut RenderTrace,
) -> String {
    let seo = match page.item {
        Some(item) => match seo_description(store, item).await {
            Ok(seo) => seo,
            Err(e) => {
                tracing::warn!(error = %e, "reading seo.description failed");
                // May be transient: the excerpt fallback must not stay cached.
                trace.volatile = true;
                None
            }
        },
        None => None,
    };
    let core = core_head(site, page, seo.as_deref());
    let head = match run_filter(
        host,
        "render.head",
        FilterValue::Head(core.clone()),
        req,
        trace,
    )
    .await
    {
        FilterValue::Head(h) => h,
        // The host returns the variant it was given.
        _ => core,
    };
    render_head(&head, site)
}

async fn seo_description(
    store: &dyn Store,
    item: &PageItem,
) -> Result<Option<String>, ferrite_store::StoreError> {
    Ok(store
        .read()
        .meta()
        .get(item.item.id, SEO_DESCRIPTION_META)
        .await?
        .and_then(|v| v.as_str().map(str::to_owned))
        .filter(|s| !s.trim().is_empty()))
}

/// The core SEO head: title (`<item> – <site>` or the site title), meta description
/// (`seo` text, else the excerpt cut to [`DESCRIPTION_CHARS`], else the site
/// tagline on item-less pages), canonical link, and Open Graph title, description,
/// URL, type (`article` for items, `website` otherwise) and image (featured media).
pub fn core_head(site: HeadSite<'_>, page: HeadPage<'_>, seo: Option<&str>) -> Head {
    let canonical = absolute(site.url, page.path);
    let item = page.item;
    let title = match item.map(|i| i.item.title.trim()) {
        Some(t) if !t.is_empty() => format!("{t} – {}", site.title),
        _ => site.title.to_owned(),
    };
    let description = match (seo, item) {
        (Some(s), _) => truncate(s),
        (None, Some(i)) => truncate(&i.item.excerpt),
        (None, None) => truncate(site.description),
    };
    let og_title = item
        .map(|i| i.item.title.trim())
        .filter(|t| !t.is_empty())
        .unwrap_or(site.title);
    let mut metas = Vec::new();
    if !description.is_empty() {
        metas.push(name_meta("description", &description));
    }
    metas.push(og("og:title", og_title));
    if !description.is_empty() {
        metas.push(og("og:description", &description));
    }
    metas.push(og("og:url", &canonical));
    metas.push(og(
        "og:type",
        if item.is_some() { "article" } else { "website" },
    ));
    if let Some(image) = item.and_then(|i| i.featured.as_ref()) {
        metas.push(og("og:image", &absolute(site.url, &image.url)));
    }
    Head {
        title: Some(title),
        metas,
        links: vec![LinkTag {
            rel: "canonical".to_owned(),
            href: canonical,
            hreflang: None,
            kind: None,
        }],
        scripts: Vec::new(),
    }
}

fn name_meta(name: &str, content: &str) -> MetaTag {
    MetaTag {
        name: Some(name.to_owned()),
        property: None,
        content: content.to_owned(),
    }
}

fn og(property: &str, content: &str) -> MetaTag {
    MetaTag {
        name: None,
        property: Some(property.to_owned()),
        content: content.to_owned(),
    }
}

/// Whitespace collapsed, at most [`DESCRIPTION_CHARS`] characters (the last one `…`
/// when cut).
fn truncate(text: &str) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= DESCRIPTION_CHARS {
        return collapsed;
    }
    let mut cut: String = collapsed.chars().take(DESCRIPTION_CHARS - 1).collect();
    cut.truncate(cut.trim_end().len());
    cut.push('…');
    cut
}

/// `path` on the site, unless it is already an absolute `http(s)` URL.
fn absolute(site_url: &str, path: &str) -> String {
    if path.starts_with("https://") || path.starts_with("http://") {
        return path.to_owned();
    }
    format!("{}{path}", site_url.trim_end_matches('/'))
}

/// Render `head` as HTML (see the module docs for the rules). A `None` title falls
/// back to the site title.
pub fn render_head(head: &Head, site: HeadSite<'_>) -> String {
    let mut out = String::new();
    let title = head.title.as_deref().unwrap_or(site.title);
    out.push_str(&format!("<title>{}</title>\n", escape(title)));
    for m in &head.metas {
        render_meta(m, &mut out);
    }
    for l in &head.links {
        render_link(l, &mut out);
    }
    for s in &head.scripts {
        render_script(s, site.url, &mut out);
    }
    out
}

fn render_meta(m: &MetaTag, out: &mut String) {
    if m.name.is_none() && m.property.is_none() {
        tracing::debug!("head meta without name or property dropped");
        return;
    }
    out.push_str("<meta");
    attr(out, "name", m.name.as_deref());
    attr(out, "property", m.property.as_deref());
    attr(out, "content", Some(&m.content));
    out.push_str(">\n");
}

/// `rel` tokens that make the browser fetch (and, for stylesheets/preloads, apply)
/// the target: plain `http://` would be mixed content.
const LOADING_RELS: [&str; 7] = [
    "stylesheet",
    "preload",
    "modulepreload",
    "prefetch",
    "icon",
    "apple-touch-icon",
    "manifest",
];

fn render_link(l: &LinkTag, out: &mut String) {
    let loads = l
        .rel
        .split_ascii_whitespace()
        .any(|t| LOADING_RELS.iter().any(|r| t.eq_ignore_ascii_case(r)));
    let href_ok = l.href.starts_with("https://")
        || (!loads && l.href.starts_with("http://"))
        || is_root_relative(&l.href);
    if !href_ok || !clean_url(&l.href) || l.rel.trim().is_empty() {
        tracing::debug!(rel = %l.rel, "head link with a disallowed href or empty rel dropped");
        return;
    }
    out.push_str("<link");
    attr(out, "rel", Some(&l.rel));
    attr(out, "href", Some(&l.href));
    attr(out, "hreflang", l.hreflang.as_deref());
    attr(out, "type", l.kind.as_deref());
    out.push_str(">\n");
}

fn render_script(s: &ScriptTag, site_url: &str, out: &mut String) {
    if let Some(src) = &s.src {
        if script_src_allowed(src, site_url) {
            out.push_str("<script");
            attr(out, "src", Some(src));
            out.push_str("></script>\n");
        } else {
            tracing::warn!(src = %truncate(src), "head script with a disallowed src dropped");
        }
    }
    if let Some(json) = &s.json_ld {
        match json_ld(json) {
            Some(body) => {
                out.push_str("<script type=\"application/ld+json\">");
                out.push_str(&body);
                out.push_str("</script>\n");
            }
            None => tracing::warn!("head json-ld that is not valid JSON dropped"),
        }
    }
}

fn is_root_relative(url: &str) -> bool {
    url.starts_with('/') && !url.starts_with("//")
}

/// No backslash (browsers read `/\host` as `//host`), whitespace or control characters.
fn clean_url(url: &str) -> bool {
    !url.chars()
        .any(|c| c == '\\' || c.is_whitespace() || c.is_control())
}

/// `https://…`, the site's own origin, or root-relative, and never a same-site path
/// under [`SCRIPT_REFUSED_PREFIXES`]. The src is resolved against `site_url` as a
/// browser resolves it (R24 N1); a src or site URL that does not resolve is refused.
fn script_src_allowed(src: &str, site_url: &str) -> bool {
    // Shape first: an absolute URL (it has a scheme) or root-relative text; never
    // protocol-relative `//…` or path-relative `a.js`.
    let absolute = url::Url::parse(src).is_ok();
    if !clean_url(src) || !(absolute || is_root_relative(src)) {
        return false;
    }
    let Ok(site) = url::Url::parse(site_url) else {
        return false;
    };
    let Ok(url) = site.join(src) else {
        return false;
    };
    let same_host = host_key(&url).is_some() && host_key(&url) == host_key(&site);
    let same_origin = same_host
        && url.scheme() == site.scheme()
        && url.port_or_known_default() == site.port_or_known_default();
    if url.scheme() != "https" && !same_origin {
        return false;
    }
    !(same_host && refused_script_path(url.path(), site.path()))
}

/// First path segments a same-site head script may not load from (ruling R22): plugin
/// routes and uploaded media serve bytes that plugins and authors control.
const SCRIPT_REFUSED_PREFIXES: [&str; 2] = ["_p", "_media"];

/// The host as `url` parsed it (lowercase, IDNA-mapped, IPv4 canonical), without a
/// trailing dot; `None` without a host.
fn host_key(url: &url::Url) -> Option<&str> {
    url.host_str().map(|h| h.trim_end_matches('.'))
}

/// Percent-decodes until nothing changes (at most 8 rounds; a value still changing
/// after that is returned as is, and checked as is).
fn decode(s: &str) -> String {
    let mut cur = s.to_owned();
    for _ in 0..8 {
        let next = percent_encoding::percent_decode_str(&cur)
            .decode_utf8_lossy()
            .into_owned();
        if next == cur {
            break;
        }
        cur = next;
    }
    cur
}

/// The segments of `path` (query and fragment cut) after decoding, lowercasing,
/// treating `\` as `/`, dropping empty and `.` segments and resolving `..`.
fn normalised_segments(path: &str) -> Vec<String> {
    let path = path.split(['?', '#']).next().unwrap_or_default();
    let decoded = decode(path).to_lowercase().replace('\\', "/");
    let mut out: Vec<String> = Vec::new();
    for seg in decoded.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            s => out.push(s.to_owned()),
        }
    }
    out
}

/// Whether `path` (as normalised) starts with a refused prefix, from the root or from
/// the site's base path `base`.
fn refused_script_path(path: &str, base: &str) -> bool {
    let segs = normalised_segments(path);
    let base = normalised_segments(base);
    let refused =
        |s: Option<&String>| s.is_some_and(|s| SCRIPT_REFUSED_PREFIXES.contains(&s.as_str()));
    refused(segs.first()) || (segs.starts_with(&base) && refused(segs.get(base.len())))
}

/// Valid JSON re-serialised with `<`, `>` and `&` escaped (still valid JSON).
fn json_ld(src: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(src).ok()?;
    let compact = value.to_string();
    let mut out = String::with_capacity(compact.len());
    for c in compact.chars() {
        match c {
            '<' => out.push_str("\\u003c"),
            '>' => out.push_str("\\u003e"),
            '&' => out.push_str("\\u0026"),
            c => out.push(c),
        }
    }
    Some(out)
}

fn attr(out: &mut String, name: &str, value: Option<&str>) {
    if let Some(v) = value {
        out.push_str(&format!(" {name}=\"{}\"", escape(v)));
    }
}

/// Escape for text and double- or single-quoted attribute values.
fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#x27;"),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests;
