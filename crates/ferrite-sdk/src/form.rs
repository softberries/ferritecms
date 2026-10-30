//! `application/x-www-form-urlencoded` body parsing for route handlers.

use core::fmt;

/// Most pairs [`parse_urlencoded`] accepts.
pub const MAX_PAIRS: usize = 100;

/// Why a form body was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormError {
    /// A `%` not followed by two hex digits.
    BadEscape,
    /// A key or value that is not UTF-8 once decoded.
    InvalidUtf8,
    /// More than [`MAX_PAIRS`] pairs.
    TooManyPairs,
}

impl fmt::Display for FormError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::BadEscape => "bad percent escape in form body",
            Self::InvalidUtf8 => "form body is not valid UTF-8",
            Self::TooManyPairs => "too many form fields",
        })
    }
}

impl core::error::Error for FormError {}

/// Parse a urlencoded body into `(key, value)` pairs, in order.
///
/// `+` becomes a space and `%XX` is decoded. Empty segments (`a=1&&b=2`) are skipped and
/// a key with no `=` gets an empty value. At most [`MAX_PAIRS`] pairs.
pub fn parse_urlencoded(body: &[u8]) -> Result<Vec<(String, String)>, FormError> {
    let mut pairs = Vec::new();
    for segment in body.split(|&b| b == b'&') {
        if segment.is_empty() {
            continue;
        }
        if pairs.len() == MAX_PAIRS {
            return Err(FormError::TooManyPairs);
        }
        let (key, value) = match segment.iter().position(|&b| b == b'=') {
            Some(i) => (&segment[..i], &segment[i + 1..]),
            None => (segment, &segment[segment.len()..]),
        };
        pairs.push((decode(key)?, decode(value)?));
    }
    Ok(pairs)
}

fn decode(raw: &[u8]) -> Result<String, FormError> {
    let mut out = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        match raw[i] {
            b'+' => out.push(b' '),
            b'%' => {
                let hi = raw.get(i + 1).copied().and_then(hex);
                let lo = raw.get(i + 2).copied().and_then(hex);
                let (Some(hi), Some(lo)) = (hi, lo) else {
                    return Err(FormError::BadEscape);
                };
                out.push(hi << 4 | lo);
                i += 2;
            }
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8(out).map_err(|_| FormError::InvalidUtf8)
}

fn hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair(k: &str, v: &str) -> (String, String) {
        (k.to_owned(), v.to_owned())
    }

    #[test]
    fn plus_and_percent_are_decoded() {
        assert_eq!(
            parse_urlencoded(b"a=1&b=x+y%21").unwrap(),
            vec![pair("a", "1"), pair("b", "x y!")]
        );
    }

    #[test]
    fn keys_are_decoded_too() {
        assert_eq!(
            parse_urlencoded(b"a%20b=c%3Dd").unwrap(),
            vec![pair("a b", "c=d")]
        );
    }

    #[test]
    fn multibyte_utf8_round_trips() {
        assert_eq!(
            parse_urlencoded("n=%C5%82%C3%B3d%C5%BA".as_bytes()).unwrap(),
            vec![pair("n", "łódź")]
        );
    }

    #[test]
    fn empty_body_and_empty_segments() {
        assert!(parse_urlencoded(b"").unwrap().is_empty());
        assert_eq!(parse_urlencoded(b"&a=1&&").unwrap(), vec![pair("a", "1")]);
    }

    #[test]
    fn key_without_equals_has_empty_value() {
        assert_eq!(
            parse_urlencoded(b"flag&x=").unwrap(),
            vec![pair("flag", ""), pair("x", "")]
        );
    }

    #[test]
    fn only_the_first_equals_splits() {
        assert_eq!(parse_urlencoded(b"a=b=c").unwrap(), vec![pair("a", "b=c")]);
    }

    #[test]
    fn repeated_keys_keep_order() {
        assert_eq!(
            parse_urlencoded(b"t=1&t=2").unwrap(),
            vec![pair("t", "1"), pair("t", "2")]
        );
    }

    #[test]
    fn bad_escapes_are_errors() {
        assert_eq!(parse_urlencoded(b"a=%ZZ"), Err(FormError::BadEscape));
        assert_eq!(parse_urlencoded(b"a=%4"), Err(FormError::BadEscape));
        assert_eq!(parse_urlencoded(b"a=%"), Err(FormError::BadEscape));
        assert_eq!(parse_urlencoded(b"%G1=a"), Err(FormError::BadEscape));
    }

    #[test]
    fn invalid_utf8_is_an_error() {
        assert_eq!(parse_urlencoded(b"a=%FF"), Err(FormError::InvalidUtf8));
        assert_eq!(
            parse_urlencoded(&[b'a', b'=', 0xC3]),
            Err(FormError::InvalidUtf8)
        );
    }

    #[test]
    fn pair_limit_is_100() {
        let ok: Vec<String> = (0..100).map(|i| format!("k{i}=v")).collect();
        assert_eq!(
            parse_urlencoded(ok.join("&").as_bytes()).unwrap().len(),
            100
        );
        let too_many: Vec<String> = (0..101).map(|i| format!("k{i}=v")).collect();
        assert_eq!(
            parse_urlencoded(too_many.join("&").as_bytes()),
            Err(FormError::TooManyPairs)
        );
    }
}
