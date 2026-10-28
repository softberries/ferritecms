//! Shortcodes and auto-embeds in post HTML (P11, P12).
//!
//! The shortcodes with a place in the block tree become HTML that [`html::convert`] maps:
//! `[caption]` a `<figure>` with a `<figcaption>`, `[gallery ids]` the attachments' images,
//! `[embed]` a YouTube/Vimeo `<iframe>` or a link, `[video]`/`[audio]` a link to the file.
//! Any other shortcode stays as text and is counted. Shortcodes inside a tag, a comment or a
//! `<pre>` are left alone.

use ferrite_core::blocks::{canonical_embed_url, embed_id};
use uuid::Uuid;

use super::doc::Node;
use super::shortcode::{Piece, scan};
use super::{ImageRef, NextMatch, Notes, Rewrite, html, is_name_byte};

/// Shortcodes nested deeper than this are kept as text.
const MAX_DEPTH: usize = 16;

/// The `src` of a gallery image starts with this, a nonce and `/`; its index into the
/// gallery media follows. The nonce keeps a body from naming one itself.
const GALLERY_SRC: &str = "ferrite-import:gallery-";

/// `[` and `]` inside a masked region; NUL is stripped from the input first, so these
/// cannot occur in it.
const MASK_OPEN: &str = "\0\u{1}";
const MASK_CLOSE: &str = "\0\u{2}";

/// HTML with its shortcodes expanded (P11, P12), as block nodes.
pub(crate) fn to_nodes(html: &str, rw: &dyn Rewrite, notes: &mut Notes) -> Vec<Node> {
    let masked = mask(&html.replace('\0', ""));
    let mut x = Expander {
        rw,
        notes,
        gallery_src: format!("{GALLERY_SRC}{}/", Uuid::now_v7().simple()),
        media: Vec::new(),
        out: String::with_capacity(masked.len()),
    };
    x.expand(&masked, 0);
    let out = x.out.replace(MASK_OPEN, "[").replace(MASK_CLOSE, "]");
    let gallery = Gallery {
        rw,
        src: x.gallery_src,
        media: x.media,
    };
    html::convert(&out, &gallery, notes)
}

/// Counts the shortcodes in `html` (outside tags, comments and `<pre>`) as unknown, without
/// expanding them: raw HTML keeps its text as written.
pub(crate) fn count_shortcodes(html: &str, notes: &mut Notes) {
    for piece in scan(&mask(&html.replace('\0', ""))) {
        if let Piece::Code { name, .. } = piece {
            notes.unknown_shortcode(&name);
        }
    }
}

/// WP's auto-embed, before `wpautop`: a line holding only an http(s) URL becomes a YouTube or
/// Vimeo player, or a link to it. Lines inside `<pre>` or `[embed]` are left alone.
pub(crate) fn autoembed(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_pre = false;
    let mut in_embed = false;
    for line in text.split_inclusive('\n') {
        let url = line.trim();
        if !in_pre && !in_embed && is_bare_url(url) {
            out.push_str(&embed_html(url, false));
            if line.ends_with('\n') {
                out.push('\n');
            }
        } else {
            out.push_str(line);
        }
        let lower = line.to_ascii_lowercase();
        in_pre = still_open(&lower, "<pre", "</pre", in_pre);
        in_embed = still_open(&lower, "[embed", "[/embed]", in_embed);
    }
    out
}

/// Whether a region opened by `open` and closed by `close` is open after `line`.
fn still_open(line: &str, open: &str, close: &str, was: bool) -> bool {
    match (last_tag(line, open), last_tag(line, close)) {
        (Some(o), Some(c)) => o > c,
        (Some(_), None) => true,
        (None, Some(_)) => false,
        (None, None) => was,
    }
}

/// The last `needle` in `line` that is a whole tag name: one ending in a name character
/// must not go on with another (`[embed` is not `[embedpress`, `<pre` not `<preview`).
fn last_tag(line: &str, needle: &str) -> Option<usize> {
    let whole = !needle.bytes().last().is_some_and(is_name_byte);
    line.match_indices(needle)
        .map(|(i, _)| i)
        .filter(|i| {
            whole
                || !line
                    .as_bytes()
                    .get(i + needle.len())
                    .copied()
                    .is_some_and(is_name_byte)
        })
        .last()
}

