//! Markdown in and out (design 12 §5.1): `markdown_to_body` and `to_markdown` together.

#![allow(clippy::unwrap_used)]

use ferrite_core::blocks::to_markdown;
use ferrite_import_md::markdown_to_body;
use serde_json::{Value, json};

const ID: &str = "0190f1c0-0000-7000-8000-000000000001";

/// md → body → md, and the result is stable: converting it again changes nothing.
fn round_trip(md: &str) -> String {
    let once = to_markdown(&markdown_to_body(md).body);
    let twice = to_markdown(&markdown_to_body(&once).body);
    assert_eq!(once, twice, "second pass differs");
    once
}

#[test]
fn round_trip_common_blocks() {
    let md = format!(
        "## Heading\n\n\
         Text with **bold**, *italic*, ~~strike~~, `code` and a [link](https://example.com/a).\n\n\
         - one\n  - nested\n  - nested too\n- two\n\n\
         3. third\n4. fourth\n\n\
         > quoted\n>\n> > deeper\n\n\
         ```rust\nfn main() {{}}\n```\n\n\
         | A | B |\n| --- | --- |\n| 1 | 2 |\n\n\
         ---\n\n\
         ![A cat](ferrite:media/{ID} \"The cat\")\n"
    );
    assert_eq!(round_trip(&md), md);
}

#[test]
fn the_body_has_the_stored_shapes() {
    let b = markdown_to_body(&format!(
        "# Title\n\n![A cat](ferrite:media/{ID} \"The cat\")\n\n[x](https://e.com/)\n"
    ));
    assert_eq!(b.title.as_deref(), Some("Title"));
    assert_eq!(
        b.body["content"][0],
        json!({"type":"image","attrs":{"src":format!("ferrite:media/{ID}"),"alt":"A cat","caption":"The cat"}})
    );
    assert_eq!(
        b.body["content"][1]["content"][0]["marks"][0],
        json!({"type":"link","attrs":{"href":"https://e.com/"}})
    );
    assert!(b.notes.is_empty());
}

#[test]
fn escaped_text_stays_text() {
    let md = "1 \\* 2 \\[x\\](y) \\<b> \\`c\\` snake_case\n\n\\# not a heading\n\n\\- not a list\n\n\
              2\\. not a list\n\n\\> not a quote\n\nAT&T \\&amp; ok\n";
    assert_eq!(round_trip(md), md);
    let b = markdown_to_body(md);
    let types: Vec<_> = b.body["content"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["type"].as_str().unwrap())
        .collect();
    assert_eq!(types, ["paragraph"; 6]);
}

#[test]
fn unknown_block_becomes_ferrite_block_fence_and_round_trips() {
    let unknown = json!({"type":"callout","attrs":{"tone":"warn"},"content":[
        {"type":"paragraph","content":[{"type":"text","text":"```tick```"}]}
    ]});
    let embed =
        json!({"type":"embed","attrs":{"provider":"youtube","url":"https://youtu.be/dQw4w9WgXcQ"}});
    let body = json!({"type":"doc","content":[
        {"type":"paragraph","content":[{"type":"text","text":"before"}]},
        unknown, embed,
        {"type":"bulletList","content":[{"type":"listItem","content":[
            {"type":"paragraph","content":[{"type":"text","text":"in a list"}]},
            {"type":"callout"}
        ]}]},
    ]});
    let md = to_markdown(&body);
    assert!(md.contains("```ferrite-block\n"), "{md}");
    let back = markdown_to_body(&md);
    assert!(back.notes.is_empty(), "{:?}", back.notes);
    assert_eq!(back.body, body);
    assert_eq!(to_markdown(&back.body), md);
}

#[test]
fn a_fence_that_is_not_a_node_stays_code_with_a_note() {
    let b = markdown_to_body("```ferrite-block\nnot json\n```\n");
    assert_eq!(b.body["content"][0]["type"], "codeBlock");
    assert_eq!(b.notes.len(), 1, "{:?}", b.notes);
}

#[test]
fn media_reference_survives_both_ways() {
    let body = json!({"type":"doc","content":[
        {"type":"image","attrs":{"src":format!("ferrite:media/{ID}"),"alt":"a [b]","caption":"c \"d\""}},
    ]});
    let md = to_markdown(&body);
    assert_eq!(
        md,
        format!("![a \\[b\\]](ferrite:media/{ID} \"c \\\"d\\\"\")\n")
    );
    assert_eq!(markdown_to_body(&md).body, body);
}

