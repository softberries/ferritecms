//! The streaming WXR reader.
//!
//! One pass over the file with `quick_xml::Reader`; records go to the sink as soon as they
//! close, so memory is bounded by the largest single item.
//!
//! Elements are matched by their literal qualified names (`wp:`, `content:`, `excerpt:`,
//! `dc:`), the prefixes every WordPress export writes; namespace URIs are not resolved.
//! WordPress writes the channel in a fixed order (header fields, authors, terms, items), and
//! `<channel>` fields that come after the first author, term or item are ignored.
//!
//! Safety (ruling P22): a DOCTYPE is refused, so no entity is ever declared, let alone
//! expanded; only the five predefined entities and character references are resolved. One
//! node (text, CDATA, tag, comment or PI) over 64 MiB is refused before it is buffered, and
//! so are elements nested deeper than 256 and invalid UTF-8, with its byte offset.

use std::io::{self, BufRead, Read};

use quick_xml::XmlVersion;
use quick_xml::events::{BytesStart, Event};
use quick_xml::reader::Reader;

use crate::error::ImportError;
use crate::model::{Author, Channel, Comment, Item, ItemTerm, Record, Term};

/// The largest text node read, in bytes.
const MAX_TEXT: usize = 64 * 1024 * 1024;

/// Bytes one event may read beyond the text cap: the markup around a text node at the cap.
const EVENT_SLACK: usize = 64 * 1024;

/// The deepest element nesting read.
const MAX_DEPTH: usize = 256;

/// What [`parse`] read.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ParseStats {
    pub authors: u64,
    pub terms: u64,
    pub items: u64,
    pub comments: u64,
}

/// Reads a WXR file and hands every record to `sink`, in file order, the [`Channel`] first.
/// An error from `sink` stops the parse and is returned as is.
///
/// The per-node cap is checked at each refill of `r`, so a `BufReader` overshoots it by at
/// most one buffer; an in-memory slice is a single refill and is copied before the check.
pub fn parse<R: BufRead>(
    r: R,
    sink: impl FnMut(Record) -> Result<(), ImportError>,
) -> Result<ParseStats, ImportError> {
    parse_with_limit(r, sink, MAX_TEXT)
}

