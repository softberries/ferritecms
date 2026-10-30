//! Site options (capability `options.read`).

use crate::bindings::ferrite::plugin::options as wit_options;

/// The option `key` when its value is a JSON string, decoded. `None` when the option is
/// not readable by this plugin, is unset, or is any other JSON type (number, bool, object).
pub fn get(key: &str) -> Option<String> {
    wit_options::get(key).and_then(|json| decode_json_string(&json))
}

/// Decode JSON text that is exactly one string literal. Anything else gives `None`.
pub(crate) fn decode_json_string(json: &str) -> Option<String> {
    let inner = json.trim().strip_prefix('"')?;
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
    use super::decode_json_string as d;

    #[test]
    fn plain_string() {
        assert_eq!(d("\"My Site\"").as_deref(), Some("My Site"));
        assert_eq!(d("\"\"").as_deref(), Some(""));
        assert_eq!(d("  \"x\"\n").as_deref(), Some("x"));
    }

    #[test]
    fn simple_escapes() {
        assert_eq!(d(r#""a\"b\\c\/d""#).as_deref(), Some("a\"b\\c/d"));
        assert_eq!(d(r#""\b\f\n\r\t""#).as_deref(), Some("\u{8}\u{c}\n\r\t"));
    }

    #[test]
    fn unicode_escapes_and_surrogate_pairs() {
        assert_eq!(d(r#""\u0041\u00e9\u0142""#).as_deref(), Some("Aéł"));
        assert_eq!(d(r#""\ud83d\ude00""#).as_deref(), Some("😀"));
        assert_eq!(d(r#""\u00E9""#).as_deref(), Some("é"));
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
            r#""\ud83d\u0041""#, // high surrogate followed by a non-surrogate escape
            r#""\ud83d\ud83d""#, // high surrogate followed by another high one
            "\"a\nb\"",          // raw control character
            r#""abc\""#,         // escaped closing quote
        ] {
            assert_eq!(d(j), None, "{j}");
        }
    }
}
