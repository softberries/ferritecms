//! A port of WordPress's `wpautop`: paragraphs and line breaks for classic-editor bodies.

/// WP's `$allblocks`: tags that start and end a paragraph.
const BLOCKS: &[&str] = &[
    "table",
    "thead",
    "tfoot",
    "caption",
    "col",
    "colgroup",
    "tbody",
    "tr",
    "td",
    "th",
    "div",
    "dl",
    "dd",
    "dt",
    "ul",
    "ol",
    "li",
    "pre",
    "form",
    "map",
    "area",
    "blockquote",
    "address",
    "style",
    "p",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "hr",
    "fieldset",
    "legend",
    "section",
    "article",
    "aside",
    "hgroup",
    "header",
    "footer",
    "nav",
    "figure",
    "figcaption",
    "details",
    "menu",
    "summary",
];

/// Turns blank-line-separated text into `<p>`s and single newlines into `<br />`, leaving
/// block-level elements unwrapped and `<pre>` untouched.
///
/// The steps follow WP's `wpautop` (with `$br = true`): `<pre>` set aside; two `<br>`s
/// become a paragraph break; block tags get blank lines around them; the text is split on
/// blank lines; each chunk is wrapped in `<p>` unless it starts with a block tag (inside a
/// leading `<blockquote>` it is), closed before a trailing block tag; other newlines
/// become `<br />` except next to a block tag. Differences: newlines inside a tag become
/// spaces (WP restores them), and `<figcaption>` does not split a paragraph instead of
/// being split and then joined again.
pub fn wpautop(text: &str) -> String {
    if text.trim().is_empty() {
        return String::new();
    }
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    let (text, pres) = set_aside_pre(&text);
    let spaced = space_blocks(&text);
    let mut out = String::new();
    for chunk in chunks(&spaced) {
        out.push_str(&paragraph(&chunk));
        out.push('\n');
    }
    for (i, pre) in pres.iter().enumerate() {
        out = out.replacen(&pre_placeholder(i), pre, 1);
    }
    out
}

fn pre_placeholder(i: usize) -> String {
    format!("<pre wp-pre-tag-{i}></pre>")
}

/// Replaces each `<pre…>…</pre>` with a placeholder (itself a block tag), as WP does.
fn set_aside_pre(text: &str) -> (String, Vec<String>) {
    if !text.contains("<pre") {
        return (text.to_owned(), Vec::new());
    }
    let mut parts: Vec<&str> = text.split("</pre>").collect();
    let last = parts.pop().unwrap_or_default();
    let mut out = String::new();
    let mut pres = Vec::new();
    for part in parts {
        match part.find("<pre") {
            Some(start) => {
                out.push_str(&part[..start]);
                out.push_str(&pre_placeholder(pres.len()));
                pres.push(format!("{}</pre>", &part[start..]));
            }
            // Malformed HTML: a `</pre>` without an opener is dropped, as in WP.
            None => out.push_str(part),
        }
    }
    out.push_str(last);
    (out, pres)
}

/// A tag's lower-cased name and whether it is a closing tag; `None` for comments and other
/// markup.
fn tag_name(tag: &str) -> Option<(String, bool)> {
    let inner = tag.strip_prefix('<')?;
    let (closing, inner) = match inner.strip_prefix('/') {
        Some(r) => (true, r),
        None => (false, inner),
    };
    let end = inner
        .find(|c: char| !c.is_ascii_alphanumeric())
        .unwrap_or(inner.len());
    let name = &inner[..end];
    (!name.is_empty() && name.as_bytes()[0].is_ascii_alphabetic())
        .then(|| (name.to_ascii_lowercase(), closing))
}

fn is_block(name: &str) -> bool {
    BLOCKS.contains(&name)
}

/// Splits `text` into text runs and tags (`<x …>`, `</x>`, `<!-- … -->`); newlines inside a
/// tag become spaces.
fn tokens(text: &str) -> Vec<(bool, String)> {
    let mut out = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        let Some(lt) = rest.find('<') else {
            out.push((false, rest.to_owned()));
            break;
        };
        let after = &rest[lt + 1..];
        let is_tag = after.starts_with(|c: char| c.is_ascii_alphabetic() || c == '/' || c == '!');
        let end = if !is_tag {
            None
        } else if after.starts_with("!--") {
            rest[lt..].find("-->").map(|e| lt + e + 3)
        } else {
            rest[lt..].find('>').map(|e| lt + e + 1)
        };
        let Some(end) = end else {
            // `<` that opens no tag: text up to and including it.
            out.push((false, rest[..=lt].to_owned()));
            rest = &rest[lt + 1..];
            continue;
        };
        if lt > 0 {
            out.push((false, rest[..lt].to_owned()));
        }
        out.push((true, rest[lt..end].replace('\n', " ")));
        rest = &rest[end..];
    }
    out
}

