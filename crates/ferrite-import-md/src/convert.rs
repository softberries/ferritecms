//! Markdown to the stored block tree (P62).
//!
//! The Markdown is parsed by pulldown-cmark (CommonMark plus GFM tables and
//! strikethrough). The first `# H1` is taken out of the event stream as the title; when
//! another h1 is left, every heading moves one level down (capped at 6). The events are
//! rendered to HTML, which the WordPress importer's HTML converter turns into blocks, so
//! raw HTML is cleaned the same way as in a WordPress import.

use std::cell::RefCell;

use ferrite_import_wp::convert::{ImageRef, Notes, Rewrite, doc, html};
use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use serde_json::Value;
use uuid::Uuid;

/// A Markdown file converted to the block tree.
#[derive(Debug, Clone, PartialEq)]
pub struct Converted {
    /// The text of the first `# H1`, which is left out of `body`.
    pub title: Option<String>,
    /// The `doc` block tree.
    pub body: Value,
    /// Relative image sources (as the HTML holds them), in order, without duplicates: the
    /// caller resolves them inside the import's folder.
    pub images: Vec<String>,
    /// Relative links to a `.md` file or to a folder (ending in `/`), and any other
    /// relative link the [`Rewrite`] changed (a folder named without its `/`), as written
    /// with any `#fragment`, in order, without duplicates.
    pub links: Vec<String>,
}

/// Converts `markdown` (a BOM is dropped, CRLF is accepted) to blocks. Images and links go
/// through `rw` as in the WordPress import; what is lost is counted in `notes`.
pub fn convert(markdown: &str, rw: &dyn Rewrite, notes: &mut Notes) -> Converted {
    let markdown = markdown.strip_prefix('\u{feff}').unwrap_or(markdown);
    let options = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH;
    let mut events: Vec<Event<'_>> = Parser::new_ext(markdown, options).collect();
    let title = take_first_h1(&mut events);
    if events.iter().any(|e| {
        matches!(
            e,
            Event::Start(Tag::Heading {
                level: HeadingLevel::H1,
                ..
            })
        )
    }) {
        shift_headings(&mut events);
    }
    for e in &mut events {
        if let Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(info))) = e {
            *info = first_word(info).to_ascii_lowercase().into();
        }
    }
    let mut out = String::new();
    pulldown_cmark::html::push_html(&mut out, events.into_iter());

    let rec = Recording {
        inner: rw,
        images: RefCell::new(Vec::new()),
        links: RefCell::new(Vec::new()),
    };
    let mut nodes = html::convert(&out, &rec, notes);
    if doc::cap_depth(&mut nodes) > 0 {
        notes.too_deep();
    }
    Converted {
        title,
        body: doc::to_json(&nodes),
        images: rec.images.into_inner(),
        links: rec.links.into_inner(),
    }
}

/// Removes the first h1 and returns its text (`None` when there is none or it is empty).
fn take_first_h1(events: &mut Vec<Event<'_>>) -> Option<String> {
    let start = events.iter().position(|e| {
        matches!(
            e,
            Event::Start(Tag::Heading {
                level: HeadingLevel::H1,
                ..
            })
        )
    })?;
    let len = events[start..]
        .iter()
        .position(|e| matches!(e, Event::End(TagEnd::Heading(HeadingLevel::H1))))?;
    let mut title = String::new();
    for e in events.drain(start..=start + len) {
        match e {
            Event::Text(t) | Event::Code(t) => title.push_str(&t),
            Event::SoftBreak | Event::HardBreak => title.push(' '),
            _ => {}
        }
    }
    let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
    (!title.is_empty()).then_some(title)
}

