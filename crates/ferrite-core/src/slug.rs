//! Slug generation and validation.

use crate::CoreError;
use std::collections::HashSet;

const MAX_LEN: usize = 200;

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
pub fn validate(input: &str) -> Result<String, CoreError> {
    let s = input.trim().to_lowercase();
    if s.is_empty() {
        return Err(CoreError::Invalid("slug must not be empty".into()));
    }
    if s.chars().count() > MAX_LEN {
        return Err(CoreError::Invalid(format!(
            "slug longer than {MAX_LEN} characters"
        )));
    }
    if s.chars()
        .any(|c| c == '/' || c.is_whitespace() || c.is_control())
    {
        return Err(CoreError::Invalid(
            "slug must not contain '/', whitespace or control characters".into(),
        ));
    }
    Ok(s)
}

/// `base` if free, else the smallest `base-N` (N >= 2) not in `taken`.
pub fn next_free(base: &str, taken: &[String]) -> String {
    let taken: HashSet<&str> = taken.iter().map(String::as_str).collect();
    if !taken.contains(base) {
        return base.to_owned();
    }
    (2u64..)
        .map(|n| format!("{base}-{n}"))
        .find(|c| !taken.contains(c.as_str()))
        .unwrap_or_else(|| base.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

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
        for bad in ["a/b", "", "   ", "a b", "a\tb", "a\u{0}b", &"x".repeat(201)] {
            assert!(
                matches!(validate(bad), Err(CoreError::Invalid(_))),
                "{bad:?}"
            );
        }
        assert!(validate(&"x".repeat(200)).is_ok());
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
