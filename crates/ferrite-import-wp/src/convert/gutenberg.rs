//! The Gutenberg block-comment grammar, as WordPress's `parse_blocks` reads it.
//!
//! A block is `<!-- wp:ns/name {json} -->…<!-- /wp:ns/name -->`, or void as
//! `<!-- wp:name {json} /-->`. A bare name gets the `core/` namespace. HTML outside any block
//! becomes a `core/freeform` block (whitespace-only runs are dropped).

use serde_json::{Map, Value};

use super::NextMatch;

/// One parsed block.
#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    /// Always namespaced: `core/paragraph`, `acme/fancy-callout`.
    pub name: String,
    pub attrs: Map<String, Value>,
    pub inner: Vec<Block>,
    /// The block's own HTML interleaved with its children, like WP's `innerContent`.
    pub inner_html: Vec<HtmlPart>,
    /// The comment carried attributes that are not a JSON object; `attrs` is then empty.
    pub bad_attrs: bool,
}

/// A piece of a block's inner content.
#[derive(Debug, Clone, PartialEq)]
pub enum HtmlPart {
    Html(String),
    /// The index of a child in [`Block::inner`].
    Child(usize),
}

/// Blocks nested deeper than this are read as text, so later recursion stays bounded.
const MAX_DEPTH: usize = 64;

/// Whether `content` holds block comments (WP's `has_blocks`).
pub fn has_blocks(content: &str) -> bool {
    content.contains("<!-- wp:")
}

/// Parses `content` into its top-level blocks.
///
/// Like WP: an unclosed block is closed at the end of the content. Unlike WP, which stops at
/// the first stray closer and keeps the rest as freeform, a closer that matches no open block
/// is kept as text and parsing goes on; a closer of an outer block closes the inner ones.
pub fn parse(content: &str) -> Vec<Block> {
    let mut p = Parser {
        src: content,
        out: Vec::new(),
        stack: Vec::new(),
        free: String::new(),
        skipped: Vec::new(),
    };
    let mut scan = Scanner::new(content);
    let mut pos = 0;
    while let Some(d) = scan.next_delimiter(pos) {
        p.text(pos, d.start);
        p.delimiter(d);
        pos = d.end;
    }
    p.text(pos, content.len());
    while !p.stack.is_empty() {
        p.close_top();
    }
    p.flush_free();
    p.out
}

/// A block comment found in the source: `src[start..end]`.
#[derive(Debug, Clone, Copy)]
struct Delimiter<'a> {
    start: usize,
    end: usize,
    closer: bool,
    namespace: Option<&'a str>,
    name: &'a str,
    attrs: Option<&'a str>,
    void: bool,
}

impl Delimiter<'_> {
    fn full_name(&self) -> String {
        format!("{}/{}", self.namespace.unwrap_or("core"), self.name)
    }
}

struct Scanner<'a> {
    src: &'a str,
    /// `-->` searches, memoised so a run of `<!--` without any `-->` stays linear.
    close: NextMatch<'a>,
}

impl<'a> Scanner<'a> {
    fn new(src: &'a str) -> Self {
        Self {
            src,
            close: NextMatch::new(src, "-->"),
        }
    }

