//! Scrubbing of node diagnostics (M2a-2b P18, R16, Review Focus 5): error texts that
//! nodes publish in `nodes.meta` and the admin API shows (`GET /plugins`, `GET /themes`,
//! `GET /system/nodes`) can carry storage-backend text: local paths, S3 or database
//! endpoints, bucket names, key ids, credentials, addresses. [`scrub`] replaces those
//! with markers and cuts the result to [`MAX_DIAGNOSTIC`]; the full text stays in the
//! node's log, where it is produced. Over-scrubbing is accepted, a leak is not (R16).
//!
//! The scan is regex-free and linear in the input: the text is split on whitespace
//! (kept as is), and each token, with the punctuation `"'()[]{},;` `` ` `` `<>` (and a
//! trailing `:`, `.`, `!` or `?`) kept aside, is classified, first match wins:
//!
//! * a credential: the value of a secret key (`password=`, `secret_key:`, `token=`,
//!   `key=`, `bucket=`, `X-Amz-Signature=` in a query, XML such as `<AWSAccessKeyId>…`
//!   or `<BucketName>…`) → `<secret>`; a key alone (`password:`, `Authorization:`)
//!   makes the next token `<secret>`, and an auth scheme there (`Bearer`, `Basic`, …)
//!   the one after it too; a bucket word (`bucket`, `NoSuchBucket`) makes a quoted next
//!   token `<secret>`; a bare AWS key id (`AKIA`/`ASIA` + 16 `[A-Z0-9]`) → `<secret>`;
//! * a URL (contains `://`), or `user:password@host` without a scheme → `<url>`;
//! * a filesystem path (`/a/b…` with any letters, `//server/…`, `./`, `../`, `~/`,
//!   `X:\…`, `X:/…`, `\\server\…`), at the token start, after a character that cannot be
//!   part of a segment (`path=/…`) or after an ANSI escape → `<path>`. A path runs on
//!   across spaces (`/Users/john smith/…`, `C:\Program Files\…`) up to a quote, `)`, a
//!   line break or tab, `: ` or `, ` (a colon or comma before whitespace or the end), or
//!   the end of the text;
//! * an IPv4 address (lenient: leading zeros, optional `/prefix` and `:port`), an IPv6
//!   address (bracketed or not, zone, port), or a `host:port` with a numeric port, also
//!   after `=` or `@` → `<addr>` (`index.html:3` and other `file.ext:line` forms of a
//!   few template and source extensions are kept).
//!
//! Anything else is kept, so manifest and template problems (`capabilities.http: bare
//! IP addresses …`, `templates/index.html:3: …`) stay readable. The input is read up
//! to [`MAX_SCAN`] bytes, so the work per call is bounded. Scrubbing a scrubbed text
//! keeps every marker and never brings text back; on unusual input it may scrub or
//! decorate a marker further (it is not strictly idempotent).

use std::net::Ipv6Addr;

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
/// Opening quotes: a bucket word followed by a token starting with one of them.
const QUOTES: &[char] = &['"', '\'', '`'];

/// Words that name a secret (ASCII lowercase, `-`/`_` removed).
const SECRET_KEYS: &[&str] = &[
    "password",
    "passwd",
    "pwd",
    "secret",
    "secretkey",
    "secretaccesskey",
    "awssecretaccesskey",
    "awssecretkey",
    "accesskey",
    "accesskeyid",
    "awsaccesskey",
    "awsaccesskeyid",
    "key",
    "privatekey",
    "clientsecret",
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
    "bucket",
    "bucketname",
];

/// Words after which a quoted token is a bucket name (folded).
const BUCKET_WORDS: &[&str] = &["bucket", "bucketname", "nosuchbucket"];

/// Authorization schemes: the token after one of them is the credential (folded).
const AUTH_SCHEMES: &[&str] = &[
    "bearer",
    "basic",
    "token",
    "digest",
    "negotiate",
    "aws4hmacsha256",
];

/// `file.<ext>:<line>` keeps its line number for these extensions (not `host:port`).
const FILE_EXTS: &[&str] = &[
    "html", "htm", "txt", "css", "js", "json", "toml", "xml", "svg", "md", "rs", "wit", "wasm",
    "jinja", "j2", "tpl",
];