/// WP's `^\s*(https?://[^\s<>"]+)\s*$`, on a trimmed line.
pub(crate) fn is_bare_url(s: &str) -> bool {
    let lower = s.get(..8).unwrap_or(s).to_ascii_lowercase();
    let rest = if lower.starts_with("https://") {
        &s[8..]
    } else if lower.starts_with("http://") {
        &s[7..]
    } else {
        return false;
    };
    !rest.is_empty()
        && !rest
            .chars()
            .any(|c| c.is_whitespace() || matches!(c, '<' | '>' | '"'))
}

/// A YouTube or Vimeo `url` as an `<iframe>` [`html::convert`] turns into an embed node; any
/// other as a link (in a paragraph of its own when `own_paragraph`).
pub(crate) fn embed_html(url: &str, own_paragraph: bool) -> String {
    for provider in ["youtube", "vimeo"] {
        if let Some(canonical) =
            embed_id(provider, url).and_then(|id| canonical_embed_url(provider, &id))
        {
            return format!("<iframe src=\"{}\"></iframe>", escape(&canonical));
        }
    }
    let a = format!("<a href=\"{0}\">{0}</a>", escape(url));
    if own_paragraph {
        format!("<p>{a}</p>")
    } else {
        a
    }
}

/// Text or an attribute value as HTML.
pub(crate) fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}

/// `html` with `[` and `]` replaced by the mask inside tags, comments and `<pre>` elements,
/// so the shortcode scan does not see them there. One pass: every search is memoised.
fn mask(html: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let b = html.as_bytes();
    let mut gt = NextMatch::new(html, ">");
    let mut comment_end = NextMatch::new(html, "-->");
    let mut pre_end = NextMatch::new(&lower, "</pre");
    let mut out = String::with_capacity(html.len());
    let mut copied = 0;
    let mut from = 0;
    while let Some(rel) = html[from..].find('<') {
        let lt = from + rel;
        let end = if html[lt..].starts_with("<!--") {
            comment_end.find(lt + 4).map(|e| e + 3)
        } else if b
            .get(lt + 1)
            .is_some_and(|c| c.is_ascii_alphabetic() || matches!(c, b'/' | b'!' | b'?'))
        {
            let tag_end = gt.find(lt + 1).map(|e| e + 1);
            let is_pre = lower[lt..].starts_with("<pre")
                && !b.get(lt + 4).is_some_and(u8::is_ascii_alphanumeric);
            match tag_end {
                Some(e) if is_pre => pre_end
                    .find(e)
                    .and_then(|c| gt.find(c))
                    .map_or(Some(e), |c| Some(c + 1)),
                e => e,
            }
        } else {
            None
        };
        match end {
            Some(end) => {
                out.push_str(&html[copied..lt]);
                for c in html[lt..end].chars() {
                    match c {
                        '[' => out.push_str(MASK_OPEN),
                        ']' => out.push_str(MASK_CLOSE),
                        c => out.push(c),
                    }
                }
                copied = end;
                from = end;
            }
            None => from = lt + 1,
        }
    }
    out.push_str(&html[copied..]);
    out
}

struct Expander<'a, 'n> {
    rw: &'a dyn Rewrite,
    notes: &'n mut Notes,
    /// The `src` prefix of gallery images.
    gallery_src: String,
    /// Gallery images, by the index in their `src`.
    media: Vec<Uuid>,
    out: String,
}

impl Expander<'_, '_> {
    fn expand(&mut self, text: &str, depth: usize) {
        for piece in scan(text) {
            match piece {
                Piece::Text(t) => self.out.push_str(&t),
                Piece::Code {
                    name,
                    attrs,
                    inner,
                    open,
                } => {
                    let handled = depth < MAX_DEPTH
                        && match name.as_str() {
                            "caption" | "wp_caption" => {
                                self.caption(attrs.get("caption"), inner.as_deref(), depth)
                            }
                            "gallery" => self.gallery(
                                attrs
                                    .get("ids")
                                    .or_else(|| attrs.get("include"))
                                    .map(String::as_str),
                            ),
                            "embed" => {
                                let url = inner.as_deref().map(str::trim).unwrap_or_default();
                                let url = if url.is_empty() {
                                    attrs.get("src").map_or("", |s| s.trim())
                                } else {
                                    url
                                };
                                self.media_link(url, true)
                            }
                            "video" | "audio" => {
                                // `"0"`: a URL written without a key (`[audio URL]`).
                                let keys: &[&str] = if name == "video" {
                                    &["src", "mp4", "m4v", "webm", "ogv", "flv", "0"]
                                } else {
                                    &["src", "mp3", "ogg", "flac", "m4a", "wav", "wma", "0"]
                                };
                                let src = keys
                                    .iter()
                                    .find_map(|k| attrs.get(*k).filter(|v| !v.trim().is_empty()));
                                src.is_some_and(|s| self.media_link(s.trim(), false))
                            }
                            _ => false,
                        };
                    if handled {
                        continue;
                    }
                    // Kept as written (WP leaves unregistered shortcodes alone), its content
                    // expanded.
                    self.notes.unknown_shortcode(&name);
                    self.out.push_str(&open);
                    if let Some(inner) = inner {
                        if depth < MAX_DEPTH {
                            self.expand(&inner, depth + 1);
                        } else {
                            self.out.push_str(&inner);
                        }
                        self.out.push_str("[/");
                        self.out.push_str(&name);
                        self.out.push(']');
                    }
                }
            }
        }
    }