fn parse_with_limit<R: BufRead>(
    r: R,
    sink: impl FnMut(Record) -> Result<(), ImportError>,
    max_text: usize,
) -> Result<ParseStats, ImportError> {
    let mut reader = Reader::from_reader(Capped {
        inner: r,
        used: 0,
        limit: max_text.saturating_add(EVENT_SLACK),
        exceeded: false,
    });
    let config = reader.config_mut();
    config.trim_text(false);
    config.check_end_names = true;
    config.allow_dangling_amp = false;

    let mut p = Parser {
        sink,
        max_text,
        stack: Vec::new(),
        text: String::new(),
        offset: 0,
        channel: None,
        channel_emitted: false,
        author: None,
        term: None,
        item: None,
        item_term: None,
        meta: None,
        comment: None,
        stats: ParseStats::default(),
    };
    let mut buf = Vec::new();
    loop {
        p.offset = reader.buffer_position();
        reader.get_mut().used = 0;
        let event = match reader.read_event_into(&mut buf) {
            Ok(event) => event,
            Err(_) if reader.get_ref().exceeded => {
                return Err(ImportError::TooLarge { offset: p.offset });
            }
            Err(quick_xml::Error::Io(e)) => {
                return Err(ImportError::Io {
                    offset: reader.buffer_position(),
                    msg: e.to_string(),
                });
            }
            Err(e) => {
                return Err(ImportError::Xml {
                    offset: reader.error_position(),
                    msg: e.to_string(),
                });
            }
        };
        match event {
            Event::Start(e) => {
                utf8(&e, p.offset + 1)?;
                p.start(&e)?;
            }
            Event::End(e) => {
                utf8(&e, p.offset + 2)?;
                p.end()?;
            }
            Event::Text(e) => {
                utf8(&e, p.offset)?;
                p.push_text(&e.xml10_content().map_err(|e| p.xml(e))?)?;
            }
            Event::CData(e) => {
                utf8(&e, p.offset + "<![CDATA[".len() as u64)?;
                p.push_text(&e.xml10_content().map_err(|e| p.xml(e))?)?;
            }
            Event::GeneralRef(e) => {
                utf8(&e, p.offset + 1)?;
                let c = match e.resolve_char_ref().map_err(|e| p.xml(e))? {
                    Some(c) => c,
                    None => match &*e {
                        b"lt" => '<',
                        b"gt" => '>',
                        b"amp" => '&',
                        b"apos" => '\'',
                        b"quot" => '"',
                        _ => return Err(p.xml("undeclared entity")),
                    },
                };
                p.push_text(c.encode_utf8(&mut [0; 4]))?;
            }
            Event::DocType(_) => return Err(ImportError::Doctype),
            Event::Comment(e) => utf8(&e, p.offset + "<!--".len() as u64)?,
            Event::PI(e) => utf8(&e, p.offset + "<?".len() as u64)?,
            Event::Decl(e) => utf8(&e, p.offset + "<?".len() as u64)?,
            Event::Empty(e) => {
                utf8(&e, p.offset + 1)?;
                p.start(&e)?;
                p.end()?;
            }
            Event::Eof => break,
        }
        buf.clear();
    }
    p.offset = reader.buffer_position();
    if let Some(open) = p.stack.last() {
        let msg = format!(
            "unexpected end of file inside <{}>",
            String::from_utf8_lossy(open)
        );
        return Err(p.xml(msg));
    }
    if !p.channel_emitted {
        return Err(p.missing("channel"));
    }
    Ok(p.stats)
}

/// Refuses invalid UTF-8 in an event's raw bytes; `at` is the offset of those bytes.
fn utf8(raw: &[u8], at: u64) -> Result<(), ImportError> {
    std::str::from_utf8(raw)
        .map(drop)
        .map_err(|e| ImportError::Utf8 {
            offset: at + e.valid_up_to() as u64,
        })
}

/// Counts the bytes one event reads and fails the read once they pass `limit`, so a huge node
/// is refused before quick-xml has buffered it. `used` is reset before every event.
struct Capped<R> {
    inner: R,
    used: usize,
    limit: usize,
    exceeded: bool,
}

impl<R: BufRead> Read for Capped<R> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        let n = {
            let available = self.fill_buf()?;
            let n = available.len().min(out.len());
            out[..n].copy_from_slice(&available[..n]);
            n
        };
        self.consume(n);
        Ok(n)
    }
}

impl<R: BufRead> BufRead for Capped<R> {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        if self.used > self.limit {
            self.exceeded = true;
            return Err(io::Error::other("node larger than the cap"));
        }
        self.inner.fill_buf()
    }

    fn consume(&mut self, n: usize) {
        self.used = self.used.saturating_add(n);
        self.inner.consume(n);
    }
}

struct Parser<S> {
    sink: S,
    max_text: usize,
    /// Qualified names of the open elements.
    stack: Vec<Vec<u8>>,
    /// The text of the innermost element since its start tag.
    text: String,
    /// Offset of the current event.
    offset: u64,
    channel: Option<Channel>,
    channel_emitted: bool,
    author: Option<Author>,
    term: Option<TermDraft>,
    item: Option<ItemDraft>,
    item_term: Option<ItemTerm>,
    meta: Option<(String, String)>,
    comment: Option<Comment>,
    stats: ParseStats,
}

#[derive(Default)]
struct TermDraft {
    term: Term,
    has_slug: bool,
}

#[derive(Default)]
struct ItemDraft {
    item: Item,
    has_id: bool,
    has_type: bool,
}

