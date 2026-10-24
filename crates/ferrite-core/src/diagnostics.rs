//! Scrubbing of node diagnostics (M2a-2b P18, Review Focus 5): error texts that nodes
//! publish in `nodes.meta` and the admin API shows (`GET /plugins`, `GET /themes`,
//! `GET /system/nodes`) can carry storage-backend text: local paths, S3 or database
//! endpoints, credentials in URLs, addresses. [`scrub`] replaces those with markers and
//! cuts the result to [`MAX_DIAGNOSTIC`]; the full text stays in the node's log, where
//! it is produced.
//!
//! The scan is token based and regex-free: the text is split on whitespace (kept as
//! is), and each token, with the punctuation `"'()[]{},;` (and a trailing `:`, `.`,
//! `!` or `?`) kept aside, is classified:
//!
//! * a URL (contains `<scheme>://`), or `user:password@host` without a scheme →
//!   `<url>`;
//! * an absolute path (`/` followed by at least two `[A-Za-z0-9._~-]+` segments,
//!   `./…`, `../…`, `~/…`, `X:\…`, `X:/…`, `\\server\…`), also after a prefix such as
//!   `file=` → that prefix + `<path>`;
//! * an IPv4 address (optionally `:port`) or an IPv6 address (parses as `Ipv6Addr`,
//!   bracketed or not, optionally `[…]:port`), also after `=` or `@` → `<addr>`;
//! * a credential: a token naming a secret (`password=…`, `secret_access_key:…`,
//!   `<AWSAccessKeyId>…`, `X-Amz-Signature=…`) has its value replaced by `<secret>`,
//!   and a token that is only such a key (`password:`) makes the next token `<secret>`.
//!
//! Anything else is kept, so manifest and template problems (`capabilities.http: bare
//! IP addresses …`, `templates/index.html:3: …`) stay readable. The input is read up
//! to [`MAX_SCAN`] bytes, so the work per call is bounded.

use std::net::{Ipv4Addr, Ipv6Addr};

/// Longest scrubbed text (bytes), cut on a character boundary.
pub const MAX_DIAGNOSTIC: usize = 1024;
/// Most input bytes looked at; the rest is dropped before scanning.
pub const MAX_SCAN: usize = 16 * 1024;

/// Punctuation kept aside around a token before it is classified (`<`/`>` too, so the
/// markers this module writes are kept as they are when a text is scrubbed again).
const WRAP: &[char] = &[
    '"', '\'', '(', ')', '[', ']', '{', '}', ',', ';', '`', '<', '>',
];
/// Kept aside at the end of a token only (sentence punctuation, `path:` in messages).
const TAIL: &[char] = &[':', '.', '!', '?'];

/// Words that name a secret (ASCII lowercase, `-`/`_` removed): a token
/// `<key>=<value>` or `<key>:<value>` has its value replaced, as has an XML element of
/// that name.
const SECRET_KEYS: &[&str] = &[
    "password",
    "passwd",
    "pwd",
    "secret",
    "secretaccesskey",
    "awssecretaccesskey",
    "accesskeyid",
    "awsaccesskeyid",
    "token",
    "sessiontoken",
    "securitytoken",
    "apikey",
    "authorization",
    "credential",
    "credentials",
    "signature",
    "xamzsignature",
    "xamzcredential",
    "xamzsecuritytoken",
];

/// Keys whose value may be the next token (`password: hunter2`); a subset of
/// [`SECRET_KEYS`] without words that are common in error messages (`token`).
const NEXT_TOKEN_KEYS: &[&str] = &[
    "password",
    "passwd",
    "secretaccesskey",
    "awssecretaccesskey",
    "sessiontoken",
    "apikey",
    "authorization",
];

const SECRET: &str = "<secret>";

