//! The iterative walk over one block document (design 16 §4.3): images, headings,
//! links, tables and `html` blocks, in document order, with the item's caps.

use std::collections::HashSet;

use ferrite_domain::ContentId;
use serde_json::{Map, Value, json};

use super::rules::known;
use super::{
    Finding, FoundLink, Links, Location, MAX_CONTEXT, MAX_ITEM_FINDINGS, MAX_ITEM_LINKS, MAX_KEY,
    MediaRef, clean, cut, vague,
};
use crate::blocks::{self, MAX_DEPTH};

/// The scheme of an inline media image's `src`.
const MEDIA_SCHEME: &str = "ferrite:media/";

/// Which document of the item is walked.
#[derive(Debug, Clone)]
pub(super) enum Place {
    Body,
    Field(String),
}

impl Place {
    fn at(&self, path: &[u32]) -> Location {
        match self {
            Place::Body => Location::Body {
                path: path.to_vec(),
            },
            Place::Field(name) => Location::Field {
                name: name.clone(),
                path: path.to_vec(),
            },
        }
    }
}

/// Where an `href` leads.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Target {
    /// `#fragment` in the same document.
    Anchor(String),
    /// A path (and query) on this site.
    Internal(String),
    /// An absolute `http(s)` URL elsewhere (no fragment).
    External(String),
    /// `mailto:`, `tel:`, a relative path and the like: not checked.
    Other,
}

/// Classifies `href` against the site's origin (`https://example.com`, as
/// `url::Origin::ascii_serialization` spells it; empty when unknown).
pub(super) fn classify(href: &str, origin: &str) -> Target {
    let href = href.trim();
    if let Some(frag) = href.strip_prefix('#') {
        return Target::Anchor(frag.to_owned());
    }
    if href.starts_with('/') && !href.starts_with("//") && !href.starts_with("/\\") {
        let path = href.split('#').next().unwrap_or(href);
        return Target::Internal(path.to_owned());
    }
    let absolute = if href.starts_with("//") {
        format!("https:{href}")
    } else {
        href.to_owned()
    };
    let Ok(mut url) = url::Url::parse(&absolute) else {
        return Target::Other;
    };
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Target::Other;
    }
    url.set_fragment(None);
    if !origin.is_empty() && url.origin().ascii_serialization() == origin {
        let mut path = url.path().to_owned();
        if let Some(q) = url.query() {
            path.push('?');
            path.push_str(q);
        }
        return Target::Internal(path);
    }
    Target::External(url.into())
}

/// Plain text under `root` (each `hardBreak` a space), bounded by depth; iterative.
fn text_of(root: &Value) -> String {
    let mut out = String::new();
    let mut stack = vec![(root, 0usize)];
    while let Some((v, depth)) = stack.pop() {
        if depth > MAX_DEPTH {
            continue;
        }
        let Some(map) = v.as_object() else { continue };
        match map.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(t) = map.get("text").and_then(Value::as_str) {
                    out.push_str(t);
                }
            }
            Some("hardBreak") => out.push(' '),
            _ => {
                if let Some(Value::Array(items)) = map.get("content") {
                    stack.extend(items.iter().rev().map(|c| (c, depth + 1)));
                }
            }
        }
    }
    out
}

/// The children of a table row (any node type), with their indices.
fn cells(row: &Value) -> Vec<(usize, &Value)> {
    match row.get("content") {
        Some(Value::Array(items)) => items.iter().enumerate().collect(),
        _ => Vec::new(),
    }
}

fn node_type(v: &Value) -> &str {
    v.get("type").and_then(Value::as_str).unwrap_or("")
}

fn attr<'a>(map: &'a Map<String, Value>, key: &str) -> Option<&'a Value> {
    map.get("attrs")?.as_object()?.get(key)
}

fn attr_str<'a>(map: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    attr(map, key)?.as_str()
}