impl<S: FnMut(Record) -> Result<(), ImportError>> Parser<S> {
    fn xml(&self, msg: impl ToString) -> ImportError {
        ImportError::Xml {
            offset: self.offset,
            msg: msg.to_string(),
        }
    }

    fn missing(&self, what: &'static str) -> ImportError {
        ImportError::Missing {
            what,
            offset: self.offset,
        }
    }

    fn push_text(&mut self, s: &str) -> Result<(), ImportError> {
        if self.text.len() + s.len() > self.max_text {
            return Err(ImportError::TooLarge {
                offset: self.offset,
            });
        }
        self.text.push_str(s);
        Ok(())
    }

    fn parent_is(&self, name: &[u8]) -> bool {
        self.stack.last().is_some_and(|n| n == name)
    }

    fn start(&mut self, e: &BytesStart<'_>) -> Result<(), ImportError> {
        if self.stack.len() >= MAX_DEPTH {
            return Err(self.xml(format!("elements nested deeper than {MAX_DEPTH}")));
        }
        let name = e.name().as_ref().to_vec();
        self.text.clear();
        if self.parent_is(b"channel") && self.channel.is_some() {
            match name.as_slice() {
                b"wp:author" | b"wp:category" | b"wp:tag" | b"wp:term" | b"item" => {
                    self.emit_channel()?
                }
                _ => {}
            }
        }
        match name.as_slice() {
            b"channel" if self.parent_is(b"rss") && !self.channel_emitted => {
                self.channel = Some(Channel::default());
            }
            b"wp:author" if self.parent_is(b"channel") => self.author = Some(Author::default()),
            b"wp:category" if self.parent_is(b"channel") => {
                self.term = Some(TermDraft::new("category"));
            }
            b"wp:tag" if self.parent_is(b"channel") => {
                self.term = Some(TermDraft::new("post_tag"));
            }
            b"wp:term" if self.parent_is(b"channel") => self.term = Some(TermDraft::new("")),
            b"item" if self.parent_is(b"channel") => self.item = Some(ItemDraft::default()),
            b"category" if self.parent_is(b"item") && self.item.is_some() => {
                let mut t = ItemTerm::default();
                for a in e.attributes() {
                    let a = a.map_err(|err| self.xml(err))?;
                    let v = a
                        .normalized_value(XmlVersion::Implicit1_0)
                        .map_err(|err| self.xml(err))?;
                    match a.key.as_ref() {
                        b"domain" => t.taxonomy = v.into_owned(),
                        b"nicename" => t.slug = v.into_owned(),
                        _ => {}
                    }
                }
                self.item_term = Some(t);
            }
            b"wp:postmeta" if self.parent_is(b"item") && self.item.is_some() => {
                self.meta = Some((String::new(), String::new()));
            }
            b"wp:comment" if self.parent_is(b"item") && self.item.is_some() => {
                self.comment = Some(Comment::default());
            }
            _ => {}
        }
        self.stack.push(name);
        Ok(())
    }

