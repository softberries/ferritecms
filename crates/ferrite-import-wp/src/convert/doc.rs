//! The stored block tree (TipTap JSON) as Rust values: exactly the nodes and marks the
//! admin editor and the core renderer support (`ferrite-core/tests/fixtures/editor-schema.json`).

use serde_json::{Map, Value, json};

/// A block-tree node.
#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    Paragraph(Vec<Node>),
    /// `level` is 2–6 (body headings, m2b P18); [`to_json`] clamps anything else.
    Heading {
        level: u8,
        content: Vec<Node>,
    },
    BulletList(Vec<Node>),
    OrderedList {
        start: u64,
        content: Vec<Node>,
    },
    ListItem(Vec<Node>),
    Blockquote(Vec<Node>),
    CodeBlock {
        language: Option<String>,
        text: String,
    },
    HorizontalRule,
    HardBreak,
    Image {
        src: String,
        alt: Option<String>,
        title: Option<String>,
        caption: Option<String>,
    },
    Table(Vec<Node>),
    TableRow(Vec<Node>),
    TableHeader {
        colspan: u32,
        rowspan: u32,
        content: Vec<Node>,
    },
    TableCell {
        colspan: u32,
        rowspan: u32,
        content: Vec<Node>,
    },
    Html(String),
    Embed {
        provider: String,
        url: String,
    },
    Text {
        text: String,
        marks: Vec<Mark>,
    },
}

/// A text mark.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mark {
    Bold,
    Italic,
    Strike,
    Code,
    Link {
        href: String,
        target: Option<String>,
    },
}

/// The `doc` root holding `nodes`, as TipTap's `getJSON()` writes it: absent attributes are
/// left out, empty `content` is left out, an empty document holds one empty paragraph.
/// Every string has NUL stripped (the store refuses it); text that ends up empty is dropped.
/// Marks are put in the editor schema's order (link, bold, italic, strike) without
/// duplicates; `code` excludes every other mark in TipTap, so it is dropped next to one.
pub fn to_json(nodes: &[Node]) -> Value {
    let mut content = list(nodes);
    if content.is_empty() {
        content.push(json!({"type": "paragraph"}));
    }
    json!({"type": "doc", "content": content})
}

fn list(nodes: &[Node]) -> Vec<Value> {
    nodes.iter().filter_map(node).collect()
}

fn clean(s: &str) -> String {
    s.replace('\0', "")
}

fn node(n: &Node) -> Option<Value> {
    let (ty, attrs, content): (&str, Map<String, Value>, &[Node]) = match n {
        Node::Paragraph(c) => ("paragraph", Map::new(), c),
        Node::Heading { level, content } => {
            let level = (*level).clamp(2, 6);
            ("heading", obj([("level", json!(level))]), content)
        }
        Node::BulletList(c) => ("bulletList", Map::new(), c),
        Node::OrderedList { start, content } => {
            ("orderedList", obj([("start", json!(start))]), content)
        }
        Node::ListItem(c) => ("listItem", Map::new(), c),
        Node::Blockquote(c) => ("blockquote", Map::new(), c),
        Node::CodeBlock { language, text } => {
            let mut v = json!({"type": "codeBlock"});
            if let Some(l) = language {
                v["attrs"] = json!({"language": clean(l)});
            }
            let text = clean(text);
            if !text.is_empty() {
                v["content"] = json!([{"type": "text", "text": text}]);
            }
            return Some(v);
        }
        Node::HorizontalRule => ("horizontalRule", Map::new(), &[]),
        Node::HardBreak => ("hardBreak", Map::new(), &[]),
        Node::Image {
            src,
            alt,
            title,
            caption,
        } => {
            let mut a = obj([("src", json!(clean(src)))]);
            for (k, v) in [("alt", alt), ("title", title), ("caption", caption)] {
                if let Some(v) = v {
                    a.insert(k.to_owned(), json!(clean(v)));
                }
            }
            ("image", a, &[])
        }
        Node::Table(c) => ("table", Map::new(), c),
        Node::TableRow(c) => ("tableRow", Map::new(), c),
        Node::TableHeader {
            colspan,
            rowspan,
            content,
        } => ("tableHeader", spans(*colspan, *rowspan), content),
        Node::TableCell {
            colspan,
            rowspan,
            content,
        } => ("tableCell", spans(*colspan, *rowspan), content),
        Node::Html(h) => ("html", obj([("html", json!(clean(h)))]), &[]),
        Node::Embed { provider, url } => (
            "embed",
            obj([
                ("provider", json!(clean(provider))),
                ("url", json!(clean(url))),
            ]),
            &[],
        ),
        Node::Text { text, marks } => {
            let text = clean(text);
            if text.is_empty() {
                return None;
            }
            let mut v = json!({"type": "text", "text": text});
            let marks = mark_list(marks);
            if !marks.is_empty() {
                v["marks"] = Value::Array(marks);
            }
            return Some(v);
        }
    };
    let mut v = obj([("type", json!(ty))]);
    if !attrs.is_empty() {
        v.insert("attrs".to_owned(), Value::Object(attrs));
    }
    let content = list(content);
    if !content.is_empty() {
        v.insert("content".to_owned(), Value::Array(content));
    }
    Some(Value::Object(v))
}

