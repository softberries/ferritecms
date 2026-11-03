//! Block tree → Markdown (design 12 §5.1, §5.3).
//!
//! The text an AI client reads and writes instead of the editor's JSON. Every node and
//! mark the renderer knows maps to CommonMark plus GFM tables and strikethrough; what
//! Markdown cannot say (an `embed`, raw `html`, a table with merged cells or block cells,
//! a node type this build does not know) becomes a fenced `ferrite-block` block holding
//! the node's JSON, so it comes back unchanged. Images are
//! `![alt](ferrite:media/<id> "caption")`.
//!
//! The output is deterministic, and text that would turn into markup on the way back is
//! backslash-escaped. The way in is `ferrite_import_md::markdown_to_body`.

use super::MAX_DEPTH;
use super::text::collect_text;
use serde_json::{Map, Value};
use std::slice;

/// The info string of the fence that holds a node Markdown cannot express.
pub const BLOCK_FENCE: &str = "ferrite-block";

/// Renders a `doc` block tree as Markdown. The result ends with one newline, or is empty
/// for an empty document; a body that is not a block tree reads as no content.
pub fn to_markdown(body: &Value) -> String {
    let nodes: &[Value] = match body {
        Value::Object(map) if node_type(map) == "doc" => content(map),
        Value::Object(_) => slice::from_ref(body),
        _ => return String::new(),
    };
    let blocks = render_blocks(nodes, 0);
    if blocks.is_empty() {
        return String::new();
    }
    let mut out = blocks
        .into_iter()
        .map(|b| b.text)
        .collect::<Vec<_>>()
        .join("\n\n");
    out.push('\n');
    out
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Paragraph,
    Bullet,
    Ordered,
    Other,
}

struct Block {
    text: String,
    kind: Kind,
    /// A list written with its second marker (`*`, `)`), so the list after it can use
    /// the first again: two lists in a row with the same marker would be one.
    alt: bool,
}

impl Block {
    fn new(text: String, kind: Kind) -> Self {
        Self {
            text,
            kind,
            alt: false,
        }
    }
}

/// Where inline content sits: it decides what a hard break is.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Ctx {
    Paragraph,
    Heading,
    Cell,
}

fn node_type(map: &Map<String, Value>) -> &str {
    map.get("type").and_then(Value::as_str).unwrap_or("")
}

fn content(map: &Map<String, Value>) -> &[Value] {
    match map.get("content") {
        Some(Value::Array(items)) => items,
        _ => &[],
    }
}

fn attr<'a>(map: &'a Map<String, Value>, key: &str) -> Option<&'a Value> {
    map.get("attrs")?.get(key)
}

fn attr_str<'a>(map: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    attr(map, key)?.as_str()
}

fn render_blocks(nodes: &[Value], depth: usize) -> Vec<Block> {
    if depth > MAX_DEPTH {
        let mut text = String::new();
        for n in nodes {
            collect_text(n, &mut text);
        }
        return if text.is_empty() {
            Vec::new()
        } else {
            vec![Block::new(
                escape_line_starts(&escape(&text.replace(['\n', '\r'], " "))),
                Kind::Paragraph,
            )]
        };
    }
    let mut out: Vec<Block> = Vec::new();
    // Inline nodes that sit directly among blocks read as one paragraph.
    let mut stray: Vec<Value> = Vec::new();
    for node in nodes {
        let Some(map) = node.as_object() else {
            continue;
        };
        let ty = node_type(map);
        if matches!(ty, "text" | "hardBreak") {
            stray.push(node.clone());
            continue;
        }
        flush_stray(&mut stray, &mut out);
        // A list straight after a list of its kind takes the other marker.
        let alt_after = |kind: Kind| out.last().is_some_and(|b| b.kind == kind && !b.alt);
        let (alt_bullet, alt_ordered) = (alt_after(Kind::Bullet), alt_after(Kind::Ordered));
        let block = match ty {
            "paragraph" => paragraph(content(map)),
            "heading" => heading(map),
            "bulletList" => Some(Block {
                alt: alt_bullet,
                ..Block::new(list(map, false, alt_bullet, depth), Kind::Bullet)
            }),
            "orderedList" => Some(Block {
                alt: alt_ordered,
                ..Block::new(list(map, true, alt_ordered, depth), Kind::Ordered)
            }),
            "blockquote" => Some(Block::new(blockquote(map, depth), Kind::Other)),
            "codeBlock" => Some(Block::new(code_block(map, node), Kind::Other)),
            "horizontalRule" => Some(Block::new("---".to_owned(), Kind::Other)),
            "image" => Some(Block::new(
                image(map).unwrap_or_else(|| block_fence(node)),
                Kind::Other,
            )),
            "table" => Some(Block::new(
                table(map).unwrap_or_else(|| block_fence(node)),
                Kind::Other,
            )),
            _ => Some(Block::new(block_fence(node), Kind::Other)),
        };
        out.extend(block);
    }
    flush_stray(&mut stray, &mut out);
    out
}