    /// `[caption]`: the image (with its link) and the caption text after it, or the
    /// `caption` attribute, as a figure (`img_caption_shortcode`). Without an image the
    /// content stays.
    fn caption(&mut self, attr: Option<&String>, inner: Option<&str>, depth: usize) -> bool {
        let Some(inner) = inner else {
            return false;
        };
        let Some((image, rest)) = split_image(inner) else {
            self.expand(inner, depth + 1);
            return true;
        };
        self.out.push_str("<figure>");
        self.out.push_str(image);
        let caption = attr.map_or(rest, String::as_str).trim();
        if !caption.is_empty() {
            self.out.push_str("<figcaption>");
            self.expand(caption, depth + 1);
            self.out.push_str("</figcaption>");
        }
        self.out.push_str("</figure>");
        true
    }

    /// `[gallery ids="1,2"]`: an image per imported attachment; one not imported is counted
    /// as missing. A gallery without ids (the item's attached images) is not handled.
    fn gallery(&mut self, ids: Option<&str>) -> bool {
        let ids: Vec<i64> = ids
            .unwrap_or("")
            .split(',')
            .filter_map(|id| id.trim().parse().ok())
            .collect();
        if ids.is_empty() {
            return false;
        }
        for id in ids {
            match self.rw.attachment(id) {
                Some(media) => {
                    self.out.push_str("<img src=\"");
                    self.out.push_str(&self.gallery_src);
                    self.out.push_str(&self.media.len().to_string());
                    self.out.push_str("\">");
                    self.media.push(media);
                }
                None => self.notes.missing_media(),
            }
        }
        true
    }

    /// An embed (`[embed]`, YouTube/Vimeo) or a link to `url` in a paragraph of its own.
    fn media_link(&mut self, url: &str, embed: bool) -> bool {
        let url = url
            .replace(MASK_OPEN, "[")
            .replace(MASK_CLOSE, "]")
            .replace('\0', "");
        if url.is_empty() {
            return false;
        }
        if embed {
            self.out.push_str(&embed_html(&url, true));
        } else {
            self.out
                .push_str(&format!("<p><a href=\"{0}\">{0}</a></p>", escape(&url)));
        }
        true
    }
}

/// `[caption]` content split into the image, with a link around it, and the text after it
/// (WP's `((?:<a [^>]+>\s*)?<img [^>]+>(?:\s*</a>)?)(.*)`).
fn split_image(inner: &str) -> Option<(&str, &str)> {
    let lower = inner.to_ascii_lowercase();
    let img = lower.find("<img")?;
    let img_end = img + lower[img..].find('>')? + 1;
    let before = lower[..img].trim_end();
    let start = match before.rfind("<a ") {
        Some(a) if before.ends_with('>') && !before[a + 1..].contains('<') => a,
        _ => img,
    };
    let mut end = img_end;
    if start < img {
        let after = lower[img_end..].trim_start();
        if after.starts_with("</a>") {
            end = lower.len() - after.len() + 4;
        }
    }
    Some((&inner[start..end], &inner[end..]))
}

/// Resolves the gallery images' `src`; everything else goes to the caller's [`Rewrite`].
struct Gallery<'a> {
    rw: &'a dyn Rewrite,
    src: String,
    media: Vec<Uuid>,
}

