//! WordPress shortcodes: `[name attrs]`, `[name attrs /]`, `[name attrs]inner[/name]`, and
//! `[[name]]` for a literal `[name]`, read the way `get_shortcode_regex` and
//! `shortcode_parse_atts` read them, for any name.

use std::collections::{BTreeMap, HashMap};

use super::NextMatch;

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
        /// The opening tag as written, `[name attrs]`; the closing one is always `[/name]`.
        open: String,
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
        closers: closer_positions(text),
        bracket: NextMatch::new(text, "]"),
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

fn is_name_byte(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c == b'-'
}

/// Where each `[/name]` starts, by name, in order: one pass, so any number of distinct
/// unclosed openers stays linear.
fn closer_positions(text: &str) -> HashMap<&str, Vec<usize>> {
    let b = text.as_bytes();
    let mut map: HashMap<&str, Vec<usize>> = HashMap::new();
    let mut from = 0;
    while let Some(rel) = text[from..].find("[/") {
        let at = from + rel;
        let name_start = at + 2;
        let mut i = name_start;
        while b.get(i).copied().is_some_and(is_name_byte) {
            i += 1;
        }
        if i > name_start && b.get(i) == Some(&b']') {
            map.entry(&text[name_start..i]).or_default().push(at);
        }
        from = name_start;
    }
    map
}

struct Scanner<'a> {
    src: &'a str,
    closers: HashMap<&'a str, Vec<usize>>,
    /// `]` searches, memoised so a run of `[name` without any `]` stays linear.
    bracket: NextMatch<'a>,
}

impl Scanner<'_> {
    /// The first `[/name]` at or after `from`.
    fn find_closer(&self, name: &str, from: usize) -> Option<usize> {
        let at = self.closers.get(name)?;
        at.get(at.partition_point(|&p| p < from)).copied()
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
        while b.get(i).copied().is_some_and(is_name_byte) {
            i += 1;
        }
        let name = &src[name_start..i];
        // Attributes: anything up to the first `]`; a `/` right before it closes the tag.
        let attrs_start = i;
        let bracket = self.bracket.find(attrs_start)?;
        let self_closing = bracket > attrs_start && b[bracket - 1] == b'/';
        let attrs_end = if self_closing { bracket - 1 } else { bracket };
        let attrs = parse_attrs(&src[attrs_start..attrs_end]);
        let mut end = bracket + 1;
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
                open: src[start..=bracket].to_owned(),
            },
            end,
        ))
    }
}

/// PCRE's `\s` without the `u` flag (WP's attribute regex has none).
fn is_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\x0b' | '\x0c' | '\r')
}

/// `shortcode_parse_atts`: `k="v"`, `k='v'`, `k=v`, `"v"`, `'v'` and bare `v`, each followed
/// by whitespace or the end. Keys are lower-cased; positional values are numbered. Values
/// go through `stripcslashes`, and one with a `<` that no `>` closes becomes empty.
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
        rest = rest.trim_start_matches(is_space);
        if rest.is_empty() {
            break;
        }
        let (key, value, after) = attr(rest);
        let value = value_of(value);
        match key {
            Some(k) => {
                attrs.insert(k.to_ascii_lowercase(), value);
            }
            None => {
                attrs.insert(positional.to_string(), value);
                positional += 1;
            }
        }
        rest = after;
    }
    attrs
}

/// A raw value after `stripcslashes`, emptied when it holds an unclosed `<` (WP's
/// `/^[^<]*+(?:<[^>]*+>[^<]*+)*+$/` check).
fn value_of(raw: &str) -> String {
    let v = stripcslashes(raw);
    let mut rest = v.as_str();
    while let Some(lt) = rest.find('<') {
        match rest[lt..].find('>') {
            Some(gt) => rest = &rest[lt + gt + 1..],
            None => return String::new(),
        }
    }
    v
}