/// Blank lines before block openers and after block closers (and `<hr>`); `<br><br>` → a
/// blank line.
fn space_blocks(text: &str) -> String {
    let toks = tokens(text);
    let mut out = String::new();
    let mut i = 0;
    while i < toks.len() {
        let (is_tag, t) = &toks[i];
        i += 1;
        if !is_tag {
            out.push_str(t);
            continue;
        }
        let Some((name, closing)) = tag_name(t) else {
            out.push_str(t);
            continue;
        };
        if name == "br" {
            // A second `<br>`, with only whitespace between, makes a paragraph break.
            let mut j = i;
            if toks
                .get(j)
                .is_some_and(|(tag, s)| !tag && s.trim().is_empty())
            {
                j += 1;
            }
            if let Some((true, next)) = toks.get(j)
                && tag_name(next).is_some_and(|(n, c)| n == "br" && !c)
            {
                out.push_str("\n\n");
                i = j + 1;
                continue;
            }
            out.push_str(t);
            continue;
        }
        if !is_block(&name) || name == "figcaption" {
            out.push_str(t);
            continue;
        }
        if !closing {
            out.push_str("\n\n");
        }
        out.push_str(t);
        if closing || name == "hr" {
            out.push_str("\n\n");
        }
    }
    out
}

/// Runs of lines separated by whitespace-only lines.
fn chunks(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur: Vec<&str> = Vec::new();
    for line in text.split('\n') {
        if line.trim().is_empty() {
            if !cur.is_empty() {
                out.push(cur.join("\n"));
                cur.clear();
            }
        } else {
            cur.push(line);
        }
    }
    if !cur.is_empty() {
        out.push(cur.join("\n"));
    }
    out
}

/// The leading run of block tags in `s` (with whitespace between): its end and whether the
/// last one opens a `<blockquote>`.
fn leading_blocks(s: &str) -> (usize, bool) {
    let mut end = 0;
    let mut quote = false;
    loop {
        let rest = &s[end..];
        let ws = rest.len() - rest.trim_start().len();
        let rest = &rest[ws..];
        if !rest.starts_with('<') {
            return (end, quote);
        }
        let Some(close) = rest.find('>') else {
            return (end, quote);
        };
        let tag = &rest[..=close];
        match tag_name(tag) {
            Some((name, closing)) if is_block(&name) => {
                quote = name == "blockquote" && !closing;
                end += ws + close + 1;
            }
            _ => return (end, quote),
        }
    }
}

/// The start of the trailing run of block tags in `s`.
fn trailing_blocks(s: &str) -> usize {
    let mut start = s.len();
    loop {
        let rest = s[..start].trim_end();
        if !rest.ends_with('>') {
            return start;
        }
        let Some(open) = rest.rfind('<') else {
            return start;
        };
        match tag_name(&rest[open..]) {
            Some((name, _)) if is_block(&name) => start = open,
            _ => return start,
        }
    }
}

fn starts_with_block(s: &str) -> bool {
    leading_blocks(s).0 > 0
}

fn ends_with_block(s: &str) -> bool {
    trailing_blocks(s) < s.len()
}

/// One chunk: `<p>` around its text, `<br />` for its inner newlines.
fn paragraph(chunk: &str) -> String {
    let chunk = chunk.trim_matches('\n');
    let (lead_end, quote) = leading_blocks(chunk);
    let trail_start = trailing_blocks(chunk).max(lead_end);
    let (lead, middle, trail) = (
        &chunk[..lead_end],
        &chunk[lead_end..trail_start],
        &chunk[trail_start..],
    );
    let wrap = !middle.trim().is_empty() && (lead_end == 0 || quote);
    let mut out = String::from(lead);
    if wrap {
        out.push_str("<p>");
        out.push_str(middle.trim_matches('\n'));
        out.push_str("</p>");
    } else {
        out.push_str(middle);
    }
    out.push_str(trail);
    breaks(&out.replace("<br>", "<br />").replace("<br/>", "<br />"))
}

