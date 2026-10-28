//! Gutenberg blocks to block-tree nodes (P11, P12).
//!
//! Blocks whose markup [`html::convert`] already reads (paragraphs, headings, lists, tables,
//! code, images) are converted from their HTML with the inner blocks' HTML in place.
//! Layout blocks (columns, groups, covers, media-text, buttons, details) are flattened:
//! their own HTML and their inner blocks are converted piece by piece, in order. Embeds,
//! media and raw HTML are mapped directly; an unknown block keeps its HTML as an `html`
//! node, or is dropped when that is empty, and is counted either way.

use std::cell::Cell;

use serde_json::Value;
use uuid::Uuid;

use super::doc::Node;
use super::gutenberg::{Block, HtmlPart};
use super::shortcodes::{count_shortcodes, embed_html, escape, is_bare_url, to_nodes};
use super::{ImageRef, Notes, Rewrite, body, html};
use ferrite_core::blocks::embed_id;

/// Converts parsed blocks, in order.
pub fn convert(blocks: &[Block], rw: &dyn Rewrite, notes: &mut Notes) -> Vec<Node> {
    let mut out = Vec::new();
    for b in blocks {
        block(b, rw, notes, &mut out);
    }
    out
}

fn block(b: &Block, rw: &dyn Rewrite, notes: &mut Notes, out: &mut Vec<Node>) {
    if b.bad_attrs {
        notes.bad_block_attrs();
    }
    let name = b.name.as_str();
    match name {
        "core/paragraph" => {
            let html = render(b);
            match paragraph_url(&html) {
                Some(url) => out.extend(html::convert(&embed_html(url, true), rw, notes)),
                None => out.extend(to_nodes(&html, rw, notes)),
            }
        }
        "core/heading" | "core/list" | "core/list-item" | "core/table" | "core/pullquote" => {
            out.extend(to_nodes(&render(b), rw, notes));
        }
        "core/code" => out.extend(code(b, rw, notes, true)),
        "core/preformatted" => out.extend(code(b, rw, notes, false)),
        "core/verse" => out.extend(to_nodes(&verse(&render(b)), rw, notes)),
        "core/quote" => {
            let content = flatten(b, rw, rw, notes);
            if !content.is_empty() {
                out.push(Node::Blockquote(content));
            }
        }
        "core/separator" => out.push(Node::HorizontalRule),
        "core/image" => {
            let pinned = Pinned::new(rw, attachment(b, "id", rw));
            out.extend(to_nodes(&render(b), &pinned, notes));
        }
        "core/gallery" => {
            // Before WP 5.9 a gallery is one block whose images are its `ids`, in order.
            let ids = b.attrs.get("ids").and_then(Value::as_array);
            let nodes = match ids {
                Some(ids) if b.inner.is_empty() => {
                    let in_order = InOrder {
                        rw,
                        media: ids
                            .iter()
                            .map(|id| id_of(id).and_then(|id| rw.attachment(id)))
                            .collect(),
                        next: Cell::new(0),
                    };
                    flatten(b, &in_order, rw, notes)
                }
                _ => flatten(b, rw, rw, notes),
            };
            out.extend(unlist(nodes));
        }
        "core/embed" => embed(b, rw, notes, out),
        _ if name.starts_with("core-embed/") => embed(b, rw, notes, out),
        "core/video" | "core/audio" | "core/file" => media(b, rw, notes, out),
        "core/buttons" | "core/button" | "core/columns" | "core/column" | "core/group"
        | "core/row" | "core/stack" | "core/details" => out.extend(flatten(b, rw, rw, notes)),
        "core/cover" => {
            let pinned = Pinned::new(rw, attachment(b, "id", rw));
            out.extend(flatten(b, &pinned, rw, notes));
        }
        "core/media-text" => {
            let pinned = Pinned::new(rw, attachment(b, "mediaId", rw));
            out.extend(flatten(b, &pinned, rw, notes));
        }
        "core/html" => {
            let html = render(b);
            count_shortcodes(&html, notes);
            html_node(&html, notes, out);
        }
        "core/shortcode" | "core/freeform" => out.extend(body::classic(&render(b), rw, notes)),
        "core/nextpage" => notes.page_break(),
        // Layout only, or a marker with no meaning in the stored body.
        "core/spacer" | "core/more" => {}
        _ => {
            notes.unknown_block(name);
            html_node(&render(b), notes, out);
        }
    }
}