    fn end(&mut self) -> Result<(), ImportError> {
        let Some(name) = self.stack.pop() else {
            return Err(self.xml("end tag without a start tag"));
        };
        let text = std::mem::take(&mut self.text);
        let parent = self.stack.last().map(Vec::as_slice).unwrap_or_default();
        match (parent, name.as_slice()) {
            (b"rss", b"channel") => {
                if self.channel.is_some() {
                    self.emit_channel()?;
                }
            }
            (b"channel", b"wp:author") => self.finish_author()?,
            (b"channel", b"wp:category" | b"wp:tag" | b"wp:term") => self.finish_term()?,
            (b"channel", b"item") => self.finish_item()?,
            (b"channel", field) => self.channel_field(field, text),
            (b"wp:author", field) => self.author_field(field, text)?,
            (b"wp:category" | b"wp:tag" | b"wp:term", field) => self.term_field(field, text)?,
            (b"item", b"category") => {
                if let (Some(mut t), Some(d)) = (self.item_term.take(), self.item.as_mut()) {
                    t.name = text;
                    d.item.terms.push(t);
                }
            }
            (b"item", b"wp:postmeta") => {
                if let (Some(m), Some(d)) = (self.meta.take(), self.item.as_mut()) {
                    d.item.meta.push(m);
                }
            }
            (b"item", b"wp:comment") => {
                if let (Some(c), Some(d)) = (self.comment.take(), self.item.as_mut()) {
                    d.item.comments.push(c);
                    self.stats.comments += 1;
                }
            }
            (b"item", field) => self.item_field(field, text)?,
            (b"wp:postmeta", field) => {
                if let Some(m) = self.meta.as_mut() {
                    match field {
                        b"wp:meta_key" => m.0 = text,
                        b"wp:meta_value" => m.1 = text,
                        _ => {}
                    }
                }
            }
            (b"wp:comment", field) => self.comment_field(field, text)?,
            _ => {}
        }
        Ok(())
    }

    fn emit_channel(&mut self) -> Result<(), ImportError> {
        if let Some(c) = self.channel.take() {
            self.channel_emitted = true;
            (self.sink)(Record::Channel(c))?;
        }
        Ok(())
    }

    fn channel_field(&mut self, field: &[u8], text: String) {
        let Some(c) = self.channel.as_mut() else {
            return;
        };
        match field {
            b"title" => c.title = text,
            b"link" => c.link = text,
            b"description" => c.description = text,
            b"language" => c.language = text,
            b"wp:wxr_version" => c.wxr_version = text,
            b"wp:base_site_url" => c.base_site_url = text,
            b"wp:base_blog_url" => c.base_blog_url = text,
            _ => {}
        }
    }

    fn author_field(&mut self, field: &[u8], text: String) -> Result<(), ImportError> {
        let id = match field {
            b"wp:author_id" => Some(self.int(&text, "wp:author_id")?),
            _ => None,
        };
        let Some(a) = self.author.as_mut() else {
            return Ok(());
        };
        match field {
            b"wp:author_id" => a.wp_id = id.unwrap_or_default(),
            b"wp:author_login" => a.login = text,
            b"wp:author_email" => a.email = text,
            b"wp:author_display_name" => a.display_name = text,
            b"wp:author_first_name" => a.first_name = text,
            b"wp:author_last_name" => a.last_name = text,
            _ => {}
        }
        Ok(())
    }

    fn finish_author(&mut self) -> Result<(), ImportError> {
        let Some(a) = self.author.take() else {
            return Ok(());
        };
        if a.login.is_empty() {
            return Err(self.missing("wp:author_login"));
        }
        self.stats.authors += 1;
        (self.sink)(Record::Author(a))
    }

    fn term_field(&mut self, field: &[u8], text: String) -> Result<(), ImportError> {
        let id = match field {
            b"wp:term_id" => Some(self.int(&text, "wp:term_id")?),
            _ => None,
        };
        let Some(d) = self.term.as_mut() else {
            return Ok(());
        };
        let t = &mut d.term;
        match field {
            b"wp:term_id" => t.wp_id = id.unwrap_or_default(),
            b"wp:term_taxonomy" => t.taxonomy = text,
            b"wp:category_nicename" | b"wp:tag_slug" | b"wp:term_slug" => {
                d.has_slug = !text.is_empty();
                t.slug = text;
            }
            b"wp:cat_name" | b"wp:tag_name" | b"wp:term_name" => t.name = text,
            b"wp:category_description" | b"wp:tag_description" | b"wp:term_description" => {
                t.description = text;
            }
            b"wp:category_parent" | b"wp:term_parent" => {
                t.parent_slug = (!text.is_empty()).then_some(text);
            }
            _ => {}
        }
        Ok(())
    }

