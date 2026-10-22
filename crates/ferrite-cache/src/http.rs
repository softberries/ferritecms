//! Conditional-request helpers (RFC 9110 §8.8.3, §13.1.2). Pure functions; the HTTP
//! layer wires them to headers.

use std::fmt::Write as _;

use sha2::{Digest, Sha256};

/// A strong ETag for `body`: the first 16 hex digits of its SHA-256, quoted
/// (`"0123456789abcdef"`).
pub fn etag_for(body: &[u8]) -> String {
    let digest = Sha256::digest(body);
    let mut out = String::with_capacity(18);
    out.push('"');
    for b in digest.iter().take(8) {
        let _ = write!(out, "{b:02x}");
    }
    out.push('"');
    out
}

/// Whether an `If-None-Match` header value matches `etag` (a quoted entity-tag, weak
/// or strong), so the response is `304 Not Modified`.
///
/// RFC 9110 §13.1.2: `*` matches any current representation; otherwise the value is
/// a comma-separated list of entity-tags compared with the **weak** comparison
/// function (opaque tags equal, `W/` ignored on both sides). Entity-tags are parsed
/// as quoted strings, so a comma inside the quotes does not split the list. A
/// malformed value matches nothing (the full response is sent, which is always safe).
pub fn if_none_match_matches(header: &str, etag: &str) -> bool {
    let Some(target) = opaque(etag) else {
        return false;
    };
    let value = header.trim_matches(|c| c == ' ' || c == '\t');
    if value == "*" {
        return true;
    }
    let mut rest = value;
    loop {
        rest = rest.trim_start_matches([' ', '\t', ',']);
        if rest.is_empty() {
            return false;
        }
        let Some((tag, after)) = next_tag(rest) else {
            return false;
        };
        if tag == target {
            return true;
        }
        rest = after;
    }
}

/// The opaque tag (with quotes) of a single entity-tag, ignoring a `W/` prefix.
fn opaque(etag: &str) -> Option<&str> {
    match next_tag(etag.trim()) {
        Some((tag, "")) => Some(tag),
        _ => None,
    }
}

/// Parses one entity-tag at the start of `s`: `[W/]"<etagc>*"`. Returns the opaque tag
/// (quotes included) and the rest after it.
fn next_tag(s: &str) -> Option<(&str, &str)> {
    let s = s.strip_prefix("W/").unwrap_or(s);
    let body = s.strip_prefix('"')?;
    let end = body.find('"')?;
    let tag_body = &body[..end];
    // etagc = %x21 / %x23-7E / obs-text
    if !tag_body
        .bytes()
        .all(|b| b == 0x21 || (0x23..=0x7e).contains(&b) || b >= 0x80)
    {
        return None;
    }
    let tag = &s[..end + 2];
    let rest = &s[end + 2..];
    // After a tag only whitespace or a list separator may follow.
    if !(rest.is_empty() || rest.starts_with([' ', '\t', ','])) {
        return None;
    }
    Some((tag, rest))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn etag_is_quoted_16_hex_and_stable() {
        let a = etag_for(b"hello");
        assert_eq!(a.len(), 18);
        assert!(a.starts_with('"') && a.ends_with('"'));
        assert!(a[1..17].bytes().all(|b| b.is_ascii_hexdigit()));
        // sha256("hello") = 2cf24dba5fb0a30e...
        assert_eq!(a, "\"2cf24dba5fb0a30e\"");
        assert_ne!(etag_for(b"hello!"), a);
    }

    #[test]
    fn if_none_match_rules() {
        let e = "\"abc\"";
        assert!(if_none_match_matches("*", e));
        assert!(if_none_match_matches(" * ", e));
        assert!(if_none_match_matches("\"abc\"", e));
        assert!(if_none_match_matches("W/\"abc\"", e), "weak comparison");
        assert!(if_none_match_matches("\"abc\"", "W/\"abc\""));
        assert!(if_none_match_matches("\"x\", \"abc\"", e));
        assert!(if_none_match_matches("\"x\",W/\"abc\" ,\"y\"", e));
        assert!(
            if_none_match_matches("\"a,b\", \"abc\"", e),
            "comma in quotes"
        );
        assert!(!if_none_match_matches("\"a,b\"", "\"a\""));
        assert!(!if_none_match_matches("\"abcd\"", e));
        assert!(!if_none_match_matches("abc", e), "unquoted is malformed");
        assert!(!if_none_match_matches("", e));
        assert!(!if_none_match_matches("\"abc", e));
        assert!(!if_none_match_matches("\"abc\"x", e));
        assert!(
            !if_none_match_matches("w/\"abc\"", e),
            "W/ is case-sensitive"
        );
        assert!(!if_none_match_matches("*", "not-quoted"));
    }
}