/// The `href` of a text node's first `link` mark.
fn link_href(v: &Value) -> Option<&str> {
    v.get("marks")?
        .as_array()?
        .iter()
        .find(|m| node_type(m) == "link")
        .and_then(|m| m.as_object())
        .and_then(|m| attr_str(m, "href"))
}

/// Whether `alt` reads as a file or camera name, or a generic word (design 16 §3).
pub(super) fn alt_is_filename(alt: &str) -> bool {
    let lower = alt.trim().to_ascii_lowercase();
    const EXT: &[&str] = &[".jpg", ".jpeg", ".png", ".gif", ".webp", ".avif", ".svg"];
    const WORDS: &[&str] = &["image", "photo", "picture", "img", "graphic"];
    if EXT.iter().any(|e| lower.ends_with(e)) || WORDS.contains(&lower.as_str()) {
        return true;
    }
    let camera = |prefix: &str| {
        lower.strip_prefix(prefix).is_some_and(|rest| {
            let rest = rest.trim_start_matches(['_', '-']);
            !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit() || b == b'_')
        })
    };
    camera("img") || camera("dsc") || camera("pxl") || camera("dscn") || camera("dji")
}

fn is_bare_url(text: &str) -> bool {
    (text.starts_with("http://") || text.starts_with("https://"))
        && !text.chars().any(char::is_whitespace)
}

fn context(s: &str) -> Option<String> {
    Some(cut(s, MAX_CONTEXT))
}

/// Why the walk stopped early (`check-truncated`'s `detail.reason`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stop {
    Depth,
    Findings,
    Links,
}

/// One item's walk state: shared by its body and its rich-text fields, so the caps and
/// the table and `html` counters cover the whole item.
pub(super) struct Walker<'a> {
    locale: &'a str,
    origin: String,
    pub(super) findings: Vec<Finding>,
    pub(super) links: Links,
    pub(super) media: Vec<MediaRef>,
    link_count: usize,
    stopped: Option<Stop>,
    tables: u32,
    htmls: u32,
}

impl<'a> Walker<'a> {
    pub(super) fn new(locale: &'a str, site_url: &str) -> Self {
        let origin = url::Url::parse(site_url)
            .map(|u| u.origin().ascii_serialization())
            .unwrap_or_default();
        Self {
            locale,
            origin: if origin == "null" {
                String::new()
            } else {
                origin
            },
            findings: Vec::new(),
            links: Links::default(),
            media: Vec::new(),
            link_count: 0,
            stopped: None,
            tables: 0,
            htmls: 0,
        }
    }

    pub(super) fn stopped(&self) -> bool {
        self.stopped.is_some()
    }

    fn stop(&mut self, why: Stop) {
        if self.stopped.is_some() {
            return;
        }
        self.stopped = Some(why);
        let reason = match why {
            Stop::Depth => "depth",
            Stop::Findings => "findings",
            Stop::Links => "links",
        };
        self.findings.push(Finding {
            rule: known("check-truncated"),
            location: Location::Item,
            key: String::new(),
            context: None,
            detail: json!({ "reason": reason }),
        });
    }

    pub(super) fn push(
        &mut self,
        rule: &'static str,
        location: Location,
        key: &str,
        ctx: Option<String>,
        detail: Value,
    ) {
        if self.stopped() {
            return;
        }
        if self.findings.len() >= MAX_ITEM_FINDINGS {
            self.stop(Stop::Findings);
            return;
        }
        self.findings.push(Finding {
            rule: known(rule),
            location,
            key: cut(key, MAX_KEY),
            context: ctx,
            detail,
        });
    }

    /// Records the featured image's media id.
    pub(super) fn featured(&mut self, id: ContentId) {
        self.media.push(MediaRef {
            id,
            location: Location::FeaturedImage,
            key: crate::fields::FEATURED_IMAGE.to_owned(),
            featured: true,
        });
    }

