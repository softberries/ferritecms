//! WordPress shortcodes: `[name attrs]`, `[name attrs /]`, `[name attrs]inner[/name]`, and
//! `[[name]]` for a literal `[name]`, read the way `get_shortcode_regex` and
//! `shortcode_parse_atts` read them, for any name.

use std::collections::{BTreeMap, HashMap};

/// A run of text, or one shortcode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Piece {
    Text(String),
    Code {
        name: String,
        /// Named attributes with lower-cased keys; positional values under `"0"`, `"1"`, ….
        attrs: BTreeMap<String, String>,
        /// The enclosed content when the shortcode has a closing tag.
        inner: Option<String>,
    },
}

/// Splits `text` into text and shortcodes; adjacent text is merged.
///
/// Unlike WP, which only matches registered names, any name matches; it must start with an
/// ASCII letter or `_` and go on with `[A-Za-z0-9_-]`, so `[1]` stays text.
pub fn scan(text: &str) -> Vec<Piece> {
    let mut out = Vec::new();
    let mut s = Scanner {
        src: text,
        closers: HashMap::new(),
    };
    let mut pos = 0;
    let mut from = 0;
    while let Some(rel) = text[from..].find('[') {
        let at = from + rel;
        match s.shortcode_at(at) {
            Some((piece, end)) => {
                push_text(&mut out, &text[pos..at]);
                match piece {
                    Piece::Text(t) => push_text(&mut out, &t),
                    code => out.push(code),
                }
                pos = end;
                from = end;
            }
            None => from = at + 1,
        }
    }
    push_text(&mut out, &text[pos..]);
    out
}

fn push_text(out: &mut Vec<Piece>, t: &str) {
    if t.is_empty() {
        return;
    }
    match out.last_mut() {
        Some(Piece::Text(prev)) => prev.push_str(t),
        _ => out.push(Piece::Text(t.to_owned())),
    }
}

struct Scanner<'a> {
    src: &'a str,
    /// Per closer `[/name]`: where the last search started and what it found, so a run of
    /// unclosed openers stays linear.
    closers: HashMap<String, (usize, Option<usize>)>,
}

impl Scanner<'_> {
    /// The first `[/name]` at or after `from`.
    fn find_closer(&mut self, name: &str, from: usize) -> Option<usize> {
        if let Some(&(searched, found)) = self.closers.get(name)
            && searched <= from
        {
            match found {
                None => return None,
                Some(c) if c >= from => return Some(c),
                Some(_) => {}
            }
        }
        let closer = format!("[/{name}]");
        let found = self.src[from..].find(&closer).map(|r| from + r);
        self.closers.insert(name.to_owned(), (from, found));
        found
    }

    /// WP's shortcode regex at the `[` at `at`: the piece (text for an escaped one) and the
    /// end of the match. A half-escaped `[[name]` gives the outer `[` back as text.
    fn shortcode_at(&mut self, at: usize) -> Option<(Piece, usize)> {
        let src = self.src;
        let b = src.as_bytes();
        let escaped_open = b.get(at + 1) == Some(&b'[');
        let start = if escaped_open { at + 1 } else { at };
        let name_start = start + 1;
        if !b
            .get(name_start)
            .is_some_and(|c| c.is_ascii_alphabetic() || *c == b'_')
        {
            return None;
        }
        let mut i = name_start + 1;
        while b
            .get(i)
            .is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_' || *c == b'-')
        {
            i += 1;
        }
        let name = &src[name_start..i];
        // Attributes: anything but `]`, and `/` only when not followed by `]`.
        let attrs_start = i;
        let (attrs_end, self_closing) = loop {
            match b.get(i)? {
                b']' => break (i, false),
                b'/' if b.get(i + 1) == Some(&b']') => break (i, true),
                _ => i += 1,
            }
        };
        let attrs = parse_attrs(&src[attrs_start..attrs_end]);
        let mut end = attrs_end + if self_closing { 2 } else { 1 };
        let mut inner = None;
        if !self_closing && let Some(c) = self.find_closer(name, end) {
            inner = Some(src[end..c].to_owned());
            end = c + name.len() + 3;
        }
        if escaped_open {
            if b.get(end) == Some(&b']') {
                return Some((Piece::Text(src[start..end].to_owned()), end + 1));
            }
            // `[[name]`: the outer `[` is text, the shortcode is read from the inner one.
            return None;
        }
        Some((
            Piece::Code {
                name: name.to_owned(),
                attrs,
                inner,
            },
            end,
        ))
    }
}

/// `shortcode_parse_atts`: `k="v"`, `k='v'`, `k=v`, `"v"`, `'v'` and bare `v`, each followed
/// by whitespace or the end. Keys are lower-cased; positional values are numbered.
fn parse_attrs(raw: &str) -> BTreeMap<String, String> {
    let text: String = raw
        .chars()
        .map(|c| {
            if matches!(c, '\u{a0}' | '\u{200b}') {
                ' '
            } else {
                c
            }
        })
        .collect();
    let mut attrs = BTreeMap::new();
    let mut positional = 0;
    let mut rest = text.as_str();
    loop {
        rest = rest.trim_start();
        if rest.is_empty() {
            break;
        }
        let (key, value, after) = attr(rest);
        match key {
            Some(k) => {
                attrs.insert(k.to_ascii_lowercase(), value.to_owned());
            }
            None => {
                attrs.insert(positional.to_string(), value.to_owned());
                positional += 1;
            }
        }
        rest = after;
    }
    attrs
}

