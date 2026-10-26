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
    /// Maps a media id from `ferrite:media/<uuid>` to a URL; `None` drops the image.
    pub media_url: &'a dyn Fn(&str) -> Option<String>,
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
