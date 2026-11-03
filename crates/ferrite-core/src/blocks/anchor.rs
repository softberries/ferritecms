//! Heading ids (M4-3b P54) and the table-of-contents entries built from them.

use super::MAX_DEPTH;
use super::text::heading_text;
use ferrite_domain::html::is_reserved_fragment_id;
use serde::Serialize;
use serde_json::Value;
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
/// unique within it and never reserved ([`is_reserved_fragment_id`]: a theme's fixed id, or
/// one it builds from a comment's id).
#[derive(Debug)]
pub(super) struct Anchors {
    used: HashSet<String>,
    /// The last suffix tried per base id.
    suffix: HashMap<String, u32>,
}

impl Anchors {
    pub(super) fn new() -> Self {
        Self {
            used: HashSet::new(),
            suffix: HashMap::new(),
        }
    }

    /// The unique id for a heading with this text: the base from [`base_id`], then
    /// `-1`, `-2`, … when it is already taken or reserved.
    pub(super) fn id(&mut self, text: &str) -> String {
        let base = base_id(text);
        if self.take(&base) {
            return base;
        }
        let mut n = self.suffix.get(&base).copied().unwrap_or(0);
        let id = loop {
            n += 1;
            let candidate = format!("{base}-{n}");
            if self.take(&candidate) {
                break candidate;
            }
        };
        self.suffix.insert(base, n);
        id
    }

    /// Marks `id` used unless it is reserved or already used.
    fn take(&mut self, id: &str) -> bool {
        !is_reserved_fragment_id(id) && self.used.insert(id.to_owned())
    }
}

/// The node types whose children the renderer renders as nodes (`render_object`); every
/// other type renders its text only, so headings under it get no id.
const CONTAINERS: &[&str] = &[
    "doc",
    "paragraph",
    "heading",
    "bulletList",
    "orderedList",
    "listItem",
    "blockquote",
    "table",
    "tableRow",
    "tableHeader",
    "tableCell",
];

/// The `id`s [`render_html_toc`](super::render_html_toc) gives the headings of `doc`, in
/// document order, without rendering (design 16 §4.3: `link-anchor-missing` compares
/// fragments against them). Iterative: any depth.
pub fn heading_ids(doc: &Value) -> Vec<String> {
    let mut anchors = Anchors::new();
    let mut ids = Vec::new();
    let mut stack = vec![(doc, 0usize)];
    while let Some((v, depth)) = stack.pop() {
        if depth > MAX_DEPTH {
            continue;
        }
        let children: &[Value] = match v {
            Value::Array(items) => items,
            Value::Object(map) => {
                let ty = map.get("type").and_then(Value::as_str).unwrap_or("");
                if !CONTAINERS.contains(&ty) {
                    continue;
                }
                if ty == "heading"
                    && map
                        .get("attrs")
                        .and_then(|a| a.get("level"))
                        .and_then(Value::as_u64)
                        .is_some_and(|l| (1..=6).contains(&l))
                {
                    ids.push(anchors.id(&heading_text(v)));
                }
                match map.get("content") {
                    Some(Value::Array(items)) => items,
                    _ => &[],
                }
            }
            _ => continue,
        };
        stack.extend(children.iter().rev().map(|c| (c, depth + 1)));
    }
    ids
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
    use ferrite_domain::html::RESERVED_FRAGMENT_IDS;

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

    /// Ids the themes build from a comment's id are never given either (final
    /// re-review): a suffix moves the heading off them.
    #[test]
    fn comment_anchor_ids_are_never_given() {
        let u = "0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b";
        let mut a = Anchors::new();
        assert_eq!(a.id(&format!("Comment {u}")), format!("comment-{u}-1"));
        assert_eq!(a.id(&format!("reply {u}")), format!("reply-{u}-1"));
        assert_eq!(
            a.id(&format!("Reply {u} email")),
            format!("reply-{u}-email-1")
        );
        assert_eq!(a.id(&format!("comment {u}")), format!("comment-{u}-2"));
    }

    /// The plugin sanitiser keeps a heading id only when `heading_id_allowed` accepts
    /// it, so every id the rule gives must pass it.
    #[test]
    fn every_given_id_survives_the_plugin_sanitiser() {
        let mut a = Anchors::new();
        let texts = [
            "Install the binary",
            "What's new?",
            "Zażółć GĘŚLĄ",
            "ΟΔΟΣ ΣΑΣ",
            "İstanbul",
            "ǅemal",
            "Привет мир",
            "日本語",
            "🎉",
            "",
            "a  b",
            "snake_case",
            "Content",
            "Comment pending",
            "CF Name",
            "Setup",
            "Setup",
            "\"><script>alert(1)</script>",
        ];
        for text in texts.into_iter().chain(RESERVED_FRAGMENT_IDS) {
            let id = a.id(text);
            assert!(
                ferrite_domain::html::heading_id_allowed(&id),
                "{text:?} gave {id:?}"
            );
        }
    }

    #[test]
    fn empty_heading_id_is_counted_like_any_other() {
        let mut a = Anchors::new();
        assert_eq!(a.id("🎉"), "section");
        assert_eq!(a.id("section"), "section-1");
        assert_eq!(a.id("!!!"), "section-2");
    }
}