    fn finish_term(&mut self) -> Result<(), ImportError> {
        let Some(d) = self.term.take() else {
            return Ok(());
        };
        if !d.has_slug {
            return Err(self.missing("term slug"));
        }
        if d.term.taxonomy.is_empty() {
            return Err(self.missing("wp:term_taxonomy"));
        }
        self.stats.terms += 1;
        (self.sink)(Record::Term(d.term))
    }

    fn item_field(&mut self, field: &[u8], text: String) -> Result<(), ImportError> {
        let num = match field {
            b"wp:post_id" if !text.trim().is_empty() => Some(self.int(&text, "wp:post_id")?),
            b"wp:post_parent" => Some(self.int(&text, "wp:post_parent")?),
            b"wp:menu_order" => Some(self.int(&text, "wp:menu_order")?),
            _ => None,
        };
        let Some(d) = self.item.as_mut() else {
            return Ok(());
        };
        let i = &mut d.item;
        match field {
            b"wp:post_id" => {
                if let Some(n) = num {
                    i.wp_id = n;
                    d.has_id = true;
                }
            }
            b"wp:post_parent" => i.parent_wp_id = num.unwrap_or_default(),
            b"wp:menu_order" => {
                i.menu_order =
                    i32::try_from(num.unwrap_or_default()).map_err(|_| ImportError::Xml {
                        offset: self.offset,
                        msg: "wp:menu_order is out of range".into(),
                    })?;
            }
            b"wp:post_type" => {
                d.has_type = !text.is_empty();
                i.post_type = text;
            }
            b"title" => i.title = text,
            b"link" => i.link = text,
            b"guid" => i.guid = text,
            b"dc:creator" => i.creator_login = text,
            b"content:encoded" => i.content = text,
            b"excerpt:encoded" => i.excerpt = text,
            b"wp:status" => i.status = text,
            b"wp:post_name" => i.post_name = text,
            b"wp:post_date" => i.date_local = text,
            b"wp:post_date_gmt" => i.date_gmt = text,
            b"wp:post_modified_gmt" => i.modified_gmt = text,
            b"wp:comment_status" => i.comment_status = text,
            b"wp:is_sticky" => i.is_sticky = text.trim() == "1",
            b"wp:attachment_url" => i.attachment_url = Some(text),
            _ => {}
        }
        Ok(())
    }

    fn finish_item(&mut self) -> Result<(), ImportError> {
        let Some(d) = self.item.take() else {
            return Ok(());
        };
        if !d.has_id {
            return Err(self.missing("wp:post_id"));
        }
        if !d.has_type {
            return Err(self.missing("wp:post_type"));
        }
        self.stats.items += 1;
        (self.sink)(Record::Item(Box::new(d.item)))
    }

    fn comment_field(&mut self, field: &[u8], text: String) -> Result<(), ImportError> {
        let num = match field {
            b"wp:comment_id" => Some(self.int(&text, "wp:comment_id")?),
            b"wp:comment_parent" => Some(self.int(&text, "wp:comment_parent")?),
            b"wp:comment_user_id" => Some(self.int(&text, "wp:comment_user_id")?),
            _ => None,
        };
        let Some(c) = self.comment.as_mut() else {
            return Ok(());
        };
        let num = num.unwrap_or_default();
        match field {
            b"wp:comment_id" => c.wp_id = num,
            b"wp:comment_parent" => c.parent_wp_id = num,
            b"wp:comment_user_id" => c.user_id = num,
            b"wp:comment_author" => c.author = text,
            b"wp:comment_author_email" => c.author_email = text,
            b"wp:comment_author_url" => c.author_url = text,
            b"wp:comment_date" => c.date_local = text,
            b"wp:comment_date_gmt" => c.date_gmt = text,
            b"wp:comment_content" => c.content = text,
            b"wp:comment_approved" => c.approved = text,
            b"wp:comment_type" => c.comment_type = text,
            _ => {}
        }
        Ok(())
    }