/// PHP's `stripcslashes`: C escapes (`\n`, `\t`, `\r`, `\a`, `\v`, `\b`, `\f`, `\xHH`, octal
/// `\OOO`) become their bytes, any other `\c` becomes `c`; a final `\` stays. Bytes that are
/// not UTF-8 become U+FFFD.
fn stripcslashes(s: &str) -> String {
    if !s.contains('\\') {
        return s.to_owned();
    }
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'\\' || i + 1 == b.len() {
            out.push(b[i]);
            i += 1;
            continue;
        }
        i += 1;
        let c = b[i];
        i += 1;
        out.push(match c {
            b'n' => b'\n',
            b't' => b'\t',
            b'r' => b'\r',
            b'a' => 0x07,
            b'v' => 0x0b,
            b'b' => 0x08,
            b'f' => 0x0c,
            b'x' if b.get(i).is_some_and(u8::is_ascii_hexdigit) => {
                let mut v = 0u8;
                for _ in 0..2 {
                    match b.get(i).and_then(|d| (*d as char).to_digit(16)) {
                        Some(d) => {
                            v = v.wrapping_mul(16).wrapping_add(d as u8);
                            i += 1;
                        }
                        None => break,
                    }
                }
                v
            }
            b'0'..=b'7' => {
                let mut v = c - b'0';
                for _ in 0..2 {
                    match b.get(i) {
                        Some(d @ b'0'..=b'7') => {
                            v = v.wrapping_mul(8).wrapping_add(d - b'0');
                            i += 1;
                        }
                        _ => break,
                    }
                }
                v
            }
            other => other,
        });
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// One attribute at the start of `s` (no leading whitespace): `(key, value, rest)`. Keys
/// are `[\w-]+` with PCRE's ASCII `\w`.
fn attr(s: &str) -> (Option<&str>, &str, &str) {
    let key_len = s
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'))
        .unwrap_or(s.len());
    if key_len > 0 {
        let after_key = s[key_len..].trim_start_matches(is_space);
        if let Some(v) = after_key.strip_prefix('=') {
            let v = v.trim_start_matches(is_space);
            if let Some((value, rest)) = quoted(v) {
                return (Some(&s[..key_len]), value, rest);
            }
            let end = v
                .find(|c: char| is_space(c) || c == '"' || c == '\'')
                .unwrap_or(v.len());
            if end > 0 {
                return (Some(&s[..key_len]), &v[..end], &v[end..]);
            }
        }
    }
    if let Some((value, rest)) = quoted(s) {
        return (None, value, rest);
    }
    let end = s.find(is_space).unwrap_or(s.len());
    (None, &s[..end], &s[end..])
}

/// A `"…"` or `'…'` value at the start of `s`, followed by whitespace or the end.
fn quoted(s: &str) -> Option<(&str, &str)> {
    let q = s.chars().next().filter(|c| matches!(c, '"' | '\''))?;
    let close = s[1..].find(q)? + 1;
    let rest = &s[close + 1..];
    (rest.is_empty() || rest.starts_with(is_space)).then(|| (&s[1..close], rest))
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
            open: String::new(),
        }
    }

    /// `scan` with `open` cleared, to compare with [`code`].
    fn scan_t(src: &str) -> Vec<Piece> {
        scan(src)
            .into_iter()
            .map(|p| match p {
                Piece::Code {
                    name, attrs, inner, ..
                } => Piece::Code {
                    name,
                    attrs,
                    inner,
                    open: String::new(),
                },
                t => t,
            })
            .collect()
    }

    #[test]
    fn open_is_the_opening_tag_as_written() {
        let opens: Vec<String> = scan("a [x  k='v' /] [[y]] [[z] [b]in[/b]")
            .into_iter()
            .filter_map(|p| match p {
                Piece::Code { open, .. } => Some(open),
                Piece::Text(_) => None,
            })
            .collect();
        assert_eq!(opens, ["[x  k='v' /]", "[z]", "[b]"]);
    }

    #[test]
    fn caption_with_inner() {
        let src = "<p>[caption id=\"attachment_5\" align=\"aligncenter\" width=\"300\"]<img src=\"/a.jpg\" alt=\"x\" /> The harbour[/caption]</p>";
        assert_eq!(
            scan_t(src),
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
        assert_eq!(scan_t("a [[gallery]] b"), [text("a [gallery] b")]);
        assert_eq!(
            scan_t("[[caption]x[/caption]]"),
            [text("[caption]x[/caption]")]
        );
    }

    #[test]
    fn half_escaped_keeps_the_bracket() {
        assert_eq!(
            scan_t("[[gallery]"),
            [text("["), code("gallery", &[], None)]
        );
    }

    #[test]
    fn self_closing_and_open_without_closer() {
        assert_eq!(
            scan_t("[gallery ids=\"2,3\" columns=2 /] and [video src='x.mp4']"),
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
            scan_t("[embed]https://vimeo.com/76979871[/embed]"),
            [code("embed", &[], Some("https://vimeo.com/76979871"))]
        );
    }

    #[test]
    fn inner_stops_at_the_first_closer() {
        assert_eq!(
            scan_t("[b]x[/b]y[/b]"),
            [code("b", &[], Some("x")), text("y[/b]")]
        );
    }

    #[test]
    fn attribute_forms() {
        assert_eq!(
            scan_t("[x Key=\"v 1\" k2='v2' k3=v3 \"pos one\" 'pos2' flag]"),
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
            scan_t("[x a=\u{a0}\"1\"\u{200b}b=2]"),
            [code("x", &[("a", "1"), ("b", "2")], None)]
        );
    }

    #[test]
    fn not_shortcodes() {
        // A name must start with a letter or `_` (WP takes any registered name; footnote
        // markers like `[1]` are not shortcodes here).
        for s in ["[", "[]", "[ x]", "[1] note", "[x", "a [/x] b"] {
            assert_eq!(scan_t(s), [text(s)], "{s}");
        }
        assert!(scan_t("").is_empty());
        // As in WP, a `/` not followed by `]` belongs to the attributes.
        assert_eq!(scan_t("[x/y]"), [code("x", &[("0", "/y")], None)]);
    }

    #[test]
    fn hyphenated_names() {
        assert_eq!(
            scan_t("[contact-form-7 id=\"12\" title=\"Contact\"]"),
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
        assert_eq!(scan_t(&src).len(), 20_000);
    }

    #[test]
    fn attribute_keys_are_ascii_word_characters() {
        assert_eq!(
            scan_t("[x k\u{e9}y=\"v\" ok=1]"),
            [code("x", &[("0", "k\u{e9}y=\"v\""), ("ok", "1")], None)]
        );
    }

    #[test]
    fn attribute_values_are_stripcslashed() {
        assert_eq!(
            scan_t(r#"[x a="tab\there" b="\x41\101\q\x" c=end\]"#),
            [code(
                "x",
                &[("a", "tab\there"), ("b", "AAqx"), ("c", "end\\")],
                None
            )]
        );
    }

    #[test]
    fn values_with_an_unclosed_lt_are_emptied() {
        assert_eq!(
            scan_t("[x a=\"<b>ok</b>\" b=\"<script\" c=\"1 < 2\" d=\"a<b>c\"]"),
            [code(
                "x",
                &[("a", "<b>ok</b>"), ("b", ""), ("c", ""), ("d", "a<b>c")],
                None
            )]
        );
    }

    #[test]
    fn unclosed_openers_stay_linear() {
        let start = std::time::Instant::now();
        let src = "[x".repeat(200_000);
        assert_eq!(scan_t(&src), [text(&src)]);
        let distinct: String = (0..200_000).map(|i| format!("[a{i}]")).collect();
        assert_eq!(scan_t(&distinct).len(), 200_000);
        assert!(start.elapsed() < std::time::Duration::from_secs(10));
    }
}