/// `<br />` for every newline that is not already after a `<br>` and not next to a block
/// tag.
fn breaks(s: &str) -> String {
    let lines: Vec<&str> = s.split('\n').collect();
    let mut out = String::new();
    for (i, line) in lines.iter().enumerate() {
        if i + 1 == lines.len() {
            out.push_str(line);
            break;
        }
        let before = line.trim_end();
        out.push_str(before);
        let after = lines[i + 1];
        let after_br = ["<br />", "<br>", "<br/>"]
            .iter()
            .any(|b| before.ends_with(b));
        if !after_br && !ends_with_block(before) && !starts_with_block(after) {
            out.push_str("<br />");
        }
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use crate::model::Record;

    fn site_b_classic() -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/site-b.xml");
        let bytes = std::fs::read(path).unwrap();
        let mut body = None;
        crate::wxr::parse(&bytes[..], |r| {
            if let Record::Item(i) = r
                && i.wp_id == 5
            {
                body = Some(i.content);
            }
            Ok(())
        })
        .unwrap();
        body.unwrap()
    }

    #[test]
    fn site_b_paragraphs_breaks_and_blocks() {
        let out = wpautop(&site_b_classic());
        assert!(
            out.starts_with(
                "<p>First paragraph written in the classic editor.<br />\n\
                 It has a single line break here.</p>\n\
                 <p>Second paragraph with <strong>bold</strong> and \
                 <a href=\"https://example.org/\" target=\"_blank\" rel=\"noopener\">an external link</a>.</p>\n"
            ),
            "{out}"
        );
        assert!(out.contains("\n<h2>A classic heading</h2>\n<ul>\n<li>Classic list one</li>\n<li>Classic list two</li>\n</ul>\n"), "{out}");
        assert!(
            !out.contains("<p><ul>") && !out.contains("<p><li>"),
            "{out}"
        );
        assert!(
            out.contains("<blockquote><p>A classic quote.</p></blockquote>\n"),
            "{out}"
        );
        assert!(
            out.contains("<p>[gallery ids=\"2,3\" columns=\"2\"]</p>\n"),
            "{out}"
        );
        assert!(
            out.contains("<p>Text after the more tag.</p>\n<pre>preformatted\n  classic</pre>\n"),
            "{out}"
        );
        assert!(
            out.contains("\n<table>\n<tr>\n<td>x</td>\n<td>y</td>\n</tr>\n</table>\n"),
            "{out}"
        );
        assert!(out.contains("\n<h1>An h1 inside the body</h1>\n"), "{out}");
        assert!(out.ends_with("<p><img src=\"http://localhost:8099/wp-content/uploads/not-in-library.jpg\" alt=\"missing file\" /></p>\n"), "{out}");
    }

    #[test]
    fn double_newline_is_a_paragraph_single_a_break() {
        assert_eq!(wpautop("a\n\nb"), "<p>a</p>\n<p>b</p>\n");
        assert_eq!(wpautop("a  \nb"), "<p>a<br />\nb</p>\n");
        assert_eq!(wpautop("a\r\n\r\nb\rc"), "<p>a</p>\n<p>b<br />\nc</p>\n");
        assert_eq!(wpautop("a\n \t\n\n\nb"), "<p>a</p>\n<p>b</p>\n");
        assert_eq!(wpautop("a<br />\nb"), "<p>a<br />\nb</p>\n");
    }

    #[test]
    fn empty_is_empty() {
        assert_eq!(wpautop(""), "");
        assert_eq!(wpautop(" \n\n "), "");
    }

    #[test]
    fn two_brs_make_a_paragraph_break() {
        assert_eq!(wpautop("a<br><br />b"), "<p>a</p>\n<p>b</p>\n");
    }

    #[test]
    fn pre_is_untouched() {
        assert_eq!(
            wpautop("x\n<pre class=\"c\">a\n\n  b\n</pre>\ny"),
            "<p>x</p>\n<pre class=\"c\">a\n\n  b\n</pre>\n<p>y</p>\n"
        );
    }

    #[test]
    fn newlines_inside_tags_are_not_breaks() {
        assert_eq!(
            wpautop("<a\nhref=\"/x\">l</a>"),
            "<p><a href=\"/x\">l</a></p>\n"
        );
    }

    #[test]
    fn text_inside_a_div_is_not_wrapped() {
        assert_eq!(wpautop("<div>\ntext\n</div>"), "<div>\ntext\n</div>\n");
        assert_eq!(wpautop("<div>text</div>"), "<div>text</div>\n");
    }

    #[test]
    fn stray_lt_and_comments() {
        assert_eq!(
            wpautop("a < b\n\n<!-- more -->"),
            "<p>a < b</p>\n<p><!-- more --></p>\n"
        );
    }
}