/// Every heading one level down, capped at h6.
fn shift_headings(events: &mut [Event<'_>]) {
    fn down(level: HeadingLevel) -> HeadingLevel {
        match level {
            HeadingLevel::H1 => HeadingLevel::H2,
            HeadingLevel::H2 => HeadingLevel::H3,
            HeadingLevel::H3 => HeadingLevel::H4,
            HeadingLevel::H4 => HeadingLevel::H5,
            HeadingLevel::H5 | HeadingLevel::H6 => HeadingLevel::H6,
        }
    }
    for e in events {
        match e {
            Event::Start(Tag::Heading { level, .. }) => *level = down(*level),
            Event::End(TagEnd::Heading(level)) => *level = down(*level),
            _ => {}
        }
    }
}

/// The language of a fence's info string: its first word (`rust,ignore` is `rust`).
fn first_word(info: &str) -> &str {
    info.split(|c: char| c.is_whitespace() || matches!(c, ',' | '{'))
        .next()
        .unwrap_or("")
}

/// Whether `target` is a relative URL: no scheme, not rooted, not only a fragment or query.
fn is_relative(target: &str) -> bool {
    let Some(first) = target.chars().next() else {
        return false;
    };
    if matches!(first, '/' | '\\' | '#' | '?') {
        return false;
    }
    let head = target.split(['/', '?', '#']).next().unwrap_or("");
    !head.contains(':')
}

/// Whether a relative link names a `.md` file or a folder (`…/`).
fn is_page_link(href: &str) -> bool {
    let path = href.split(['?', '#']).next().unwrap_or("");
    path.ends_with('/') || path.to_ascii_lowercase().ends_with(".md")
}

fn push_new(list: &RefCell<Vec<String>>, s: &str) {
    let mut list = list.borrow_mut();
    if !list.iter().any(|x| x == s) {
        list.push(s.to_owned());
    }
}

/// The caller's [`Rewrite`], noting the relative images and page links it is asked about.
struct Recording<'a> {
    inner: &'a dyn Rewrite,
    images: RefCell<Vec<String>>,
    links: RefCell<Vec<String>>,
}