/// `text` with URLs, paths, addresses and credentials replaced (see the module docs),
/// cut to [`MAX_DIAGNOSTIC`] bytes on a character boundary. Scrubbing a scrubbed text
/// changes nothing.
pub fn scrub(text: &str) -> String {
    let text = cut(text, MAX_SCAN);
    let mut out = String::with_capacity(text.len().min(MAX_DIAGNOSTIC + 64));
    let mut secret_next = false;
    let mut rest = text;
    while !rest.is_empty() && out.len() <= MAX_DIAGNOSTIC {
        let ws = rest
            .find(|c: char| !c.is_whitespace())
            .unwrap_or(rest.len());
        out.push_str(&rest[..ws]);
        rest = &rest[ws..];
        let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        let (token, after) = rest.split_at(end);
        rest = after;
        if !token.is_empty() {
            secret_next = token_into(token, secret_next, &mut out);
        }
    }
    cut(&out, MAX_DIAGNOSTIC).to_owned()
}

/// Appends `token` scrubbed; returns whether the next token is a secret value.
fn token_into(token: &str, secret: bool, out: &mut String) -> bool {
    if secret {
        out.push_str(if token == SECRET { token } else { SECRET });
        return false;
    }
    // A bracketed IPv6 address with a port (`[fe80::1]:443`) as one piece.
    let bracketed = token
        .trim_start_matches(|c| c != '[' && WRAP.contains(&c))
        .trim_end_matches(|c| c != ']' && (WRAP.contains(&c) || TAIL.contains(&c)));
    if bracketed.starts_with('[') && is_addr(bracketed) {
        let at = token.len()
            - token
                .trim_start_matches(|c| c != '[' && WRAP.contains(&c))
                .len();
        out.push_str(&token[..at]);
        out.push_str("<addr>");
        out.push_str(&token[at + bracketed.len()..]);
        return false;
    }
    let rest = token.trim_start_matches(WRAP);
    let lead = &token[..token.len() - rest.len()];
    let core = rest.trim_end_matches(|c| WRAP.contains(&c) || TAIL.contains(&c));
    let tail = &rest[core.len()..];
    out.push_str(lead);
    if core.is_empty() {
        out.push_str(tail);
        return false;
    }
    if let Some(at) = secret_value_start(core) {
        // The rest of the core goes, unless it is the marker already (`password=<secret`
        // with its `>` in `tail`).
        if core[at..] == SECRET[..SECRET.len() - 1] {
            out.push_str(core);
        } else {
            out.push_str(&core[..at]);
            out.push_str(SECRET);
        }
        out.push_str(tail);
        return false;
    }
    let next = (tail.starts_with(':') || tail.starts_with('='))
        && NEXT_TOKEN_KEYS.contains(&fold(core).as_str());
    if is_url(core) {
        out.push_str("<url>");
    } else if let Some(at) = path_start(core) {
        out.push_str(&core[..at]);
        out.push_str("<path>");
    } else if let Some(at) = addr_start(core) {
        out.push_str(&core[..at]);
        out.push_str("<addr>");
    } else {
        out.push_str(core);
    }
    out.push_str(tail);
    next
}

/// A URL anywhere in the token (`://`), or credentials before an `@` (`user:pass@host`).
fn is_url(core: &str) -> bool {
    core.contains("://")
        || core
            .rfind('@')
            .is_some_and(|at| core[..at].contains(':') && at + 1 < core.len())
}

fn path_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '~' | '-')
}

/// Where a filesystem path starts in `core`, if one does: at the start or after a
/// character that cannot be part of a path segment (`=`, `:`, …).
fn path_start(core: &str) -> Option<usize> {
    let mut prev: Option<char> = None;
    for (i, c) in core.char_indices() {
        let boundary = prev.is_none_or(|p| !(path_char(p) || p == '/' || p == '\\'));
        prev = Some(c);
        if !boundary {
            continue;
        }
        let at = &core[i..];
        let unix = at.strip_prefix('/').is_some_and(two_segments);
        let relative = ["./", "../", "~/"].iter().any(|p| at.starts_with(p));
        let unc = at.len() > 2 && at.starts_with("\\\\");
        let b = at.as_bytes();
        let drive = b.len() > 3
            && b[0].is_ascii_alphabetic()
            && b[1] == b':'
            && matches!(b[2], b'\\' | b'/');
        if unix || relative || unc || drive {
            return Some(i);
        }
    }
    None
}

