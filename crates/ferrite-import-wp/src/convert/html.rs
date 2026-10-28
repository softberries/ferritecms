//! HTML to the block tree.
//!
//! The HTML is parsed as a `<body>` fragment by html5ever (through scraper) and walked once.
//! Inline content collects into a paragraph (or heading) until a block element ends it; an
//! image or another block found inside inline content ends the paragraph, goes after it,
//! and the rest of the text starts a new one. Whitespace collapses as a browser would,
//! except in `<pre>`.

use ferrite_core::blocks::{canonical_embed_url, embed_id, safe_href};
use scraper::{ElementRef, Html, Node as Dom};

use super::doc::{Mark, Node};
use super::{ImageRef, Notes, Rewrite};

/// Elements nested deeper than this contribute their text only.
const MAX_DEPTH: usize = 64;

/// Dropped with everything inside them, and counted.
const DROPPED: &[&str] = &[
    "script", "style", "noscript", "template", "object", "embed", "applet", "canvas", "svg",
    "math", "map", "form", "input", "select", "textarea", "button", "head", "title", "meta",
    "link", "base", "frame", "frameset", "param", "source", "track", "dialog",
];

/// Containers whose content is kept as blocks; they end the paragraph before and after.
const BLOCK_CONTAINERS: &[&str] = &[
    "div",
    "section",
    "article",
    "header",
    "footer",
    "main",
    "aside",
    "nav",
    "address",
    "center",
    "details",
    "summary",
    "hgroup",
    "fieldset",
    "legend",
    "dl",
    "dt",
    "dd",
    "figcaption",
    "caption",
    "li",
    "search",
    "body",
];

/// Inline elements with no meaning in the block tree whose text is kept without a count.
const PLAIN_INLINE: &[&str] = &[
    "span", "abbr", "acronym", "cite", "q", "small", "big", "time", "dfn", "var", "bdi", "bdo",
    "data", "label", "wbr", "nobr", "ruby", "rb", "rt", "rp",
];

/// Converts an HTML fragment to block nodes.
///
/// Links and image sources go through `rw`; links that are not safe hrefs keep their text
/// only. h1 becomes h2. Formatting the tree has no mark for keeps its text and is counted
/// in `notes`, and so are dropped elements. NUL is stripped before parsing.
pub fn convert(html: &str, rw: &dyn Rewrite, notes: &mut Notes) -> Vec<Node> {
    let html = html.replace('\0', "");
    let doc = Html::parse_fragment(&html);
    let mut c = Conv { rw, notes };
    let mut flow = Flow::default();
    c.children(doc.root_element(), &mut flow, &Style::default(), 0);
    flow.finish()
}

/// The marks in effect.
#[derive(Debug, Clone, Default)]
struct Style {
    bold: bool,
    italic: bool,
    strike: bool,
    code: bool,
    link: Option<Mark>,
}

impl Style {
    fn has_other_than_code(&self) -> bool {
        self.bold || self.italic || self.strike || self.link.is_some()
    }

    /// `code` excludes every other mark in the editor; next to one it is dropped.
    fn marks(&self) -> Vec<Mark> {
        let mut m = Vec::new();
        if let Some(link) = &self.link {
            m.push(link.clone());
        }
        if self.bold {
            m.push(Mark::Bold);
        }
        if self.code && !self.has_other_than_code() {
            m.push(Mark::Code);
        }
        if self.italic {
            m.push(Mark::Italic);
        }
        if self.strike {
            m.push(Mark::Strike);
        }
        m
    }
}

/// The blocks of one container, with the inline content not yet closed into a paragraph.
#[derive(Debug, Default)]
struct Flow {
    blocks: Vec<Node>,
    inline: Vec<Node>,
    /// The level of the heading being filled; `None` for a paragraph.
    heading: Option<u8>,
}

fn is_html_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0c')
}

/// Runs of HTML whitespace as one space.
fn collapse(raw: &str) -> String {
    let mut s = String::with_capacity(raw.len());
    let mut space = false;
    for c in raw.chars() {
        if is_html_space(c) {
            space = true;
        } else {
            if space {
                s.push(' ');
            }
            space = false;
            s.push(c);
        }
    }
    if space {
        s.push(' ');
    }
    s
}

impl Flow {
    fn text(&mut self, raw: &str, marks: Vec<Mark>) {
        let s = collapse(raw);
        let line_start = match self.inline.last() {
            None | Some(Node::HardBreak) => true,
            Some(Node::Text { text, .. }) => text.ends_with(' '),
            Some(_) => false,
        };
        let s = if line_start {
            s.trim_start_matches(' ')
        } else {
            &s
        };
        if s.is_empty() {
            return;
        }
        if let Some(Node::Text { text, marks: m }) = self.inline.last_mut()
            && *m == marks
        {
            text.push_str(s);
            return;
        }
        self.inline.push(Node::Text {
            text: s.to_owned(),
            marks,
        });
    }

    fn hard_break(&mut self) {
        self.trim_end();
        self.inline.push(Node::HardBreak);
    }