fn obj<const N: usize>(pairs: [(&str, Value); N]) -> Map<String, Value> {
    pairs.into_iter().map(|(k, v)| (k.to_owned(), v)).collect()
}

fn spans(colspan: u32, rowspan: u32) -> Map<String, Value> {
    obj([("colspan", json!(colspan)), ("rowspan", json!(rowspan))])
}

/// The rank of a mark in the editor schema (TipTap: link has priority 1000, then
/// StarterKit's order: bold, code, italic, strike).
fn rank(m: &Mark) -> u8 {
    match m {
        Mark::Link { .. } => 0,
        Mark::Bold => 1,
        Mark::Code => 2,
        Mark::Italic => 3,
        Mark::Strike => 4,
    }
}

fn mark_list(marks: &[Mark]) -> Vec<Value> {
    let mut ms: Vec<&Mark> = marks.iter().collect();
    ms.sort_by_key(|m| rank(m));
    ms.dedup_by_key(|m| rank(m));
    if ms.len() > 1 {
        ms.retain(|m| **m != Mark::Code);
    }
    ms.into_iter()
        .map(|m| match m {
            Mark::Bold => json!({"type": "bold"}),
            Mark::Italic => json!({"type": "italic"}),
            Mark::Strike => json!({"type": "strike"}),
            Mark::Code => json!({"type": "code"}),
            Mark::Link { href, target } => {
                let mut attrs = obj([("href", json!(clean(href)))]);
                if let Some(t) = target {
                    attrs.insert("target".to_owned(), json!(clean(t)));
                }
                json!({"type": "link", "attrs": attrs})
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(t: &str, marks: Vec<Mark>) -> Node {
        Node::Text {
            text: t.to_owned(),
            marks,
        }
    }

    #[test]
    fn empty_doc_has_one_empty_paragraph() {
        assert_eq!(
            to_json(&[]),
            json!({"type": "doc", "content": [{"type": "paragraph"}]})
        );
    }

    #[test]
    fn every_node_serialises_like_tiptap() {
        let nodes = vec![
            Node::Heading {
                level: 3,
                content: vec![text("H", vec![])],
            },
            Node::Paragraph(vec![
                text("a", vec![Mark::Italic, Mark::Bold]),
                Node::HardBreak,
                text(
                    "l",
                    vec![
                        Mark::Bold,
                        Mark::Link {
                            href: "/x".into(),
                            target: Some("_blank".into()),
                        },
                    ],
                ),
                text("c", vec![Mark::Code]),
            ]),
            Node::BulletList(vec![Node::ListItem(vec![Node::Paragraph(vec![])])]),
            Node::OrderedList {
                start: 3,
                content: vec![],
            },
            Node::Blockquote(vec![Node::Paragraph(vec![text("q", vec![])])]),
            Node::CodeBlock {
                language: Some("rust".into()),
                text: "fn x() {}".into(),
            },
            Node::CodeBlock {
                language: None,
                text: String::new(),
            },
            Node::HorizontalRule,
            Node::Image {
                src: "ferrite:media/x".into(),
                alt: Some(String::new()),
                title: None,
                caption: Some("cap".into()),
            },
            Node::Table(vec![Node::TableRow(vec![
                Node::TableHeader {
                    colspan: 2,
                    rowspan: 1,
                    content: vec![Node::Paragraph(vec![])],
                },
                Node::TableCell {
                    colspan: 1,
                    rowspan: 1,
                    content: vec![Node::Paragraph(vec![])],
                },
            ])]),
            Node::Html("<div>x</div>".into()),
            Node::Embed {
                provider: "vimeo".into(),
                url: "https://vimeo.com/1".into(),
            },
        ];
        let p = json!({"type": "paragraph"});
        assert_eq!(
            to_json(&nodes),
            json!({"type": "doc", "content": [
                {"type": "heading", "attrs": {"level": 3}, "content": [{"type": "text", "text": "H"}]},
                {"type": "paragraph", "content": [
                    {"type": "text", "text": "a", "marks": [{"type": "bold"}, {"type": "italic"}]},
                    {"type": "hardBreak"},
                    {"type": "text", "text": "l", "marks": [
                        {"type": "link", "attrs": {"href": "/x", "target": "_blank"}},
                        {"type": "bold"}]},
                    {"type": "text", "text": "c", "marks": [{"type": "code"}]},
                ]},
                {"type": "bulletList", "content": [{"type": "listItem", "content": [p]}]},
                {"type": "orderedList", "attrs": {"start": 3}},
                {"type": "blockquote", "content": [{"type": "paragraph", "content": [{"type": "text", "text": "q"}]}]},
                {"type": "codeBlock", "attrs": {"language": "rust"}, "content": [{"type": "text", "text": "fn x() {}"}]},
                {"type": "codeBlock"},
                {"type": "horizontalRule"},
                {"type": "image", "attrs": {"src": "ferrite:media/x", "alt": "", "caption": "cap"}},
                {"type": "table", "content": [{"type": "tableRow", "content": [
                    {"type": "tableHeader", "attrs": {"colspan": 2, "rowspan": 1}, "content": [p]},
                    {"type": "tableCell", "attrs": {"colspan": 1, "rowspan": 1}, "content": [p]},
                ]}]},
                {"type": "html", "attrs": {"html": "<div>x</div>"}},
                {"type": "embed", "attrs": {"provider": "vimeo", "url": "https://vimeo.com/1"}},
            ]})
        );
    }

    #[test]
    fn heading_levels_are_clamped_to_two_to_six() {
        let h = |level| Node::Heading {
            level,
            content: vec![],
        };
        let v = to_json(&[h(1), h(7)]);
        assert_eq!(v["content"][0]["attrs"]["level"], 2);
        assert_eq!(v["content"][1]["attrs"]["level"], 6);
    }

    #[test]
    fn code_excludes_other_marks_and_duplicates_collapse() {
        let v = to_json(&[Node::Paragraph(vec![text(
            "x",
            vec![Mark::Code, Mark::Bold, Mark::Bold],
        )])]);
        assert_eq!(
            v["content"][0]["content"][0]["marks"],
            json!([{"type": "bold"}])
        );
    }

    #[test]
    fn nul_is_stripped_and_empty_text_dropped() {
        let v = to_json(&[
            Node::Paragraph(vec![text("a\0b", vec![]), text("\0", vec![])]),
            Node::Image {
                src: "/a\0.png".into(),
                alt: Some("\0alt".into()),
                title: Some("t\0".into()),
                caption: None,
            },
            Node::Html("<b>\0</b>".into()),
        ]);
        assert_eq!(
            v["content"][0],
            json!({"type": "paragraph", "content": [{"type": "text", "text": "ab"}]})
        );
        assert_eq!(
            v["content"][1]["attrs"],
            json!({"src": "/a.png", "alt": "alt", "title": "t"})
        );
        assert_eq!(v["content"][2]["attrs"]["html"], "<b></b>");
        assert!(!v.to_string().contains("\\u0000"));
    }

    #[test]
    fn output_is_editor_safe() {
        let v = to_json(&[Node::Heading {
            level: 1,
            content: vec![text("t", vec![])],
        }]);
        assert!(ferrite_core::blocks::is_editor_safe(&v));
    }
}
