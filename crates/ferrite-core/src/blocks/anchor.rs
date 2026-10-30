//! Heading ids (M4-3b P54) and the table-of-contents entries built from them.

use ferrite_domain::html::RESERVED_FRAGMENT_IDS;
use serde::Serialize;
use std::collections::{HashMap, HashSet};

/// The id given when a heading's text leaves nothing after the rule.
const EMPTY_ID: &str = "section";

/// One heading of a rendered body, in document order (`post.toc` in templates).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TocEntry {
    /// The heading level, 1 to 6.
    pub level: u8,
    /// The `id` attribute the heading was rendered with.
    pub id: String,
    /// The heading's plain text (marks ignored).
    pub text: String,
}

/// GitHub's heading-id rule with its duplicate counter: one per document, so ids are
/// unique within it and never one of [`RESERVED_FRAGMENT_IDS`].
#[derive(Debug)]
pub(super) struct Anchors {
    used: HashSet<String>,
    /// The last suffix tried per base id.
    suffix: HashMap<String, u32>,
}

impl Anchors {
    pub(super) fn new() -> Self {
        Self {
            used: RESERVED_FRAGMENT_IDS
                .iter()
                .map(|s| (*s).to_owned())
                .collect(),
            suffix: HashMap::new(),
        }
    }

    /// The unique id for a heading with this text: the base from [`base_id`], then
    /// `-1`, `-2`, … when it is already taken.
    pub(super) fn id(&mut self, text: &str) -> String {
        let base = base_id(text);
        if self.used.insert(base.clone()) {
            return base;
        }
        let n = self.suffix.entry(base.clone()).or_insert(0);
        loop {
            *n += 1;
            let candidate = format!("{base}-{n}");
            if self.used.insert(candidate.clone()) {
                return candidate;
            }
        }
    }
}

/// Lowercase, keep alphanumerics, `-`, `_` and spaces, turn each space into `-` (runs
/// are not collapsed); `section` when nothing is left.
fn base_id(text: &str) -> String {
    let id: String = text
        .to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | ' '))
        .map(|c| if c == ' ' { '-' } else { c })
        .collect();
    if id.is_empty() {
        EMPTY_ID.to_owned()
    } else {
        id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(text: &str) -> String {
        Anchors::new().id(text)
    }

    #[test]
    fn github_rule() {
        for (text, id) in [
            ("Install the binary", "install-the-binary"),
            ("What's new?", "whats-new"),
            ("Zażółć gęślą", "zażółć-gęślą"),
            ("Привет мир", "привет-мир"),
            ("日本語", "日本語"),
            ("🎉", "section"),
            ("", "section"),
            ("a  b", "a--b"),
            ("snake_case", "snake_case"),
            ("v1.2 (beta)", "v12-beta"),
            ("A-B", "a-b"),
        ] {
            assert_eq!(one(text), id, "{text:?}");
        }
    }

    #[test]
    fn only_safe_characters_remain() {
        let id = one("\"><script>alert(1)</script> & 'x'\t\n");
        assert!(
            id.chars()
                .all(|c| c.is_alphanumeric() || matches!(c, '-' | '_')),
            "{id}"
        );
    }

    #[test]
    fn duplicates_count_up() {
        let mut a = Anchors::new();
        assert_eq!(a.id("Setup"), "setup");
        assert_eq!(a.id("Setup"), "setup-1");
        assert_eq!(a.id("Setup"), "setup-2");
        // A heading whose own base is a generated id still gets a unique one.
        assert_eq!(a.id("Setup 1"), "setup-1-1");
        assert_eq!(a.id("setup-2"), "setup-2-1");
    }

    #[test]
    fn reserved_ids_are_never_given() {
        for reserved in RESERVED_FRAGMENT_IDS {
            let mut a = Anchors::new();
            assert_eq!(a.id(reserved), format!("{reserved}-1"));
            assert_eq!(a.id(&reserved.to_uppercase()), format!("{reserved}-2"));
        }
        assert_eq!(one("Content"), "content-1");
    }

    #[test]
    fn empty_heading_id_is_counted_like_any_other() {
        let mut a = Anchors::new();
        assert_eq!(a.id("🎉"), "section");
        assert_eq!(a.id("section"), "section-1");
        assert_eq!(a.id("!!!"), "section-2");
    }
}