    /// Drops trailing spaces of the inline content.
    fn trim_end(&mut self) {
        if let Some(Node::Text { text, .. }) = self.inline.last_mut() {
            let len = text.trim_end_matches(' ').len();
            text.truncate(len);
            if text.is_empty() {
                self.inline.pop();
            }
        }
    }

    /// Closes the inline content into a paragraph or heading; one without text is dropped.
    fn flush(&mut self) {
        loop {
            self.trim_end();
            if matches!(self.inline.last(), Some(Node::HardBreak)) {
                self.inline.pop();
            } else {
                break;
            }
        }
        let mut inline = std::mem::take(&mut self.inline);
        let lead = inline
            .iter()
            .take_while(|n| matches!(n, Node::HardBreak))
            .count();
        inline.drain(..lead);
        if !inline.iter().any(|n| matches!(n, Node::Text { .. })) {
            return;
        }
        self.blocks.push(match self.heading {
            Some(level) => Node::Heading {
                level,
                content: inline,
            },
            None => Node::Paragraph(inline),
        });
    }

    fn block(&mut self, n: Node) {
        self.flush();
        self.blocks.push(n);
    }

    fn finish(mut self) -> Vec<Node> {
        self.flush();
        self.blocks
    }
}

struct Conv<'a> {
    rw: &'a dyn Rewrite,
    notes: &'a mut Notes,
}