const SECRET: &str = "<secret>";

/// What the previous token asks of the next one.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Next {
    Plain,
    /// The next token is a secret value.
    Secret,
    /// The next token is a secret value if it starts with a quote (a bucket name).
    IfQuoted,
}

/// `text` with credentials, URLs, paths and addresses replaced (see the module docs),
/// cut to [`MAX_DIAGNOSTIC`] bytes on a character boundary.
pub fn scrub(text: &str) -> String {
    let text = cut(text, MAX_SCAN);
    let mut out = String::with_capacity(text.len().min(MAX_DIAGNOSTIC + 64));
    let mut next = Next::Plain;
    let mut pos = 0;
    while pos < text.len() && out.len() <= MAX_DIAGNOSTIC {
        let rest = &text[pos..];
        let ws = rest
            .find(|c: char| !c.is_whitespace())
            .unwrap_or(rest.len());
        out.push_str(&rest[..ws]);
        pos += ws;
        let rest = &text[pos..];
        let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        if end == 0 {
            break;
        }
        let (used, after) = token_into(text, pos, &rest[..end], next, &mut out);
        next = after;
        pos += used.max(1).min(text.len() - pos);
        while !text.is_char_boundary(pos) {
            pos += 1;
        }
    }
    cut(&out, MAX_DIAGNOSTIC).to_owned()
}

/// Appends the scrubbed token that starts at `text[pos..]`; returns how many bytes of
/// `text` it used (more than the token when a path runs on) and what the next token is.
fn token_into(text: &str, pos: usize, token: &str, next: Next, out: &mut String) -> (usize, Next) {
    match next {
        Next::Secret => {
            out.push_str(if token == SECRET { token } else { SECRET });
            let scheme = AUTH_SCHEMES.contains(&fold(token.trim_matches(WRAP)).as_str());
            return (token.len(), if scheme { Next::Secret } else { Next::Plain });
        }
        Next::IfQuoted if token.starts_with(QUOTES) => {
            out.push_str(SECRET);
            return (token.len(), Next::Plain);
        }
        _ => {}
    }
    // A bracketed IPv6 address with a port (`[fe80::1]:443`) as one piece.
    let open = token.trim_start_matches(|c| c != '[' && WRAP.contains(&c));
    let bracketed = open.trim_end_matches(|c| c != ']' && (WRAP.contains(&c) || TAIL.contains(&c)));
    if bracketed.starts_with('[') && is_addr(bracketed) {
        let at = token.len() - open.len();
        out.push_str(&token[..at]);
        out.push_str("<addr>");
        out.push_str(&token[at + bracketed.len()..]);
        return (token.len(), Next::Plain);
    }
    let rest = token.trim_start_matches(WRAP);
    let lead = &token[..token.len() - rest.len()];
    let core = rest.trim_end_matches(|c| WRAP.contains(&c) || TAIL.contains(&c));
    let tail = &rest[core.len()..];
    out.push_str(lead);
    if core.is_empty() {
        out.push_str(tail);
        return (token.len(), Next::Plain);
    }
    let core_at = pos + lead.len();
    let mut after = Next::Plain;
    if is_url(core) {
        out.push_str("<url>");
    } else if let Some(at) = secret_value_start(core) {
        let value = &core[at..];
        // The rest of the core goes, unless it is the marker already (`password=<secret`
        // with its `>` in `tail`).
        if value == &SECRET[..SECRET.len() - 1] {
            out.push_str(core);
        } else {
            out.push_str(&core[..at]);
            out.push_str(SECRET);
        }
        if AUTH_SCHEMES.contains(&fold(value).as_str()) {
            after = Next::Secret;
        }
    } else if has_aws_key_id(core) {
        out.push_str(SECRET);
    } else if let Some(at) = path_start(text, core_at, core) {
        out.push_str(&core[..at]);
        out.push_str("<path>");
        let end = path_end(text, core_at + at);
        return (end - pos, Next::Plain);
    } else if let Some(at) = addr_start(core) {
        out.push_str(&core[..at]);
        out.push_str("<addr>");
    } else {
        out.push_str(core);
        let key = tail.starts_with(':') || tail.starts_with('=');
        let word = fold(last_part(core));
        if key && (is_secret_word(&word) || BUCKET_WORDS.contains(&word.as_str())) {
            after = Next::Secret;
        } else if BUCKET_WORDS.contains(&word.as_str()) {
            after = Next::IfQuoted;
        }
    }
    out.push_str(tail);
    (token.len(), after)
}

