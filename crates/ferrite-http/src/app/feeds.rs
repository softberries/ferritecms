//! Machine-readable routes: RSS 2.0 feeds, sitemaps and `robots.txt`. All go through
//! the page cache like HTML pages, with the dependency keys of the reads behind them.
//!
//! * **Feeds** (`/feed/`, `/{taxonomy}/{slug}/feed/`): the latest
//!   [`FEED_LEN`](ferrite_core::site::FEED_LEN) public items from `SiteReader::feed`;
//!   channel title (`<term> – <site>` for a term feed), link, an
//!   `atom:link rel="self"` to the feed itself, and `lastBuildDate` = the newest
//!   `updated_at` of the items; per item title, absolute link, `guid
//!   isPermaLink="true"` (the link), `pubDate` (RFC 2822, UTC) and the excerpt as
//!   `description`. Plugin content filters do not run on feeds. An unknown term, or
//!   one without public posts, is 404 (R7). Atom feeds are not offered yet.
//! * **Sitemaps**: `/sitemap.xml` indexes `/sitemap-<kind>-<n>.xml` for every public
//!   content type with public items; each file lists up to
//!   [`SITEMAP_FILE_LEN`](ferrite_core::site::SITEMAP_FILE_LEN) URLs with `lastmod`
//!   (published only, via `SiteReader::sitemap`); the static front page is listed as
//!   `/`. `SiteReader::sitemap_files` gives, from one store query per type that loads
//!   no rows (M4-4 P76), each file's start cursor and `lastmod` (the newest
//!   `updated_at` of its items): the index is built from it alone, a file seeks to its
//!   start and walks only its own rows, and a file number past the last one is 404
//!   without walking. The set of content types is read per index render but
//!   types are only registered at startup (see `ferrite_core::events`), so a new type
//!   appears after the restart that registers it; no dependency key covers types.
//! * **robots.txt**: `User-agent: *`, `Disallow: /` when option
//!   `reading.discourage_indexing` is `true` (else an empty `Disallow:`), and the
//!   sitemap index URL.
//!
//! XML text and attributes are escaped, and characters that XML 1.0 does not allow
//! (control characters other than tab, line feed and carriage return; U+FFFE, U+FFFF)
//! are dropped.

use std::fmt::Write as _;

use chrono::{DateTime, SecondsFormat, Utc};
use ferrite_cache::CachedPage;
use ferrite_core::{
    CoreError, DepKeys, FeedSource,
    events::option_key,
    permalink::{encode_segment, term_path},
};
use ferrite_domain::{MEDIA_KIND, Timestamp};

use super::{
    AppState,
    pipeline::{RenderError, cached_page},
};
use crate::route::SitemapPart;

/// Option: `true` asks search engines not to index the site.
pub const DISCOURAGE_INDEXING_OPTION: &str = "reading.discourage_indexing";
const RSS: &str = "application/rss+xml; charset=utf-8";
const XML: &str = "application/xml; charset=utf-8";
const TEXT: &str = "text/plain; charset=utf-8";
/// The RSS feed of `source`; `None` for an unknown or empty term.
pub(crate) async fn feed(
    state: &AppState,
    source: &FeedSource,
) -> Result<Option<CachedPage>, RenderError> {
    let mut deps = DepKeys::default();
    let read = state.reader.feed(source).await?;
    deps.extend(read.deps);
    let Some(items) = read.value else {
        return Ok(None);
    };
    let site = state.site_texts(&mut deps).await?;
    let base = &state.config.site_url;
    let (channel_title, channel_link, self_link) = match source {
        FeedSource::Kind(_) => (
            site.title.clone(),
            format!("{base}/"),
            format!("{base}/feed/"),
        ),
        FeedSource::Term { taxonomy, slug } => {
            let path = term_path(taxonomy, slug);
            let name = items
                .iter()
                .flat_map(|i| &i.terms)
                .find(|t| t.taxonomy == *taxonomy && t.slug == *slug)
                .map_or(slug.as_str(), |t| t.name.as_str());
            (
                format!("{name} – {}", site.title),
                format!("{base}{path}"),
                format!("{base}{path}feed/"),
            )
        }
    };
    let mut out = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <rss version=\"2.0\" xmlns:atom=\"http://www.w3.org/2005/Atom\"><channel>",
    );
    tag(&mut out, "title", &channel_title);
    tag(&mut out, "link", &channel_link);
    let _ = write!(
        out,
        "<atom:link href=\"{}\" rel=\"self\" type=\"application/rss+xml\"/>",
        escape(&self_link)
    );
    tag(&mut out, "description", &site.description);
    tag(&mut out, "language", &site.locale);
    if let Some(latest) = items.iter().map(|i| i.item.updated_at).max() {
        tag(&mut out, "lastBuildDate", &rfc2822(latest));
    }
    for item in &items {
        let link = format!("{base}{}", item.url);
        out.push_str("<item>");
        tag(&mut out, "title", &item.item.title);
        tag(&mut out, "link", &link);
        out.push_str("<guid isPermaLink=\"true\">");
        out.push_str(&escape(&link));
        out.push_str("</guid>");
        if let Some(t) = item.item.published_at {
            tag(&mut out, "pubDate", &rfc2822(t));
        }
        tag(&mut out, "description", &item.item.excerpt);
        out.push_str("</item>");
    }
    out.push_str("</channel></rss>\n");
    Ok(Some(cached_page(RSS, out, deps)))
}