impl Conv<'_> {
    fn children(&mut self, e: ElementRef<'_>, flow: &mut Flow, style: &Style, depth: usize) {
        for child in e.children() {
            match child.value() {
                Dom::Text(t) => self.text(flow, &t.text, style),
                Dom::Element(_) => {
                    if let Some(el) = ElementRef::wrap(child) {
                        self.element(el, flow, style, depth + 1);
                    }
                }
                _ => {}
            }
        }
    }

    fn text(&mut self, flow: &mut Flow, raw: &str, style: &Style) {
        if style.code && style.has_other_than_code() && !raw.trim().is_empty() {
            self.notes.dropped_mark("code");
        }
        flow.text(raw, style.marks());
    }

    /// The content of `e` as its own list of blocks.
    fn blocks(&mut self, e: ElementRef<'_>, style: &Style, depth: usize) -> Vec<Node> {
        let mut flow = Flow::default();
        self.children(e, &mut flow, style, depth);
        flow.finish()
    }

    fn element(&mut self, e: ElementRef<'_>, flow: &mut Flow, style: &Style, depth: usize) {
        let name = e.value().name();
        if DROPPED.contains(&name) {
            self.notes.unsupported_tag(name);
            return;
        }
        if depth > MAX_DEPTH {
            self.text(flow, &visible_text(e), style);
            return;
        }
        self.presentation(e, name);
        let mut inner = style.clone();
        match name {
            "p" => {
                flow.flush();
                let outer = flow.heading.take();
                self.children(e, flow, style, depth);
                flow.flush();
                flow.heading = outer;
            }
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                let level = name.as_bytes()[1] - b'0';
                if level == 1 {
                    self.notes.h1_demoted();
                }
                flow.flush();
                let outer = flow.heading.replace(level.max(2));
                self.children(e, flow, style, depth);
                flow.flush();
                flow.heading = outer;
            }
            "br" => flow.hard_break(),
            "hr" => flow.block(Node::HorizontalRule),
            "ul" | "ol" | "menu" => {
                flow.flush();
                if let Some(list) = self.list(e, style, depth) {
                    flow.block(list);
                }
            }
            "blockquote" => {
                flow.flush();
                let content = self.blocks(e, style, depth);
                if !content.is_empty() {
                    flow.block(Node::Blockquote(content));
                }
            }
            "pre" | "listing" | "xmp" => {
                flow.flush();
                if let Some(code) = code_block(e) {
                    flow.block(code);
                }
            }
            "table" => self.table(e, flow, style, depth),
            "img" => self.image(e, flow, style),
            "figure" => self.figure(e, flow, style, depth),
            "iframe" => self.iframe(e, flow),
            "video" | "audio" => {
                self.notes.unsupported_tag(name);
                let src = e.attr("src").map(str::to_owned).or_else(|| {
                    e.descendent_elements()
                        .filter(|d| d.value().name() == "source")
                        .find_map(|d| d.attr("src").map(str::to_owned))
                });
                if let Some(src) = src {
                    self.link_paragraph(flow, &src);
                }
            }
            "a" => {
                if let Some(raw) = e.attr("href") {
                    let href = self.rw.href(raw.trim());
                    inner.link = match safe_href(&href) {
                        Some(href) => Some(Mark::Link {
                            href,
                            target: (e.attr("target") == Some("_blank"))
                                .then(|| "_blank".to_owned()),
                        }),
                        None => {
                            self.notes.dropped_mark("link");
                            None
                        }
                    };
                }
                self.children(e, flow, &inner, depth);
            }
            "strong" | "b" => {
                inner.bold = true;
                self.children(e, flow, &inner, depth);
            }
            "em" | "i" => {
                inner.italic = true;
                self.children(e, flow, &inner, depth);
            }
            "s" | "strike" | "del" => {
                inner.strike = true;
                self.children(e, flow, &inner, depth);
            }
            "code" | "kbd" | "samp" | "tt" => {
                inner.code = true;
                self.children(e, flow, &inner, depth);
            }
            "u" | "ins" | "sup" | "sub" | "mark" | "font" => {
                let mark = match name {
                    "u" | "ins" => "underline",
                    "mark" => "highlight",
                    other => other,
                };
                self.notes.dropped_mark(mark);
                self.children(e, flow, style, depth);
            }
            _ if PLAIN_INLINE.contains(&name) => self.children(e, flow, style, depth),
            _ if BLOCK_CONTAINERS.contains(&name) => {
                flow.flush();
                self.children(e, flow, style, depth);
                flow.flush();
            }
            _ => {
                self.notes.unsupported_tag(name);
                self.children(e, flow, style, depth);
            }
        }
    }

    /// Counts the presentation `e` loses (P11): `alignment` for `<center>`, an `align`
    /// attribute, WP's `align*` and `has-text-align-*` classes and `text-align` in `style`;
    /// `style` for any other inline CSS.
    fn presentation(&mut self, e: ElementRef<'_>, name: &str) {
        let mut aligned = name == "center" || e.attr("align").is_some_and(|a| !a.trim().is_empty());
        if let Some(class) = e.attr("class") {
            aligned |= class.split_ascii_whitespace().any(|c| {
                matches!(c, "aligncenter" | "alignleft" | "alignright" | "alignnone")
                    || c.starts_with("has-text-align-")
            });
        }
        let mut other_css = false;
        for decl in e.attr("style").unwrap_or("").split(';') {
            let prop = decl.split(':').next().unwrap_or("").trim();
            if prop.eq_ignore_ascii_case("text-align") {
                aligned = true;
            } else if !prop.is_empty() {
                other_css = true;
            }
        }
        if aligned {
            self.notes.dropped_mark("alignment");
        }
        if other_css {
            self.notes.dropped_mark("style");
        }
    }

    fn list(&mut self, e: ElementRef<'_>, style: &Style, depth: usize) -> Option<Node> {
        let mut items: Vec<Node> = Vec::new();
        for child in e.children() {
            match child.value() {
                Dom::Text(t) if !t.text.trim().is_empty() => {
                    let mut flow = Flow::default();
                    self.text(&mut flow, &t.text, style);
                    items.push(list_item(flow.finish()));
                }
                Dom::Element(_) => {
                    let Some(el) = ElementRef::wrap(child) else {
                        continue;
                    };
                    let name = el.value().name();
                    if name == "li" {
                        items.push(list_item(self.blocks(el, style, depth + 1)));
                        continue;
                    }
                    let mut flow = Flow::default();
                    self.element(el, &mut flow, style, depth + 1);
                    let blocks = flow.finish();
                    // Old WP bodies nest a list straight in a list: it joins the item before.
                    if matches!(name, "ul" | "ol")
                        && let Some(Node::ListItem(prev)) = items.last_mut()
                    {
                        prev.extend(blocks);
                    } else if !blocks.is_empty() {
                        items.push(list_item(blocks));
                    }
                }
                _ => {}
            }
        }
        if items.is_empty() {
            return None;
        }
        Some(if e.value().name() == "ol" {
            let start = e
                .attr("start")
                .and_then(|s| s.trim().parse::<i64>().ok())
                .unwrap_or(1)
                .max(1);
            Node::OrderedList {
                start: start.unsigned_abs(),
                content: items,
            }
        } else {
            Node::BulletList(items)
        })
    }

    fn table(&mut self, e: ElementRef<'_>, flow: &mut Flow, style: &Style, depth: usize) {
        flow.flush();
        let mut rows = Vec::new();
        let mut captions = Vec::new();
        for child in e.child_elements() {
            match child.value().name() {
                "tr" => rows.extend(self.row(child, style, depth + 1)),
                "thead" | "tbody" | "tfoot" => {
                    for tr in child.child_elements() {
                        if tr.value().name() == "tr" {
                            rows.extend(self.row(tr, style, depth + 2));
                        }
                    }
                }
                "caption" => captions.extend(self.blocks(child, style, depth + 1)),
                _ => {}
            }
        }
        if !rows.is_empty() {
            flow.block(Node::Table(rows));
        }
        flow.blocks.extend(captions);
    }

    fn row(&mut self, tr: ElementRef<'_>, style: &Style, depth: usize) -> Option<Node> {
        let mut cells = Vec::new();
        for cell in tr.child_elements() {
            let header = match cell.value().name() {
                "th" => true,
                "td" => false,
                _ => continue,
            };
            let mut content = self.blocks(cell, style, depth + 1);
            if content.is_empty() {
                content.push(Node::Paragraph(Vec::new()));
            }
            let colspan = span(cell.attr("colspan"));
            let rowspan = span(cell.attr("rowspan"));
            cells.push(if header {
                Node::TableHeader {
                    colspan,
                    rowspan,
                    content,
                }
            } else {
                Node::TableCell {
                    colspan,
                    rowspan,
                    content,
                }
            });
        }
        (!cells.is_empty()).then_some(Node::TableRow(cells))
    }

    fn image(&mut self, e: ElementRef<'_>, flow: &mut Flow, style: &Style) {
        let raw = e.attr("src").map_or("", str::trim);
        let src = if raw.is_empty() {
            None
        } else {
            match self.rw.image(raw) {
                ImageRef::Media(id) => Some(format!("ferrite:media/{id}")),
                ImageRef::Url(u) => image_src(&u),
                ImageRef::Missing(u) => {
                    let src = image_src(&u);
                    if src.is_some() {
                        self.notes.missing_media();
                    }
                    src
                }
            }
        };
        let Some(src) = src else {
            self.notes.unsupported_tag("img");
            return;
        };
        if style.link.is_some() {
            self.notes.dropped_mark("link");
        }
        flow.block(Node::Image {
            src,
            alt: e.attr("alt").map(str::to_owned),
            title: e.attr("title").filter(|t| !t.is_empty()).map(str::to_owned),
            caption: None,
        });
    }

    /// A figure's content, with its `<figcaption>` text as the caption of the last image in
    /// it, or as a paragraph when there is none.
    fn figure(&mut self, e: ElementRef<'_>, flow: &mut Flow, style: &Style, depth: usize) {
        flow.flush();
        let start = flow.blocks.len();
        let mut caption = None;
        for child in e.children() {
            match child.value() {
                Dom::Text(t) => self.text(flow, &t.text, style),
                Dom::Element(_) => {
                    let Some(el) = ElementRef::wrap(child) else {
                        continue;
                    };
                    if el.value().name() == "figcaption" && caption.is_none() {
                        caption = Some(el);
                    } else {
                        self.element(el, flow, style, depth + 1);
                    }
                }
                _ => {}
            }
        }
        flow.flush();
        let Some(fc) = caption else {
            return;
        };
        let text = collapse(&visible_text(fc)).trim().to_owned();
        let image = flow.blocks[start..]
            .iter()
            .rposition(|n| matches!(n, Node::Image { caption: None, .. }));
        if let (Some(i), false) = (image, text.is_empty())
            && let Node::Image { caption, .. } = &mut flow.blocks[start + i]
        {
            *caption = Some(text);
            return;
        }
        self.children(fc, flow, style, depth + 1);
        flow.flush();
    }

    /// A YouTube or Vimeo player becomes an embed; any other frame a link to its source.
    fn iframe(&mut self, e: ElementRef<'_>, flow: &mut Flow) {
        let src = e.attr("src").map_or("", str::trim);
        for provider in ["youtube", "vimeo"] {
            if let Some(url) =
                embed_id(provider, src).and_then(|id| canonical_embed_url(provider, &id))
            {
                flow.block(Node::Embed {
                    provider: provider.to_owned(),
                    url,
                });
                return;
            }
        }
        self.notes.unsupported_tag("iframe");
        self.link_paragraph(flow, src);
    }

    /// A paragraph holding a link to `raw` (rewritten), when it is a safe href.
    fn link_paragraph(&mut self, flow: &mut Flow, raw: &str) {
        let href = self.rw.href(raw.trim());
        if let Some(href) = safe_href(&href) {
            flow.block(Node::Paragraph(vec![Node::Text {
                text: href.clone(),
                marks: vec![Mark::Link { href, target: None }],
            }]));
        }
    }
}

