//! Exact decoding of one JSON scalar (a string, a bool or a number), the shapes options and
//! settings come as, so the SDK needs no JSON crate. Anything that is not exactly one such
//! literal, with only JSON whitespace around it, gives `None`.

/// The whitespace JSON allows around a value: space, tab, line feed, carriage return.
fn trim(json: &str) -> &str {
    json.trim_matches([' ', '\t', '\n', '\r'])
}

/// Decode JSON text that is exactly one string literal.
pub(crate) fn string(json: &str) -> Option<String> {
    let inner = trim(json).strip_prefix('"')?;
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    loop {
        match chars.next()? {
            '"' => return chars.as_str().is_empty().then_some(out),
            '\\' => match chars.next()? {
                '"' => out.push('"'),
                '\\' => out.push('\\'),
                '/' => out.push('/'),
                'b' => out.push('\u{8}'),
                'f' => out.push('\u{c}'),
                'n' => out.push('\n'),
                'r' => out.push('\r'),
                't' => out.push('\t'),
                'u' => out.push(unicode_escape(&mut chars)?),
                _ => return None,
            },
            c if c < ' ' => return None,
            c => out.push(c),
        }
    }
}

/// Decode JSON text that is exactly `true` or `false`.
pub(crate) fn boolean(json: &str) -> Option<bool> {
    match trim(json) {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

/// Decode JSON text that is exactly one number literal (`-?(0|[1-9][0-9]*)(.[0-9]+)?
/// ([eE][+-]?[0-9]+)?`). A number too large for an `f64` is `None`, as it is to
/// `serde_json`.
pub(crate) fn number(json: &str) -> Option<f64> {
    let text = trim(json);
    let b = text.as_bytes();
    let digits = |from: usize| b[from..].iter().take_while(|c| c.is_ascii_digit()).count();
    let mut i = usize::from(b.first() == Some(&b'-'));
    match b.get(i) {
        Some(b'0') => i += 1,
        Some(b'1'..=b'9') => i += digits(i),
        _ => return None,
    }
    if b.get(i) == Some(&b'.') {
        let n = digits(i + 1);
        if n == 0 {
            return None;
        }
        i += 1 + n;
    }
    if matches!(b.get(i), Some(b'e' | b'E')) {
        i += 1;
        if matches!(b.get(i), Some(b'+' | b'-')) {
            i += 1;
        }
        let n = digits(i);
        if n == 0 {
            return None;
        }
        i += n;
    }
    if i != b.len() {
        return None;
    }
    text.parse::<f64>().ok().filter(|n| n.is_finite())
}

/// The four hex digits after `\u`, joined with a following `\uXXXX` when they are a high
/// surrogate. A lone surrogate is invalid.
fn unicode_escape(chars: &mut core::str::Chars<'_>) -> Option<char> {
    let first = hex4(chars)?;
    match first {
        0xD800..=0xDBFF => {
            if chars.next()? != '\\' || chars.next()? != 'u' {
                return None;
            }
            let second = hex4(chars)?;
            if !(0xDC00..=0xDFFF).contains(&second) {
                return None;
            }
            char::from_u32(0x10000 + ((first - 0xD800) << 10) + (second - 0xDC00))
        }
        0xDC00..=0xDFFF => None,
        _ => char::from_u32(first),
    }
}

fn hex4(chars: &mut core::str::Chars<'_>) -> Option<u32> {
    let mut v = 0;
    for _ in 0..4 {
        v = v << 4 | chars.next()?.to_digit(16)?;
    }
    Some(v)
}

#[cfg(test)]
mod tests {
    use super::{boolean, number, string as d};

    #[test]
    fn plain_string() {
        assert_eq!(d("\"My Site\"").as_deref(), Some("My Site"));
        assert_eq!(d("\"\"").as_deref(), Some(""));
        assert_eq!(d("  \"x\"\n").as_deref(), Some("x"));
        assert_eq!(d("\t\r\"x\"").as_deref(), Some("x"));
    }

    #[test]
    fn only_json_whitespace_surrounds_a_value() {
        // A no-break space or an ideographic space is not JSON whitespace.
        assert_eq!(d("\u{a0}\"x\""), None);
        assert_eq!(d("\"x\"\u{3000}"), None);
        assert_eq!(boolean("\u{a0}true"), None);
        assert_eq!(number("5\u{a0}"), None);
    }

    #[test]
    fn simple_escapes() {
        assert_eq!(d(r#""a\"b\\c\/d""#).as_deref(), Some("a\"b\\c/d"));
        assert_eq!(d(r#""\b\f\n\r\t""#).as_deref(), Some("\u{8}\u{c}\n\r\t"));
    }

    #[test]
    fn unicode_escapes_and_surrogate_pairs() {
        assert_eq!(d(r#""Aéł""#).as_deref(), Some("Aéł"));
        assert_eq!(d(r#""😀""#).as_deref(), Some("😀"));
        assert_eq!(d(r#""é""#).as_deref(), Some("é"));
    }

    #[test]
    fn raw_non_ascii_passes_through() {
        assert_eq!(d("\"Zażółć\"").as_deref(), Some("Zażółć"));
    }

    #[test]
    fn non_strings_are_none() {
        for j in ["42", "true", "null", "[\"a\"]", "{\"a\":1}", "", "  "] {
            assert_eq!(d(j), None, "{j}");
        }
    }

    #[test]
    fn malformed_strings_are_none() {
        for j in [
            "\"abc",             // unterminated
            "abc\"",             // no opening quote
            "\"a\"b\"",          // unescaped quote inside
            "\"a\" \"b\"",       // trailing content
            r#""\x""#,           // unknown escape
            r#""\u12""#,         // short unicode escape
            r#""\u12G4""#,       // non-hex
            r#""\ud83d""#,       // lone high surrogate
            r#""\ude00""#,       // lone low surrogate
            r#""\ud83dA""#,      // high surrogate followed by a non-surrogate escape
            r#""\ud83d\ud83d""#, // high surrogate followed by another high one
            "\"a\nb\"",          // raw control character
            r#""abc\""#,         // escaped closing quote
        ] {
            assert_eq!(d(j), None, "{j}");
        }
    }

    #[test]
    fn booleans() {
        assert_eq!(boolean("true"), Some(true));
        assert_eq!(boolean(" false\n"), Some(false));
        for j in ["True", "1", "\"true\"", "null", "", "truex", "true false"] {
            assert_eq!(boolean(j), None, "{j}");
        }
    }

    #[test]
    fn numbers_follow_the_json_grammar() {
        for (j, n) in [
            ("0", 0.0),
            ("-0", 0.0),
            ("90", 90.0),
            (" 7 ", 7.0),
            ("-7", -7.0),
            ("1.5", 1.5),
            ("2e3", 2000.0),
            ("2E+3", 2000.0),
            ("25e-1", 2.5),
            ("366.4", 366.4),
            ("18446744073709551616", 18_446_744_073_709_551_616.0),
        ] {
            assert_eq!(number(j), Some(n), "{j}");
        }
        for j in [
            "", "-", "+1", "01", "1.", ".5", "1e", "1e+", "0x10", "inf", "NaN", "1 2", "\"5\"",
            "true", "null", "1_000",
        ] {
            assert_eq!(number(j), None, "{j}");
        }
    }

    #[test]
    fn a_number_out_of_range_is_none() {
        assert_eq!(number("1e400"), None);
        assert_eq!(number("-1e400"), None);
    }
}