/// Raw markup as an `html` node; dropped when empty. Markup nested too deep for the
/// renderer's parser to handle safely becomes text instead (counted).
fn html_node(html: &str, notes: &mut Notes, out: &mut Vec<Node>) {
    let html = html.trim();
    if html.is_empty() {
        return;
    }
    if html::nesting_depth(html) > html::MAX_PARSE_DEPTH {
        notes.too_deep();
        out.extend(html::plain_paragraphs(html));
    } else {
        out.push(Node::Html(html.to_owned()));
    }
}

/// The URL of a paragraph holding nothing but a YouTube or Vimeo URL (WP auto-embeds
/// `<p>URL</p>`).
fn paragraph_url(html: &str) -> Option<&str> {
    let html = html.trim();
    let lower = html.to_ascii_lowercase();
    let after_name = lower.as_bytes().get(2).copied();
    if !lower.starts_with("<p")
        || !after_name.is_some_and(|c| c == b'>' || c == b'/' || c.is_ascii_whitespace())
        || !lower.ends_with("</p>")
    {
        return None;
    }
    let start = start_tag_end(html)?;
    let end = lower.len() - "</p>".len();
    let url = html.get(start..end)?.trim();
    (is_bare_url(url)
        && ["youtube", "vimeo"]
            .iter()
            .any(|p| embed_id(p, url).is_some()))
    .then_some(url)
}

/// The end (after `>`) of the start tag `html` begins with: a `>` inside a quoted attribute
/// value does not end it.
fn start_tag_end(html: &str) -> Option<usize> {
    let mut quote = None;
    let mut after_eq = false;
    for (i, c) in html.bytes().enumerate().skip(1) {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => continue,
            None => match c {
                b'>' => return Some(i + 1),
                b'"' | b'\'' if after_eq => {
                    quote = Some(c);
                    continue;
                }
                _ => {}
            },
        }
        if !c.is_ascii_whitespace() {
            after_eq = c == b'=';
        }
    }
    None
}

/// The block's HTML with its inner blocks' HTML in place (WP's serialised `innerContent`).
fn render(b: &Block) -> String {
    let mut s = String::new();
    render_into(b, &mut s);
    s
}

fn render_into(b: &Block, s: &mut String) {
    for part in &b.inner_html {
        match part {
            HtmlPart::Html(h) => s.push_str(h),
            HtmlPart::Child(i) => {
                if let Some(c) = b.inner.get(*i) {
                    render_into(c, s);
                }
            }
        }
    }
}

/// The block's own HTML pieces (through `own_rw`) and its inner blocks (through `rw`), in
/// order. A piece of own HTML is converted on its own: the parser closes what it opens.
fn flatten(b: &Block, own_rw: &dyn Rewrite, rw: &dyn Rewrite, notes: &mut Notes) -> Vec<Node> {
    let mut out = Vec::new();
    for part in &b.inner_html {
        match part {
            HtmlPart::Html(h) => {
                if !h.trim().is_empty() {
                    out.extend(to_nodes(h, own_rw, notes));
                }
            }
            HtmlPart::Child(i) => {
                if let Some(c) = b.inner.get(*i) {
                    block(c, rw, notes, &mut out);
                }
            }
        }
    }
    out
}

/// A list's items' content in place of the list (an old-style gallery is a `<ul>` of
/// figures); empty paragraphs (a list item's lead) are dropped.
fn unlist(nodes: Vec<Node>) -> Vec<Node> {
    let mut out = Vec::new();
    for n in nodes {
        match n {
            Node::BulletList(items) | Node::OrderedList { content: items, .. } => {
                for item in items {
                    match item {
                        Node::ListItem(content) => out.extend(unlist(content)),
                        other => out.push(other),
                    }
                }
            }
            Node::Paragraph(c) if c.is_empty() => {}
            other => out.push(other),
        }
    }
    out
}

/// `core/code` (language from a `language-x` class on the markup or in `className`) and
/// `core/preformatted` (never a language).
fn code(b: &Block, rw: &dyn Rewrite, notes: &mut Notes, language: bool) -> Vec<Node> {
    let class_name = b
        .attrs
        .get("className")
        .and_then(Value::as_str)
        .and_then(html::class_language);
    let mut nodes = to_nodes(&render(b), rw, notes);
    for n in &mut nodes {
        if let Node::CodeBlock { language: l, .. } = n {
            if !language {
                *l = None;
            } else if l.is_none() {
                l.clone_from(&class_name);
            }
        }
    }
    nodes
}