/// Whether `s` starts with `segment/segment` (segments of [`path_char`]s).
fn two_segments(s: &str) -> bool {
    let Some((first, rest)) = s.split_once('/') else {
        return false;
    };
    !first.is_empty() && first.chars().all(path_char) && rest.starts_with(path_char)
}

/// Where an IP address (with an optional port) starts in `core`: the whole token, or
/// after the last `=` or `@`.
fn addr_start(core: &str) -> Option<usize> {
    if is_addr(core) {
        return Some(0);
    }
    let at = core.rfind(['=', '@'])? + 1;
    is_addr(&core[at..]).then_some(at)
}

/// An IPv4 address with an optional port, or an IPv6 address with an optional zone,
/// bracketed with an optional port or not.
fn is_addr(s: &str) -> bool {
    if s.parse::<Ipv4Addr>().is_ok() || is_v6(s) {
        return true;
    }
    if let Some(inner) = s.strip_prefix('[') {
        let (addr, port) = inner.split_once(']').unwrap_or((inner, ""));
        let port_ok = port.is_empty() || port.strip_prefix(':').is_some_and(is_port);
        return port_ok && is_v6(addr);
    }
    s.rsplit_once(':')
        .is_some_and(|(addr, port)| is_port(port) && addr.parse::<Ipv4Addr>().is_ok())
}

fn is_v6(s: &str) -> bool {
    let (addr, zone) = s.split_once('%').unwrap_or((s, "x"));
    !zone.is_empty() && addr.parse::<Ipv6Addr>().is_ok()
}

fn is_port(s: &str) -> bool {
    !s.is_empty() && s.len() <= 5 && s.bytes().all(|b| b.is_ascii_digit())
}