/// A URL anywhere in the token (`://`), or credentials before an `@` (`user:pass@host`).
fn is_url(core: &str) -> bool {
    core.contains("://")
        || core
            .rfind('@')
            .is_some_and(|at| core[..at].contains(':') && at + 1 < core.len())
}

/// A path segment character (any letter or digit, `.`, `_`, `~`, `-`).
fn path_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '.' | '_' | '~' | '-')
}

/// Where a filesystem path starts in `core` (which starts at `text[core_at..]`): at
/// the start, after a character that cannot be part of a segment, or right after an
/// ANSI escape sequence (`ESC [ … letter`).
fn path_start(text: &str, core_at: usize, core: &str) -> Option<usize> {
    let mut boundary = true;
    // 0: none, 1: after ESC, 2: inside `ESC [ …`.
    let mut esc = 0u8;
    for (i, c) in core.char_indices() {
        if boundary && is_path_at(&text[core_at + i..]) {
            return Some(i);
        }
        let csi_end = esc == 2 && c.is_ascii_alphabetic();
        esc = match (esc, c) {
            (_, '\u{1b}') => 1,
            (1, '[') => 2,
            (2, c) if c.is_ascii_digit() || c == ';' => 2,
            _ => 0,
        };
        boundary = csi_end || !(path_char(c) || c == '/' || c == '\\');
    }
    None
}

/// Whether a path starts at the beginning of `s`. Bounded: at most 256 characters of
/// a first segment are looked at.
fn is_path_at(s: &str) -> bool {
    let mut chars = s.chars();
    let first = chars.next();
    let second = chars.next();
    let third = chars.next();
    let segment_after = |prefix: usize| s[prefix..].starts_with(path_char);
    match (first, second, third) {
        (Some('/'), Some('/'), _) => segment_after(2),
        (Some('/'), Some(c), _) if path_char(c) => {
            // A first segment (spaces allowed inside), then `/` and a segment character.
            for (n, (i, c)) in s.char_indices().skip(1).enumerate() {
                if n > 256 {
                    return false;
                }
                if c == '/' {
                    return segment_after(i + 1);
                }
                if !(path_char(c) || c == ' ') {
                    return false;
                }
            }
            false
        }
        (Some('.'), Some('/'), _) | (Some('~'), Some('/'), _) => segment_after(2),
        (Some('.'), Some('.'), Some('/')) => segment_after(3),
        (Some('\\'), Some('\\'), _) => segment_after(2),
        (Some(d), Some(':'), Some('\\' | '/')) if d.is_ascii_alphabetic() => segment_after(3),
        _ => false,
    }
}

/// Where the path starting at `text[start..]` ends: at a quote, `)`, a line break or
/// tab, a `:` or `,` before whitespace or the end, or the end of the text.
fn path_end(text: &str, start: usize) -> usize {
    let mut chars = text[start..].char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        let stop = match c {
            '"' | '\'' | '`' | ')' | '\n' | '\r' | '\t' => true,
            ':' | ',' => chars.peek().is_none_or(|(_, n)| n.is_whitespace()),
            _ => false,
        };
        if stop && i > 0 {
            return start + i;
        }
    }
    text.len()
}

/// Where an address (with an optional port) starts in `core`: the whole token, or after
/// the last `=` or `@`.
fn addr_start(core: &str) -> Option<usize> {
    if is_addr(core) {
        return Some(0);
    }
    let at = core.rfind(['=', '@'])? + 1;
    is_addr(&core[at..]).then_some(at)
}

/// An IPv4 address (lenient, optional `/prefix` and `:port`), an IPv6 address with an
/// optional zone, bracketed with an optional port or not, or `host:port`.
fn is_addr(s: &str) -> bool {
    if is_v4(s) || is_v6(s) {
        return true;
    }
    if let Some(inner) = s.strip_prefix('[') {
        let (addr, port) = inner.split_once(']').unwrap_or((inner, ""));
        let port_ok = port.is_empty() || port.strip_prefix(':').is_some_and(is_port);
        return port_ok && is_v6(addr);
    }
    s.rsplit_once(':')
        .is_some_and(|(host, port)| is_port(port) && (is_v4(host) || is_host(host)))
}

