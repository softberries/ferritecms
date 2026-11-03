//! [`check_document`]: the item-scope rules that need no store and no probe.

use serde_json::{Map, Value};

use super::walk::{Place, Walker};
use super::{DocInput, Finding, Links, MediaRef};
use crate::fields::{FEATURED_IMAGE, FieldDef, FieldKind, MAX_ROWS};

/// Runs every item-scope rule that needs no store read and no path probe over the
/// title, the body and the rich-text fields (inside repeaters too) of `input`, in
/// document order. Returns the findings (at most 500 and `check-truncated`), the links
/// split into internal (local paths and URLs on `site_url`'s origin) and external, and
/// the media the item shows (inline `ferrite:media/<id>` images and the featured
/// image), for the facts step. Input is untrusted: wrong JSON types are skipped.
pub fn check_document(input: &DocInput, site_url: &str) -> (Vec<Finding>, Links, Vec<MediaRef>) {
    let mut w = Walker::new(input.locale, site_url);
    w.doc(input.body, &Place::Body, true);
    if let Some(obj) = input.fields.as_object() {
        walk_fields(&mut w, input.field_defs, obj, "");
        if let Some(id) = obj
            .get(FEATURED_IMAGE)
            .and_then(Value::as_str)
            .and_then(|s| s.parse().ok())
        {
            w.featured(id);
        }
    }
    (w.findings, w.links, w.media)
}