#[test]
fn other_images_are_kept_or_dropped_with_a_note() {
    let b = markdown_to_body(
        "![a](https://cdn.example.com/a.png)\n\n![b](/uploads/b.png)\n\n![c](pics/c.png)\n\n\
         ![d](ferrite:media/not-a-uuid)\n",
    );
    let srcs: Vec<_> = b.body["content"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["attrs"]["src"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(srcs, ["https://cdn.example.com/a.png", "/uploads/b.png"]);
    assert_eq!(b.notes.len(), 1, "{:?}", b.notes);
    assert!(b.notes[0].contains("2 image"), "{:?}", b.notes);
}

#[test]
fn links_are_left_as_written() {
    let b = markdown_to_body("[a](other.md) [b](/x/y/) [c](https://e.com/?q=1&r=2)\n");
    let hrefs: Vec<_> = b.body["content"][0]["content"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|n| n["marks"][0]["attrs"]["href"].as_str())
        .collect();
    assert_eq!(hrefs, ["other.md", "/x/y/", "https://e.com/?q=1&r=2"]);
}

#[test]
fn marks_and_breaks_round_trip() {
    let md = "a **b** ***c*** `d` e\\\nnext line\n\n**bold** [**linked**](https://e.com/) end\n";
    assert_eq!(round_trip(md), md);
}

#[test]
fn table_cells_with_pipes_and_breaks() {
    let md = "| a \\| b | c<br>d |\n| --- | --- |\n| `x \\| y` | z |\n";
    let b = markdown_to_body(md);
    let cell = &b.body["content"][0]["content"][0]["content"][0];
    assert_eq!(cell["content"][0]["content"][0]["text"], "a | b");
    assert_eq!(round_trip(md), md);
}

#[test]
fn a_tool_written_body_comes_back_equal() {
    // What a client sends and what the editor shows should agree after one trip.
    let body: Value = json!({"type":"doc","content":[
        {"type":"heading","attrs":{"level":2},"content":[{"type":"text","text":"Plan"}]},
        {"type":"orderedList","attrs":{"start":1},"content":[
            {"type":"listItem","content":[{"type":"paragraph","content":[{"type":"text","text":"first"}]}]},
            {"type":"listItem","content":[{"type":"paragraph","content":[
                {"type":"text","text":"second "},
                {"type":"text","text":"bold","marks":[{"type":"bold"}]}
            ]}]},
        ]},
        {"type":"codeBlock","attrs":{"language":"toml"},"content":[{"type":"text","text":"a = 1\n"}]},
    ]});
    let md = to_markdown(&body);
    assert_eq!(markdown_to_body(&md).body, body);
}

#[test]
fn a_fence_holding_a_doc_or_a_non_object_stays_a_code_block() {
    for text in [
        r#"{"type":"doc","content":[]}"#,
        r#"[{"type":"paragraph"}]"#,
        r#""paragraph""#,
        "42",
        r#"{"type":3}"#,
        r#"{"type":""}"#,
        r#"{"content":[]}"#,
    ] {
        let b = markdown_to_body(&format!("```ferrite-block\n{text}\n```\n"));
        assert_eq!(b.body["content"].as_array().unwrap().len(), 1, "{text}");
        let block = &b.body["content"][0];
        assert_eq!(block["type"], "codeBlock", "{text}");
        assert_eq!(block["attrs"]["language"], "ferrite-block", "{text}");
        assert_eq!(block["content"][0]["text"], format!("{text}\n"), "{text}");
        assert_eq!(b.notes.len(), 1, "{text}: {:?}", b.notes);
    }
}

/// The deepest `content` nesting in a tree, counted in nodes (iterative).
fn depth_of(root: &Value) -> usize {
    let mut deepest = 0;
    let mut stack = vec![(root, 1)];
    while let Some((node, depth)) = stack.pop() {
        deepest = deepest.max(depth);
        for child in node["content"].as_array().into_iter().flatten() {
            stack.push((child, depth + 1));
        }
    }
    deepest
}

fn text_of(root: &Value) -> String {
    let mut out = String::new();
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        if let Some(t) = node["text"].as_str() {
            out.push_str(t);
        }
        stack.extend(node["content"].as_array().into_iter().flatten().rev());
    }
    out
}

#[test]
fn pathological_nesting_converts_in_bounded_time_and_depth() {
    let quotes = format!("{} deep\n", ">".repeat(500));
    let mut lists = String::new();
    for i in 0..500 {
        lists.push_str(&format!("{}- level {i}\n", "  ".repeat(i)));
    }
    let brackets = format!("{} end\n", "[".repeat(100_000));
    // The converter caps nesting at 48 blocks; deeper input is flattened to text.
    for (name, md, word) in [
        ("quotes", &quotes, "deep"),
        ("lists", &lists, "level 499"),
        ("brackets", &brackets, "end"),
    ] {
        let start = std::time::Instant::now();
        let b = markdown_to_body(md);
        let out = to_markdown(&b.body);
        assert!(
            start.elapsed() < std::time::Duration::from_secs(10),
            "{name}: {:?}",
            start.elapsed()
        );
        let depth = depth_of(&b.body);
        assert!(depth <= 60, "{name}: depth {depth}");
        // The words survive, flattened to plain text beyond the cap.
        assert!(text_of(&b.body).contains(word), "{name}");
        assert!(out.contains(word), "{name}");
        // And the Markdown written back converts to a body with the same text.
        assert_eq!(
            text_of(&markdown_to_body(&out).body).replace(' ', ""),
            text_of(&b.body).replace(' ', ""),
            "{name}"
        );
    }
}

/// A decorative image (design 16 §4.1) goes out as a `ferrite-block` fence and comes
/// back with its flag; `![](…)` in is not decorative.
#[test]
fn decorative_image_round_trips_through_markdown() {
    let image = json!({"type":"image","attrs":{
        "src": format!("ferrite:media/{ID}"), "alt": "", "decorative": true}});
    let body = json!({"type":"doc","content":[image]});
    let md = to_markdown(&body);
    assert!(md.starts_with("```ferrite-block\n"), "{md}");
    let back = markdown_to_body(&md);
    assert!(back.notes.is_empty(), "{:?}", back.notes);
    assert_eq!(back.body, body);
    let plain = markdown_to_body(&format!("![](ferrite:media/{ID})\n"));
    let attrs = &plain.body["content"][0]["attrs"];
    assert!(attrs.get("decorative").is_none(), "{attrs}");
}