/// An image `src` the renderer shows: http(s) or a local path.
fn image_src(u: &str) -> Option<String> {
    let s = safe_href(u)?;
    let lower = s.to_ascii_lowercase();
    (s.starts_with('/') || lower.starts_with("http://") || lower.starts_with("https://"))
        .then_some(s)
}

fn span(v: Option<&str>) -> u32 {
    v.and_then(|s| s.trim().parse::<u32>().ok())
        .unwrap_or(1)
        .clamp(1, 100)
}

/// A list item; the editor wants a paragraph first.
fn list_item(mut content: Vec<Node>) -> Node {
    if !matches!(content.first(), Some(Node::Paragraph(_))) {
        content.insert(0, Node::Paragraph(Vec::new()));
    }
    Node::ListItem(content)
}

/// The text of `e` outside dropped elements: one pre-order walk that skips dropped
/// subtrees (iterative, so any depth).
fn visible_text(e: ElementRef<'_>) -> String {
    let mut out = String::new();
    let mut next = e.first_child();
    while let Some(n) = next {
        let descend = match n.value() {
            Dom::Text(t) => {
                out.push_str(&t.text);
                false
            }
            Dom::Element(el) => !DROPPED.contains(&el.name()),
            _ => false,
        };
        next = if descend { n.first_child() } else { None };
        let mut cur = n;
        while next.is_none() {
            if let Some(sibling) = cur.next_sibling() {
                next = Some(sibling);
            } else {
                match cur.parent() {
                    Some(p) if p.id() != e.id() => cur = p,
                    _ => break,
                }
            }
        }
    }
    out
}