fn flush_stray(stray: &mut Vec<Value>, out: &mut Vec<Block>) {
    if stray.is_empty() {
        return;
    }
    out.extend(paragraph(stray));
    stray.clear();
}

/// An empty paragraph has no Markdown, so it is left out.
fn paragraph(nodes: &[Value]) -> Option<Block> {
    let text = inline(nodes, Ctx::Paragraph);
    let text = escape_line_starts(&text);
    (!text.trim().is_empty()).then(|| Block::new(text, Kind::Paragraph))
}

fn heading(map: &Map<String, Value>) -> Option<Block> {
    let Some(level @ 1..=6) = attr(map, "level").and_then(Value::as_u64) else {
        return paragraph(content(map));
    };
    let mut text = inline(content(map), Ctx::Heading);
    // A trailing `#` run would read as the closing sequence.
    if text.ends_with('#') && !text.ends_with("\\#") {
        text.insert(text.len() - 1, '\\');
    }
    let marker = "#".repeat(level as usize);
    let text = if text.trim().is_empty() {
        marker
    } else {
        format!("{marker} {}", text.trim())
    };
    Some(Block::new(text, Kind::Other))
}

fn list(map: &Map<String, Value>, ordered: bool, alt: bool, depth: usize) -> String {
    let start = attr(map, "start")
        .and_then(Value::as_u64)
        .filter(|n| (1..=999_999_999).contains(n))
        .unwrap_or(1);
    let mut items: Vec<String> = Vec::new();
    for (i, item) in content(map).iter().enumerate() {
        let Some(item_map) = item.as_object() else {
            continue;
        };
        // Anything but a `listItem` in a list is an item of its own.
        let blocks = if node_type(item_map) == "listItem" {
            render_blocks(content(item_map), depth + 1)
        } else {
            render_blocks(slice::from_ref(item), depth + 1)
        };
        let marker = if ordered {
            let n = start + i as u64;
            format!("{n}{} ", if alt { ')' } else { '.' })
        } else if alt {
            "* ".to_owned()
        } else {
            "- ".to_owned()
        };
        items.push(list_item(&marker, &blocks));
    }
    items.join("\n")
}

/// One item: the marker, then the blocks, every line after the first indented to the
/// text. A list straight after a paragraph keeps the item tight.
fn list_item(marker: &str, blocks: &[Block]) -> String {
    let mut body = String::new();
    for (i, b) in blocks.iter().enumerate() {
        if i > 0 {
            let tight = blocks[i - 1].kind == Kind::Paragraph
                && matches!(b.kind, Kind::Bullet | Kind::Ordered);
            body.push_str(if tight { "\n" } else { "\n\n" });
        }
        body.push_str(&b.text);
    }
    if body.is_empty() {
        return marker.trim_end().to_owned();
    }
    let pad = " ".repeat(marker.len());
    let mut out = String::new();
    for (i, line) in body.split('\n').enumerate() {
        if i == 0 {
            out.push_str(marker);
        } else {
            out.push('\n');
            if !line.is_empty() {
                out.push_str(&pad);
            }
        }
        out.push_str(line);
    }
    out
}

