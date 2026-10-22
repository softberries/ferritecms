//! TipTap/ProseMirror JSON → safe HTML, plain text and excerpts.
//!
//! Input is untrusted: every function copes with arbitrary JSON (wrong types, huge
//! nesting) without panicking or overflowing the stack.

mod render;
mod text;
mod url;

pub use render::render_html;
pub use text::{excerpt, extract_text};

/// Nodes nested deeper than this render their text content only.
pub const MAX_DEPTH: usize = 64;

/// Context for rendering.
pub struct RenderCtx<'a> {
    /// Maps a media id from `ferrite:media/<uuid>` to a URL; `None` drops the image.
    pub media_url: &'a dyn Fn(&str) -> Option<String>,
}