/// Four dot-separated groups of 1..=3 digits, optionally `/<prefix length>`.
fn is_v4(s: &str) -> bool {
    let s = match s.split_once('/') {
        Some((a, len))
            if (1..=3).contains(&len.len()) && len.bytes().all(|b| b.is_ascii_digit()) =>
        {
            a
        }
        Some(_) => return false,
        None => s,
    };
    let mut groups = 0;
    for g in s.split('.') {
        groups += 1;
        if groups > 4 || !(1..=3).contains(&g.len()) || !g.bytes().all(|b| b.is_ascii_digit()) {
            return false;
        }
    }
    groups == 4
}

fn is_v6(s: &str) -> bool {
    let (addr, zone) = s.split_once('%').unwrap_or((s, "x"));
    !zone.is_empty() && addr.contains(':') && addr.parse::<Ipv6Addr>().is_ok()
}

/// A host name for `host:port`: ASCII letters, digits, `.`, `-`, at least one letter,
/// and not `file.<ext>` of [`FILE_EXTS`].
fn is_host(s: &str) -> bool {
    let shape = !s.is_empty()
        && s.len() <= 253
        && !s.starts_with(['.', '-'])
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
        && s.bytes().any(|b| b.is_ascii_alphabetic());
    let file = s
        .rsplit_once('.')
        .is_some_and(|(_, ext)| FILE_EXTS.contains(&ext.to_ascii_lowercase().as_str()));
    shape && !file
}

fn is_port(s: &str) -> bool {
    !s.is_empty() && s.len() <= 5 && s.bytes().all(|b| b.is_ascii_digit())
}

/// `AKIA`/`ASIA` followed by 16 `[A-Z0-9]` anywhere in `core`.
fn has_aws_key_id(core: &str) -> bool {
    let b = core.as_bytes();
    b.windows(20).any(|w| {
        (w.starts_with(b"AKIA") || w.starts_with(b"ASIA"))
            && w[4..]
                .iter()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
    })
}

