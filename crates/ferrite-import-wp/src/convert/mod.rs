//! Content conversion: WordPress post bodies to the stored block tree.
//!
//! [`gutenberg::parse`] splits a block-editor body into blocks; classic bodies go through
//! [`autop::wpautop`] and [`shortcode::scan`]; [`html::convert`] turns HTML into [`doc::Node`]s,
//! asking a [`Rewrite`] for media and link targets and counting what it drops in [`Notes`].

use std::collections::BTreeMap;

use uuid::Uuid;

pub mod autop;
pub mod doc;
pub mod gutenberg;
pub mod html;
pub mod shortcode;

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
    /// The new target of a link (an attachment's public URL, an item's new path), or `href`.
    fn href(&self, href: &str) -> String;
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
}

/// What the conversion lost or could not resolve, for the import report.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Notes {
    /// The WP id of the item being converted; recorded as an example with every count.
    pub item: Option<i64>,
    /// Formatting dropped with the text kept: `underline`, `sup`, `sub`, `highlight`, `font`,
    /// `style`, `link` (unsafe, or around an image), `code` (combined with other marks).
    pub dropped_marks: BTreeMap<String, Tally>,
    pub h1_demoted: Tally,
    /// Elements the block tree has no place for, by tag name.
    pub unsupported_tags: BTreeMap<String, Tally>,
    /// Images whose upload was not imported.
    pub missing_media: Tally,
    pub unknown_shortcodes: BTreeMap<String, Tally>,
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
}
