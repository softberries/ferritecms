//! Markdown text to a block-tree body (design 12 §5.1): the way in for the MCP tools'
//! Markdown bodies. [`ferrite_core::blocks::to_markdown`] is the way out.

use ferrite_core::blocks::BLOCK_FENCE;
use ferrite_import_wp::convert::{ImageRef, Notes, Rewrite};
use serde_json::{Map, Value};
use uuid::Uuid;

use crate::convert::convert;

/// How deep [`restore`] looks for fences; the converter caps nesting well below this.
const MAX_DEPTH: usize = 64;

/// Markdown converted to the block tree.
#[derive(Debug, Clone, PartialEq)]
pub struct MarkdownBody {
    /// The `doc` block tree.
    pub body: Value,
    /// The text of the first `# H1`, which is left out of `body`.
    pub title: Option<String>,
    /// What was dropped or changed, one sentence each.
    pub notes: Vec<String>,
}

/// Converts `markdown` to a body. `![alt](ferrite:media/<id> "caption")` becomes the
/// stored media image; `http(s)` and rooted images and all links stay as written; any
/// other image is dropped with a note. A `ferrite-block` fence holding a JSON node
/// becomes that node. Raw HTML is cleaned as in a WordPress import.
pub fn markdown_to_body(markdown: &str) -> MarkdownBody {
    let mut notes = Notes::default();
    let converted = convert(markdown, &Known, &mut notes);
    let mut body = converted.body;
    let mut messages = describe(&notes);
    restore(&mut body, &mut messages, 0);
    MarkdownBody {
        body,
        title: converted.title,
        notes: messages,
    }
}

/// Resolves the sources the Markdown names itself: a `ferrite:media/<id>` image is a known
/// media item, everything else is left for the converter's own checks.
struct Known;

impl Rewrite for Known {
    fn image(&self, src: &str) -> ImageRef {
        match src.strip_prefix("ferrite:media/").map(Uuid::parse_str) {
            Some(Ok(id)) => ImageRef::Media(id),
            _ => ImageRef::Url(src.to_owned()),
        }
    }

    fn attachment(&self, _: i64) -> Option<Uuid> {
        None
    }

    fn href(&self, href: &str) -> String {
        href.to_owned()
    }
}

fn describe(notes: &Notes) -> Vec<String> {
    let mut out = Vec::new();
    for (mark, t) in &notes.dropped_marks {
        out.push(format!("dropped {mark} formatting ({} time(s))", t.count));
    }
    for (tag, t) in &notes.unsupported_tags {
        out.push(if tag == "img" {
            format!(
                "dropped {} image(s): use ferrite:media/<id> or an http(s) URL",
                t.count
            )
        } else {
            format!("dropped {} <{tag}> element(s) that have no block", t.count)
        });
    }
    if notes.too_deep.count > 0 {
        out.push(format!(
            "flattened {} block(s) nested too deeply",
            notes.too_deep.count
        ));
    }
    out
}

/// In the converted tree: a `ferrite-block` code block becomes the node its JSON holds,
/// and an image's title (the Markdown way to give a caption) becomes its caption.
fn restore(node: &mut Value, notes: &mut Vec<String>, depth: usize) {
    if depth > MAX_DEPTH {
        return;
    }
    let Some(map) = node.as_object_mut() else {
        return;
    };
    if map.get("type").and_then(Value::as_str) == Some("image") {
        caption_from_title(map);
        return;
    }
    let Some(Value::Array(children)) = map.get_mut("content") else {
        return;
    };
    for child in children {
        if let Some(original) = fenced_node(child, notes) {
            *child = original;
        }
        restore(child, notes, depth + 1);
    }
}

fn caption_from_title(map: &mut Map<String, Value>) {
    let Some(Value::Object(attrs)) = map.get_mut("attrs") else {
        return;
    };
    if attrs.contains_key("caption") {
        return;
    }
    if let Some(title) = attrs.remove("title") {
        attrs.insert("caption".to_owned(), title);
    }
}

/// The node a `ferrite-block` code block holds, `None` for any other node. A fence whose
/// text is not a JSON node stays a code block, with a note.
fn fenced_node(node: &Value, notes: &mut Vec<String>) -> Option<Value> {
    let map = node.as_object()?;
    if map.get("type").and_then(Value::as_str) != Some("codeBlock")
        || map
            .get("attrs")
            .and_then(|a| a.get("language"))
            .and_then(Value::as_str)
            != Some(BLOCK_FENCE)
    {
        return None;
    }
    let text: String = map
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|t| t.get("text").and_then(Value::as_str))
        .collect();
    match serde_json::from_str::<Value>(&text) {
        Ok(v)
            if v.get("type")
                .and_then(Value::as_str)
                .is_some_and(|t| !t.is_empty() && t != "doc") =>
        {
            Some(v)
        }
        _ => {
            notes.push(format!(
                "a {BLOCK_FENCE} fence did not hold a JSON block node; kept as a code block"
            ));
            None
        }
    }
}