/// `core/verse` is a `<pre>` of poetry: a paragraph with its lines as breaks, not code.
fn verse(html: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let inner = lower.find("<pre").and_then(|open| {
        let start = open + lower[open..].find('>')? + 1;
        let end = lower.rfind("</pre")?;
        (start <= end).then(|| &html[start..end])
    });
    match inner {
        Some(inner) => format!("<p>{}</p>", inner.trim_matches('\n').replace('\n', "<br>")),
        None => html.to_owned(),
    }
}

/// A YouTube or Vimeo embed becomes an `embed` node; any other a paragraph linking to it.
/// The URL is `attrs.url`, else the text of the markup. A caption follows as a paragraph.
fn embed(b: &Block, rw: &dyn Rewrite, notes: &mut Notes, out: &mut Vec<Node>) {
    let html = render(b);
    let text = markup_text(&html);
    let url = b
        .attrs
        .get("url")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|u| !u.is_empty())
        .or_else(|| {
            let text = text.trim();
            is_bare_url(text).then_some(text)
        });
    match url {
        Some(url) => {
            out.extend(html::convert(&embed_html(url, true), rw, notes));
            if let Some(caption) = figcaption(&html) {
                out.extend(to_nodes(caption, rw, notes));
            }
        }
        None => out.extend(to_nodes(&html, rw, notes)),
    }
}

/// The text of `html` outside its figcaption, tags removed and `&amp;` decoded: an embed's
/// URL when its block has no `url` attribute.
fn markup_text(html: &str) -> String {
    let html = match figcaption(html) {
        Some(c) => html.replacen(c, "", 1),
        None => html.to_owned(),
    };
    let mut out = String::new();
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&amp;", "&")
}

/// `core/video` and `core/audio` become a paragraph linking to the file, `core/file` its
/// link; a caption follows as a paragraph.
fn media(b: &Block, rw: &dyn Rewrite, notes: &mut Notes, out: &mut Vec<Node>) {
    let html = render(b);
    let Some(doc) = html::parse_guarded(&html, notes) else {
        out.extend(html::plain_paragraphs(&html));
        return;
    };
    let root = doc.root_element();
    let link = if b.name == "core/file" {
        root.descendent_elements()
            .find(|e| e.value().name() == "a" && e.attr("href").is_some())
            .and_then(|a| {
                let href = a.attr("href")?.trim();
                let text: String = a.text().collect();
                let text = text.trim();
                Some((
                    href.to_owned(),
                    if text.is_empty() { href } else { text }.to_owned(),
                ))
            })
    } else {
        root.descendent_elements()
            .filter(|e| matches!(e.value().name(), "video" | "audio" | "source"))
            .find_map(|e| e.attr("src").map(str::trim).filter(|s| !s.is_empty()))
            .map(|src| (src.to_owned(), src.to_owned()))
    };
    if let Some((href, text)) = link {
        let p = format!("<p><a href=\"{}\">{}</a></p>", escape(&href), escape(&text));
        out.extend(html::convert(&p, rw, notes));
    }
    if let Some(caption) = figcaption(&html) {
        out.extend(to_nodes(caption, rw, notes));
    }
}

/// The first `<figcaption>…</figcaption>` in `html`.
fn figcaption(html: &str) -> Option<&str> {
    let lower = html.to_ascii_lowercase();
    let start = lower.find("<figcaption")?;
    let end = start + lower[start..].find("</figcaption>")? + "</figcaption>".len();
    Some(&html[start..end])
}

/// The imported media of the attachment in `attrs[key]` (a number, or a numeric string).
fn attachment(b: &Block, key: &str, rw: &dyn Rewrite) -> Option<Uuid> {
    rw.attachment(id_of(b.attrs.get(key)?)?)
}

/// A JSON number or numeric string as an id.
fn id_of(v: &Value) -> Option<i64> {
    match v {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

/// Shows the images, in document order, as `media` (an old gallery's `ids`); an image past
/// the list, or whose attachment was not imported, goes by its `src`.
struct InOrder<'a> {
    rw: &'a dyn Rewrite,
    media: Vec<Option<Uuid>>,
    next: Cell<usize>,
}

impl Rewrite for InOrder<'_> {
    fn image(&self, src: &str) -> ImageRef {
        let i = self.next.get();
        self.next.set(i + 1);
        match self.media.get(i).copied().flatten() {
            Some(id) => ImageRef::Media(id),
            None => self.rw.image(src),
        }
    }

    fn attachment(&self, wp_id: i64) -> Option<Uuid> {
        self.rw.attachment(wp_id)
    }

    fn href(&self, href: &str) -> String {
        self.rw.href(href)
    }
}