/// `key` folded for the word lists: ASCII lowercase without `-`/`_` (empty when it is
/// longer than any listed word, so long tokens cost nothing).
fn fold(key: &str) -> String {
    if key.len() > 64 {
        return String::new();
    }
    key.chars()
        .filter(|c| !matches!(c, '-' | '_'))
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// The part of `key` after its last `.`, `?`, `&`, `/`, `;` or `,` (query parameters
/// and dotted config keys count by their last name).
fn last_part(key: &str) -> &str {
    key.rsplit(['.', '?', '&', '/', ';', ','])
        .next()
        .unwrap_or(key)
}

fn is_secret_word(folded: &str) -> bool {
    SECRET_KEYS.contains(&folded)
}

/// Where the secret value starts in `core`: after `<secret key>=` or `<secret key>:`
/// (the key being the part since the last separator), or at 0 for a run of XML naming a
/// secret element (`AWSAccessKeyId>AKIA…</…`; the token's outer `<`/`>` were kept
/// aside). One pass over `core`.
fn secret_value_start(core: &str) -> Option<usize> {
    let mut key_at = 0;
    for (i, c) in core.char_indices() {
        match c {
            '=' | ':' => {
                if i > key_at && i + 1 < core.len() && is_secret_word(&fold(&core[key_at..i])) {
                    return Some(i + 1);
                }
                key_at = i + 1;
            }
            '.' | '?' | '&' | '/' | ';' | ',' | '<' | '>' => key_at = i + 1,
            _ => {}
        }
    }
    let xml = core.contains('>')
        && core
            .split(['<', '>'])
            .any(|part| is_secret_word(&fold(part.trim_start_matches('/'))));
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
            scrub(r"open C:\ferrite\media\x: failed"),
            "open <path>: failed"
        );
        assert_eq!(
            scrub("open D:/ferrite/media, failed"),
            "open <path>, failed"
        );
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
        // Scrubbing ordinary output again changes nothing.
        let once = scrub("s3 https://a:b@h/x 10.0.0.5 password=x at /var/lib/x: gone");
        assert_eq!(once, "s3 <url> <addr> password=<secret> at <path>: gone");
        assert_eq!(scrub(&once), once);
    }

    /// Review round 1 (R16: over-scrubbing is accepted, a leak is not).
    #[test]
    fn scrub_review_cases() {
        let cases: &[(&str, &str)] = &[
            // I1: paths with spaces run to their end.
            (
                "open /Users/john smith/ferrite data/media/x: denied",
                "open <path>: denied",
            ),
            (
                r"open C:\Program Files\Ferrite\media\x: denied",
                "open <path>: denied",
            ),
            (
                "path: \"/Users/john smith/Library/Application Support/ferrite/x\" (os error 2)",
                "path: \"<path>\" (os error 2)",
            ),
            ("at /var/lib/x/y, then z", "at <path>, then z"),
            ("at /var/lib/x/y\nnext line", "at <path>\nnext line"),
            ("at /var/lib/x y z", "at <path>"),
            // Non-ASCII segments, `//server/share`, UNC, ANSI escapes.
            ("open /ünïcode/media/x failed", "open <path>"),
            ("open /home/józef/media/x: no", "open <path>: no"),
            ("share //server/share/file: gone", "share <path>: gone"),
            (r"share \\fs01\media share\x: gone", "share <path>: gone"),
            (
                "error: \u{1b}[31m/var/lib/x/y\u{1b}[0m",
                "error: \u{1b}[31m<path>",
            ),
            ("x:/Users/me/a", "<path>"),
            // I2: credential values.
            (
                "bucket=my-secret-bucket key=artifacts/ab/cd",
                "bucket=<secret> key=<secret>",
            ),
            ("key=AKIAIOSFODNN7EXAMPLE", "key=<secret>"),
            ("secret_key: abc def", "secret_key: <secret> def"),
            ("access_key=abc", "access_key=<secret>"),
            ("token: abc def", "token: <secret> def"),
            ("token=abc", "token=<secret>"),
            (
                "Authorization: Bearer abcdef next",
                "Authorization: <secret> <secret> next",
            ),
            (
                "authorization: Basic dXNlcjpwdw==",
                "authorization: <secret> <secret>",
            ),
            (
                "Authorization=Bearer abcdef",
                "Authorization=<secret> <secret>",
            ),
            // Bare AWS key ids.
            ("id AKIAIOSFODNN7EXAMPLE used", "id <secret> used"),
            ("(ASIAABCDEFGHIJKLMNOP)", "(<secret>)"),
            // Bare host:port.
            (
                "connect to db.internal:5432: refused",
                "connect to <addr>: refused",
            ),
            ("minio:9000 down", "<addr> down"),
            ("localhost:8080", "<addr>"),
            // Lenient IPv4.
            ("net 10.0.0.5/24 ok", "net <addr> ok"),
            ("host 010.0.0.1 ok", "host <addr> ok"),
            // Quoted bucket names in S3 phrases.
            (
                "NoSuchBucket: \"acme-prod-media\" missing",
                "NoSuchBucket: <secret> missing",
            ),
            (
                "the bucket 'acme-prod-media' does not exist",
                "the bucket <secret> does not exist",
            ),
            (
                "exist</Message><BucketName>acme-prod-media</BucketName>",
                "<secret>>",
            ),
        ];
        for (input, want) in cases {
            assert_eq!(&scrub(input), want, "{input:?}");
        }
        // Not a bucket name (unquoted), not host:port (a template line).
        assert_eq!(scrub("the bucket list"), "the bucket list");
        assert_eq!(scrub("in index.html:3: x"), "in index.html:3: x");
        // Linear: tokens full of separators are fast, and still bounded.
        for c in ["=", ":", "a:", "/=", "=/a", "\u{1b}[", ". "] {
            let t = std::time::Instant::now();
            let s = scrub(&c.repeat(MAX_SCAN));
            assert!(s.len() <= MAX_DIAGNOSTIC);
            assert!(t.elapsed() < std::time::Duration::from_millis(250), "{c:?}");
        }
    }
}