    /// The next block comment at or after `from`; other `<!--` are skipped as text.
    fn next_delimiter(&mut self, from: usize) -> Option<Delimiter<'a>> {
        let mut at = from;
        while let Some(rel) = self.src[at..].find("<!--") {
            let start = at + rel;
            if let Some(d) = self.delimiter_at(start) {
                return Some(d);
            }
            at = start + 4;
        }
        None
    }

    /// The WP tokenizer at one `<!--`:
    /// `<!--\s+(/)?wp:(ns/)?name\s+({json}\s+)?(/)?-->`.
    fn delimiter_at(&mut self, start: usize) -> Option<Delimiter<'a>> {
        let src = self.src;
        let b = src.as_bytes();
        let mut i = start + 4;
        i = skip_ws(b, i, true)?;
        let closer = b.get(i) == Some(&b'/');
        if closer {
            i += 1;
        }
        if !src[i..].starts_with("wp:") {
            return None;
        }
        i += 3;
        let first_end = ident(b, i)?;
        let (namespace, name, after) = if b.get(first_end) == Some(&b'/') {
            let name_end = ident(b, first_end + 1)?;
            (
                Some(&src[i..first_end]),
                &src[first_end + 1..name_end],
                name_end,
            )
        } else {
            (None, &src[i..first_end], first_end)
        };
        i = skip_ws(b, after, true)?;
        let mut attrs = None;
        if b.get(i) == Some(&b'{') {
            // The attrs end at the `}` before the first `-->`; WP's own serialiser escapes
            // `--` inside attrs, so a literal `-->` never sits inside the JSON.
            let close = self.close.find(i)?;
            let mut j = close;
            if b.get(j - 1) == Some(&b'/') {
                j -= 1;
            }
            let ws_start = src[i..j].trim_end_matches(is_ws).len() + i;
            if ws_start == j || b.get(ws_start - 1) != Some(&b'}') {
                return None;
            }
            attrs = Some(&src[i..ws_start]);
            i = ws_start;
            i = skip_ws(b, i, true)?;
        }
        let void = b.get(i) == Some(&b'/');
        if void {
            i += 1;
        }
        if !src[i..].starts_with("-->") {
            return None;
        }
        Some(Delimiter {
            start,
            end: i + 3,
            closer,
            namespace,
            name,
            attrs,
            void,
        })
    }
}

/// PCRE's `\s`.
fn is_ws(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\x0b' | '\x0c' | '\r')
}

/// Skips whitespace from `i`; `None` when at least one is required and there is none.
fn skip_ws(b: &[u8], mut i: usize, required: bool) -> Option<usize> {
    let from = i;
    while b.get(i).is_some_and(|&c| is_ws(c as char)) {
        i += 1;
    }
    (!required || i > from).then_some(i)
}

/// `[a-z][a-z0-9_-]*` from `i`; returns its end.
fn ident(b: &[u8], mut i: usize) -> Option<usize> {
    if !b.get(i).is_some_and(u8::is_ascii_lowercase) {
        return None;
    }
    i += 1;
    while b
        .get(i)
        .is_some_and(|&c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_' || c == b'-')
    {
        i += 1;
    }
    Some(i)
}

struct Parser<'a> {
    src: &'a str,
    out: Vec<Block>,
    stack: Vec<Block>,
    /// Top-level text not yet emitted as a freeform block.
    free: String,
    /// Names of openers past [`MAX_DEPTH`] kept as text, so their closers are text too.
    skipped: Vec<String>,
}

