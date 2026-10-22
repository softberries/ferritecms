//! Request path splitting and strict, single-pass percent-decoding.

use super::MAX_SEGMENTS;

/// The decoded segments of `raw` (a path starting with `/`; one trailing `/` allowed),
/// or `None` when any segment is rejected by [`decode_segment`] or there are more than
/// [`MAX_SEGMENTS`]. `/` is the empty list. Empty interior segments (`//x`, `/a//b/`)
/// are rejected rather than collapsed.
pub(super) fn segments(raw: &str) -> Option<Vec<String>> {
    let inner = raw.strip_prefix('/')?;
    if inner.is_empty() {
        return Some(Vec::new());
    }
    let inner = inner.strip_suffix('/').unwrap_or(inner);
    decode_all(inner.split('/'))
}

/// Decodes every segment of `parts`, at most [`MAX_SEGMENTS`] of them.
pub(super) fn decode_all<'a>(parts: impl Iterator<Item = &'a str>) -> Option<Vec<String>> {
    let mut out = Vec::new();
    for part in parts {
        if out.len() == MAX_SEGMENTS {
            return None;
        }
        out.push(decode_segment(part)?);
    }
    Some(out)
}

/// Percent-decodes one raw segment exactly once. `None` when the segment is empty, `.`
/// or `..` (raw or decoded), has a malformed escape, decodes to invalid UTF-8, or
/// contains a control character (`%00` included), `/` or `\`.
pub(super) fn decode_segment(raw: &str) -> Option<String> {
    if raw.is_empty() {
        return None;
    }
    let s = unescape(raw)?;
    let bad = s == "." || s == ".." || s.chars().any(|c| c.is_control() || c == '/' || c == '\\');
    (!bad).then_some(s)
}

/// Like [`decode_segment`] for `application/x-www-form-urlencoded` values: `+` is a
/// space. Only rejects malformed escapes, invalid UTF-8 and control characters.
pub(super) fn decode_form(raw: &str) -> Option<String> {
    let s = unescape(&raw.replace('+', " "))?;
    (!s.chars().any(char::is_control)).then_some(s)
}

/// Replaces every `%XX` escape (either case of hex) by its byte; `None` on a malformed
/// escape or when the result is not UTF-8.
fn unescape(raw: &str) -> Option<String> {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hi = hex(*bytes.get(i + 1)?)?;
            let lo = hex(*bytes.get(i + 2)?)?;
            out.push(hi << 4 | lo);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// `n` digits only (no sign, no space).
pub(super) fn digits(s: &str, n: usize) -> bool {
    s.len() == n && s.bytes().all(|b| b.is_ascii_digit())
}

/// A page number in `1..=max` written without a leading zero.
pub(super) fn number(s: &str, max: u32) -> Option<u32> {
    if s.is_empty() || s.starts_with('0') || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok().filter(|n| (1..=max).contains(n))
}
