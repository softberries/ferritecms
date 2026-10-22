use super::MAX_DEPTH;
use serde_json::{Map, Value};

fn node_type(map: &Map<String, Value>) -> &str {
    map.get("type").and_then(Value::as_str).unwrap_or("")
}

fn children(map: &Map<String, Value>) -> &[Value] {
    match map.get("content") {
        Some(Value::Array(items)) => items,
        _ => &[],
    }
}

/// All text under `root`, concatenated, with no separators. Iterative: safe for any depth.
pub(super) fn collect_text(root: &Value, out: &mut String) {
    let mut stack = vec![root];
    while let Some(v) = stack.pop() {
        match v {
            Value::Array(items) => stack.extend(items.iter().rev()),
            Value::Object(map) => {
                if node_type(map) == "text" {
                    if let Some(t) = map.get("text").and_then(Value::as_str) {
                        out.push_str(t);
                    }
                } else {
                    stack.extend(children(map).iter().rev());
                }
            }
            _ => {}
        }
    }
}

fn push_break(out: &mut String) {
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
}

enum Item<'a> {
    Node(&'a Value, usize),
    Break,
}

/// Plain text of a document. Paragraphs, headings, list items, code blocks, table rows
/// and cells and hard breaks end a line. Nodes deeper than `MAX_DEPTH` contribute their
/// text only. Iterative.
pub fn extract_text(doc: &Value) -> String {
    let mut out = String::new();
    let mut stack = vec![Item::Node(doc, 0)];
    while let Some(item) = stack.pop() {
        let (v, depth) = match item {
            Item::Break => {
                push_break(&mut out);
                continue;
            }
            Item::Node(v, d) => (v, d),
        };
        if depth > MAX_DEPTH {
            collect_text(v, &mut out);
            continue;
        }
        match v {
            Value::Array(items) => {
                stack.extend(items.iter().rev().map(|c| Item::Node(c, depth + 1)));
            }
            Value::Object(map) => match node_type(map) {
                "text" => {
                    if let Some(t) = map.get("text").and_then(Value::as_str) {
                        out.push_str(t);
                    }
                }
                "hardBreak" | "horizontalRule" => push_break(&mut out),
                ty => {
                    if matches!(
                        ty,
                        "paragraph"
                            | "heading"
                            | "listItem"
                            | "codeBlock"
                            | "tableRow"
                            | "tableCell"
                            | "tableHeader"
                    ) {
                        stack.push(Item::Break);
                    }
                    stack.extend(children(map).iter().rev().map(|c| Item::Node(c, depth + 1)));
                }
            },
            _ => {}
        }
    }
    out.truncate(out.trim_end_matches('\n').len());
    out
}

/// First `max_chars` Unicode scalar values of the document text (whitespace collapsed),
/// cut at the last whitespace before the limit; `…` is appended only when cut.
pub fn excerpt(doc: &Value, max_chars: usize) -> String {
    let text = extract_text(doc);
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let chars: Vec<char> = normalized.chars().collect();
    if chars.len() <= max_chars {
        return normalized;
    }
    let head = &chars[..max_chars];
    let cut = if chars[max_chars].is_whitespace() {
        head
    } else {
        match head.iter().rposition(|c| c.is_whitespace()) {
            Some(i) => &head[..i],
            None => head,
        }
    };
    let mut s: String = cut.iter().collect();
    s.truncate(s.trim_end().len());
    s.push('…');
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn doc() -> Value {
        json!({"type":"doc","content":[
            {"type":"heading","attrs":{"level":1},"content":[{"type":"text","text":"Title"}]},
            {"type":"paragraph","content":[
                {"type":"text","text":"Hello "},
                {"type":"text","text":"world","marks":[{"type":"bold"}]},
                {"type":"hardBreak"},
                {"type":"text","text":"next"}]},
            {"type":"bulletList","content":[
                {"type":"listItem","content":[{"type":"paragraph","content":[{"type":"text","text":"one"}]}]},
                {"type":"listItem","content":[{"type":"paragraph","content":[{"type":"text","text":"two"}]}]}]}
        ]})
    }

    #[test]
    fn extracts_with_boundaries() {
        assert_eq!(extract_text(&doc()), "Title\nHello world\nnext\none\ntwo");
        assert_eq!(extract_text(&json!(null)), "");
        assert_eq!(extract_text(&json!({"type":"doc"})), "");
    }

    #[test]
    fn excerpt_cuts_at_word() {
        let d = doc();
        assert_eq!(excerpt(&d, 1000), "Title Hello world next one two");
        assert_eq!(excerpt(&d, 14), "Title Hello…");
        assert_eq!(excerpt(&d, 17), "Title Hello world…");
        assert_eq!(excerpt(&d, 5), "Title…");
        assert_eq!(excerpt(&d, 3), "Tit…");
    }

    #[test]
    fn excerpt_counts_scalars_not_bytes() {
        let d = json!({"type":"paragraph","content":[{"type":"text","text":"żółć gęś jaźń"}]});
        assert_eq!(excerpt(&d, 9), "żółć gęś…");
        assert_eq!(excerpt(&d, 13), "żółć gęś jaźń");
    }
}