impl Rewrite for Gallery<'_> {
    fn image(&self, src: &str) -> ImageRef {
        src.strip_prefix(self.src.as_str())
            .and_then(|i| i.parse::<usize>().ok())
            .and_then(|i| self.media.get(i))
            .map_or_else(|| self.rw.image(src), |id| ImageRef::Media(*id))
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

    struct Rw;

    impl Rewrite for Rw {
        fn image(&self, src: &str) -> ImageRef {
            ImageRef::Url(src.to_owned())
        }

        fn attachment(&self, wp_id: i64) -> Option<Uuid> {
            (wp_id < 100).then(|| Uuid::from_u128(u128::try_from(wp_id).unwrap()))
        }

        fn href(&self, href: &str) -> String {
            href.to_owned()
        }
    }

    fn conv(html: &str) -> (Vec<Node>, Notes) {
        let mut notes = Notes::default();
        (to_nodes(html, &Rw, &mut notes), notes)
    }

    fn t(s: &str) -> Node {
        Node::Text {
            text: s.to_owned(),
            marks: vec![],
        }
    }

    fn link_p(url: &str) -> Node {
        Node::Paragraph(vec![Node::Text {
            text: url.to_owned(),
            marks: vec![Mark::Link {
                href: url.to_owned(),
                target: None,
            }],
        }])
    }

    fn image(src: &str, alt: Option<&str>, caption: Option<&str>) -> Node {
        Node::Image {
            src: src.to_owned(),
            alt: alt.map(str::to_owned),
            title: None,
            caption: caption.map(str::to_owned),
        }
    }

    #[test]
    fn caption_from_the_text_after_the_image() {
        let (nodes, _) = conv(
            "<p>[caption id=\"attachment_1\" width=\"300\"]<a href=\"/big.jpg\"> <img src=\"/a.jpg\" alt=\"A\" /> </a> The <b>harbour</b>[/caption]</p>",
        );
        assert_eq!(nodes, [image("/a.jpg", Some("A"), Some("The harbour"))]);
    }

    #[test]
    fn caption_attribute_wins_and_no_image_keeps_the_content() {
        let (nodes, _) = conv(
            "[caption caption=\"Attr cap\"]<img src=\"/a.jpg\">[/caption]\
             <p>[caption]just text[/caption]</p>",
        );
        assert_eq!(
            nodes,
            [
                image("/a.jpg", None, Some("Attr cap")),
                Node::Paragraph(vec![t("just text")])
            ]
        );
    }

    #[test]
    fn gallery_by_attachment_ids() {
        let (nodes, notes) = conv("<p>[gallery columns=\"2\" ids=\"2, 3,x,500\"]</p>");
        let m = |i: u128| format!("ferrite:media/{}", Uuid::from_u128(i));
        assert_eq!(nodes, [image(&m(2), None, None), image(&m(3), None, None)]);
        assert_eq!(notes.missing_media.count, 1);
        let (nodes, notes) = conv("<p>[gallery include=\"4\"] [gallery]</p>");
        assert_eq!(
            nodes,
            [
                image(&m(4), None, None),
                Node::Paragraph(vec![t("[gallery]")])
            ]
        );
        assert_eq!(notes.unknown_shortcodes["gallery"].count, 1);
    }

    #[test]
    fn a_gallery_src_written_in_the_body_is_not_resolved() {
        let (nodes, _) =
            conv("<img src=\"/a.png\"><img src=\"ferrite-import:gallery-/0\">[gallery ids=\"7\"]");
        assert_eq!(
            nodes,
            [
                image("/a.png", None, None),
                image(&format!("ferrite:media/{}", Uuid::from_u128(7)), None, None)
            ]
        );
    }

    #[test]
    fn embeds_and_media_links() {
        let (nodes, notes) = conv(
            "[embed]https://youtu.be/dQw4w9WgXcQ[/embed]\
             [embed]https://example.org/post[/embed]\
             [embed src=\"https://vimeo.com/1\"][/embed]\
             [video mp4=\"https://cdn.example/v.mp4\"][/video]\
             [audio src=\"/a.mp3\"]\
             [audio https://cdn.example/b.mp3]\
             [video]",
        );
        assert_eq!(
            nodes,
            [
                Node::Embed {
                    provider: "youtube".into(),
                    url: "https://www.youtube.com/watch?v=dQw4w9WgXcQ".into()
                },
                link_p("https://example.org/post"),
                Node::Embed {
                    provider: "vimeo".into(),
                    url: "https://vimeo.com/1".into()
                },
                link_p("https://cdn.example/v.mp4"),
                link_p("/a.mp3"),
                link_p("https://cdn.example/b.mp3"),
                Node::Paragraph(vec![t("[video]")]),
            ]
        );
        assert_eq!(notes.unknown_shortcodes["video"].count, 1);
        assert!(notes.unsupported_tags.is_empty());
    }

    #[test]
    fn unknown_shortcodes_stay_as_written_with_known_ones_inside_expanded() {
        let (nodes, notes) =
            conv("<p>[box  Title='x']a [embed]https://vimeo.com/2[/embed] b[/box] [[gallery]]</p>");
        assert_eq!(
            nodes,
            [
                Node::Paragraph(vec![t("[box Title='x']a")]),
                Node::Embed {
                    provider: "vimeo".into(),
                    url: "https://vimeo.com/2".into()
                },
                Node::Paragraph(vec![t("b[/box] [gallery]")]),
            ]
        );
        assert_eq!(notes.unknown_shortcodes.len(), 1);
        assert_eq!(notes.unknown_shortcodes["box"].count, 1);
    }

    #[test]
    fn shortcodes_in_tags_comments_and_pre_are_left_alone() {
        let (nodes, notes) = conv(
            "<p title=\"[gallery ids=1]\">x</p><!-- [embed]https://vimeo.com/3[/embed] -->\
             <pre class=\"x\">[gallery ids=\"1\"] [[b]]</pre><PRE>[embed]a[/embed]</PRE>",
        );
        assert_eq!(
            nodes,
            [
                Node::Paragraph(vec![t("x")]),
                Node::CodeBlock {
                    language: None,
                    text: "[gallery ids=\"1\"] [[b]]".into()
                },
                Node::CodeBlock {
                    language: None,
                    text: "[embed]a[/embed]".into()
                },
            ]
        );
        assert_eq!(notes, Notes::default());
    }

    #[test]
    fn autoembed_bare_url_lines() {
        assert_eq!(
            autoembed(
                "a\n  https://www.youtube.com/watch?v=aqz-KE-bpKQ \r\nhttps://example.org/x\n\
                 text https://vimeo.com/1\n<pre>\nhttps://vimeo.com/2\n</pre>\n\
                 [embed]\nhttps://vimeo.com/3\n[/embed]\nHTTPS://vimeo.com/4"
            ),
            "a\n<iframe src=\"https://www.youtube.com/watch?v=aqz-KE-bpKQ\"></iframe>\n\
             <a href=\"https://example.org/x\">https://example.org/x</a>\n\
             text https://vimeo.com/1\n<pre>\nhttps://vimeo.com/2\n</pre>\n\
             [embed]\nhttps://vimeo.com/3\n[/embed]\n<iframe src=\"https://vimeo.com/4\"></iframe>"
        );
        assert_eq!(autoembed("https://\nhttp://a\"b"), "https://\nhttp://a\"b");
        assert_eq!(
            autoembed("[embedpress id=1]\nhttps://vimeo.com/5\n<preview>\nhttps://vimeo.com/6"),
            "[embedpress id=1]\n<iframe src=\"https://vimeo.com/5\"></iframe>\n<preview>\n\
             <iframe src=\"https://vimeo.com/6\"></iframe>"
        );
    }

    #[test]
    fn nul_never_unmasks() {
        let (nodes, _) = conv("<p a=\"[\0x]\">\0[\u{1}y]</p>");
        assert_eq!(nodes, [Node::Paragraph(vec![t("[\u{1}y]")])]);
    }

    #[test]
    fn deep_nesting_is_capped() {
        let html = format!(
            "{}x{}",
            (0..1000).map(|i| format!("[n{i}]")).collect::<String>(),
            (0..1000)
                .rev()
                .map(|i| format!("[/n{i}]"))
                .collect::<String>()
        );
        let (nodes, notes) = conv(&html);
        assert_eq!(nodes.len(), 1);
        assert_eq!(notes.unknown_shortcodes.len(), MAX_DEPTH + 1);
    }

    #[test]
    fn hostile_input_stays_linear() {
        let inputs = [
            "<a".repeat(200_000),
            "<pre>".repeat(100_000),
            "<pre>[x]</pre".repeat(100_000),
            "<!--".repeat(100_000),
            "[caption]<img".repeat(50_000),
            "[x".repeat(200_000),
            "https://a\n".repeat(100_000),
        ];
        // The scans here; html5ever's tree building is not (deep nesting is quadratic there).
        for html in inputs {
            let start = std::time::Instant::now();
            let mut notes = Notes::default();
            let mut x = Expander {
                rw: &Rw,
                notes: &mut notes,
                gallery_src: GALLERY_SRC.to_owned(),
                media: Vec::new(),
                out: String::new(),
            };
            x.expand(&mask(&html), 0);
            let _ = autoembed(&html);
            assert!(start.elapsed() < std::time::Duration::from_secs(10));
        }
    }
}