fn walk_fields(w: &mut Walker, defs: &[FieldDef], obj: &Map<String, Value>, prefix: &str) {
    for def in defs {
        if w.stopped() {
            return;
        }
        let Some(value) = obj.get(&def.name) else {
            continue;
        };
        let name = if prefix.is_empty() {
            def.name.clone()
        } else {
            format!("{prefix}.{}", def.name)
        };
        match def.kind {
            FieldKind::RichText if value.is_object() => {
                w.doc(value, &Place::Field(name), false);
            }
            FieldKind::Repeater => {
                if let Some(rows) = value.as_array() {
                    for (i, row) in rows.iter().enumerate().take(MAX_ROWS) {
                        if let Some(o) = row.as_object() {
                            walk_fields(w, &def.fields, o, &format!("{name}[{i}]"));
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::super::walk::{Target, alt_is_filename, classify};
    use super::super::{Location, MAX_ITEM_FINDINGS, MAX_ITEM_LINKS};
    use super::*;
    use serde_json::json;

    const SITE: &str = "https://example.com";

    fn run_in(body: Value, locale: &str) -> (Vec<Finding>, Links, Vec<MediaRef>) {
        run_ref(&body, locale)
    }

    fn run_ref(body: &Value, locale: &str) -> (Vec<Finding>, Links, Vec<MediaRef>) {
        check_document(
            &DocInput {
                title: "T",
                body,
                fields: &json!({}),
                field_defs: &[],
                locale,
            },
            SITE,
        )
    }

    fn rules(body: Value) -> Vec<&'static str> {
        run_in(body, "en").0.iter().map(|f| f.rule.id).collect()
    }

    fn doc(content: Value) -> Value {
        json!({"type": "doc", "content": content})
    }

    fn p(content: Value) -> Value {
        json!({"type": "paragraph", "content": content})
    }

    fn link(text: &str, href: &str) -> Value {
        json!({"type": "text", "text": text, "marks": [{"type": "link", "attrs": {"href": href}}]})
    }

    fn img(attrs: Value) -> Value {
        json!({"type": "image", "attrs": attrs})
    }

    fn h(level: u64, text: &str) -> Value {
        if text.is_empty() {
            json!({"type": "heading", "attrs": {"level": level}})
        } else {
            json!({"type": "heading", "attrs": {"level": level}, "content": [{"type": "text", "text": text}]})
        }
    }

    #[test]
    fn image_rules() {
        assert_eq!(
            rules(doc(json!([img(json!({"src": "/a.png"}))]))),
            ["image-alt-missing"]
        );
        assert_eq!(
            rules(doc(json!([img(json!({"src": "/a.png", "alt": ""}))]))),
            ["image-alt-empty"]
        );
        assert_eq!(
            rules(doc(json!([img(
                json!({"src": "/a.png", "alt": "\u{200B}\u{FEFF} "})
            )]))),
            ["image-alt-empty"],
            "zero-width only is blank"
        );
        assert_eq!(
            rules(doc(json!([img(
                json!({"src": "/a.png", "alt": "IMG_1234.JPG"})
            )]))),
            ["image-alt-filename"]
        );
        assert!(rules(doc(json!([img(json!({"src": "/a.png", "alt": "A lake"}))]))).is_empty());
        for decorative in [
            json!({"src": "/a.png", "decorative": true}),
            json!({"src": "/a.png", "alt": "", "decorative": true}),
            json!({"src": "/a.png", "alt": "photo", "decorative": true}),
        ] {
            assert!(
                rules(doc(json!([img(decorative.clone())]))).is_empty(),
                "{decorative}"
            );
        }
        // No src: nothing renders, nothing to check.
        assert!(rules(doc(json!([img(json!({"alt": ""}))]))).is_empty());
        // The key is the src.
        let (f, _, _) = run_in(doc(json!([img(json!({"src": "/a.png"}))])), "en");
        assert_eq!(f[0].key, "/a.png");
        assert_eq!(f[0].location, Location::Body { path: vec![0] });
    }

    #[test]
    fn filename_shapes() {
        for yes in [
            "a.jpg",
            "X.WEBP",
            "IMG_1234",
            "DSC01234",
            "PXL_20230101_1234",
            "image",
            "Photo",
        ] {
            assert!(alt_is_filename(yes), "{yes}");
        }
        for no in [
            "A jpg file of a lake",
            "Images of lakes",
            "img of a cat",
            "dsc",
        ] {
            assert!(!alt_is_filename(no), "{no}");
        }
    }

    #[test]
    fn media_refs() {
        let id = "0190f1c0-0000-7000-8000-000000000001";
        let body = doc(json!([img(
            json!({"src": format!("ferrite:media/{id}"), "alt": "x"})
        )]));
        let fields = json!({"featured_image": id});
        let (_, _, media) = check_document(
            &DocInput {
                title: "T",
                body: &body,
                fields: &fields,
                field_defs: &[],
                locale: "en",
            },
            SITE,
        );
        assert_eq!(media.len(), 2);
        assert!(!media[0].featured);
        assert!(media[1].featured);
        assert_eq!(media[1].location, Location::FeaturedImage);
    }

    #[test]
    fn heading_rules() {
        assert_eq!(rules(doc(json!([h(2, "")]))), ["heading-empty"]);
        assert_eq!(rules(doc(json!([h(2, "\u{200B}")]))), ["heading-empty"]);
        assert!(rules(doc(json!([h(2, "A"), h(3, "B"), h(2, "C")]))).is_empty());
        // The title is level 1: a body starting with h3 skips h2.
        assert_eq!(rules(doc(json!([h(3, "A")]))), ["heading-level-skipped"]);
        assert_eq!(
            rules(doc(json!([h(2, "A"), h(4, "B")]))),
            ["heading-level-skipped"]
        );
        let (f, _, _) = run_in(doc(json!([h(2, "A"), h(4, "B")])), "en");
        assert_eq!(f[0].key, "h4:B");
        assert_eq!(f[0].detail, json!({"level": 4, "previous": 2}));
    }

    #[test]
    fn link_text_rules() {
        assert_eq!(
            rules(doc(json!([p(json!([link(" ", "/x")]))]))),
            ["link-text-empty"]
        );
        assert_eq!(
            rules(doc(json!([p(json!([link("Read more…", "/x")]))]))),
            ["link-text-vague"]
        );
        assert_eq!(
            rules(doc(json!([p(json!([link(
                "https://a.example/x",
                "https://a.example/x"
            )]))]))),
            ["link-text-url"]
        );
        assert!(rules(doc(json!([p(json!([link("The lake guide", "/x")]))]))).is_empty());
        // Joined across marks: "read" + "more" with one href is "read more".
        let bold = json!({"type": "text", "text": "read ", "marks": [
            {"type": "bold"}, {"type": "link", "attrs": {"href": "/x"}}]});
        assert_eq!(
            rules(doc(json!([p(json!([bold, link("more", "/x")]))]))),
            ["link-text-vague"]
        );
        // Two different hrefs are two links.
        assert!(
            rules(doc(json!([p(json!([
                link("read ", "/x"),
                link("more lakes", "/y")
            ]))])))
            .is_empty()
        );
        let (f, _, _) = run_in(
            doc(json!([p(
                json!([{"type":"text","text":"a "}, link("here", "/x")])
            )])),
            "en",
        );
        assert_eq!(f[0].key, "/x|here");
        assert_eq!(f[0].location, Location::Body { path: vec![0, 1] });
    }

    #[test]
    fn vague_lists_by_locale() {
        let body = || doc(json!([p(json!([link("Weiterlesen", "/x")]))]));
        assert_eq!(run_in(body(), "de-AT").0.len(), 1);
        assert_eq!(run_in(body(), "ja").0.len(), 0);
        let here = || doc(json!([p(json!([link("here", "/x")]))]));
        assert_eq!(run_in(here(), "de-AT").0.len(), 1);
        assert_eq!(run_in(here(), "ja").0.len(), 1);
    }

    #[test]
    fn tables() {
        let row = |cells: Vec<(&str, &str)>| {
            json!({"type": "tableRow", "content": cells.into_iter().map(|(t, x)| {
                if x.is_empty() { json!({"type": t}) }
                else { json!({"type": t, "content": [p(json!([{"type":"text","text": x}]))]}) }
            }).collect::<Vec<_>>()})
        };
        let table = |rows: Vec<Value>| json!({"type": "table", "content": rows});
        // Header row.
        assert!(
            rules(doc(json!([table(vec![
                row(vec![("tableHeader", "A"), ("tableHeader", "B")]),
                row(vec![("tableCell", "1"), ("tableCell", "2")]),
            ])])))
            .is_empty()
        );
        // Header column.
        assert!(
            rules(doc(json!([table(vec![
                row(vec![("tableHeader", "A"), ("tableCell", "1")]),
                row(vec![("tableHeader", "B"), ("tableCell", "2")]),
            ])])))
            .is_empty()
        );
        assert_eq!(
            rules(doc(json!([table(vec![row(vec![("tableCell", "1")])])]))),
            ["table-header-missing"]
        );
        assert_eq!(
            rules(doc(json!([table(vec![row(vec![
                ("tableHeader", ""),
                ("tableHeader", "B")
            ])])]))),
            ["table-header-empty"]
        );
        let (f, _, _) = run_in(
            doc(json!([
                table(vec![row(vec![("tableCell", "1")])]),
                table(vec![row(vec![("tableCell", "1")])])
            ])),
            "en",
        );
        assert_eq!(f[1].key, "table:2");
    }

    #[test]
    fn html_and_anchors() {
        assert_eq!(
            rules(doc(
                json!([{"type": "html", "attrs": {"html": "<b>x</b>"}}])
            )),
            ["html-unchecked"]
        );
        assert!(
            rules(doc(json!([
                h(2, "Setup"),
                p(json!([link("Go to setup", "#setup")]))
            ])))
            .is_empty()
        );
        assert_eq!(
            rules(doc(json!([
                h(2, "Setup"),
                p(json!([link("Go to install", "#install")]))
            ]))),
            ["link-anchor-missing"]
        );
        // Ids follow `heading_ids`: the second "Setup" is `setup-1`.
        assert!(
            rules(doc(json!([
                h(2, "Setup"),
                h(2, "Setup"),
                p(json!([link("The second", "#setup-1")]))
            ])))
            .is_empty()
        );
        // Percent-encoded fragments match.
        assert!(
            rules(doc(json!([
                h(2, "Zażółć"),
                p(json!([link("Jump", "#za%C5%BC%C3%B3%C5%82%C4%87")]))
            ])))
            .is_empty()
        );
    }

    #[test]
    fn link_classification() {
        let o = "https://example.com";
        assert_eq!(
            classify("/a/b?x=1#f", o),
            Target::Internal("/a/b?x=1".into())
        );
        assert_eq!(
            classify("https://example.com/a?q#z", o),
            Target::Internal("/a?q".into())
        );
        assert_eq!(
            classify("https://other.example/a#z", o),
            Target::External("https://other.example/a".into())
        );
        assert_eq!(
            classify("//other.example/a", o),
            Target::External("https://other.example/a".into())
        );
        assert_eq!(classify("#x", o), Target::Anchor("x".into()));
        for other in [
            "mailto:a@b.c",
            "tel:1",
            "relative/x",
            "javascript:alert(1)",
            "ftp://a.b/",
        ] {
            assert_eq!(classify(other, o), Target::Other, "{other}");
        }
        let (_, links, _) = run_in(
            doc(json!([p(
                json!([link("A guide", "/guide/"), {"type":"text","text":" "}, link("Other site", "https://w3.org/x")])
            )])),
            "en",
        );
        assert_eq!(links.internal[0].target, "/guide/");
        assert_eq!(links.external[0].target, "https://w3.org/x");
    }

    #[test]
    fn rich_text_fields_inside_repeaters() {
        let defs: Vec<FieldDef> = serde_json::from_value(json!([
            {"name": "intro", "kind": "rich_text"},
            {"name": "chapters", "kind": "repeater", "fields": [
                {"name": "summary", "kind": "rich_text"}, {"name": "title", "kind": "text"}]}
        ]))
        .unwrap();
        let fields = json!({
            "intro": doc(json!([img(json!({"src": "/a.png"}))])),
            "chapters": [
                {"summary": "not a doc", "title": "x"},
                {"summary": doc(json!([p(json!([link("here", "/x")]))]))}
            ]
        });
        let body = doc(json!([]));
        let (f, _, _) = check_document(
            &DocInput {
                title: "T",
                body: &body,
                fields: &fields,
                field_defs: &defs,
                locale: "en",
            },
            SITE,
        );
        assert_eq!(f.len(), 2);
        assert_eq!(
            f[0].location,
            Location::Field {
                name: "intro".into(),
                path: vec![0]
            }
        );
        assert_eq!(
            f[1].location,
            Location::Field {
                name: "chapters[1].summary".into(),
                path: vec![0, 0]
            }
        );
        // Heading levels are checked in the body only.
        let fields = json!({"intro": doc(json!([h(4, "x")]))});
        let (f, _, _) = check_document(
            &DocInput {
                title: "T",
                body: &body,
                fields: &fields,
                field_defs: &defs,
                locale: "en",
            },
            SITE,
        );
        assert!(f.is_empty());
    }

    #[test]
    fn context_is_cut_on_a_char_boundary() {
        let alt = format!("{}.png", "ż".repeat(200));
        let (f, _, _) = run_in(
            doc(json!([img(json!({"src": "/a.png", "alt": alt}))])),
            "en",
        );
        assert_eq!(f[0].rule.id, "image-alt-filename");
        assert_eq!(f[0].context.as_ref().unwrap().chars().count(), 120);
    }

    // Hostile input.

    /// Builds a deep tree by hand: `json!` would recurse.
    fn deep(levels: usize) -> Value {
        let mut v = json!({"type": "text", "text": "x"});
        for _ in 0..levels {
            let mut m = Map::new();
            m.insert("type".into(), "blockquote".into());
            m.insert("content".into(), Value::Array(vec![v]));
            v = Value::Object(m);
        }
        let mut m = Map::new();
        m.insert("type".into(), "doc".into());
        m.insert("content".into(), Value::Array(vec![v]));
        Value::Object(m)
    }

    #[test]
    fn deep_nesting_is_truncated_not_overflowed() {
        std::thread::Builder::new()
            .stack_size(256 * 1024)
            .spawn(|| {
                let d = deep(100_000);
                let (f, _, _) = run_ref(&d, "en");
                assert_eq!(f.len(), 1);
                assert_eq!(f[0].rule.id, "check-truncated");
                assert_eq!(f[0].detail, json!({"reason": "depth"}));
                std::mem::forget(d); // Value's Drop recurses
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn wrong_types_are_skipped() {
        for body in [
            json!({"type": "doc", "content": "x"}),
            json!({"type": "doc", "content": [{"type": "image", "attrs": []}]}),
            json!({"type": "doc", "content": [{"type": "image", "attrs": {"src": 5, "alt": 5}}]}),
            json!({"type": "doc", "content": [{"type": "heading", "attrs": {"level": "2"}}]}),
            json!({"type": "doc", "content": [{"type": "paragraph", "content": [
                {"type": "text", "text": 5, "marks": [{"type": "link", "attrs": {"href": 5}}]},
                {"type": "text", "text": "a", "marks": "link"},
                {"type": "text", "text": "a", "marks": [{"type": "link", "attrs": []}]}]}]}),
            json!({"type": "doc", "content": [{"type": "table", "content": [5, {"type": "tableRow", "content": "x"}]}]}),
            json!([1, 2]),
            json!(null),
            json!(5),
        ] {
            let (f, l, m) = run_in(body.clone(), "en");
            assert!(f.iter().all(|f| f.rule.id != "check-truncated"), "{body}");
            assert!(l.internal.is_empty() && m.is_empty(), "{body}");
        }
        // A string `alt` with a numeric `src` is not an image with a source.
        assert!(rules(doc(json!([img(json!({"src": 5}))]))).is_empty());
    }

    #[test]
    fn findings_capped_at_500() {
        let images: Vec<Value> = (0..600)
            .map(|i| img(json!({"src": format!("/{i}.png")})))
            .collect();
        let (f, _, _) = run_in(doc(Value::Array(images)), "en");
        assert_eq!(f.len(), MAX_ITEM_FINDINGS + 1);
        assert_eq!(f.last().unwrap().rule.id, "check-truncated");
        assert_eq!(f.last().unwrap().detail, json!({"reason": "findings"}));
    }

    #[test]
    fn links_capped_at_1000() {
        let mut inline = Vec::new();
        for i in 0..1_200 {
            inline.push(link(&format!("Guide {i}"), &format!("/g/{i}")));
            inline.push(json!({"type": "text", "text": " "}));
        }
        let (f, l, _) = run_in(doc(json!([p(Value::Array(inline))])), "en");
        assert_eq!(l.internal.len(), MAX_ITEM_LINKS);
        assert_eq!(f.last().unwrap().detail, json!({"reason": "links"}));
    }

    #[test]
    fn million_text_nodes_bounded() {
        let nodes: Vec<Value> = (0..1_000_000)
            .map(|_| json!({"type": "text", "text": "a"}))
            .collect();
        let body = doc(json!([p(Value::Array(nodes))]));
        let start = std::time::Instant::now();
        let (f, _, _) = run_in(body, "en");
        assert!(f.is_empty());
        let limit = if cfg!(debug_assertions) { 10 } else { 2 };
        assert!(start.elapsed().as_secs() < limit, "{:?}", start.elapsed());
    }
}