    /// Walks one document. `levels`: check heading levels (the body only; the title is
    /// level 1).
    pub(super) fn doc(&mut self, doc: &Value, place: &Place, levels: bool) {
        if self.stopped() {
            return;
        }
        let ids: HashSet<String> = blocks::heading_ids(doc).into_iter().collect();
        let mut previous_level = 1u64;
        let mut stack: Vec<(&Value, Vec<u32>, usize)> = vec![(doc, Vec::new(), 0)];
        while let Some((v, path, depth)) = stack.pop() {
            if self.stopped() {
                return;
            }
            if depth > MAX_DEPTH {
                self.stop(Stop::Depth);
                return;
            }
            let Some(map) = v.as_object() else { continue };
            match node_type(v) {
                "image" => self.image(map, place.at(&path)),
                "heading" => {
                    if let Some(level) = attr(map, "level")
                        .and_then(Value::as_u64)
                        .filter(|l| (1..=6).contains(l))
                    {
                        let text = clean(&text_of(v));
                        let key = format!("h{level}:{text}");
                        if text.is_empty() {
                            self.push(
                                "heading-empty",
                                place.at(&path),
                                &key,
                                None,
                                json!({ "level": level }),
                            );
                        }
                        if levels {
                            if level > previous_level + 1 {
                                self.push(
                                    "heading-level-skipped",
                                    place.at(&path),
                                    &key,
                                    context(&text),
                                    json!({ "level": level, "previous": previous_level }),
                                );
                            }
                            previous_level = level;
                        }
                    }
                }
                "table" => self.table(map, place, &path),
                "html" => {
                    self.htmls += 1;
                    let key = format!("html:{}", self.htmls);
                    self.push("html-unchecked", place.at(&path), &key, None, json!({}));
                }
                // A code block renders its text only: nothing inside is a link or image.
                "codeBlock" | "text" => continue,
                _ => {}
            }
            if let Some(Value::Array(children)) = map.get("content") {
                self.inline_links(children, place, &path, &ids);
                for (i, c) in children.iter().enumerate().rev() {
                    let mut p = path.clone();
                    p.push(u32::try_from(i).unwrap_or(u32::MAX));
                    stack.push((c, p, depth + 1));
                }
            }
        }
    }

    fn image(&mut self, map: &Map<String, Value>, at: Location) {
        let Some(src) = attr_str(map, "src").filter(|s| !s.trim().is_empty()) else {
            // No `src`: the renderer writes nothing.
            return;
        };
        if let Some(id) = src.strip_prefix(MEDIA_SCHEME)
            && let Ok(id) = id.parse::<ContentId>()
        {
            self.media.push(MediaRef {
                id,
                location: at.clone(),
                key: cut(src, MAX_KEY),
                featured: false,
            });
        }
        if attr(map, "decorative").and_then(Value::as_bool) == Some(true) {
            return;
        }
        let detail = json!({ "src": cut(src, MAX_KEY) });
        match attr_str(map, "alt") {
            None => self.push("image-alt-missing", at, src, None, detail),
            Some(alt) => {
                let alt = clean(alt);
                if alt.is_empty() {
                    self.push("image-alt-empty", at, src, None, detail);
                } else if alt_is_filename(&alt) {
                    self.push("image-alt-filename", at, src, context(&alt), detail);
                }
            }
        }
    }