/// A sitemap file; `None` for a type without public items or a file number past the
/// last one.
pub(crate) async fn sitemap(
    state: &AppState,
    part: &SitemapPart,
) -> Result<Option<CachedPage>, RenderError> {
    let mut deps = DepKeys::default();
    let base = &state.config.site_url;
    match part {
        SitemapPart::Index => {
            let mut out = String::from(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
                 <sitemapindex xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">",
            );
            let kinds: Vec<String> = state
                .store
                .read()
                .types()
                .list()
                .await
                .map_err(CoreError::from)?
                .into_iter()
                .map(|t| t.name)
                .filter(|n| n != MEDIA_KIND)
                .collect();
            for kind in kinds {
                let files = state.reader.sitemap_files(&kind).await?;
                deps.extend(files.deps);
                for (n, file) in (1..).zip(&files.value) {
                    out.push_str("<sitemap>");
                    let loc = format!("{base}/sitemap-{}-{n}.xml", encode_segment(&kind));
                    tag(&mut out, "loc", &loc);
                    tag(&mut out, "lastmod", &w3c(file.lastmod));
                    out.push_str("</sitemap>");
                }
            }
            out.push_str("</sitemapindex>\n");
            Ok(Some(cached_page(XML, out, deps)))
        }
        SitemapPart::Kind { kind, n } => {
            // Past the last file: 404 from the bounds, before walking any rows; else
            // seek to the file's start and walk its rows only.
            let files = state.reader.sitemap_files(kind).await?;
            deps.extend(files.deps);
            let index = n.checked_sub(1).and_then(|i| usize::try_from(i).ok());
            let Some(file) = index.and_then(|i| files.value.get(i)) else {
                return Ok(None);
            };
            let read = state.reader.sitemap(kind, file.after.as_deref()).await?;
            deps.extend(read.deps);
            if read.value.entries.is_empty() {
                return Ok(None);
            }
            let mut out = String::from(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
                 <urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">",
            );
            for entry in &read.value.entries {
                out.push_str("<url>");
                tag(&mut out, "loc", &format!("{base}{}", entry.url));
                tag(&mut out, "lastmod", &w3c(entry.lastmod));
                out.push_str("</url>");
            }
            out.push_str("</urlset>\n");
            Ok(Some(cached_page(XML, out, deps)))
        }
    }
}

/// `robots.txt`.
pub(crate) async fn robots(state: &AppState) -> Result<CachedPage, RenderError> {
    let mut deps = DepKeys::default();
    deps.insert(option_key(DISCOURAGE_INDEXING_OPTION));
    let discourage = state
        .store
        .read()
        .options()
        .get(DISCOURAGE_INDEXING_OPTION)
        .await
        .map_err(CoreError::from)?
        .is_some_and(|e| e.value.as_bool() == Some(true));
    let mut out = String::from("User-agent: *\n");
    out.push_str(if discourage {
        "Disallow: /\n"
    } else {
        "Disallow:\n"
    });
    let _ = writeln!(out, "\nSitemap: {}/sitemap.xml", state.config.site_url);
    Ok(cached_page(TEXT, out, deps))
}

fn tag(out: &mut String, name: &str, text: &str) {
    let _ = write!(out, "<{name}>{}</{name}>", escape(text));
}

/// XML-escaped `s` without the characters XML 1.0 forbids.
pub(crate) fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            '\t' | '\n' | '\r' => out.push(c),
            c if (c as u32) < 0x20 || c == '\u{FFFE}' || c == '\u{FFFF}' => {}
            c => out.push(c),
        }
    }
    out
}

fn utc(t: Timestamp) -> DateTime<Utc> {
    DateTime::from_timestamp_micros(t.as_micros()).unwrap_or_default()
}

fn rfc2822(t: Timestamp) -> String {
    utc(t).to_rfc2822()
}

/// W3C datetime (sitemaps), UTC to the second.
fn w3c(t: Timestamp) -> String {
    utc(t).to_rfc3339_opts(SecondsFormat::Secs, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_drops_invalid_xml_chars() {
        assert_eq!(
            escape("a & <b> \"c\" 'd'"),
            "a &amp; &lt;b&gt; &quot;c&quot; &apos;d&apos;"
        );
        assert_eq!(escape("x\u{0}\u{1}\u{8}\u{B}\u{C}\u{1F}y"), "xy");
        assert_eq!(escape("t\tn\nr\r"), "t\tn\nr\r");
        assert_eq!(escape("\u{FFFE}\u{FFFF}ok\u{10000}"), "ok\u{10000}");
        assert_eq!(escape("é☃"), "é☃");
    }

    #[test]
    fn dates() {
        let t = Timestamp::from_micros(1_700_000_000_000_000).unwrap();
        assert_eq!(rfc2822(t), "Tue, 14 Nov 2023 22:13:20 +0000");
        assert_eq!(w3c(t), "2023-11-14T22:13:20Z");
    }
}