impl Parser<'_> {
    fn text(&mut self, from: usize, to: usize) {
        if from < to {
            let t = &self.src[from..to];
            self.push_text(t);
        }
    }

    fn push_text(&mut self, t: &str) {
        match self.stack.last_mut() {
            Some(top) => match top.inner_html.last_mut() {
                Some(HtmlPart::Html(h)) => h.push_str(t),
                _ => top.inner_html.push(HtmlPart::Html(t.to_owned())),
            },
            None => self.free.push_str(t),
        }
    }

    fn delimiter(&mut self, d: Delimiter<'_>) {
        let raw = &self.src[d.start..d.end];
        let name = d.full_name();
        if d.closer {
            if self.skipped.last() == Some(&name) {
                self.skipped.pop();
                self.push_text(raw);
            } else if let Some(at) = self.stack.iter().rposition(|b| b.name == name) {
                // Openers kept as text past the cap were all inside the blocks closed here.
                self.skipped.clear();
                while self.stack.len() > at {
                    self.close_top();
                }
            } else {
                self.push_text(raw);
            }
            return;
        }
        if self.stack.len() >= MAX_DEPTH || !self.skipped.is_empty() {
            if !d.void {
                self.skipped.push(name);
            }
            self.push_text(raw);
            return;
        }
        let (attrs, bad_attrs) = match d.attrs.map(serde_json::from_str::<Value>) {
            None => (Map::new(), false),
            Some(Ok(Value::Object(m))) => (m, false),
            Some(_) => (Map::new(), true),
        };
        let block = Block {
            name,
            attrs,
            inner: Vec::new(),
            inner_html: Vec::new(),
            bad_attrs,
        };
        if d.void {
            self.add(block);
        } else {
            self.flush_free_if_top();
            self.stack.push(block);
        }
    }

    fn close_top(&mut self) {
        if let Some(b) = self.stack.pop() {
            self.add(b);
        }
    }

    fn add(&mut self, b: Block) {
        match self.stack.last_mut() {
            Some(top) => {
                top.inner_html.push(HtmlPart::Child(top.inner.len()));
                top.inner.push(b);
            }
            None => {
                self.flush_free();
                self.out.push(b);
            }
        }
    }

    fn flush_free_if_top(&mut self) {
        if self.stack.is_empty() {
            self.flush_free();
        }
    }

    fn flush_free(&mut self) {
        let text = std::mem::take(&mut self.free);
        if !text.trim().is_empty() {
            self.out.push(Block {
                name: "core/freeform".to_owned(),
                attrs: Map::new(),
                inner: Vec::new(),
                inner_html: vec![HtmlPart::Html(text)],
                bad_attrs: false,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use crate::model::Record;
    use serde_json::json;

    fn corpus_body(file: &str, wp_id: i64) -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/corpus")
            .join(file);
        let bytes = std::fs::read(path).unwrap();
        let mut body = None;
        crate::wxr::parse(&bytes[..], |r| {
            if let Record::Item(i) = r
                && i.wp_id == wp_id
            {
                body = Some(i.content);
            }
            Ok(())
        })
        .unwrap();
        body.expect("item in corpus")
    }

    fn every_core_block() -> Vec<Block> {
        parse(&corpus_body("site-a.xml", 7))
    }

    fn find<'a>(blocks: &'a [Block], name: &str) -> Vec<&'a Block> {
        let mut out = Vec::new();
        for b in blocks {
            if b.name == name {
                out.push(b);
            }
            out.extend(find(&b.inner, name));
        }
        out
    }

    fn html(s: &str) -> HtmlPart {
        HtmlPart::Html(s.to_owned())
    }

    fn freeform_text(b: &Block) -> &str {
        assert_eq!(b.name, "core/freeform");
        match &b.inner_html[..] {
            [HtmlPart::Html(t)] => t,
            other => panic!("freeform content {other:?}"),
        }
    }

    #[test]
    fn site_a_top_level_has_no_freeform_and_namespaced_names() {
        let blocks = every_core_block();
        let names: Vec<&str> = blocks.iter().map(|b| b.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "core/paragraph",
                "core/heading",
                "core/heading",
                "core/list",
                "core/list",
                "core/quote",
                "core/image",
                "core/gallery",
                "core/code",
                "core/preformatted",
                "core/table",
                "core/separator",
                "core/embed",
                "core/buttons",
                "core/columns",
                "core/html",
                "core/shortcode",
                "acme/fancy-callout",
                "core/paragraph",
            ]
        );
    }

    #[test]
    fn nested_list_in_list_keeps_interleaving() {
        let blocks = every_core_block();
        let list = &blocks[3];
        assert_eq!(list.inner.len(), 2);
        assert_eq!(
            list.inner_html,
            [
                html("\n<ul class=\"wp-block-list\">"),
                HtmlPart::Child(0),
                html("\n\n"),
                HtmlPart::Child(1),
                html("</ul>\n"),
            ]
        );
        let second = &list.inner[1];
        assert_eq!(second.name, "core/list-item");
        assert_eq!(
            second.inner_html,
            [
                html("\n<li>Second item"),
                HtmlPart::Child(0),
                html("</li>\n")
            ]
        );
        let nested = &second.inner[0];
        assert_eq!(nested.name, "core/list");
        assert_eq!(nested.inner.len(), 1);
        assert_eq!(
            nested.inner[0].inner_html,
            [html("\n<li>Nested item</li>\n")]
        );
        assert_eq!(
            blocks[4].attrs,
            *json!({"ordered": true, "start": 3}).as_object().unwrap()
        );
    }

    #[test]
    fn heading_level_attrs() {
        let blocks = every_core_block();
        assert!(blocks[1].attrs.is_empty());
        assert_eq!(blocks[2].attrs.get("level"), Some(&json!(3)));
        assert!(!blocks[2].bad_attrs);
    }

    #[test]
    fn gallery_with_two_child_images() {
        let blocks = every_core_block();
        let gallery = find(&blocks, "core/gallery");
        assert_eq!(gallery.len(), 1);
        let g = gallery[0];
        let ids: Vec<&Value> = g.inner.iter().map(|b| &b.attrs["id"]).collect();
        assert_eq!(ids, [&json!(5), &json!(6)]);
        assert!(g.inner.iter().all(|b| b.name == "core/image"));
        assert_eq!(g.attrs.get("linkTo"), Some(&json!("none")));
        let children = g
            .inner_html
            .iter()
            .filter(|p| matches!(p, HtmlPart::Child(_)))
            .count();
        assert_eq!(children, 2);
    }

    #[test]
    fn namespaced_unknown_block() {
        let blocks = every_core_block();
        let b = &blocks[17];
        assert_eq!(b.name, "acme/fancy-callout");
        assert_eq!(b.attrs.get("tone"), Some(&json!("warm")));
        assert_eq!(
            b.inner_html,
            [html(
                "\n<div class=\"wp-block-acme-fancy-callout\">A block from a plugin the importer does not know.</div>\n"
            )]
        );
    }

    #[test]
    fn void_block() {
        let blocks = parse("<!-- wp:latest-posts {\"postsToShow\":3} /-->\n<!-- wp:spacer /-->");
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].name, "core/latest-posts");
        assert_eq!(blocks[0].attrs.get("postsToShow"), Some(&json!(3)));
        assert!(blocks[0].inner.is_empty() && blocks[0].inner_html.is_empty());
        assert_eq!(blocks[1].name, "core/spacer");
        assert!(blocks[1].attrs.is_empty());
    }

    #[test]
    fn void_block_inside_a_block_is_a_child() {
        let blocks = parse("<!-- wp:group --><div><!-- wp:spacer /--></div><!-- /wp:group -->");
        assert_eq!(blocks.len(), 1);
        assert_eq!(
            blocks[0].inner_html,
            [html("<div>"), HtmlPart::Child(0), html("</div>")]
        );
        assert_eq!(blocks[0].inner[0].name, "core/spacer");
    }

    #[test]
    fn freeform_around_blocks() {
        let blocks = parse(
            "<p>before</p>\n<!-- wp:paragraph --><p>x</p><!-- /wp:paragraph -->\n\n<p>after</p>",
        );
        assert_eq!(blocks.len(), 3);
        assert_eq!(freeform_text(&blocks[0]), "<p>before</p>\n");
        assert_eq!(blocks[1].name, "core/paragraph");
        assert_eq!(blocks[1].inner_html, [html("<p>x</p>")]);
        assert_eq!(freeform_text(&blocks[2]), "\n\n<p>after</p>");
    }

    #[test]
    fn no_blocks_is_one_freeform() {
        assert!(!has_blocks("<p>classic</p>"));
        let blocks = parse("<p>classic</p>\n<!-- more -->\n<p>rest</p>");
        assert_eq!(blocks.len(), 1);
        assert_eq!(
            freeform_text(&blocks[0]),
            "<p>classic</p>\n<!-- more -->\n<p>rest</p>"
        );
        assert!(parse("  \n\n ").is_empty());
        assert!(parse("").is_empty());
    }

    #[test]
    fn has_blocks_matches_wp() {
        assert!(has_blocks("x<!-- wp:paragraph -->"));
        assert!(!has_blocks("<!--wp:paragraph-->"));
    }

    #[test]
    fn malformed_delimiters_are_text() {
        // No whitespace after `<!--`, upper-case name, no `-->`: none is a block comment.
        let src = "<!--wp:paragraph--><!-- wp:Para --><!-- wp:paragraph";
        let blocks = parse(src);
        assert_eq!(blocks.len(), 1);
        assert_eq!(freeform_text(&blocks[0]), src);
    }

    #[test]
    fn unbalanced_closer_is_text() {
        let blocks = parse(
            "<p>a</p><!-- /wp:paragraph --><!-- wp:paragraph --><p>b</p><!-- /wp:paragraph -->",
        );
        assert_eq!(blocks.len(), 2);
        assert_eq!(freeform_text(&blocks[0]), "<p>a</p><!-- /wp:paragraph -->");
        assert_eq!(blocks[1].inner_html, [html("<p>b</p>")]);
    }

    #[test]
    fn closer_of_another_block_inside_a_block_is_text() {
        let blocks = parse("<!-- wp:quote --><p>q</p><!-- /wp:heading --><!-- /wp:quote -->");
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].inner_html, [html("<p>q</p><!-- /wp:heading -->")]);
    }

    #[test]
    fn closer_of_an_outer_block_closes_the_inner_ones() {
        let blocks =
            parse("<!-- wp:group --><div><!-- wp:paragraph --><p>x</p></div><!-- /wp:group -->");
        assert_eq!(blocks.len(), 1);
        let g = &blocks[0];
        assert_eq!(g.name, "core/group");
        assert_eq!(g.inner[0].name, "core/paragraph");
        assert_eq!(g.inner[0].inner_html, [html("<p>x</p></div>")]);
    }

    #[test]
    fn unclosed_block_is_closed_at_the_end() {
        let blocks = parse("<!-- wp:paragraph --><p>open");
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].name, "core/paragraph");
        assert_eq!(blocks[0].inner_html, [html("<p>open")]);
    }

    #[test]
    fn invalid_json_attrs_are_empty_and_flagged() {
        let blocks = parse(
            "<!-- wp:heading {\"level\":} --><h2>x</h2><!-- /wp:heading -->\
             <!-- wp:heading [1] --><h2>y</h2><!-- /wp:heading -->",
        );
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].name, "core/heading");
        assert!(blocks[0].attrs.is_empty() && blocks[0].bad_attrs);
        // `[1]` is not an attrs object, so the comment is not a delimiter at all (as in WP).
        assert_eq!(blocks[0].inner_html, [html("<h2>x</h2>")]);
        assert_eq!(
            freeform_text(&blocks[1]),
            "<!-- wp:heading [1] --><h2>y</h2><!-- /wp:heading -->"
        );
    }

    #[test]
    fn attrs_with_braces_and_escapes() {
        let blocks =
            parse("<!-- wp:acme/x {\"a\":{\"b\":\"}\"},\"c\":\"\\u002d\\u002d\\u003e\"} /-->");
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].attrs["a"], json!({"b": "}"}));
        assert_eq!(blocks[0].attrs["c"], json!("-->"));
    }

    #[test]
    fn closing_an_outer_block_past_the_cap_ends_the_text_mode() {
        let src = format!(
            "{}<!-- wp:quote -->q{}<!-- wp:paragraph --><p>after</p><!-- /wp:paragraph -->",
            "<!-- wp:group -->".repeat(MAX_DEPTH),
            "<!-- /wp:group -->".repeat(MAX_DEPTH)
        );
        let blocks = parse(&src);
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].name, "core/group");
        assert_eq!(blocks[1].name, "core/paragraph");
        assert_eq!(blocks[1].inner_html, [html("<p>after</p>")]);
    }

    #[test]
    fn nesting_beyond_the_cap_is_text() {
        let depth = MAX_DEPTH + 10;
        let src = format!(
            "{}x{}",
            "<!-- wp:group -->".repeat(depth),
            "<!-- /wp:group -->".repeat(depth)
        );
        let blocks = parse(&src);
        assert_eq!(blocks.len(), 1);
        let mut b = &blocks[0];
        let mut levels = 1;
        while let Some(child) = b.inner.first() {
            b = child;
            levels += 1;
        }
        assert_eq!(levels, MAX_DEPTH);
        let HtmlPart::Html(text) = &b.inner_html[0] else {
            panic!("innermost holds text");
        };
        assert_eq!(text.matches("<!-- wp:group -->").count(), 10);
        assert_eq!(text.matches("<!-- /wp:group -->").count(), 10);
    }
}