/// Shows every image as `media` (the block's own attachment, P14: by id before `src`).
struct Pinned<'a> {
    rw: &'a dyn Rewrite,
    media: Option<Uuid>,
}

impl<'a> Pinned<'a> {
    fn new(rw: &'a dyn Rewrite, media: Option<Uuid>) -> Self {
        Self { rw, media }
    }
}

impl Rewrite for Pinned<'_> {
    fn image(&self, src: &str) -> ImageRef {
        match self.media {
            Some(id) => ImageRef::Media(id),
            None => self.rw.image(src),
        }
    }

    fn attachment(&self, wp_id: i64) -> Option<Uuid> {
        self.rw.attachment(wp_id)
    }

    fn href(&self, href: &str) -> String {
        self.rw.href(href)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use crate::convert::doc::Mark;
    use crate::convert::gutenberg::parse;

    struct Rw;

    impl Rewrite for Rw {
        fn image(&self, src: &str) -> ImageRef {
            ImageRef::Url(src.to_owned())
        }

        fn attachment(&self, wp_id: i64) -> Option<Uuid> {
            (wp_id < 100).then(|| media(wp_id))
        }

        fn href(&self, href: &str) -> String {
            href.replace("/old/", "/new/")
        }
    }

    fn media(id: i64) -> Uuid {
        Uuid::from_u128(u128::try_from(id).unwrap())
    }

    fn conv(content: &str) -> (Vec<Node>, Notes) {
        let mut notes = Notes {
            item: Some(1),
            ..Notes::default()
        };
        (convert(&parse(content), &Rw, &mut notes), notes)
    }

    fn t(s: &str) -> Node {
        Node::Text {
            text: s.to_owned(),
            marks: vec![],
        }
    }

    fn p(s: &str) -> Node {
        Node::Paragraph(vec![t(s)])
    }

    fn link_p(text: &str, href: &str) -> Node {
        Node::Paragraph(vec![Node::Text {
            text: text.to_owned(),
            marks: vec![Mark::Link {
                href: href.to_owned(),
                target: None,
            }],
        }])
    }

    fn img(src: &str, alt: Option<&str>) -> Node {
        Node::Image {
            src: src.to_owned(),
            alt: alt.map(str::to_owned),
            title: None,
            caption: None,
        }
    }

    #[test]
    fn image_by_attachment_id_before_src() {
        let (nodes, _) = conv(
            "<!-- wp:image {\"id\":7} --><figure><img src=\"/a.png\" alt=\"\"/></figure><!-- /wp:image -->\
             <!-- wp:image {\"id\":700} --><figure><img src=\"/b.png\" alt=\"\"/></figure><!-- /wp:image -->\
             <!-- wp:image {\"id\":\"8\"} --><figure><img src=\"/c.png\" alt=\"\"/></figure><!-- /wp:image -->",
        );
        let m = |i| format!("ferrite:media/{}", media(i));
        assert_eq!(
            nodes,
            [
                img(&m(7), Some("")),
                img("/b.png", Some("")),
                img(&m(8), Some(""))
            ]
        );
    }

    #[test]
    fn old_gallery_list_becomes_images() {
        let (nodes, _) = conv(
            "<!-- wp:gallery {\"ids\":[1,200]} --><figure class=\"wp-block-gallery\"><ul class=\"blocks-gallery-grid\">\
             <li class=\"blocks-gallery-item\"><figure><img src=\"/1.png\" alt=\"one\"/></figure></li>\
             <li class=\"blocks-gallery-item\"><figure><img src=\"/2.png\" alt=\"two\"/><figcaption>Two</figcaption></figure></li>\
             </ul><figcaption class=\"blocks-gallery-caption\">All</figcaption></figure><!-- /wp:gallery -->",
        );
        assert_eq!(
            nodes,
            [
                img(&format!("ferrite:media/{}", media(1)), Some("one")),
                Node::Image {
                    src: "/2.png".into(),
                    alt: Some("two".into()),
                    title: None,
                    caption: Some("Two".into())
                },
                p("All")
            ]
        );
    }

    #[test]
    fn code_language_from_class_name_and_preformatted_without() {
        let (nodes, _) = conv(
            "<!-- wp:code {\"className\":\"language-rust\"} --><pre class=\"wp-block-code\"><code>x</code></pre><!-- /wp:code -->\
             <!-- wp:code --><pre class=\"wp-block-code\"><code class=\"language-php\">y</code></pre><!-- /wp:code -->\
             <!-- wp:preformatted --><pre class=\"wp-block-preformatted language-js\">z</pre><!-- /wp:preformatted -->",
        );
        let code = |language: Option<&str>, text: &str| Node::CodeBlock {
            language: language.map(str::to_owned),
            text: text.to_owned(),
        };
        assert_eq!(
            nodes,
            [
                code(Some("rust"), "x"),
                code(Some("php"), "y"),
                code(None, "z")
            ]
        );
    }

    #[test]
    fn verse_and_pullquote() {
        let (nodes, _) = conv(
            "<!-- wp:verse --><pre class=\"wp-block-verse\">Roses are red\nViolets blue</pre><!-- /wp:verse -->\
             <!-- wp:pullquote --><figure class=\"wp-block-pullquote\"><blockquote><p>Pull</p><cite>Me</cite></blockquote></figure><!-- /wp:pullquote -->",
        );
        assert_eq!(
            nodes,
            [
                Node::Paragraph(vec![t("Roses are red"), Node::HardBreak, t("Violets blue")]),
                Node::Blockquote(vec![p("Pull"), p("Me")]),
            ]
        );
    }

    #[test]
    fn embeds_by_provider() {
        let (nodes, _) = conv(
            "<!-- wp:embed {\"url\":\"https://vimeo.com/76979871\",\"providerNameSlug\":\"vimeo\"} -->\
             <figure class=\"wp-block-embed\"><div class=\"wp-block-embed__wrapper\">\nhttps://vimeo.com/76979871\n</div><figcaption>A film</figcaption></figure><!-- /wp:embed -->\
             <!-- wp:embed {\"url\":\"https://twitter.com/x/status/1\"} --><figure><div>https://twitter.com/x/status/1</div></figure><!-- /wp:embed -->\
             <!-- wp:core-embed/youtube {\"url\":\"https://youtu.be/dQw4w9WgXcQ\"} --><figure><div>https://youtu.be/dQw4w9WgXcQ</div></figure><!-- /wp:core-embed/youtube -->\
             <!-- wp:embed --><figure><div>no url</div></figure><!-- /wp:embed -->",
        );
        assert_eq!(
            nodes,
            [
                Node::Embed {
                    provider: "vimeo".into(),
                    url: "https://vimeo.com/76979871".into()
                },
                p("A film"),
                link_p(
                    "https://twitter.com/x/status/1",
                    "https://twitter.com/x/status/1"
                ),
                Node::Embed {
                    provider: "youtube".into(),
                    url: "https://www.youtube.com/watch?v=dQw4w9WgXcQ".into()
                },
                p("no url"),
            ]
        );
    }

    #[test]
    fn video_audio_and_file_become_links() {
        let (nodes, notes) = conv(
            "<!-- wp:video {\"id\":3} --><figure class=\"wp-block-video\"><video controls src=\"/old/v.mp4\"></video><figcaption>Clip</figcaption></figure><!-- /wp:video -->\
             <!-- wp:audio --><figure class=\"wp-block-audio\"><audio controls><source src=\"/a.mp3\"></audio></figure><!-- /wp:audio -->\
             <!-- wp:file {\"id\":4} --><div class=\"wp-block-file\"><a href=\"/old/menu.pdf\">Lunch &amp; menu</a><a href=\"/old/menu.pdf\" class=\"wp-block-file__button\" download>Download</a></div><!-- /wp:file -->\
             <!-- wp:video --><figure class=\"wp-block-video\"></figure><!-- /wp:video -->",
        );
        assert_eq!(
            nodes,
            [
                link_p("/old/v.mp4", "/new/v.mp4"),
                p("Clip"),
                link_p("/a.mp3", "/a.mp3"),
                link_p("Lunch & menu", "/new/menu.pdf"),
            ]
        );
        assert!(notes.unsupported_tags.is_empty());
    }

    #[test]
    fn layout_blocks_are_flattened() {
        let (nodes, notes) = conv(
            "<!-- wp:group --><div class=\"wp-block-group\"><!-- wp:columns --><div class=\"wp-block-columns\">\
             <!-- wp:column --><div class=\"wp-block-column\"><!-- wp:heading --><h2>In a column</h2><!-- /wp:heading --></div><!-- /wp:column -->\
             </div><!-- /wp:columns --></div><!-- /wp:group -->\
             <!-- wp:cover {\"id\":9} --><div class=\"wp-block-cover\"><span class=\"wp-block-cover__background\"></span>\
             <img class=\"wp-block-cover__image-background\" src=\"/cover.jpg\" alt=\"\"/><div class=\"wp-block-cover__inner-container\">\
             <!-- wp:paragraph --><p>On the cover</p><!-- /wp:paragraph --></div></div><!-- /wp:cover -->\
             <!-- wp:media-text {\"mediaId\":10} --><div class=\"wp-block-media-text\"><figure class=\"wp-block-media-text__media\"><img src=\"/m.jpg\" alt=\"M\"/></figure>\
             <div class=\"wp-block-media-text__content\"><!-- wp:paragraph --><p>Beside</p><!-- /wp:paragraph --></div></div><!-- /wp:media-text -->\
             <!-- wp:details --><details class=\"wp-block-details\"><summary>More</summary><!-- wp:paragraph --><p>Hidden</p><!-- /wp:paragraph --></details><!-- /wp:details -->\
             <!-- wp:buttons --><div class=\"wp-block-buttons\"><!-- wp:button --><div class=\"wp-block-button\"><a class=\"wp-block-button__link\" href=\"/old/a\">A</a></div><!-- /wp:button -->\
             <!-- wp:button --><div class=\"wp-block-button\"><a class=\"wp-block-button__link\" href=\"/b\">B</a></div><!-- /wp:button --></div><!-- /wp:buttons -->",
        );
        let m = |i| format!("ferrite:media/{}", media(i));
        assert_eq!(
            nodes,
            [
                Node::Heading {
                    level: 2,
                    content: vec![t("In a column")]
                },
                img(&m(9), Some("")),
                p("On the cover"),
                img(&m(10), Some("M")),
                p("Beside"),
                p("More"),
                p("Hidden"),
                link_p("A", "/new/a"),
                link_p("B", "/b"),
            ]
        );
        assert_eq!(
            notes,
            Notes {
                item: Some(1),
                ..Notes::default()
            }
        );
    }

    #[test]
    fn html_shortcode_and_freeform_blocks() {
        let (nodes, notes) = conv(
            "<!-- wp:html -->\n<marquee>hi</marquee>\n<!-- /wp:html --><!-- wp:html --> <!-- /wp:html -->\
             <!-- wp:shortcode -->[gallery ids=\"5\"]<!-- /wp:shortcode -->\
             Classic line one\nline two\n\n[foo]",
        );
        assert_eq!(
            nodes,
            [
                Node::Html("<marquee>hi</marquee>".into()),
                img(&format!("ferrite:media/{}", media(5)), None),
                Node::Paragraph(vec![t("Classic line one"), Node::HardBreak, t("line two")]),
                p("[foo]"),
            ]
        );
        assert_eq!(notes.unknown_shortcodes["foo"].count, 1);
    }

    #[test]
    fn unknown_blocks_keep_their_html_or_are_dropped_and_counted() {
        let (nodes, notes) = conv(
            "<!-- wp:latest-posts {\"postsToShow\":3} /-->\
             <!-- wp:acme/box --><div class=\"box\"><!-- wp:paragraph --><p>Inside</p><!-- /wp:paragraph --></div><!-- /wp:acme/box -->\
             <!-- wp:spacer {\"height\":\"20px\"} --><div style=\"height:20px\" aria-hidden=\"true\" class=\"wp-block-spacer\"></div><!-- /wp:spacer -->\
             <!-- wp:more --><!--more--><!-- /wp:more --><!-- wp:separator {bad json} --><hr/><!-- /wp:separator -->",
        );
        assert_eq!(
            nodes,
            [
                Node::Html("<div class=\"box\"><p>Inside</p></div>".into()),
                Node::HorizontalRule
            ]
        );
        assert_eq!(notes.unknown_blocks.len(), 2);
        assert_eq!(notes.unknown_blocks["core/latest-posts"].count, 1);
        assert_eq!(notes.unknown_blocks["acme/box"].items, [1]);
        assert_eq!(notes.bad_block_attrs.count, 1);
    }

    #[test]
    fn shortcodes_inside_paragraph_blocks_are_expanded() {
        let (nodes, _) = conv(
            "<!-- wp:paragraph --><p>[embed]https://vimeo.com/5[/embed]</p><!-- /wp:paragraph -->",
        );
        assert_eq!(
            nodes,
            [Node::Embed {
                provider: "vimeo".into(),
                url: "https://vimeo.com/5".into()
            }]
        );
    }

    #[test]
    fn embed_url_from_the_markup_and_paragraph_auto_embed() {
        let (nodes, _) = conv(
            "<!-- wp:embed --><figure><div class=\"wp-block-embed__wrapper\">\nhttps://youtu.be/dQw4w9WgXcQ\n</div><figcaption>Cap https://x.org</figcaption></figure><!-- /wp:embed -->\
             <!-- wp:embed --><figure><div>https://example.org/a?b=1&amp;c=2</div></figure><!-- /wp:embed -->\
             <!-- wp:paragraph --><p class = 'x>y' title=\"a>b\"> https://vimeo.com/42 </p><!-- /wp:paragraph -->\
             <!-- wp:paragraph --><pre>https://vimeo.com/44</p><!-- /wp:paragraph -->\
             <!-- wp:paragraph --><p>https://example.org/x</p><!-- /wp:paragraph -->\
             <!-- wp:paragraph --><p><a href=\"https://vimeo.com/43\">https://vimeo.com/43</a></p><!-- /wp:paragraph -->",
        );
        let yt = Node::Embed {
            provider: "youtube".into(),
            url: "https://www.youtube.com/watch?v=dQw4w9WgXcQ".into(),
        };
        let vimeo = Node::Embed {
            provider: "vimeo".into(),
            url: "https://vimeo.com/42".into(),
        };
        assert_eq!(
            nodes,
            [
                yt,
                p("Cap https://x.org"),
                link_p(
                    "https://example.org/a?b=1&c=2",
                    "https://example.org/a?b=1&c=2"
                ),
                vimeo,
                Node::CodeBlock {
                    language: None,
                    text: "https://vimeo.com/44".into()
                },
                p("https://example.org/x"),
                link_p("https://vimeo.com/43", "https://vimeo.com/43"),
            ]
        );
    }

    #[test]
    fn page_breaks_are_counted() {
        let (nodes, notes) = conv(
            "<!-- wp:paragraph --><p>One</p><!-- /wp:paragraph --><!-- wp:nextpage --><!--nextpage--><!-- /wp:nextpage -->\
             <!-- wp:more --><!--more--><!-- /wp:more -->",
        );
        assert_eq!(nodes, [p("One")]);
        assert_eq!(notes.page_breaks.count, 1);
        assert!(notes.unknown_blocks.is_empty());
        let mut notes = Notes::default();
        let nodes = body::classic("a\n<!--nextpage-->\nb<!--nextpage-->c", &Rw, &mut notes);
        assert_eq!(nodes.len(), 1);
        assert_eq!(notes.page_breaks.count, 2);
    }

    #[test]
    fn html_blocks_count_their_shortcodes_and_too_deep_markup_is_text() {
        let deep = format!("<div>{}x", "<span>".repeat(600));
        let content = format!(
            "<!-- wp:html --><div title=\"[no]\">[form id=1] [[esc]]</div><!-- /wp:html -->\
             <!-- wp:html -->{deep}<!-- /wp:html --><!-- wp:acme/x -->{deep}<!-- /wp:acme/x -->"
        );
        let (nodes, notes) = conv(&content);
        assert_eq!(
            nodes[0],
            Node::Html("<div title=\"[no]\">[form id=1] [[esc]]</div>".into())
        );
        assert_eq!(nodes[1..], [p(&deep), p(&deep)]);
        assert_eq!(notes.unknown_shortcodes.len(), 1);
        assert_eq!(notes.unknown_shortcodes["form"].count, 1);
        assert_eq!(notes.too_deep.count, 2);
    }

    #[test]
    fn too_deep_media_markup_is_text() {
        let (nodes, notes) = conv(&format!(
            "<!-- wp:video --><figure>{}<video src=\"/v.mp4\"></video></figure><!-- /wp:video -->",
            "<b>".repeat(600)
        ));
        assert_eq!(nodes.len(), 1);
        assert!(matches!(&nodes[0], Node::Paragraph(_)));
        assert_eq!(notes.too_deep.count, 1);
    }
}