    /// An integer field; empty means 0.
    fn int(&self, text: &str, what: &str) -> Result<i64, ImportError> {
        let t = text.trim();
        if t.is_empty() {
            return Ok(0);
        }
        t.parse()
            .map_err(|_| self.xml(format!("{what} is not an integer")))
    }
}

impl TermDraft {
    fn new(taxonomy: &str) -> Self {
        TermDraft {
            term: Term {
                taxonomy: taxonomy.to_owned(),
                ..Term::default()
            },
            has_slug: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_over_the_limit_is_refused() {
        let doc = "<rss><channel><title>0123456789</title></channel></rss>";
        let ok = parse_with_limit(doc.as_bytes(), |_| Ok(()), 10);
        assert!(ok.is_ok(), "{ok:?}");
        let err = parse_with_limit(doc.as_bytes(), |_| Ok(()), 9);
        assert!(matches!(err, Err(ImportError::TooLarge { .. })), "{err:?}");
    }

    #[test]
    fn cdata_chunks_count_together() {
        let doc = "<rss><channel><title><![CDATA[01234]]><![CDATA[56789]]></title></channel></rss>";
        let err = parse_with_limit(doc.as_bytes(), |_| Ok(()), 9);
        assert!(matches!(err, Err(ImportError::TooLarge { .. })), "{err:?}");
    }

    /// A source that yields `prefix` and then `fill` forever, counting what was read.
    struct Endless {
        prefix: &'static [u8],
        fill: u8,
        read: std::rc::Rc<std::cell::Cell<usize>>,
    }

    impl std::io::Read for Endless {
        fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
            let n = if self.prefix.is_empty() {
                out.fill(self.fill);
                out.len()
            } else {
                let n = self.prefix.len().min(out.len());
                out[..n].copy_from_slice(&self.prefix[..n]);
                self.prefix = &self.prefix[n..];
                n
            };
            self.read.set(self.read.get() + n);
            Ok(n)
        }
    }

    fn endless(prefix: &'static [u8], fill: u8) -> Result<ParseStats, ImportError> {
        let read = std::rc::Rc::new(std::cell::Cell::new(0));
        let src = std::io::BufReader::new(Endless {
            prefix,
            fill,
            read: read.clone(),
        });
        let res = parse_with_limit(src, |_| Ok(()), 1024);
        // The cap, the slack and one buffer refill at most.
        assert!(
            read.get() < 1024 + EVENT_SLACK + 64 * 1024,
            "read {}",
            read.get()
        );
        res
    }

    #[test]
    fn endless_nodes_stop_at_the_cap() {
        for (prefix, fill) in [
            (&b"<rss><channel><title>"[..], b'a'),
            (b"<rss><channel><title><![CDATA[", b'a'),
            (b"<rss><!--", b'a'),
            (b"<?pi ", b'a'),
            (b"<rss a=\"", b'a'),
            (b"<rss", b'a'),
        ] {
            let res = endless(prefix, fill);
            assert!(
                matches!(res, Err(ImportError::TooLarge { .. })),
                "{}: {res:?}",
                String::from_utf8_lossy(prefix)
            );
        }
    }

    #[test]
    fn read_failure_is_io() {
        struct Failing(&'static [u8]);
        impl std::io::Read for Failing {
            fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
                if self.0.is_empty() {
                    return Err(std::io::Error::other("disk on fire"));
                }
                let n = self.0.len().min(out.len());
                out[..n].copy_from_slice(&self.0[..n]);
                self.0 = &self.0[n..];
                Ok(n)
            }
        }
        let src = std::io::BufReader::new(Failing(b"<rss><channel><title>abc"));
        let res = parse(src, |_| Ok(()));
        match res {
            Err(ImportError::Io { msg, .. }) => assert!(msg.contains("disk on fire"), "{msg}"),
            other => panic!("expected Io, got {other:?}"),
        }
    }
}