/// One attribute at the start of `s` (no leading whitespace): `(key, value, rest)`.
fn attr(s: &str) -> (Option<&str>, &str, &str) {
    let key_len = s
        .find(|c: char| !(c.is_alphanumeric() || c == '_' || c == '-'))
        .unwrap_or(s.len());
    if key_len > 0 {
        let after_key = s[key_len..].trim_start();
        if let Some(v) = after_key.strip_prefix('=') {
            let v = v.trim_start();
            if let Some((value, rest)) = quoted(v) {
                return (Some(&s[..key_len]), value, rest);
            }
            let end = v
                .find(|c: char| c.is_whitespace() || c == '"' || c == '\'')
                .unwrap_or(v.len());
            if end > 0 {
                return (Some(&s[..key_len]), &v[..end], &v[end..]);
            }
        }
    }
    if let Some((value, rest)) = quoted(s) {
        return (None, value, rest);
    }
    let end = s.find(char::is_whitespace).unwrap_or(s.len());
    (None, &s[..end], &s[end..])
}

/// A `"…"` or `'…'` value at the start of `s`, followed by whitespace or the end.
fn quoted(s: &str) -> Option<(&str, &str)> {
    let q = s.chars().next().filter(|c| matches!(c, '"' | '\''))?;
    let close = s[1..].find(q)? + 1;
    let rest = &s[close + 1..];
    (rest.is_empty() || rest.starts_with(char::is_whitespace)).then(|| (&s[1..close], rest))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(s: &str) -> Piece {
        Piece::Text(s.to_owned())
    }

    fn code(name: &str, attrs: &[(&str, &str)], inner: Option<&str>) -> Piece {
        Piece::Code {
            name: name.to_owned(),
            attrs: attrs
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect(),
            inner: inner.map(str::to_owned),
        }
    }

    #[test]
    fn caption_with_inner() {
        let src = "<p>[caption id=\"attachment_5\" align=\"aligncenter\" width=\"300\"]<img src=\"/a.jpg\" alt=\"x\" /> The harbour[/caption]</p>";
        assert_eq!(
            scan(src),
            [
                text("<p>"),
                code(
                    "caption",
                    &[
                        ("id", "attachment_5"),
                        ("align", "aligncenter"),
                        ("width", "300")
                    ],
                    Some("<img src=\"/a.jpg\" alt=\"x\" /> The harbour"),
                ),
                text("</p>"),
            ]
        );
    }

    #[test]
    fn escaped_shortcode_is_literal_text() {
        assert_eq!(scan("a [[gallery]] b"), [text("a [gallery] b")]);
        assert_eq!(
            scan("[[caption]x[/caption]]"),
            [text("[caption]x[/caption]")]
        );
    }

    #[test]
    fn half_escaped_keeps_the_bracket() {
        assert_eq!(scan("[[gallery]"), [text("["), code("gallery", &[], None)]);
    }

    #[test]
    fn self_closing_and_open_without_closer() {
        assert_eq!(
            scan("[gallery ids=\"2,3\" columns=2 /] and [video src='x.mp4']"),
            [
                code("gallery", &[("ids", "2,3"), ("columns", "2")], None),
                text(" and "),
                code("video", &[("src", "x.mp4")], None),
            ]
        );
    }

    #[test]
    fn embed_with_url_inner() {
        assert_eq!(
            scan("[embed]https://vimeo.com/76979871[/embed]"),
            [code("embed", &[], Some("https://vimeo.com/76979871"))]
        );
    }

    #[test]
    fn inner_stops_at_the_first_closer() {
        assert_eq!(
            scan("[b]x[/b]y[/b]"),
            [code("b", &[], Some("x")), text("y[/b]")]
        );
    }

    #[test]
    fn attribute_forms() {
        assert_eq!(
            scan("[x Key=\"v 1\" k2='v2' k3=v3 \"pos one\" 'pos2' flag]"),
            [code(
                "x",
                &[
                    ("key", "v 1"),
                    ("k2", "v2"),
                    ("k3", "v3"),
                    ("0", "pos one"),
                    ("1", "pos2"),
                    ("2", "flag"),
                ],
                None
            )]
        );
        assert_eq!(
            scan("[x a=\u{a0}\"1\"\u{200b}b=2]"),
            [code("x", &[("a", "1"), ("b", "2")], None)]
        );
    }

    #[test]
    fn not_shortcodes() {
        // A name must start with a letter or `_` (WP takes any registered name; footnote
        // markers like `[1]` are not shortcodes here).
        for s in ["[", "[]", "[ x]", "[1] note", "[x", "a [/x] b"] {
            assert_eq!(scan(s), [text(s)], "{s}");
        }
        assert!(scan("").is_empty());
        // As in WP, a `/` not followed by `]` belongs to the attributes.
        assert_eq!(scan("[x/y]"), [code("x", &[("0", "/y")], None)]);
    }

    #[test]
    fn hyphenated_names() {
        assert_eq!(
            scan("[contact-form-7 id=\"12\" title=\"Contact\"]"),
            [code(
                "contact-form-7",
                &[("id", "12"), ("title", "Contact")],
                None
            )]
        );
    }

    #[test]
    fn many_unclosed_openers_stay_linear() {
        let src = "[x]".repeat(20_000);
        assert_eq!(scan(&src).len(), 20_000);
    }
}