impl Rewrite for Recording<'_> {
    fn image(&self, src: &str) -> ImageRef {
        if is_relative(src) {
            push_new(&self.images, src);
        }
        self.inner.image(src)
    }

    fn attachment(&self, wp_id: i64) -> Option<Uuid> {
        self.inner.attachment(wp_id)
    }

    fn href(&self, href: &str) -> String {
        let out = self.inner.href(href);
        // A page link by its form, or any relative link the caller rewrote (a folder
        // named without its trailing `/`).
        if is_relative(href) && (is_page_link(href) || out != href) {
            push_new(&self.links, href);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use std::cell::RefCell;

    use ferrite_import_wp::convert::ImageRef;
    use serde_json::json;
    use uuid::Uuid;

    use super::*;

    const MEDIA: Uuid = Uuid::from_u128(0x0190_f1c0_0000_7000_8000_0000_0000_0001);

    #[derive(Default)]
    struct Rw {
        images: RefCell<Vec<String>>,
        hrefs: RefCell<Vec<String>>,
    }

    impl Rewrite for Rw {
        fn image(&self, src: &str) -> ImageRef {
            self.images.borrow_mut().push(src.to_owned());
            if src == "img/x.png" {
                ImageRef::Media(MEDIA)
            } else if src.starts_with("http") {
                ImageRef::Url(src.to_owned())
            } else {
                ImageRef::Missing(src.to_owned())
            }
        }

        fn attachment(&self, _: i64) -> Option<Uuid> {
            None
        }

        fn href(&self, href: &str) -> String {
            self.hrefs.borrow_mut().push(href.to_owned());
            match href {
                "../other.md#sec" => "/docs/other/#sec".to_owned(),
                other => other.to_owned(),
            }
        }
    }

    fn conv(md: &str) -> (Converted, Notes, Rw) {
        let rw = Rw::default();
        let mut notes = Notes::default();
        let c = convert(md, &rw, &mut notes);
        (c, notes, rw)
    }

    fn content(c: &Converted) -> Vec<Value> {
        c.body["content"].as_array().unwrap().clone()
    }

    fn text(t: &str) -> Value {
        json!({"type": "text", "text": t})
    }

    fn para(t: &str) -> Value {
        json!({"type": "paragraph", "content": [text(t)]})
    }

    fn heading(level: u8, t: &str) -> Value {
        json!({"type": "heading", "attrs": {"level": level}, "content": [text(t)]})
    }

    #[test]
    fn inline_marks() {
        let (c, _, _) = conv("A **b** *c* `d` [e](https://example.com/) ~~f~~\n");
        assert_eq!(
            content(&c),
            [json!({"type": "paragraph", "content": [
                text("A "),
                {"type": "text", "text": "b", "marks": [{"type": "bold"}]},
                text(" "),
                {"type": "text", "text": "c", "marks": [{"type": "italic"}]},
                text(" "),
                {"type": "text", "text": "d", "marks": [{"type": "code"}]},
                text(" "),
                {"type": "text", "text": "e", "marks": [
                    {"type": "link", "attrs": {"href": "https://example.com/"}}
                ]},
                text(" "),
                {"type": "text", "text": "f", "marks": [{"type": "strike"}]},
            ]})]
        );
    }

    #[test]
    fn blocks() {
        let md = "## Part\n\nOne\ntwo  \nthree\n\n- a\n  - b\n- c\n\n3. x\n4. y\n\n> quoted\n\n\
                  ```rust,ignore\nfn x() {}\n```\n\n---\n\n| H1 | H2 |\n|----|----|\n| a  | b  |\n";
        let (c, _, _) = conv(md);
        assert_eq!(
            content(&c),
            [
                heading(2, "Part"),
                json!({"type": "paragraph", "content": [
                    text("One two"), {"type": "hardBreak"}, text("three")
                ]}),
                json!({"type": "bulletList", "content": [
                    {"type": "listItem", "content": [
                        para("a"),
                        {"type": "bulletList", "content": [
                            {"type": "listItem", "content": [para("b")]}
                        ]}
                    ]},
                    {"type": "listItem", "content": [para("c")]}
                ]}),
                json!({"type": "orderedList", "attrs": {"start": 3}, "content": [
                    {"type": "listItem", "content": [para("x")]},
                    {"type": "listItem", "content": [para("y")]}
                ]}),
                json!({"type": "blockquote", "content": [para("quoted")]}),
                json!({"type": "codeBlock", "attrs": {"language": "rust"},
                       "content": [text("fn x() {}\n")]}),
                json!({"type": "horizontalRule"}),
                json!({"type": "table", "content": [
                    {"type": "tableRow", "content": [
                        {"type": "tableHeader", "attrs": {"colspan": 1, "rowspan": 1},
                         "content": [para("H1")]},
                        {"type": "tableHeader", "attrs": {"colspan": 1, "rowspan": 1},
                         "content": [para("H2")]}
                    ]},
                    {"type": "tableRow", "content": [
                        {"type": "tableCell", "attrs": {"colspan": 1, "rowspan": 1},
                         "content": [para("a")]},
                        {"type": "tableCell", "attrs": {"colspan": 1, "rowspan": 1},
                         "content": [para("b")]}
                    ]}
                ]}),
            ]
        );
        assert_eq!(c.title, None);
    }

    #[test]
    fn raw_html_is_cleaned_as_in_a_wordpress_import() {
        let md = "<div class=\"x\">kept <b>bold</b></div>\n\n<script>alert(1)</script>\n\n\
                  a <span onclick=\"x()\">span</span> <!-- note -->\n";
        let (c, notes, _) = conv(md);
        assert_eq!(
            content(&c),
            [
                json!({"type": "paragraph", "content": [
                    text("kept "), {"type": "text", "text": "bold", "marks": [{"type": "bold"}]}
                ]}),
                para("a span"),
            ]
        );
        assert_eq!(notes.unsupported_tags["script"].count, 1);
        assert!(!c.body.to_string().contains("alert"));
    }

    #[test]
    fn extensions_other_than_tables_and_strikethrough_stay_off() {
        // Task lists, footnotes, math, heading attributes and smart punctuation are off.
        let (c, _, _) = conv("- [ ] task\n\nfoot[^1] $x$ \"q\"\n\n## H {#id}\n");
        let s = c.body.to_string();
        assert!(s.contains("[ ] task"), "{s}");
        assert!(s.contains(r#"foot[^1] $x$ \"q\""#), "{s}");
        assert!(s.contains("H {#id}"), "{s}");
    }

    #[test]
    fn first_h1_is_the_title() {
        let (c, notes, _) = conv("\u{feff}# Getting `started`\r\n\r\nText\r\n\r\n## Next\r\n");
        assert_eq!(c.title.as_deref(), Some("Getting started"));
        assert_eq!(content(&c), [para("Text"), heading(2, "Next")]);
        assert_eq!(notes.h1_demoted.count, 0);
        let (c, _, _) = conv("Intro\n\nTitle\n=====\n\nBody\n");
        assert_eq!(c.title.as_deref(), Some("Title"));
        assert_eq!(content(&c), [para("Intro"), para("Body")]);
    }

    #[test]
    fn a_second_h1_shifts_every_level() {
        let md = "# Title\n\n# A\n\n## B\n\n### C\n\n#### D\n\n##### E\n\n###### F\n";
        let (c, notes, _) = conv(md);
        assert_eq!(c.title.as_deref(), Some("Title"));
        assert_eq!(
            content(&c),
            [
                heading(2, "A"),
                heading(3, "B"),
                heading(4, "C"),
                heading(5, "D"),
                heading(6, "E"),
                heading(6, "F"),
            ]
        );
        assert_eq!(notes.h1_demoted.count, 0);
        // Without a second h1 the levels stay.
        let (c, _, _) = conv("# Title\n\n## B\n\n### C\n");
        assert_eq!(content(&c), [heading(2, "B"), heading(3, "C")]);
    }

    #[test]
    fn images_go_through_the_rewrite_and_are_listed() {
        let md = "![a](img/x.png)\n\n![b](https://cdn.example.com/y.png)\n\n![c](../up/z.png)\n\n\
                  ![again](img/x.png)\n\n<img src=\"raw/h.png\" alt=\"h\">\n";
        let (c, notes, rw) = conv(md);
        assert_eq!(
            content(&c)[0],
            json!({"type": "image", "attrs": {"src": format!("ferrite:media/{MEDIA}"), "alt": "a"}})
        );
        assert_eq!(
            content(&c)[1],
            json!({"type": "image", "attrs": {"src": "https://cdn.example.com/y.png", "alt": "b"}})
        );
        assert!(rw.images.borrow().contains(&"img/x.png".to_owned()));
        assert_eq!(c.images, ["img/x.png", "../up/z.png", "raw/h.png"]);
        // The HTML converter shows only http(s) and rooted sources: a relative one the
        // caller could not resolve is left out of the body.
        let images = content(&c).iter().filter(|n| n["type"] == "image").count();
        assert_eq!(images, 3);
        assert_eq!(notes.unsupported_tags["img"].count, 2);
    }

    #[test]
    fn fence_languages_are_lower_cased() {
        let (c, _, _) = conv("```Rust\nfn x() {}\n```\n\n```TOML extra\na = 1\n```\n");
        assert_eq!(
            content(&c),
            [
                json!({"type": "codeBlock", "attrs": {"language": "rust"},
                       "content": [text("fn x() {}\n")]}),
                json!({"type": "codeBlock", "attrs": {"language": "toml"},
                       "content": [text("a = 1\n")]}),
            ]
        );
    }

    #[test]
    fn aligned_table_converts_and_counts_the_alignment() {
        let md = "| L | C | R |\n|:--|:-:|--:|\n| a | b | c |\n";
        let (c, notes, _) = conv(md);
        let cell = |t: &str, kind: &str| json!({"type": kind, "attrs": {"colspan": 1, "rowspan": 1}, "content": [para(t)]});
        assert_eq!(
            content(&c),
            [json!({"type": "table", "content": [
                {"type": "tableRow", "content": [
                    cell("L", "tableHeader"), cell("C", "tableHeader"), cell("R", "tableHeader")
                ]},
                {"type": "tableRow", "content": [
                    cell("a", "tableCell"), cell("b", "tableCell"), cell("c", "tableCell")
                ]}
            ]})]
        );
        // The editor's cells have no alignment: the HTML converter reads no `style` on a
        // cell, so the column alignment is lost without a note.
        assert_eq!(notes, Notes::default());
    }

    #[test]
    fn a_folder_link_without_a_slash_is_listed_when_rewritten() {
        struct Folders;
        impl Rewrite for Folders {
            fn image(&self, src: &str) -> ImageRef {
                ImageRef::Missing(src.to_owned())
            }
            fn attachment(&self, _: i64) -> Option<Uuid> {
                None
            }
            fn href(&self, href: &str) -> String {
                match href {
                    "guides" => "/guides/".to_owned(),
                    other => other.to_owned(),
                }
            }
        }
        let mut notes = Notes::default();
        let c = convert(
            "[g](guides) [f](file.pdf) [h](guides.md)\n",
            &Folders,
            &mut notes,
        );
        assert_eq!(c.links, ["guides", "guides.md"]);
        let first = &content(&c)[0]["content"][0];
        assert_eq!(first["marks"][0]["attrs"]["href"], "/guides/");
    }

    #[test]
    fn links_are_rewritten_and_md_targets_listed() {
        let md = "[x](../other.md#sec) [y](guide.md) [z](sub/) [w](https://example.com/a.md) \
                  [v](#local) [u](file.pdf) [t](/abs.md) [s](guide.md)\n";
        let (c, _, rw) = conv(md);
        assert!(rw.hrefs.borrow().contains(&"../other.md#sec".to_owned()));
        let first = &content(&c)[0]["content"][0];
        assert_eq!(first["marks"][0]["attrs"]["href"], "/docs/other/#sec");
        assert_eq!(c.links, ["../other.md#sec", "guide.md", "sub/"]);
    }
}