    fn table(&mut self, map: &Map<String, Value>, place: &Place, path: &[u32]) {
        self.tables += 1;
        let key = format!("table:{}", self.tables);
        let rows: Vec<(usize, &Value)> = match map.get("content") {
            Some(Value::Array(items)) => items
                .iter()
                .enumerate()
                .filter(|(_, r)| node_type(r) == "tableRow")
                .collect(),
            _ => Vec::new(),
        };
        if rows.is_empty() {
            return;
        }
        let header_row = cells(rows[0].1)
            .iter()
            .any(|(_, c)| node_type(c) == "tableHeader");
        let header_column = rows.iter().all(|(_, r)| {
            cells(r)
                .first()
                .is_some_and(|(_, c)| node_type(c) == "tableHeader")
        });
        if !header_row && !header_column {
            self.push(
                "table-header-missing",
                place.at(path),
                &key,
                None,
                json!({ "table": self.tables }),
            );
        }
        for (ri, row) in &rows {
            for (ci, cell) in cells(row) {
                if node_type(cell) == "tableHeader" && clean(&text_of(cell)).is_empty() {
                    let mut p = path.to_vec();
                    p.push(u32::try_from(*ri).unwrap_or(u32::MAX));
                    p.push(u32::try_from(ci).unwrap_or(u32::MAX));
                    self.push(
                        "table-header-empty",
                        place.at(&p),
                        &key,
                        None,
                        json!({ "table": self.tables }),
                    );
                }
            }
        }
    }

    /// The links among `children` of one parent: runs of consecutive text nodes whose
    /// first `link` mark has the same `href`, their texts joined.
    fn inline_links(
        &mut self,
        children: &[Value],
        place: &Place,
        path: &[u32],
        ids: &HashSet<String>,
    ) {
        let mut run: Option<(usize, &str, String)> = None;
        for (i, c) in children.iter().enumerate() {
            let href = (node_type(c) == "text").then(|| link_href(c)).flatten();
            match (&mut run, href) {
                (Some((_, h, text)), Some(href)) if *h == href => {
                    if let Some(t) = c.get("text").and_then(Value::as_str) {
                        text.push_str(t);
                    }
                }
                (_, next) => {
                    if let Some((at, h, text)) = run.take() {
                        self.link(at, h, &text, place, path, ids);
                    }
                    if let Some(href) = next {
                        let text = c.get("text").and_then(Value::as_str).unwrap_or("");
                        run = Some((i, href, text.to_owned()));
                    }
                }
            }
            if self.stopped() {
                return;
            }
        }
        if let Some((at, h, text)) = run {
            self.link(at, h, &text, place, path, ids);
        }
    }

    fn link(
        &mut self,
        index: usize,
        href: &str,
        text: &str,
        place: &Place,
        path: &[u32],
        ids: &HashSet<String>,
    ) {
        if self.stopped() {
            return;
        }
        if self.link_count >= MAX_ITEM_LINKS {
            self.stop(Stop::Links);
            return;
        }
        self.link_count += 1;
        let mut p = path.to_vec();
        p.push(u32::try_from(index).unwrap_or(u32::MAX));
        let at = place.at(&p);
        let text = clean(text);
        let found = FoundLink {
            href: href.to_owned(),
            target: String::new(),
            text: text.clone(),
            location: at.clone(),
        };
        let key = found.key();
        let detail = json!({ "href": cut(href, MAX_KEY) });
        if text.is_empty() {
            self.push(
                "link-text-empty",
                at.clone(),
                &key,
                context(href),
                detail.clone(),
            );
        } else if vague::is_vague(&text, self.locale) {
            self.push(
                "link-text-vague",
                at.clone(),
                &key,
                context(&text),
                detail.clone(),
            );
        } else if is_bare_url(&text) {
            self.push(
                "link-text-url",
                at.clone(),
                &key,
                context(&text),
                detail.clone(),
            );
        }
        match classify(href, &self.origin) {
            Target::Anchor(frag) => {
                let decoded = percent_encoding::percent_decode_str(&frag).decode_utf8_lossy();
                if !frag.is_empty() && !ids.contains(&frag) && !ids.contains(decoded.as_ref()) {
                    self.push("link-anchor-missing", at, &key, context(href), detail);
                }
            }
            Target::Internal(target) => self.links.internal.push(FoundLink { target, ..found }),
            Target::External(target) => self.links.external.push(FoundLink { target, ..found }),
            Target::Other => {}
        }
    }
}
