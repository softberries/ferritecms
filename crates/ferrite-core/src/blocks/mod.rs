//! TipTap/ProseMirror JSON → safe HTML, plain text and excerpts.
//!
//! Input is untrusted: every function copes with arbitrary JSON (wrong types, huge
//! nesting) without panicking or overflowing the stack.

mod anchor;
mod render;
mod text;
mod url;

pub use anchor::TocEntry;
pub use render::{render_html, render_html_toc};
pub use text::{excerpt, extract_text};
pub use url::{canonical_embed_url, embed_id, safe_href};

use serde_json::Value;

/// Whether `body` is a block tree's root: an object with `"type": "doc"`. Content
/// writes refuse any other body (M2b-2 Task 1); the functions here still cope with one.
pub fn is_document(body: &Value) -> bool {
    body.get("type").and_then(Value::as_str) == Some("doc")
}

/// The node types the admin's editor can hold (`admin/src/editor/schema.ts`
/// `SUPPORTED_NODES`; both pinned to `tests/fixtures/editor-schema.json`).
const EDITOR_NODES: &[&str] = &[
    "doc",
    "paragraph",
    "heading",
    "bulletList",
    "orderedList",
    "listItem",
    "blockquote",
    "codeBlock",
    "horizontalRule",
    "hardBreak",
    "image",
    "embed",
    "table",
    "tableRow",
    "tableHeader",
    "tableCell",
    "html",
    "text",
];

/// The marks the admin's editor can hold (`SUPPORTED_MARKS`).
const EDITOR_MARKS: &[&str] = &["bold", "italic", "strike", "code", "link"];

/// Body heading levels (`HEADING_LEVELS`); level 1 is the title (m2b P18).
const EDITOR_HEADING_LEVELS: std::ops::RangeInclusive<u64> = 2..=6;

/// Whether `doc` passes the admin editor's allowlist check: the Rust twin of its
/// `validateBody` (`admin/src/editor/schema.ts`). It checks that the root is a `doc`,
/// that every node type and mark is on the editor's lists, that every heading has a level
/// of 2–6, and that `content` and `marks` are lists. It does **not** check the
/// ProseMirror content model (which node may contain which, e.g. a `listItem` straight in
/// the `doc`), nor attribute values beyond the heading level: a body that passes can
/// still fail TipTap's content check, which opens it read-only. Unlike `validateBody`,
/// which treats a missing body (`null`, `{}`) as empty, anything but a `doc` root is
/// refused. The importer emits only bodies that pass. Iterative: any depth.
pub fn is_editor_safe(doc: &Value) -> bool {
    if !is_document(doc) {
        return false;
    }
    let mut stack = vec![doc];
    while let Some(v) = stack.pop() {
        let Some(node) = v.as_object() else {
            return false;
        };
        let ty = node.get("type").and_then(Value::as_str).unwrap_or("");
        if !EDITOR_NODES.contains(&ty) {
            return false;
        }
        if ty == "heading"
            && !node
                .get("attrs")
                .and_then(|a| a.get("level"))
                .and_then(Value::as_u64)
                .is_some_and(|l| EDITOR_HEADING_LEVELS.contains(&l))
        {
            return false;
        }
        if let Some(marks) = node.get("marks") {
            let Some(marks) = marks.as_array() else {
                return false;
            };
            let known = |m: &Value| {
                m.get("type")
                    .and_then(Value::as_str)
                    .is_some_and(|t| EDITOR_MARKS.contains(&t))
            };
            if !marks.iter().all(known) {
                return false;
            }
        }
        if let Some(content) = node.get("content") {
            let Some(content) = content.as_array() else {
                return false;
            };
            stack.extend(content);
        }
    }
    true
}

/// Nodes nested deeper than this render their text content only.
pub const MAX_DEPTH: usize = 64;

/// Context for rendering.
pub struct RenderCtx<'a> {
    /// Resolves a media id from `ferrite:media/<uuid>` to what its `<img>` shows;
    /// `None` drops the image.
    pub media: &'a dyn Fn(&str) -> Option<InlineImage>,
}

/// What an inline `ferrite:media/<id>` image renders as.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlineImage {
    pub src: String,
    /// `None`: a plain `<img src alt>` (SVGs, non-images, images whose variants are not
    /// recorded yet).
    pub responsive: Option<Responsive>,
}

impl InlineImage {
    /// A plain `<img src alt>`.
    pub fn plain(src: String) -> Self {
        Self {
            src,
            responsive: None,
        }
    }
}

