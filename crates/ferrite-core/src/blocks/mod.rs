//! TipTap/ProseMirror JSON → safe HTML, plain text and excerpts.
//!
//! Input is untrusted: every function copes with arbitrary JSON (wrong types, huge
//! nesting) without panicking or overflowing the stack.

mod render;
mod text;
mod url;

pub use render::render_html;
pub use text::{excerpt, extract_text};

/// Whether `body` is a block tree's root: an object with `"type": "doc"`. Content
/// writes refuse any other body (M2b-2 Task 1); the functions here still cope with one.
pub fn is_document(body: &serde_json::Value) -> bool {
    body.get("type").and_then(serde_json::Value::as_str) == Some("doc")
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
}