fn blockquote(map: &Map<String, Value>, depth: usize) -> String {
    let blocks = render_blocks(content(map), depth + 1);
    let body = blocks
        .into_iter()
        .map(|b| b.text)
        .collect::<Vec<_>>()
        .join("\n\n");
    if body.is_empty() {
        return ">".to_owned();
    }
    body.split('\n')
        .map(|l| {
            if l.is_empty() {
                ">".to_owned()
            } else {
                format!("> {l}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn code_block(map: &Map<String, Value>, node: &Value) -> String {
    let mut text = String::new();
    collect_text(node, &mut text);
    let lang = attr_str(map, "language")
        .filter(|l| !l.is_empty() && !l.contains(|c: char| c.is_whitespace() || c == '`'))
        .unwrap_or("");
    fence(lang, text.strip_suffix('\n').unwrap_or(&text))
}

/// A fenced block long enough that nothing inside closes it.
fn fence(info: &str, body: &str) -> String {
    let ticks = "`".repeat((longest_run(body, '`') + 1).max(3));
    if body.is_empty() {
        format!("{ticks}{info}\n{ticks}")
    } else {
        format!("{ticks}{info}\n{body}\n{ticks}")
    }
}

fn longest_run(s: &str, c: char) -> usize {
    let (mut best, mut run) = (0, 0);
    for ch in s.chars() {
        run = if ch == c { run + 1 } else { 0 };
        best = best.max(run);
    }
    best
}

/// A node Markdown cannot say, as its JSON in a `ferrite-block` fence.
fn block_fence(node: &Value) -> String {
    fence(BLOCK_FENCE, &node.to_string())
}

fn image(map: &Map<String, Value>) -> Option<String> {
    // A decorative image (design 16 §4.1) keeps its flag only as a `ferrite-block`
    // fence: `![](…)` reads back as an image that is not decorative.
    if map
        .get("attrs")
        .and_then(|a| a.get("decorative"))
        .and_then(Value::as_bool)
        == Some(true)
    {
        return None;
    }
    image_markdown(map)
}

/// `![alt](src "title")` of an image, decorative or not (an inline image has no fence
/// to fall back to, so it loses the flag).
fn image_markdown(map: &Map<String, Value>) -> Option<String> {
    let src = attr_str(map, "src").filter(|s| !s.trim().is_empty())?;
    let alt = attr_str(map, "alt").unwrap_or("");
    let alt = alt
        .replace(['\n', '\r'], " ")
        .replace('\\', "\\\\")
        .replace('[', "\\[")
        .replace(']', "\\]");
    let title = ["caption", "title"]
        .into_iter()
        .filter_map(|k| attr_str(map, k))
        .map(str::trim)
        .find(|t| !t.is_empty());
    let mut out = format!("![{alt}]({}", link_dest(src));
    if let Some(t) = title {
        let t = t
            .replace(['\n', '\r'], " ")
            .replace('\\', "\\\\")
            .replace('"', "\\\"");
        out.push_str(&format!(" \"{t}\""));
    }
    out.push(')');
    Some(out)
}

/// A GFM pipe table, or `None` when the table is more than that can say: merged cells,
/// cells holding more than paragraphs, no rows.
fn table(map: &Map<String, Value>) -> Option<String> {
    let mut rows: Vec<Vec<String>> = Vec::new();
    for row in content(map) {
        let row = row.as_object().filter(|r| node_type(r) == "tableRow")?;
        let mut cells = Vec::new();
        for cell in content(row) {
            let cell = cell
                .as_object()
                .filter(|c| matches!(node_type(c), "tableCell" | "tableHeader"))?;
            for key in ["colspan", "rowspan"] {
                if attr(cell, key).is_some_and(|v| v.as_u64() != Some(1)) {
                    return None;
                }
            }
            let mut parts = Vec::new();
            for p in content(cell) {
                let p = p.as_object().filter(|p| node_type(p) == "paragraph")?;
                let text = inline(content(p), Ctx::Cell);
                if !text.trim().is_empty() {
                    parts.push(text.trim().to_owned());
                }
            }
            cells.push(parts.join("<br>").replace('|', "\\|"));
        }
        rows.push(cells);
    }
    let cols = rows.iter().map(Vec::len).max().filter(|&n| n > 0)?;
    let line = |cells: &[String]| {
        let mut s = String::from("|");
        for i in 0..cols {
            s.push(' ');
            s.push_str(cells.get(i).map_or("", String::as_str));
            s.push_str(" |");
        }
        s
    };
    let mut lines = vec![line(&rows[0])];
    lines.push(format!("|{}", " --- |".repeat(cols)));
    lines.extend(rows[1..].iter().map(|r| line(r)));
    Some(lines.join("\n"))
}

// ---- inline content ----

#[derive(Clone, Default, PartialEq, Eq)]
struct Marks<'a> {
    link: Option<&'a str>,
    bold: bool,
    italic: bool,
    strike: bool,
    code: bool,
}

enum Item<'a> {
    Text(String, Marks<'a>),
    Break,
    Image(&'a Map<String, Value>),
    Plain(String),
}

fn marks_of(map: &Map<String, Value>) -> Marks<'_> {
    let mut m = Marks::default();
    let Some(Value::Array(marks)) = map.get("marks") else {
        return m;
    };
    for mark in marks.iter().filter_map(Value::as_object) {
        match node_type(mark) {
            "bold" => m.bold = true,
            "italic" => m.italic = true,
            "strike" => m.strike = true,
            "code" => m.code = true,
            "link" if m.link.is_none() => {
                m.link = attr_str(mark, "href").filter(|h| !h.trim().is_empty());
            }
            _ => {}
        }
    }
    m
}

fn items_of(nodes: &[Value]) -> Vec<Item<'_>> {
    let mut items: Vec<Item<'_>> = Vec::new();
    for node in nodes {
        let Some(map) = node.as_object() else {
            continue;
        };
        match node_type(map) {
            "text" => {
                let Some(text) = map.get("text").and_then(Value::as_str) else {
                    continue;
                };
                let marks = marks_of(map);
                // Adjacent text with the same marks is one run, so no delimiter doubles.
                if let Some(Item::Text(prev, pm)) = items.last_mut()
                    && *pm == marks
                {
                    prev.push_str(text);
                } else {
                    items.push(Item::Text(text.to_owned(), marks));
                }
            }
            "hardBreak" => items.push(Item::Break),
            "image" => items.push(Item::Image(map)),
            _ => {
                let mut text = String::new();
                collect_text(node, &mut text);
                if !text.is_empty() {
                    items.push(Item::Plain(text));
                }
            }
        }
    }
    items
}

fn inline(nodes: &[Value], ctx: Ctx) -> String {
    let mut items = items_of(nodes);
    while matches!(items.last(), Some(Item::Break)) {
        items.pop();
    }
    let lead = items
        .iter()
        .take_while(|i| matches!(i, Item::Break))
        .count();
    items.drain(..lead);
    let mut out = String::new();
    let mut i = 0;
    while i < items.len() {
        match &items[i] {
            Item::Text(
                _,
                Marks {
                    link: Some(href), ..
                },
            ) => {
                let href = *href;
                let mut text = String::new();
                while let Some(Item::Text(t, m)) = items.get(i)
                    && m.link == Some(href)
                {
                    let plain = Marks {
                        link: None,
                        ..m.clone()
                    };
                    text.push_str(&marked(t, &plain));
                    i += 1;
                }
                if text.trim().is_empty() {
                    out.push_str(&text);
                } else {
                    out.push_str(&format!("[{text}]({})", link_dest(href)));
                }
                continue;
            }
            Item::Text(t, m) => out.push_str(&marked(t, m)),
            Item::Break => out.push_str(match ctx {
                Ctx::Paragraph => "\\\n",
                Ctx::Heading => " ",
                Ctx::Cell => "<br>",
            }),
            Item::Image(map) => out.push_str(&image_markdown(map).unwrap_or_default()),
            Item::Plain(t) => out.push_str(&escape(&t.replace(['\n', '\r'], " "))),
        }
        i += 1;
    }
    out
}

/// A text run with its marks. Whitespace at the edges stays outside the delimiters, where
/// they would not close.
fn marked(text: &str, m: &Marks<'_>) -> String {
    let text = text.replace(['\n', '\r'], " ");
    let core = text.trim_matches(' ');
    if core.is_empty() {
        return text;
    }
    let lead = &text[..text.len() - text.trim_start_matches(' ').len()];
    let trail = &text[text.trim_end_matches(' ').len()..];
    let mut s = if m.code {
        code_span(core)
    } else {
        escape(core)
    };
    if m.italic {
        s = format!("*{s}*");
    }
    if m.bold {
        s = format!("**{s}**");
    }
    if m.strike {
        s = format!("~~{s}~~");
    }
    format!("{lead}{s}{trail}")
}

fn code_span(text: &str) -> String {
    let ticks = "`".repeat(longest_run(text, '`') + 1);
    let pad = text.starts_with('`')
        || text.ends_with('`')
        || (text.starts_with(' ') && text.ends_with(' ') && text.trim() != "");
    if pad {
        format!("{ticks} {text} {ticks}")
    } else {
        format!("{ticks}{text}{ticks}")
    }
}

/// A link or image destination: bare when it can be, else in angle brackets.
fn link_dest(href: &str) -> String {
    let href = href.trim();
    if !href.is_empty()
        && !href.contains(|c: char| c.is_whitespace() || matches!(c, '(' | ')' | '<' | '>' | '\\'))
    {
        return href.to_owned();
    }
    let inner = href
        .replace('\\', "\\\\")
        .replace('<', "\\<")
        .replace('>', "\\>")
        .replace('\n', "%0A")
        .replace('\r', "%0D");
    format!("<{inner}>")
}

/// Backslash-escapes what would be markup inside a line of text. Where a line starts is
/// [`escape_line_starts`]'s business.
fn escape(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let alnum = |c: Option<&char>| c.is_some_and(|c| c.is_alphanumeric());
    let mut out = String::with_capacity(s.len() + 8);
    for (i, &c) in chars.iter().enumerate() {
        let escape = match c {
            '\\' | '`' | '*' | '[' | ']' | '<' | '~' => true,
            // Inside a word an underscore is just one.
            '_' => !(i > 0 && alnum(chars.get(i - 1)) && alnum(chars.get(i + 1))),
            '&' => entity_follows(&chars[i + 1..]),
            _ => false,
        };
        if escape {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Whether `rest` (after a `&`) reads as the rest of a character reference.
fn entity_follows(rest: &[char]) -> bool {
    let rest = rest.strip_prefix(&['#']).unwrap_or(rest);
    let n = rest
        .iter()
        .take(32)
        .take_while(|c| c.is_ascii_alphanumeric())
        .count();
    n > 0 && rest.get(n) == Some(&';')
}

/// Escapes what only means something at the start of a line: headings, quotes, bullets,
/// setext underlines and `1.` list markers.
fn escape_line_starts(s: &str) -> String {
    s.split('\n')
        .map(|line| {
            let line = line.trim_start_matches([' ', '\t']);
            let mut chars = line.chars();
            match chars.next() {
                Some('#' | '>' | '-' | '+' | '=') => format!("\\{line}"),
                Some(c) if c.is_ascii_digit() => {
                    let digits = line.chars().take_while(char::is_ascii_digit).count();
                    match line[digits..].chars().next() {
                        Some('.' | ')') if digits <= 9 => {
                            format!("{}\\{}", &line[..digits], &line[digits..])
                        }
                        _ => line.to_owned(),
                    }
                }
                _ => line.to_owned(),
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const ID: &str = "0190f1c0-0000-7000-8000-000000000001";

    fn t(s: &str) -> Value {
        json!({"type":"text","text":s})
    }

    fn m(s: &str, marks: Value) -> Value {
        json!({"type":"text","text":s,"marks":marks})
    }

    fn p(children: Vec<Value>) -> Value {
        json!({"type":"paragraph","content":children})
    }

    fn doc(children: Vec<Value>) -> Value {
        json!({"type":"doc","content":children})
    }

    fn md(children: Vec<Value>) -> String {
        to_markdown(&doc(children))
    }

    #[test]
    fn empty_and_foreign_bodies() {
        assert_eq!(to_markdown(&doc(vec![])), "");
        assert_eq!(to_markdown(&doc(vec![json!({"type":"paragraph"})])), "");
        assert_eq!(to_markdown(&json!(null)), "");
        assert_eq!(to_markdown(&json!("text")), "");
    }

    #[test]
    fn headings_and_paragraphs() {
        let out = md(vec![
            json!({"type":"heading","attrs":{"level":2},"content":[t("Title")]}),
            p(vec![t("one")]),
            json!({"type":"heading","attrs":{"level":9},"content":[t("odd")]}),
        ]);
        assert_eq!(out, "## Title\n\none\n\nodd\n");
    }

    #[test]
    fn marks_nest_and_keep_whitespace_outside_the_delimiters() {
        let out = md(vec![p(vec![
            t("a "),
            m("b ", json!([{"type":"bold"}])),
            m("c", json!([{"type":"bold"},{"type":"italic"}])),
            t(" "),
            m("d", json!([{"type":"strike"}])),
            t(" "),
            m("e", json!([{"type":"code"}])),
        ])]);
        assert_eq!(out, "a **b** ***c*** ~~d~~ `e`\n");
    }

    #[test]
    fn adjacent_runs_with_the_same_marks_merge() {
        let out = md(vec![p(vec![
            m("a", json!([{"type":"bold"}])),
            m("b", json!([{"type":"bold"}])),
        ])]);
        assert_eq!(out, "**ab**\n");
    }

    #[test]
    fn links_group_their_runs_and_quote_awkward_destinations() {
        let link = |h: &str| json!([{"type":"link","attrs":{"href":h}}]);
        let out = md(vec![p(vec![
            m("see ", link("https://e.com/")),
            m(
                "this",
                json!([{"type":"link","attrs":{"href":"https://e.com/"}},{"type":"bold"}]),
            ),
            t(" and "),
            m("that", link("/a b(c)")),
        ])]);
        assert_eq!(
            out,
            "[see **this**](https://e.com/) and [that](</a b(c)>)\n"
        );
    }

    #[test]
    fn code_spans_use_a_longer_delimiter_than_their_content() {
        let out = md(vec![p(vec![m("a`b", json!([{"type":"code"}]))])]);
        assert_eq!(out, "``a`b``\n");
        let out = md(vec![p(vec![m("`x`", json!([{"type":"code"}]))])]);
        assert_eq!(out, "`` `x` ``\n");
    }

    #[test]
    fn text_that_would_be_markup_is_escaped() {
        let out = md(vec![
            p(vec![t(
                "1 * 2 [x](y) <b> `c` ~~z~~ snake_case _em_ \\ &amp; AT&T",
            )]),
            p(vec![t("# not a heading")]),
            p(vec![t("- not a list")]),
            p(vec![t("2. not a list")]),
            p(vec![t("> not a quote")]),
            p(vec![t("a"), json!({"type":"hardBreak"}), t("---")]),
        ]);
        assert_eq!(
            out,
            "1 \\* 2 \\[x\\](y) \\<b> \\`c\\` \\~\\~z\\~\\~ snake_case \\_em\\_ \\\\ \\&amp; AT&T\n\n\
             \\# not a heading\n\n\\- not a list\n\n2\\. not a list\n\n\\> not a quote\n\n\
             a\\\n\\---\n"
        );
    }

    #[test]
    fn hard_breaks_are_a_backslash_and_a_trailing_one_is_dropped() {
        let hb = json!({"type":"hardBreak"});
        let out = md(vec![p(vec![t("a"), hb.clone(), t("b"), hb])]);
        assert_eq!(out, "a\\\nb\n");
    }

    #[test]
    fn nested_and_ordered_lists() {
        let item = |blocks: Vec<Value>| json!({"type":"listItem","content":blocks});
        let out = md(vec![
            json!({"type":"bulletList","content":[
                item(vec![p(vec![t("a")]), json!({"type":"bulletList","content":[
                    item(vec![p(vec![t("b")])])
                ]})]),
                item(vec![p(vec![t("c")])]),
            ]}),
            json!({"type":"orderedList","attrs":{"start":3},"content":[
                item(vec![p(vec![t("x")])]),
                item(vec![p(vec![t("y")]), p(vec![t("more")])]),
            ]}),
        ]);
        assert_eq!(out, "- a\n  - b\n- c\n\n3. x\n4. y\n\n   more\n");
    }

    #[test]
    fn adjacent_lists_use_another_marker_so_they_stay_two() {
        let list = |ty: &str| json!({"type":ty,"content":[{"type":"listItem","content":[p(vec![t("a")])]}]});
        assert_eq!(
            md(vec![list("bulletList"), list("bulletList")]),
            "- a\n\n* a\n"
        );
        assert_eq!(
            md(vec![list("orderedList"), list("orderedList")]),
            "1. a\n\n1) a\n"
        );
    }

    #[test]
    fn any_number_of_adjacent_lists_stay_that_many_lists() {
        let list = |ty: &str| json!({"type":ty,"content":[{"type":"listItem","content":[p(vec![t("a")])]}]});
        assert_eq!(
            md(vec![
                list("bulletList"),
                list("bulletList"),
                list("bulletList")
            ]),
            "- a\n\n* a\n\n- a\n"
        );
        assert_eq!(
            md(vec![
                list("orderedList"),
                list("orderedList"),
                list("orderedList")
            ]),
            "1. a\n\n1) a\n\n1. a\n"
        );
    }

    #[test]
    fn a_line_starting_with_a_tab_does_not_become_code() {
        let out = md(vec![
            p(vec![t("\tindented")]),
            p(vec![t("a"), json!({"type":"hardBreak"}), t(" \t- x")]),
        ]);
        assert_eq!(out, "indented\n\na\\\n\\- x\n");
    }

    #[test]
    fn blockquotes_nest() {
        let out = md(vec![json!({"type":"blockquote","content":[
            p(vec![t("one")]),
            json!({"type":"blockquote","content":[p(vec![t("two")])]}),
        ]})]);
        assert_eq!(out, "> one\n>\n> > two\n");
    }

    #[test]
    fn code_blocks_keep_their_language_and_outgrow_inner_fences() {
        let out = md(vec![
            json!({"type":"codeBlock","attrs":{"language":"rust"},"content":[t("fn x() {}\n")]}),
            json!({"type":"codeBlock","content":[t("```\ninner\n```")]}),
            json!({"type":"codeBlock"}),
        ]);
        assert_eq!(
            out,
            "```rust\nfn x() {}\n```\n\n````\n```\ninner\n```\n````\n\n```\n```\n"
        );
    }

    #[test]
    fn horizontal_rule() {
        assert_eq!(
            md(vec![p(vec![t("a")]), json!({"type":"horizontalRule"})]),
            "a\n\n---\n"
        );
    }

    #[test]
    fn images_carry_the_caption_as_the_title() {
        let out = md(vec![
            json!({"type":"image","attrs":{"src":format!("ferrite:media/{ID}"),"alt":"A [cat]","caption":"The \"cat\""}}),
            json!({"type":"image","attrs":{"src":"https://e.com/a b.png","alt":"","title":"T"}}),
        ]);
        assert_eq!(
            out,
            format!(
                "![A \\[cat\\]](ferrite:media/{ID} \"The \\\"cat\\\"\")\n\n![](<https://e.com/a b.png> \"T\")\n"
            )
        );
    }

    #[test]
    fn tables_are_pipe_tables() {
        let cell = |ty: &str, s: &str| json!({"type":ty,"attrs":{"colspan":1,"rowspan":1},"content":[p(vec![t(s)])]});
        let out = md(vec![json!({"type":"table","content":[
            {"type":"tableRow","content":[cell("tableHeader","H|1"), cell("tableHeader","H2")]},
            {"type":"tableRow","content":[cell("tableCell","a"), cell("tableCell","")]},
        ]})]);
        assert_eq!(out, "| H\\|1 | H2 |\n| --- | --- |\n| a |  |\n");
    }

    #[test]
    fn a_table_markdown_cannot_say_goes_to_a_fence() {
        let table = json!({"type":"table","content":[{"type":"tableRow","content":[
            {"type":"tableCell","attrs":{"colspan":2,"rowspan":1},"content":[p(vec![t("a")])]}
        ]}]});
        let out = md(vec![table.clone()]);
        assert_eq!(out, format!("```ferrite-block\n{table}\n```\n"));
    }

    #[test]
    fn embeds_html_and_unknown_nodes_become_ferrite_block_fences() {
        let embed = json!({"type":"embed","attrs":{"provider":"youtube","url":"https://youtu.be/dQw4w9WgXcQ"}});
        let html = json!({"type":"html","attrs":{"html":"<b>x</b>"}});
        let unknown =
            json!({"type":"callout","attrs":{"tone":"warn"},"content":[p(vec![t("`tick`")])]});
        let out = md(vec![embed.clone(), html.clone(), unknown.clone()]);
        assert_eq!(
            out,
            format!(
                "```ferrite-block\n{embed}\n```\n\n```ferrite-block\n{html}\n```\n\n\
                 ```ferrite-block\n{unknown}\n```\n"
            )
        );
    }

    #[test]
    fn a_fence_inside_a_list_item_is_indented_with_it() {
        let unknown = json!({"type":"callout"});
        let out = md(vec![json!({"type":"bulletList","content":[
            {"type":"listItem","content":[p(vec![t("a")]), unknown.clone()]}
        ]})]);
        assert_eq!(
            out,
            format!("- a\n\n  ```ferrite-block\n  {unknown}\n  ```\n")
        );
    }

    #[test]
    fn stray_inline_nodes_among_blocks_read_as_a_paragraph() {
        assert_eq!(md(vec![t("a"), t("b"), p(vec![t("c")])]), "ab\n\nc\n");
    }

    #[test]
    fn deep_nesting_ends_in_plain_text() {
        let mut node = p(vec![t("deep")]);
        for _ in 0..200 {
            node = json!({"type":"blockquote","content":[node]});
        }
        let out = md(vec![node]);
        assert!(out.contains("deep"));
    }
}