/// `<pre>` as a code block: its text with whitespace kept, `<br>` as a newline; the language
/// from a `language-x` or `lang-x` class on it or on a `<code>` inside.
fn code_block(e: ElementRef<'_>) -> Option<Node> {
    let mut text = String::new();
    let mut language = None;
    for d in e.descendants() {
        match d.value() {
            Dom::Text(t) => text.push_str(&t.text),
            Dom::Element(el) if el.name() == "br" => text.push('\n'),
            Dom::Element(el) if language.is_none() && matches!(el.name(), "pre" | "code") => {
                language = el.attr("class").and_then(class_language);
            }
            _ => {}
        }
    }
    (!text.is_empty()).then_some(Node::CodeBlock { language, text })
}

/// The renderer's `valid_language`: 1–32 of `[a-z0-9+#-]`.
pub(crate) fn class_language(class: &str) -> Option<String> {
    class.split_ascii_whitespace().find_map(|c| {
        let l = c
            .strip_prefix("language-")
            .or_else(|| c.strip_prefix("lang-"))?
            .to_ascii_lowercase();
        ((1..=32).contains(&l.len())
            && l.bytes().all(|b| {
                b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'+' | b'#' | b'-')
            }))
        .then_some(l)
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use crate::convert::ImageRef;
    use crate::convert::autop::wpautop;
    use crate::convert::doc::{Mark, to_json};
    use crate::model::Record;
    use uuid::Uuid;

    const MEDIA: Uuid = Uuid::from_u128(0x0190_f1c0_0000_7000_8000_0000_0000_0001);

    struct Rw;

    impl Rewrite for Rw {
        fn image(&self, src: &str) -> ImageRef {
            if src.contains("not-in-library") {
                ImageRef::Missing(src.to_owned())
            } else if src.contains("/wp-content/uploads/2026/") {
                ImageRef::Media(MEDIA)
            } else {
                ImageRef::Url(src.to_owned())
            }
        }

        fn attachment(&self, _: i64) -> Option<Uuid> {
            None
        }

        fn href(&self, href: &str) -> String {
            if href.ends_with("/uploads/2026/10/market.jpg") {
                "/media/market.jpg".to_owned()
            } else {
                href.to_owned()
            }
        }
    }

    fn conv(html: &str) -> (Vec<Node>, Notes) {
        let mut notes = Notes {
            item: Some(42),
            ..Notes::default()
        };
        let nodes = convert(html, &Rw, &mut notes);
        (nodes, notes)
    }

    fn t(s: &str) -> Node {
        Node::Text {
            text: s.to_owned(),
            marks: vec![],
        }
    }

    fn tm(s: &str, marks: Vec<Mark>) -> Node {
        Node::Text {
            text: s.to_owned(),
            marks,
        }
    }

    fn p(content: Vec<Node>) -> Node {
        Node::Paragraph(content)
    }

    fn count(map: &std::collections::BTreeMap<String, crate::convert::Tally>, k: &str) -> u64 {
        map.get(k).map_or(0, |t| t.count)
    }

    fn img(src: &str, alt: Option<&str>) -> Node {
        Node::Image {
            src: src.to_owned(),
            alt: alt.map(str::to_owned),
            title: None,
            caption: None,
        }
    }

    #[test]
    fn strong_em_is_bold_and_italic() {
        let (nodes, notes) = conv("<p>a <strong><em>b</em></strong> <b>c</b><i>d</i><s>e</s></p>");
        assert_eq!(
            nodes,
            [p(vec![
                t("a "),
                tm("b", vec![Mark::Bold, Mark::Italic]),
                t(" "),
                tm("c", vec![Mark::Bold]),
                tm("d", vec![Mark::Italic]),
                tm("e", vec![Mark::Strike]),
            ])]
        );
        assert_eq!(
            notes,
            Notes {
                item: Some(42),
                ..Notes::default()
            }
        );
    }

    #[test]
    fn underline_sup_and_styled_span_keep_text_and_count() {
        let (nodes, notes) = conv(
            "<p><u>u</u> x<sup>2</sup><sub>i</sub> <span style=\"color:red\">red</span> <span>plain</span></p>",
        );
        assert_eq!(nodes, [p(vec![t("u x2i red plain")])]);
        assert_eq!(count(&notes.dropped_marks, "underline"), 1);
        assert_eq!(count(&notes.dropped_marks, "sup"), 1);
        assert_eq!(count(&notes.dropped_marks, "sub"), 1);
        assert_eq!(count(&notes.dropped_marks, "style"), 1);
        assert_eq!(notes.dropped_marks["style"].items, [42]);
        assert!(notes.unsupported_tags.is_empty());
    }

    #[test]
    fn h1_becomes_h2_and_is_counted() {
        let (nodes, notes) = conv("<h1>Top</h1><h3>Three</h3>");
        assert_eq!(
            nodes,
            [
                Node::Heading {
                    level: 2,
                    content: vec![t("Top")]
                },
                Node::Heading {
                    level: 3,
                    content: vec![t("Three")]
                },
            ]
        );
        assert_eq!(notes.h1_demoted.count, 1);
    }

    #[test]
    fn unsafe_link_keeps_text_without_link() {
        let (nodes, notes) = conv(
            "<p><a href=\"javascript:alert(1)\">bad</a> <a href=\"https://e.org/\" target=\"_blank\" rel=\"noopener\">ok</a> <a name=\"x\">anchor</a></p>",
        );
        assert_eq!(
            nodes,
            [p(vec![
                t("bad "),
                tm(
                    "ok",
                    vec![Mark::Link {
                        href: "https://e.org/".into(),
                        target: Some("_blank".into())
                    }]
                ),
                t(" anchor"),
            ])]
        );
        assert_eq!(count(&notes.dropped_marks, "link"), 1);
    }

    #[test]
    fn links_go_through_the_rewrite() {
        let (nodes, _) = conv(
            "<p><a href=\"http://old.example/wp-content/uploads/2026/10/market.jpg\">menu</a></p>",
        );
        assert_eq!(
            nodes,
            [p(vec![tm(
                "menu",
                vec![Mark::Link {
                    href: "/media/market.jpg".into(),
                    target: None
                }]
            )])]
        );
    }

    #[test]
    fn image_inside_a_link_drops_the_link() {
        let (nodes, notes) = conv(
            "<p>See <a href=\"/big.jpg\"><img src=\"/small.jpg\" alt=\"a\" title=\"T\"></a>.</p>",
        );
        assert_eq!(
            nodes,
            [
                p(vec![t("See")]),
                Node::Image {
                    src: "/small.jpg".into(),
                    alt: Some("a".into()),
                    title: Some("T".into()),
                    caption: None,
                },
                p(vec![t(".")]),
            ]
        );
        assert_eq!(count(&notes.dropped_marks, "link"), 1);
    }

    #[test]
    fn image_sources() {
        let (nodes, notes) = conv(
            "<img src=\"http://localhost:8099/wp-content/uploads/2026/10/hero.png\" alt=\"\">\
             <img src=\"http://localhost:8099/wp-content/uploads/not-in-library.jpg\">\
             <img src=\"javascript:alert(1)\"><img src=\"data:image/png;base64,AA\"><img>",
        );
        assert_eq!(
            nodes,
            [
                img(&format!("ferrite:media/{MEDIA}"), Some("")),
                img(
                    "http://localhost:8099/wp-content/uploads/not-in-library.jpg",
                    None
                ),
            ]
        );
        assert_eq!(notes.missing_media.count, 1);
        assert_eq!(count(&notes.unsupported_tags, "img"), 3);
    }

    #[test]
    fn figure_caption_goes_on_the_image() {
        let (nodes, _) = conv(
            "<figure class=\"wp-block-image\"><img src=\"/a.png\" alt=\"A\"/><figcaption class=\"wp-element-caption\">The <b>hero</b>,\n medium</figcaption></figure>\
             <figure><figcaption>Lonely</figcaption></figure>",
        );
        assert_eq!(
            nodes,
            [
                Node::Image {
                    src: "/a.png".into(),
                    alt: Some("A".into()),
                    title: None,
                    caption: Some("The hero, medium".into()),
                },
                p(vec![t("Lonely")]),
            ]
        );
    }

    #[test]
    fn table_without_tbody() {
        let (nodes, _) = conv(
            "<table><tr><th colspan=\"2\">H</th></tr><tr><td>x</td><td rowspan=\"500\"></td></tr></table>",
        );
        assert_eq!(
            nodes,
            [Node::Table(vec![
                Node::TableRow(vec![Node::TableHeader {
                    colspan: 2,
                    rowspan: 1,
                    content: vec![p(vec![t("H")])],
                }]),
                Node::TableRow(vec![
                    Node::TableCell {
                        colspan: 1,
                        rowspan: 1,
                        content: vec![p(vec![t("x")])]
                    },
                    Node::TableCell {
                        colspan: 1,
                        rowspan: 100,
                        content: vec![p(vec![])]
                    },
                ]),
            ])]
        );
    }

    #[test]
    fn table_sections_and_caption() {
        let (nodes, _) = conv(
            "<figure class=\"wp-block-table\"><table><caption>Cap</caption><thead><tr><th>Name</th></tr></thead>\
             <tbody><tr><td>a</td></tr></tbody><tfoot><tr><td>f</td></tr></tfoot></table></figure>",
        );
        let Node::Table(rows) = &nodes[0] else {
            panic!("{nodes:?}")
        };
        assert_eq!(rows.len(), 3);
        assert_eq!(nodes[1], p(vec![t("Cap")]));
    }

    #[test]
    fn pre_keeps_whitespace() {
        let (nodes, _) = conv(
            "<pre class=\"wp-block-preformatted\">Preformatted   text\n  keeps <b>spaces</b><br>end</pre>\
             <pre class=\"wp-block-code\"><code class=\"language-rust\">fn main() {\n    println!(\"&lt;hello&gt;\");\n}</code></pre>",
        );
        assert_eq!(
            nodes,
            [
                Node::CodeBlock {
                    language: None,
                    text: "Preformatted   text\n  keeps spaces\nend".into()
                },
                Node::CodeBlock {
                    language: Some("rust".into()),
                    text: "fn main() {\n    println!(\"<hello>\");\n}".into(),
                },
            ]
        );
    }

    #[test]
    fn nested_lists() {
        let (nodes, _) = conv(
            "<ul class=\"wp-block-list\">\n<li>First item</li>\n\n<li>Second item\n<ul><li>Nested item</li></ul></li></ul>\
             <ol start=\"3\"><li>Three</li></ol>",
        );
        let li = |c: Vec<Node>| Node::ListItem(c);
        assert_eq!(
            nodes,
            [
                Node::BulletList(vec![
                    li(vec![p(vec![t("First item")])]),
                    li(vec![
                        p(vec![t("Second item")]),
                        Node::BulletList(vec![li(vec![p(vec![t("Nested item")])])]),
                    ]),
                ]),
                Node::OrderedList {
                    start: 3,
                    content: vec![li(vec![p(vec![t("Three")])])]
                },
            ]
        );
    }

    #[test]
    fn list_nested_directly_in_a_list_joins_the_previous_item() {
        let (nodes, _) = conv("<ul><li>a</li><ul><li>b</li></ul></ul>");
        let li = |c: Vec<Node>| Node::ListItem(c);
        assert_eq!(
            nodes,
            [Node::BulletList(vec![li(vec![
                p(vec![t("a")]),
                Node::BulletList(vec![li(vec![p(vec![t("b")])])]),
            ])])]
        );
    }

    #[test]
    fn paragraphs_inside_list_items() {
        let (nodes, _) =
            conv("<ul><li><p>a</p><p>b</p></li><li><ul><li>c</li></ul></li><li></li></ul>");
        let li = |c: Vec<Node>| Node::ListItem(c);
        assert_eq!(
            nodes,
            [Node::BulletList(vec![
                li(vec![p(vec![t("a")]), p(vec![t("b")])]),
                li(vec![
                    p(vec![]),
                    Node::BulletList(vec![li(vec![p(vec![t("c")])])])
                ]),
                li(vec![p(vec![])]),
            ])]
        );
    }

    #[test]
    fn blockquote_with_cite() {
        let (nodes, _) = conv(
            "<blockquote class=\"wp-block-quote\"><p>A quoted paragraph.</p><cite>Someone</cite></blockquote><blockquote> </blockquote>",
        );
        assert_eq!(
            nodes,
            [Node::Blockquote(vec![
                p(vec![t("A quoted paragraph.")]),
                p(vec![t("Someone")])
            ])]
        );
    }

    #[test]
    fn breaks_and_whitespace() {
        let (nodes, _) =
            conv("<p>\n a \n<br />\n b<br></p><div>x<br><br></div><p> <br> </p><hr class=\"x\"/>");
        assert_eq!(
            nodes,
            [
                p(vec![t("a"), Node::HardBreak, t("b")]),
                p(vec![t("x")]),
                Node::HorizontalRule,
            ]
        );
    }

    #[test]
    fn code_mark_is_dropped_next_to_other_marks() {
        let (nodes, notes) = conv("<p><code>c</code> <b><code>x</code></b></p>");
        assert_eq!(
            nodes,
            [p(vec![
                tm("c", vec![Mark::Code]),
                t(" "),
                tm("x", vec![Mark::Bold])
            ])]
        );
        assert_eq!(count(&notes.dropped_marks, "code"), 1);
    }

    #[test]
    fn unsupported_elements() {
        let (nodes, notes) = conv(
            "<p>a<script>alert(1)</script><style>p{}</style></p><form><input name=x>f</form>\
             <iframe src=\"https://www.youtube.com/embed/dQw4w9WgXcQ\"></iframe>\
             <iframe src=\"https://maps.example/x\"></iframe>\
             <video src=\"http://localhost:8099/wp-content/uploads/2026/10/market.jpg\"></video>\
             <marquee>m</marquee><div class=\"x\"><section>s</section></div>",
        );
        assert_eq!(
            nodes,
            [
                p(vec![t("a")]),
                Node::Embed {
                    provider: "youtube".into(),
                    url: "https://www.youtube.com/watch?v=dQw4w9WgXcQ".into(),
                },
                p(vec![tm(
                    "https://maps.example/x",
                    vec![Mark::Link {
                        href: "https://maps.example/x".into(),
                        target: None
                    }]
                )]),
                p(vec![tm(
                    "/media/market.jpg",
                    vec![Mark::Link {
                        href: "/media/market.jpg".into(),
                        target: None
                    }]
                )]),
                p(vec![t("m")]),
                p(vec![t("s")]),
            ]
        );
        for tag in ["script", "style", "form", "iframe", "video", "marquee"] {
            assert_eq!(count(&notes.unsupported_tags, tag), 1, "{tag}");
        }
        assert_eq!(notes.unsupported_tags.len(), 6);
    }

    #[test]
    fn nul_is_stripped() {
        let (nodes, _) = conv("<p>a\0b</p><img src=\"/x.png\" alt=\"c\0d\">");
        assert_eq!(nodes, [p(vec![t("ab")]), img("/x.png", Some("cd"))]);
    }

    #[test]
    fn deep_nesting_does_not_overflow() {
        let html = format!(
            "{}x{}",
            "<div><span>".repeat(5_000),
            "</span></div>".repeat(5_000)
        );
        let (nodes, _) = conv(&html);
        assert_eq!(nodes, [p(vec![t("x")])]);
    }

    #[test]
    fn site_b_classic_body() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/site-b.xml");
        let bytes = std::fs::read(path).unwrap();
        let mut bodies = Vec::new();
        crate::wxr::parse(&bytes[..], |r| {
            if let Record::Item(i) = r {
                bodies.push((i.wp_id, i.content));
            }
            Ok(())
        })
        .unwrap();
        let (_, classic) = bodies.iter().find(|(id, _)| *id == 5).unwrap();
        let (nodes, notes) = conv(&wpautop(classic));
        assert_eq!(
            nodes[0],
            p(vec![
                t("First paragraph written in the classic editor."),
                Node::HardBreak,
                t("It has a single line break here."),
            ])
        );
        assert!(nodes.contains(&Node::Heading {
            level: 2,
            content: vec![t("An h1 inside the body")]
        }));
        assert!(nodes.contains(&img(
            "http://localhost:8099/wp-content/uploads/not-in-library.jpg",
            Some("missing file")
        )));
        assert!(nodes.contains(&Node::CodeBlock {
            language: None,
            text: "preformatted\n  classic".into()
        }));
        assert!(nodes.contains(&p(vec![t("Styled text2.")])));
        assert_eq!(notes.h1_demoted.count, 1);
        assert_eq!(notes.missing_media.count, 1);
        assert_eq!(count(&notes.dropped_marks, "sup"), 1);
        assert_eq!(count(&notes.dropped_marks, "style"), 1);
        assert_eq!(count(&notes.dropped_marks, "alignment"), 1);
        for (_, body) in &bodies {
            let (nodes, _) = conv(&wpautop(body));
            assert!(ferrite_core::blocks::is_editor_safe(&to_json(&nodes)));
        }
    }

    #[test]
    fn every_corpus_body_is_editor_safe_without_nul() {
        for file in ["site-a.xml", "site-b.xml"] {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/corpus")
                .join(file);
            let bytes = std::fs::read(path).unwrap();
            crate::wxr::parse(&bytes[..], |r| {
                if let Record::Item(i) = r {
                    for body in [i.content.clone(), wpautop(&i.content)] {
                        let doc = to_json(&conv(&body).0);
                        assert!(
                            ferrite_core::blocks::is_editor_safe(&doc),
                            "{file} {}",
                            i.wp_id
                        );
                        assert!(!doc.to_string().contains("\\u0000"));
                    }
                }
                Ok(())
            })
            .unwrap();
        }
    }

    #[test]
    fn alignment_is_counted_and_content_kept() {
        let (nodes, notes) = conv(
            "<center>c</center><p align=\"right\">r</p>\
             <img class=\"aligncenter size-full\" src=\"/a.png\" alt=\"\">\
             <figure class=\"wp-block-image alignleft\"><img src=\"/b.png\" alt=\"\"></figure>\
             <p class=\"has-text-align-center\">t</p>\
             <p style=\"text-align:center; color: red\">s</p><p class=\"other\">plain</p>",
        );
        assert_eq!(
            nodes,
            [
                p(vec![t("c")]),
                p(vec![t("r")]),
                img("/a.png", Some("")),
                img("/b.png", Some("")),
                p(vec![t("t")]),
                p(vec![t("s")]),
                p(vec![t("plain")]),
            ]
        );
        assert_eq!(count(&notes.dropped_marks, "alignment"), 6);
        assert_eq!(count(&notes.dropped_marks, "style"), 1);
    }

    #[test]
    fn visible_text_skips_dropped_elements_past_the_depth_cap() {
        let html = format!(
            "{}a<script>evil()</script><b>b</b>{}",
            "<div>".repeat(100),
            "</div>".repeat(100)
        );
        let (nodes, _) = conv(&html);
        assert_eq!(nodes, [p(vec![t("ab")])]);
    }

    #[test]
    fn wide_deep_text_stays_linear() {
        let html = format!(
            "{}{}{}",
            "<div>".repeat(5_000),
            "<b>x</b>".repeat(100_000),
            "</div>".repeat(5_000)
        );
        let start = std::time::Instant::now();
        let (nodes, _) = conv(&html);
        assert_eq!(nodes.len(), 1);
        assert!(start.elapsed() < std::time::Duration::from_secs(10));
    }
}
