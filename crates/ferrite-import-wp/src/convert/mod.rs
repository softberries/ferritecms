//! Content conversion: WordPress post bodies to the stored block tree.
//!
//! [`body::convert`] is the entry point. [`gutenberg::parse`] splits a block-editor body into
//! blocks, which [`blocks`] maps; classic bodies go through [`autop::wpautop`] and
//! [`shortcodes`] (over [`shortcode::scan`]); [`html::convert`] turns HTML into
//! [`doc::Node`]s, asking a [`Rewrite`] for media and link targets and counting what it drops
//! in [`Notes`].

use std::collections::BTreeMap;

use uuid::Uuid;

pub mod autop;
pub mod blocks;
pub mod body;
mod depth;
pub mod doc;
pub mod gutenberg;
pub mod html;
pub mod shortcode;
pub mod shortcodes;

/// A byte of a shortcode or block name: ASCII alphanumeric, `_` or `-`.
pub(crate) fn is_name_byte(c: u8) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-')
}

/// PCRE's `\s` without the `u` flag (WordPress's regexes here have none).
pub(crate) fn is_pcre_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\x0b' | '\x0c' | '\r')
}

/// Where an image `src` points after the import (P14).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageRef {
    /// An imported media item: the node's `src` becomes `ferrite:media/<uuid>`.
    Media(Uuid),
    /// Not a site upload: kept as is.
    Url(String),
    /// A site upload that was not imported: the old URL is kept and counted (P13).
    Missing(String),
}

/// Maps old URLs to new ones while converting.
pub trait Rewrite {
    fn image(&self, src: &str) -> ImageRef;
    /// The imported media item of attachment `wp_id` (a block's `attrs.id`, `[gallery ids]`),
    /// or `None` when that attachment was not imported.
    fn attachment(&self, wp_id: i64) -> Option<Uuid>;
    /// The new target of a link (an attachment's public URL, an item's new path), or `href`.
    fn href(&self, href: &str) -> String;
}

/// A memoised forward search for one needle. While the positions asked for do not move
/// back past a match, every byte is searched at most once, so a scanner that asks again
/// from each candidate stays linear on input with no match left.
pub(crate) struct NextMatch<'a> {
    hay: &'a str,
    needle: &'a str,
    /// The last search: where it started and what it found.
    last: Option<(usize, Option<usize>)>,
}

impl<'a> NextMatch<'a> {
    pub(crate) fn new(hay: &'a str, needle: &'a str) -> Self {
        Self {
            hay,
            needle,
            last: None,
        }
    }

    /// The first match at or after `from`.
    pub(crate) fn find(&mut self, from: usize) -> Option<usize> {
        if let Some((searched, found)) = self.last
            && searched <= from
        {
            match found {
                None => return None,
                Some(c) if c >= from => return Some(c),
                Some(_) => {}
            }
        }
        let found = self
            .hay
            .get(from..)
            .and_then(|h| h.find(self.needle))
            .map(|r| from + r);
        self.last = Some((from, found));
        found
    }
}

/// At most this many example item ids are kept per note.
pub const MAX_EXAMPLES: usize = 5;

/// How often something happened, and in which items (the first [`MAX_EXAMPLES`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Tally {
    pub count: u64,
    pub items: Vec<i64>,
}

impl Tally {
    fn bump(&mut self, item: Option<i64>) {
        self.count += 1;
        if let Some(id) = item
            && self.items.len() < MAX_EXAMPLES
            && !self.items.contains(&id)
        {
            self.items.push(id);
        }
    }

    fn merge(&mut self, other: &Tally) {
        self.count += other.count;
        for id in &other.items {
            if self.items.len() < MAX_EXAMPLES && !self.items.contains(id) {
                self.items.push(*id);
            }
        }
    }
}

fn merge_map(into: &mut BTreeMap<String, Tally>, from: &BTreeMap<String, Tally>) {
    for (k, t) in from {
        into.entry(k.clone()).or_default().merge(t);
    }
}