/// The responsive attributes of an inline image (06 §4): rendered after `alt` (and
/// `title`) as `srcset`, `sizes`, `width`, `height`, `loading="lazy"` and
/// `decoding="async"`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Responsive {
    /// `url 768w, url 1600w, …`.
    pub srcset: String,
    pub sizes: String,
    pub width: i32,
    pub height: i32,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    #[test]
    fn only_a_doc_object_is_a_document() {
        assert!(super::is_document(&json!({"type": "doc"})));
        assert!(super::is_document(&json!({"type": "doc", "content": []})));
        for body in [
            json!({"type": "paragraph"}),
            json!({"content": []}),
            json!([]),
            json!(["doc"]),
            json!("doc"),
            json!(null),
            json!({"type": ["doc"]}),
        ] {
            assert!(!super::is_document(&body), "{body}");
        }
    }

    /// The admin's schema lists (`admin/src/editor/schema.ts`), which its own test pins
    /// to the same file, so the two cannot drift.
    fn admin_schema() -> serde_json::Value {
        serde_json::from_str(include_str!("../../tests/fixtures/editor-schema.json")).unwrap()
    }

    fn strings(v: &serde_json::Value) -> Vec<String> {
        v.as_array()
            .unwrap()
            .iter()
            .map(|s| s.as_str().unwrap().to_owned())
            .collect()
    }

    #[test]
    fn editor_lists_match_the_admin() {
        let schema = admin_schema();
        let sorted = |mut v: Vec<String>| {
            v.sort();
            v
        };
        let ours = |l: &[&str]| sorted(l.iter().map(|s| (*s).to_owned()).collect());
        assert_eq!(ours(super::EDITOR_NODES), sorted(strings(&schema["nodes"])));
        assert_eq!(ours(super::EDITOR_MARKS), sorted(strings(&schema["marks"])));
        let levels: Vec<u64> = schema["heading_levels"]
            .as_array()
            .unwrap()
            .iter()
            .map(|l| l.as_u64().unwrap())
            .collect();
        assert_eq!(super::EDITOR_HEADING_LEVELS.collect::<Vec<_>>(), levels);
        // Every provider the admin offers is one `embed_id` knows.
        let cases = schema["embed_cases"].as_array().unwrap();
        for provider in strings(&schema["embed_providers"]) {
            assert!(
                cases.iter().any(|c| c["provider"] == provider.as_str()
                    && super::embed_id(&provider, c["url"].as_str().unwrap()).is_some()),
                "{provider}"
            );
        }
    }

    #[test]
    fn every_admin_node_and_mark_is_editor_safe() {
        let schema = admin_schema();
        for ty in strings(&schema["nodes"]) {
            let node = match ty.as_str() {
                "doc" => continue,
                "heading" => json!({"type": "heading", "attrs": {"level": 2}}),
                _ => json!({"type": ty}),
            };
            let d = json!({"type": "doc", "content": [node]});
            assert!(super::is_editor_safe(&d), "{ty}");
        }
        for mark in strings(&schema["marks"]) {
            let d = json!({"type": "doc", "content": [{"type": "paragraph", "content": [
                {"type": "text", "text": "x", "marks": [{"type": mark}]}]}]});
            assert!(super::is_editor_safe(&d), "{mark}");
        }
        for level in 2..=6 {
            let d =
                json!({"type": "doc", "content": [{"type": "heading", "attrs": {"level": level}}]});
            assert!(super::is_editor_safe(&d), "h{level}");
        }
        // The admin's own every-node fixture (its round-trip test) passes too.
        let every: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../admin/src/editor/fixtures/every-node.json"
        ))
        .unwrap();
        assert!(super::is_editor_safe(&every));
        assert!(super::is_editor_safe(&json!({"type": "doc"})));
        assert!(super::is_editor_safe(
            &json!({"type": "doc", "content": []})
        ));
    }

    #[test]
    fn editor_unsafe_bodies() {
        let doc = |content: serde_json::Value| json!({"type": "doc", "content": content});
        let p = |marks: serde_json::Value| {
            doc(
                json!([{"type": "paragraph", "content": [{"type": "text", "text": "x", "marks": marks}]}]),
            )
        };
        for (why, body) in [
            (
                "h1",
                doc(json!([{"type": "heading", "attrs": {"level": 1}}])),
            ),
            (
                "h7",
                doc(json!([{"type": "heading", "attrs": {"level": 7}}])),
            ),
            (
                "level as text",
                doc(json!([{"type": "heading", "attrs": {"level": "2"}}])),
            ),
            ("no level", doc(json!([{"type": "heading"}]))),
            (
                "unknown node",
                doc(json!([{"type": "callout", "content": []}])),
            ),
            (
                "nested unknown",
                doc(json!([{"type": "blockquote", "content": [{"type": "columns"}]}])),
            ),
            ("no type", doc(json!([{"content": []}]))),
            ("not a node", doc(json!([5]))),
            ("list as node", doc(json!([[]]))),
            ("content not a list", doc(json!("x"))),
            ("underline", p(json!([{"type": "underline"}]))),
            ("mark without type", p(json!([{}]))),
            ("marks not a list", p(json!({"type": "bold"}))),
            ("paragraph root", json!({"type": "paragraph"})),
            ("null root", json!(null)),
            ("list root", json!([{"type": "doc"}])),
        ] {
            assert!(!super::is_editor_safe(&body), "{why}: {body}");
        }
    }

    #[test]
    fn is_editor_safe_copes_with_deep_nesting() {
        std::thread::Builder::new()
            .stack_size(256 * 1024)
            .spawn(|| {
                // Built by hand: `json!` serializes an embedded `Value` recursively.
                let wrap = |ty: &str, child: serde_json::Value| {
                    let mut m = serde_json::Map::new();
                    m.insert("type".into(), ty.into());
                    m.insert("content".into(), serde_json::Value::Array(vec![child]));
                    serde_json::Value::Object(m)
                };
                let mut v = json!({"type": "text", "text": "x"});
                for _ in 0..50_000 {
                    v = wrap("blockquote", v);
                }
                let d = wrap("doc", v);
                assert!(super::is_editor_safe(&d));
                std::mem::forget(d); // Value's Drop recurses
            })
            .unwrap()
            .join()
            .unwrap();
    }
}
