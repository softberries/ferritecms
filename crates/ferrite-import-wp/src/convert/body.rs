//! A WordPress post body to the stored block tree: the one entry point the content pass calls.

use serde_json::Value;

use super::doc::{self, Node};
use super::{Notes, Rewrite, autop, blocks, gutenberg, shortcodes};

/// Converts a post body (`content:encoded`) to a block-tree `doc` that the admin editor
/// opens (it passes `ferrite_core::blocks::is_editor_safe`).
///
/// A body with block comments goes through the Gutenberg mapping ([`super::blocks`]); any
/// other is classic: auto-embeds, `wpautop`, shortcodes, then HTML. What is lost or could
/// not be resolved is counted in `notes` (whose `item` the caller sets).
pub fn convert(content: &str, rw: &dyn Rewrite, notes: &mut Notes) -> Value {
    let nodes = if gutenberg::has_blocks(content) {
        blocks::convert(&gutenberg::parse(content), rw, notes)
    } else {
        classic(content, rw, notes)
    };
    doc::to_json(&nodes)
}

/// A classic (non-block) body, or the HTML of a freeform or shortcode block.
pub(crate) fn classic(text: &str, rw: &dyn Rewrite, notes: &mut Notes) -> Vec<Node> {
    let html = autop::wpautop(&shortcodes::autoembed(text));
    shortcodes::to_nodes(&html, rw, notes)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use serde_json::json;
    use uuid::Uuid;

    use super::*;
    use crate::convert::ImageRef;
    use crate::model::Record;

    /// Attachments 1–6 were imported as media `<id>`; uploads are left as URLs, so an image
    /// shows whether it was resolved by attachment id.
    struct Rw;

    impl Rewrite for Rw {
        fn image(&self, src: &str) -> ImageRef {
            if src.contains("not-in-library") {
                ImageRef::Missing(src.to_owned())
            } else {
                ImageRef::Url(src.to_owned())
            }
        }

        fn attachment(&self, wp_id: i64) -> Option<Uuid> {
            (1..=6).contains(&wp_id).then(|| media(wp_id))
        }

        fn href(&self, href: &str) -> String {
            href.replace("http://localhost:8099/about/", "/new-about/")
        }
    }

    fn media(id: i64) -> Uuid {
        Uuid::from_u128(u128::try_from(id).unwrap())
    }

    fn src(id: i64) -> String {
        format!("ferrite:media/{}", media(id))
    }

    fn body(file: &str, wp_id: i64) -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/corpus")
            .join(file);
        let bytes = std::fs::read(path).unwrap();
        let mut found = None;
        crate::wxr::parse(&bytes[..], |r| {
            if let Record::Item(i) = r
                && i.wp_id == wp_id
            {
                found = Some(i.content);
            }
            Ok(())
        })
        .unwrap();
        found.unwrap()
    }

    fn run(file: &str, wp_id: i64) -> (Vec<Value>, Notes) {
        let mut notes = Notes {
            item: Some(wp_id),
            ..Notes::default()
        };
        let doc = convert(&body(file, wp_id), &Rw, &mut notes);
        assert!(ferrite_core::blocks::is_editor_safe(&doc));
        (doc["content"].as_array().unwrap().clone(), notes)
    }

    fn kinds(nodes: &[Value]) -> Vec<String> {
        nodes
            .iter()
            .map(|n| match n["type"].as_str().unwrap() {
                "heading" => format!("heading{}", n["attrs"]["level"]),
                t => t.to_owned(),
            })
            .collect()
    }

    fn para(text: &str) -> Value {
        json!({"type": "paragraph", "content": [{"type": "text", "text": text}]})
    }

    fn link(text: &str, href: &str) -> Value {
        json!({"type": "text", "text": text, "marks": [{"type": "link", "attrs": {"href": href}}]})
    }

    #[test]
    fn site_a_every_core_block() {
        let (c, notes) = run("site-a.xml", 7);
        assert_eq!(
            kinds(&c),
            [
                "paragraph",
                "heading2",
                "heading3",
                "bulletList",
                "orderedList",
                "blockquote",
                "image",
                "image",
                "image",
                "codeBlock",
                "codeBlock",
                "table",
                "horizontalRule",
                "embed",
                "paragraph",
                "paragraph",
                "paragraph",
                "html",
                "paragraph",
                "html",
                "paragraph",
            ]
        );
        assert_eq!(c[3]["content"][1]["content"][1]["type"], "bulletList");
        assert_eq!(c[4]["attrs"]["start"], 3);
        assert_eq!(
            c[5]["content"],
            json!([para("A quoted paragraph."), para("Someone")])
        );
        assert_eq!(
            c[6]["attrs"],
            json!({"src": src(4), "alt": "A red hero banner", "caption": "The hero, medium size"})
        );
        assert_eq!(c[7]["attrs"], json!({"src": src(5), "alt": "Blue square"}));
        assert_eq!(c[8]["attrs"], json!({"src": src(6), "alt": ""}));
        assert_eq!(
            c[9],
            json!({"type": "codeBlock", "content": [{"type": "text", "text": "fn main() {\n    println!(\"<hello>\");\n}"}]})
        );
        assert_eq!(
            c[10],
            json!({"type": "codeBlock", "content": [{"type": "text", "text": "Preformatted   text\n  keeps spaces"}]})
        );
        assert_eq!(c[11]["content"].as_array().unwrap().len(), 3);
        assert_eq!(
            c[13]["attrs"],
            json!({"provider": "youtube", "url": "https://www.youtube.com/watch?v=dQw4w9WgXcQ"})
        );
        assert_eq!(
            c[14],
            json!({"type": "paragraph", "content": [link("Sign up", "https://example.org/signup")]})
        );
        assert_eq!(c[15], para("Left column."));
        assert_eq!(c[16], para("Right column."));
        assert!(
            c[17]["attrs"]["html"]
                .as_str()
                .unwrap()
                .starts_with("<div class=\"custom-widget\"")
        );
        assert_eq!(c[18], para("[contact-form-7 id=\"12\" title=\"Contact\"]"));
        assert_eq!(
            c[19]["attrs"]["html"],
            "<div class=\"wp-block-acme-fancy-callout\">A block from a plugin the importer does not know.</div>"
        );
        let links: Vec<&Value> = c[20]["content"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|t| t.get("marks").is_some())
            .collect();
        assert_eq!(links[0], &link("about page", "/new-about/"));
        assert_eq!(
            links[1],
            &link("an old ?p= link", "http://localhost:8099/?page_id=9")
        );
        assert_eq!(notes.unknown_blocks.len(), 1);
        assert_eq!(notes.unknown_blocks["acme/fancy-callout"].count, 1);
        assert_eq!(notes.unknown_blocks["acme/fancy-callout"].items, [7]);
        assert_eq!(notes.unknown_shortcodes["contact-form-7"].count, 1);
        assert_eq!(notes.bad_block_attrs.count, 0);
    }

    #[test]
    fn site_b_harbour_walk() {
        let (c, notes) = run("site-b.xml", 5);
        assert_eq!(
            kinds(&c),
            [
                "paragraph",
                "paragraph",
                "image",
                "heading2",
                "bulletList",
                "blockquote",
                "image",
                "image",
                "embed",
                "embed",
                "paragraph",
                "paragraph",
                "paragraph",
                "paragraph",
                "codeBlock",
                "table",
                "paragraph",
                "heading2",
                "image",
            ]
        );
        assert_eq!(
            c[2]["attrs"],
            json!({
                "src": "http://localhost:8099/wp-content/uploads/2026/10/harbour-300x200.jpg",
                "alt": "A harbour at dusk",
                "caption": "The harbour, captioned",
            })
        );
        assert_eq!(c[6]["attrs"], json!({"src": src(2)}));
        assert_eq!(c[7]["attrs"], json!({"src": src(3)}));
        assert_eq!(
            c[8]["attrs"],
            json!({"provider": "vimeo", "url": "https://vimeo.com/76979871"})
        );
        assert_eq!(
            c[9]["attrs"],
            json!({"provider": "youtube", "url": "https://www.youtube.com/watch?v=aqz-KE-bpKQ"})
        );
        let clip = "https://cdn.example.org/clip.mp4";
        assert_eq!(
            c[10],
            json!({"type": "paragraph", "content": [link(clip, clip)]})
        );
        assert_eq!(
            c[12],
            para("[unknown_plugin_shortcode foo=\"bar\"]Inner text[/unknown_plugin_shortcode]")
        );
        assert_eq!(c[13], para("Text after the more tag."));
        assert_eq!(notes.unknown_shortcodes.len(), 1);
        assert_eq!(
            notes.unknown_shortcodes["unknown_plugin_shortcode"].count,
            1
        );
        assert_eq!(
            notes.unknown_shortcodes["unknown_plugin_shortcode"].items,
            [5]
        );
        assert_eq!(notes.missing_media.count, 1);
        assert_eq!(notes.h1_demoted.count, 1);
    }

    #[test]
    fn site_b_contact_keeps_the_form_shortcode_as_text() {
        let (c, notes) = run("site-b.xml", 7);
        assert_eq!(c, [para("Write to us."), para("[contact-form-7 id=\"5\"]")]);
        assert_eq!(notes.unknown_shortcodes["contact-form-7"].count, 1);
    }

    #[test]
    fn empty_body_is_one_empty_paragraph() {
        let mut notes = Notes::default();
        assert_eq!(
            convert("", &Rw, &mut notes),
            json!({"type": "doc", "content": [{"type": "paragraph"}]})
        );
        assert_eq!(notes, Notes::default());
    }
}