/// `key` folded for [`SECRET_KEYS`]: ASCII lowercase without `-`/`_`.
fn fold(key: &str) -> String {
    key.chars()
        .filter(|c| !matches!(c, '-' | '_'))
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// Whether `key` (its last part after `.`, `?` or `&`, so query parameters count)
/// names a secret.
fn is_secret_key(key: &str) -> bool {
    let key = key.rsplit(['.', '?', '&']).next().unwrap_or(key);
    key.len() <= 64 && SECRET_KEYS.contains(&fold(key).as_str())
}

/// Where the secret value starts in `core`: after `<secret key>=` or `<secret key>:`,
/// or at 0 for a run of XML naming a secret element (`AWSAccessKeyId>AKIA…</…`; the
/// token's outer `<`/`>` were kept aside).
fn secret_value_start(core: &str) -> Option<usize> {
    for (i, c) in core.char_indices() {
        if matches!(c, '=' | ':') && i > 0 && i + 1 < core.len() && is_secret_key(&core[..i]) {
            return Some(i + 1);
        }
    }
    let xml = core.contains('>')
        && core
            .split(['<', '>'])
            .any(|part| is_secret_key(part.trim_start_matches('/')));
    xml.then_some(0)
}

/// `s` cut to at most `max` bytes on a character boundary.
fn cut(s: &str, max: usize) -> &str {
    let mut end = s.len().min(max);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrub_cases() {
        // A URL with credentials: nothing of it survives.
        let s = scrub("s3 error: https://key:secret@minio:9000/bucket/x");
        assert_eq!(s, "s3 error: <url>");
        assert!(!s.contains("secret") && !s.contains("minio"), "{s}");
        // DSNs and endpoints.
        assert_eq!(
            scrub("cannot connect to postgres://ferrite:pw@db.internal:5432/site"),
            "cannot connect to <url>"
        );
        assert_eq!(
            scrub(
                "Error performing GET http://127.0.0.1:9000/ferrite-test/site/artifacts/ab/x in 3ms"
            ),
            "Error performing GET <url> in 3ms"
        );
        assert_eq!(scrub("dsn ferrite:pw@db.internal"), "dsn <url>");
        // Absolute paths, with punctuation and prefixes around them.
        assert_eq!(
            scrub(
                "storage backend error: Unable to open file /var/lib/ferrite/media/artifacts/ab/abcd: Permission denied (os error 13)"
            ),
            "storage backend error: Unable to open file <path>: Permission denied (os error 13)"
        );
        assert_eq!(
            scrub("(\"/var/lib/ferrite/media/artifacts/ab/x\")"),
            "(\"<path>\")"
        );
        assert_eq!(scrub("path=/srv/site/media"), "path=<path>");
        assert_eq!(scrub("at ./site/media/x"), "at <path>");
        assert_eq!(scrub("in ~/sites/a"), "in <path>");
        assert_eq!(
            scrub(r"open C:\ferrite\media\x failed"),
            "open <path> failed"
        );
        assert_eq!(scrub("open D:/ferrite/media failed"), "open <path> failed");
        assert_eq!(scrub(r"share \\server\media\x"), "share <path>");
        // Addresses, with and without ports, bracketed IPv6.
        assert_eq!(
            scrub("connect 10.0.0.5:5432 refused"),
            "connect <addr> refused"
        );
        assert_eq!(scrub("peer [fe80::1] gone"), "peer <addr> gone");
        assert_eq!(scrub("peer [fe80::1]:443."), "peer <addr>.");
        assert_eq!(scrub("host=10.1.2.3 user=x"), "host=<addr> user=x");
        assert_eq!(scrub("via 2001:db8::7, then"), "via <addr>, then");
        assert_eq!(scrub("fe80::1%en0"), "<addr>");
        // Credentials by key.
        assert_eq!(scrub("password=hunter2 ok"), "password=<secret> ok");
        assert_eq!(scrub("password: hunter2 ok"), "password: <secret> ok");
        assert_eq!(scrub("Secret-Access-Key=abc"), "Secret-Access-Key=<secret>");
        assert_eq!(
            scrub("<Error><AWSAccessKeyId>AKIAEXAMPLE</AWSAccessKeyId></Error>"),
            "<<secret>>"
        );
        assert_eq!(
            scrub("<AWSAccessKeyId>AKIAEXAMPLE</AWSAccessKeyId>"),
            "<<secret>>"
        );
        assert!(
            !scrub("x?X-Amz-Credential=AKIA/2024&X-Amz-Signature=deadbeef").contains("deadbeef")
        );
        // Manifest and template problems survive; a listed bare IP does not.
        let manifest = "capabilities.http: bare IP addresses and localhost are not allowed (a hostname needs a dot and a last label that is not a number); invalid: \"10.0.0.1\"";
        let s = scrub(manifest);
        assert!(
            s.starts_with("capabilities.http: bare IP addresses and localhost are not allowed"),
            "{s}"
        );
        assert!(s.ends_with("invalid: \"<addr>\""), "{s}");
        for keep in [
            "templates/index.html:3: unknown filter",
            "theme \"uploaded\": hash mismatch",
            "plugin hello 0.2.0 (a1b2c3) failed: wasm trap: unreachable",
            "mime application/octet-stream is not a component",
            "ferrite_http::api::x failed: 1/2 done",
            "line\nbreaks\tstay",
            "",
        ] {
            assert_eq!(scrub(keep), keep);
        }
        // The cut keeps UTF-8 and the cap.
        let long = "é".repeat(MAX_DIAGNOSTIC);
        let s = scrub(&long);
        assert!(
            s.len() <= MAX_DIAGNOSTIC && s.len() > MAX_DIAGNOSTIC - 2,
            "{}",
            s.len()
        );
        assert!(s.chars().all(|c| c == 'é'));
        // A huge input is bounded and still scrubbed up to the cap.
        let huge = "/var/lib/ferrite/x ".repeat(100_000);
        let s = scrub(&huge);
        assert!(s.len() <= MAX_DIAGNOSTIC && !s.contains("ferrite"), "{s}");
        // Idempotent: scrubbing twice is scrubbing once.
        let once = scrub("s3 https://a:b@h/x /var/lib/x 10.0.0.5 password=x");
        assert_eq!(scrub(&once), once);
    }
}