/// What the conversion lost or could not resolve, for the import report.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Notes {
    /// The WP id of the item being converted; recorded as an example with every count.
    pub item: Option<i64>,
    /// Formatting dropped with the text kept: `underline`, `sup`, `sub`, `highlight`, `font`,
    /// `alignment`, `style` (other inline CSS), `link` (unsafe, or around an image), `code`
    /// (combined with other marks).
    pub dropped_marks: BTreeMap<String, Tally>,
    pub h1_demoted: Tally,
    /// Elements the block tree has no place for, by tag name.
    pub unsupported_tags: BTreeMap<String, Tally>,
    /// Images whose upload was not imported.
    pub missing_media: Tally,
    pub unknown_shortcodes: BTreeMap<String, Tally>,
    /// Gutenberg blocks with no mapping, by name: kept as an `html` node, or dropped when
    /// their HTML is empty (P12).
    pub unknown_blocks: BTreeMap<String, Tally>,
    /// Block comments whose attributes were not a JSON object (read as none).
    pub bad_block_attrs: Tally,
    /// Markup too costly to parse safely (nested past 512 levels, or more than 1 024
    /// `object`/`applet`/`marquee` elements), kept as text; or blocks nested past
    /// [`doc::MAX_DEPTH`], flattened to text.
    pub too_deep: Tally,
    /// Page breaks (`<!--nextpage-->`, the nextpage block) dropped: the pages are joined.
    pub page_breaks: Tally,
}

impl Notes {
    pub fn dropped_mark(&mut self, name: &str) {
        let item = self.item;
        self.dropped_marks
            .entry(name.to_owned())
            .or_default()
            .bump(item);
    }

    pub fn h1_demoted(&mut self) {
        self.h1_demoted.bump(self.item);
    }

    pub fn unsupported_tag(&mut self, name: &str) {
        let item = self.item;
        self.unsupported_tags
            .entry(name.to_owned())
            .or_default()
            .bump(item);
    }

    pub fn missing_media(&mut self) {
        self.missing_media.bump(self.item);
    }

    pub fn unknown_shortcode(&mut self, name: &str) {
        let item = self.item;
        self.unknown_shortcodes
            .entry(name.to_owned())
            .or_default()
            .bump(item);
    }

    pub fn unknown_block(&mut self, name: &str) {
        let item = self.item;
        self.unknown_blocks
            .entry(name.to_owned())
            .or_default()
            .bump(item);
    }

    pub fn bad_block_attrs(&mut self) {
        self.bad_block_attrs.bump(self.item);
    }

    pub fn too_deep(&mut self) {
        self.too_deep.bump(self.item);
    }

    pub fn page_break(&mut self) {
        self.page_breaks.bump(self.item);
    }

    /// Adds `other`'s counts and examples to these (a run's notes from each item's); `item`
    /// stays as it is.
    pub fn merge(&mut self, other: &Notes) {
        merge_map(&mut self.dropped_marks, &other.dropped_marks);
        self.h1_demoted.merge(&other.h1_demoted);
        merge_map(&mut self.unsupported_tags, &other.unsupported_tags);
        self.missing_media.merge(&other.missing_media);
        merge_map(&mut self.unknown_shortcodes, &other.unknown_shortcodes);
        merge_map(&mut self.unknown_blocks, &other.unknown_blocks);
        self.bad_block_attrs.merge(&other.bad_block_attrs);
        self.too_deep.merge(&other.too_deep);
        self.page_breaks.merge(&other.page_breaks);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tally_keeps_five_distinct_examples() {
        let mut notes = Notes::default();
        for id in [1, 1, 2, 3, 4, 5, 6] {
            notes.item = Some(id);
            notes.unknown_shortcode("foo");
        }
        let t = &notes.unknown_shortcodes["foo"];
        assert_eq!(t.count, 7);
        assert_eq!(t.items, [1, 2, 3, 4, 5]);
    }

    #[test]
    fn merge_adds_counts_and_keeps_five_examples() {
        let mut run = Notes::default();
        for id in [1, 2, 3] {
            run.item = Some(id);
            run.unknown_block("acme/x");
            run.missing_media();
        }
        let mut item = Notes {
            item: Some(9),
            ..Notes::default()
        };
        for _ in 0..2 {
            item.unknown_block("acme/x");
        }
        item.unknown_shortcode("foo");
        for id in [3, 4, 5, 6] {
            item.item = Some(id);
            item.bad_block_attrs();
        }
        run.item = Some(42);
        run.merge(&item);
        run.merge(&item);
        assert_eq!(run.item, Some(42));
        assert_eq!(run.unknown_blocks["acme/x"].count, 7);
        assert_eq!(run.unknown_blocks["acme/x"].items, [1, 2, 3, 9]);
        assert_eq!(run.unknown_shortcodes["foo"].count, 2);
        assert_eq!(run.missing_media.count, 3);
        assert_eq!(run.bad_block_attrs.count, 8);
        assert_eq!(run.bad_block_attrs.items, [3, 4, 5, 6]);
    }
}
