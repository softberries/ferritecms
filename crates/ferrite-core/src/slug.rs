//! Slug generation and validation.

use crate::CoreError;
use std::collections::HashSet;

const MAX_LEN: usize = 200;

/// Slugs a top-level item may not take: they would shadow core routes and files.
pub const RESERVED: &[&str] = &[
    "admin",
    "api",
    "feed",
    "_media",
    "_theme",
    "_preview",
    "_p",
    "_comment",
    "sitemap.xml",
    "robots.txt",
    "healthz",
    "readyz",
    "metrics",
];

/// Router words a top-level item may not take (ruling R13): `/author/{login}/`,
/// `/page/N/` and search. Taxonomy names (`/{taxonomy}/{slug}/`) and content type
/// names are reserved the same way by the content service, which knows them.
pub const ROUTE_WORDS: &[&str] = &["author", "page", "search"];

/// Whether `slug` is four ASCII digits: the shape of a `/{yyyy}/` date archive, which
/// a top-level item may not take (ruling R13).
pub fn is_year(slug: &str) -> bool {
    slug.len() == 4 && slug.bytes().all(|b| b.is_ascii_digit())
}

/// Whether `slug` is in [`RESERVED`].
pub fn is_reserved(slug: &str) -> bool {
    RESERVED.contains(&slug)
}

/// Core-generated slug: transliterate, lowercase, runs of non-`[a-z0-9]` become `-`,
/// trim `-`, at most 200 chars (cut at a `-` boundary when possible), never empty.
pub fn slugify(input: &str) -> String {
    let ascii = deunicode::deunicode(input).to_ascii_lowercase();
    let mut out = String::with_capacity(ascii.len());
    for c in ascii.chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let mut s = out.trim_matches('-');
    if s.len() > MAX_LEN {
        // Output is pure ASCII, so byte slicing is safe.
        let head = &s[..MAX_LEN];
        s = if s.as_bytes()[MAX_LEN] == b'-' {
            head
        } else {
            head.rfind('-').map_or(head, |i| &head[..i])
        };
        s = s.trim_matches('-');
    }
    if s.is_empty() {
        "untitled".to_owned()
    } else {
        s.to_owned()
    }
}

/// Validates a caller-supplied slug and returns it normalized (trimmed, lowercased).
/// [`RESERVED`] words are checked by the caller, which knows whether the item is
/// top-level.
pub fn validate(input: &str) -> Result<String, CoreError> {
    let s = input.trim().to_lowercase();
    if s.is_empty() {
        return Err(CoreError::invalid_field("slug", "slug must not be empty"));
    }
    if s.chars().count() > MAX_LEN {
        return Err(CoreError::invalid_field(
            "slug",
            format!("slug longer than {MAX_LEN} characters"),
        ));
    }
    if s == "." || s == ".." {
        return Err(CoreError::invalid_field(
            "slug",
            "slug must not be \".\" or \"..\"",
        ));
    }
    if s.chars().any(is_forbidden_char) {
        return Err(CoreError::invalid_field(
            "slug",
            "slug must not contain whitespace, control or format characters, or any of / ? # % < > \" ' \\ `",
        ));
    }
    Ok(s)
}

/// Unicode general category Cf (format characters), per Unicode 16.
fn is_format_char(c: char) -> bool {
    matches!(c as u32,
        0x00AD | 0x0600..=0x0605 | 0x061C | 0x06DD | 0x070F | 0x0890..=0x0891 | 0x08E2
        | 0x180E | 0x200B..=0x200F | 0x202A..=0x202E | 0x2060..=0x2064 | 0x2066..=0x206F
        | 0xFEFF | 0xFFF9..=0xFFFB | 0x110BD | 0x110CD | 0x13430..=0x1343F
        | 0x1BCA0..=0x1BCA3 | 0x1D173..=0x1D17A | 0xE0001 | 0xE0020..=0xE007F)
}

fn is_forbidden_char(c: char) -> bool {
    c.is_whitespace()
        || c.is_control()
        || is_format_char(c)
        || matches!(
            c,
            '/' | '?' | '#' | '%' | '<' | '>' | '"' | '\'' | '\\' | '`'
        )
}

/// Shortens `base` to at most `max` chars, cutting at a `-` boundary when possible.
fn truncate_base(base: &str, max: usize) -> &str {
    let Some((end, _)) = base.char_indices().nth(max) else {
        return base;
    };
    let head = &base[..end];
    let cut = if base[end..].starts_with('-') {
        head
    } else {
        head.rfind('-').map_or(head, |i| &head[..i])
    };
    cut.trim_end_matches('-')
}

/// `base` if free, else the smallest `base-N` (N >= 2) not in `taken`; `base` is
/// shortened when needed so the result stays within 200 chars.
pub fn next_free(base: &str, taken: &[String]) -> String {
    let taken: HashSet<&str> = taken.iter().map(String::as_str).collect();
    if !taken.contains(base) {
        return base.to_owned();
    }
    (2u64..)
        .map(|n| {
            let suffix = format!("-{n}");
            let max = MAX_LEN.saturating_sub(suffix.len());
            format!("{}{suffix}", truncate_base(base, max))
        })
        .find(|c| !taken.contains(c.as_str()))
        .unwrap_or_else(|| base.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every path prefix the public handler answers itself is reserved (P20: previews).
    #[test]
    fn handler_prefixes_are_reserved() {
        for w in [
            "_media", "_theme", "_preview", "_p", "api", "healthz", "readyz",
        ] {
            assert!(is_reserved(w), "{w}");
        }
    }

    #[test]
    fn slugify_table() {
        for (i, o) in [
            ("Hello, World!", "hello-world"),
            ("Zażółć gęślą jaźń", "zazolc-gesla-jazn"),
            ("   ", "untitled"),
            ("", "untitled"),
            ("--a__b--", "a-b"),
            ("Ünï Cödé 2", "uni-code-2"),
        ] {
            assert_eq!(slugify(i), o, "input {i:?}");
        }
    }

    #[test]
    fn slugify_long_cuts_at_dash() {
        let input = "word ".repeat(60); // 300 chars
        let s = slugify(&input);
        assert!(s.len() <= 200);
        assert!(!s.ends_with('-'));
        assert!(s.split('-').all(|w| w == "word"));
        let one = "a".repeat(300);
        assert_eq!(slugify(&one).len(), 200);
    }

    #[test]
    fn validate_rules() {
        assert_eq!(validate("  Hello-World ").unwrap(), "hello-world");
        assert_eq!(validate("Żółw").unwrap(), "żółw");
        for bad in [
            "a/b",
            "",
            "   ",
            "a b",
            "a\tb",
            "a\u{0}b",
            &"x".repeat(201),
            ".",
            "..",
            " .. ",
        ] {
            assert!(
                matches!(
                    validate(bad),
                    Err(CoreError::InvalidField { field: "slug", .. })
                ),
                "{bad:?}"
            );
        }
        assert!(validate(&"x".repeat(200)).is_ok());
        assert_eq!(validate("...").unwrap(), "...");
        assert_eq!(validate("a.b").unwrap(), "a.b");
    }

    #[test]
    fn validate_rejects_format_and_url_chars() {
        for bad in [
            "a\u{200B}b",
            "a\u{202E}b",
            "a\u{FEFF}",
            "a?b",
            "a#b",
            "a%b",
            "a<b",
            "a>b",
            "a\"b",
            "a'b",
            "a\\b",
            "a`b",
        ] {
            assert!(
                matches!(
                    validate(bad),
                    Err(CoreError::InvalidField { field: "slug", .. })
                ),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn next_free_bounds_length() {
        let taken = |v: Vec<String>| v;
        let base = "a".repeat(200);
        let r = next_free(&base, &taken(vec![base.clone()]));
        assert_eq!(r, format!("{}-2", "a".repeat(198)));
        let base = "a".repeat(199);
        let mut t = vec![base.clone()];
        t.extend((2..=9).map(|n| format!("{}-{n}", &base[..198])));
        let r = next_free(&base, &t);
        assert_eq!(r, format!("{}-10", "a".repeat(197)));
        assert_eq!(r.len(), 200);
        // dash boundary preferred, trailing dash trimmed
        let base = format!("{}-{}", "a".repeat(190), "b".repeat(9));
        let r = next_free(&base, std::slice::from_ref(&base));
        assert_eq!(r, format!("{}-2", "a".repeat(190)));
        let base = format!("{}-{}", "a".repeat(150), "b".repeat(49));
        let r = next_free(&base, std::slice::from_ref(&base));
        assert_eq!(r, format!("{}-2", "a".repeat(150)));
        // unicode: counts chars not bytes
        let base = "ż".repeat(200);
        let r = next_free(&base, std::slice::from_ref(&base));
        assert_eq!(r, format!("{}-2", "ż".repeat(198)));
        assert_eq!(r.chars().count(), 200);
    }

    #[test]
    fn next_free_picks_smallest() {
        let t = |v: &[&str]| v.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        assert_eq!(next_free("a", &t(&[])), "a");
        assert_eq!(next_free("a", &t(&["a"])), "a-2");
        assert_eq!(next_free("a", &t(&["a", "a-2", "a-4"])), "a-3");
        assert_eq!(next_free("a", &t(&["a-2"])), "a");
    }
}
