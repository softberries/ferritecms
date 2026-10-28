//! The import passes. Pass 1 maps authors to users and registers the custom types and
//! taxonomies with their terms; pass 2 imports attachments and builds the
//! [`MediaIndex`](crate::urls::MediaIndex).
//!
//! A pass keeps no state for the next one: what a later pass needs is in the site
//! (`import_map`, read through `ImportBatch::lookup`) or the IR, or is derived again
//! ([`types::type_name`], [`acf::field_defs`](crate::acf::field_defs)), so a resumed run
//! that skips a finished pass loses nothing.

pub(crate) mod content;
pub(crate) mod fixup;
mod media;
mod people;
mod terms;
pub mod types;

use std::collections::{BTreeSet, HashMap};

use ferrite_domain::Timestamp;
use futures::TryStreamExt;
use sha2::{Digest, Sha256};

use crate::ImportError;
use crate::ir::Ir;
use crate::model::Item;
use crate::run::{Report, Session};

/// Pass 1: authors, then the types and taxonomies with the terms.
pub(crate) async fn pass1(session: &Session<'_>, report: &mut Report) -> Result<(), ImportError> {
    let census = Census::scan(session.ir()).await?;
    people::import(session, report).await?;
    let acf = crate::acf::field_defs(session.ir()).await?;
    let wp_terms = session.ir().terms().await?;
    let schema = types::plan(&census, &acf, &wp_terms, report);
    terms::import(session, &schema, wp_terms, report).await
}

/// Pass 2: attachments.
pub(crate) async fn pass2(session: &Session<'_>, report: &mut Report) -> Result<(), ImportError> {
    media::import(session, report).await
}

/// What pass 1 needs to know about the items: one scan of the IR.
#[derive(Debug, Default)]
pub(crate) struct Census {
    /// Post types in file order, and whether any item of each has a parent.
    pub post_types: Vec<(String, bool)>,
    /// Per taxonomy, the post types of the items that use it.
    pub taxonomy_users: HashMap<String, BTreeSet<String>>,
    /// Per (post type, meta key), the items holding a non-empty value.
    pub meta_counts: HashMap<(String, String), u64>,
}

impl Census {
    pub(crate) async fn scan(ir: &Ir) -> Result<Census, ImportError> {
        let mut census = Census::default();
        let mut items = ir.items(&[]);
        while let Some(item) = items.try_next().await? {
            census.add(&item);
        }
        Ok(census)
    }

    fn add(&mut self, item: &Item) {
        match self
            .post_types
            .iter_mut()
            .find(|(t, _)| *t == item.post_type)
        {
            Some((_, parent)) => *parent |= item.parent_wp_id != 0,
            None => self
                .post_types
                .push((item.post_type.clone(), item.parent_wp_id != 0)),
        }
        for term in &item.terms {
            self.taxonomy_users
                .entry(term.taxonomy.clone())
                .or_default()
                .insert(item.post_type.clone());
        }
        let keys: BTreeSet<&str> = item
            .meta
            .iter()
            .filter(|(k, v)| !k.starts_with('_') && !v.is_empty())
            .map(|(k, _)| k.as_str())
            .collect();
        for key in keys {
            *self
                .meta_counts
                .entry((item.post_type.clone(), key.to_owned()))
                .or_default() += 1;
        }
    }
}

/// The SHA-256 (hex) of an item's IR record, the `hash` of its map row (P3, P4).
pub fn record_hash(item: &Item) -> Result<String, ImportError> {
    let json = serde_json::to_vec(item)?;
    Ok(Sha256::digest(&json)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

/// The value of meta `key` (the first one).
pub fn meta<'a>(item: &'a Item, key: &str) -> Option<&'a str> {
    item.meta
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

/// A WordPress `YYYY-MM-DD HH:MM:SS` in UTC: `gmt`, or `local` read as UTC when `gmt`
/// is empty or `0000-00-00 00:00:00` (P7). `None` when neither is a date.
pub fn wp_time(gmt: &str, local: &str) -> Option<Timestamp> {
    [gmt, local].into_iter().find_map(|s| {
        let s = s.trim();
        if s.is_empty() || s.starts_with("0000-00-00") {
            return None;
        }
        let rfc = format!("\"{}Z\"", s.replacen(' ', "T", 1));
        serde_json::from_str(&rfc).ok()
    })
}

/// `text` without control characters, cut to `max` characters.
pub(crate) fn clean_text(text: &str, max: usize) -> String {
    text.chars().filter(|c| !c.is_control()).take(max).collect()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn wp_dates_are_utc() {
        let at = wp_time("2024-03-15 10:30:00", "2024-03-15 12:30:00").unwrap();
        assert_eq!(at.to_rfc3339(), "2024-03-15T10:30:00+00:00");
        let local = wp_time("0000-00-00 00:00:00", "2024-03-15 12:30:00").unwrap();
        assert_eq!(local.to_rfc3339(), "2024-03-15T12:30:00+00:00");
        assert_eq!(wp_time("", ""), None);
        assert_eq!(wp_time("yesterday", "0000-00-00 00:00:00"), None);
    }

    #[test]
    fn record_hashes_follow_the_record() {
        let a = Item {
            wp_id: 1,
            title: "A".into(),
            ..Item::default()
        };
        let b = Item {
            title: "B".into(),
            ..a.clone()
        };
        assert_eq!(record_hash(&a).unwrap(), record_hash(&a.clone()).unwrap());
        assert_ne!(record_hash(&a).unwrap(), record_hash(&b).unwrap());
        assert_eq!(record_hash(&a).unwrap().len(), 64);
    }

    #[test]
    fn census_counts_types_parents_taxonomies_and_meta() {
        let mut census = Census::default();
        let item = |id, ty: &str, parent, tax: &str, meta: &[(&str, &str)]| Item {
            wp_id: id,
            post_type: ty.into(),
            parent_wp_id: parent,
            terms: vec![crate::model::ItemTerm {
                taxonomy: tax.into(),
                slug: "x".into(),
                name: "X".into(),
            }],
            meta: meta
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect(),
            ..Item::default()
        };
        census.add(&item(
            1,
            "book",
            0,
            "genre",
            &[("isbn", "1"), ("_isbn", "f")],
        ));
        census.add(&item(
            2,
            "book",
            1,
            "genre",
            &[("isbn", ""), ("pages", "3")],
        ));
        census.add(&item(
            3,
            "post",
            0,
            "genre",
            &[("isbn", "2"), ("isbn", "3")],
        ));
        assert_eq!(
            census.post_types,
            [("book".to_owned(), true), ("post".to_owned(), false)]
        );
        assert_eq!(
            census.taxonomy_users["genre"].iter().collect::<Vec<_>>(),
            ["book", "post"]
        );
        let n = |t: &str, k: &str| census.meta_counts.get(&(t.into(), k.into())).copied();
        assert_eq!(n("book", "isbn"), Some(1));
        assert_eq!(n("book", "pages"), Some(1));
        assert_eq!(n("book", "_isbn"), None);
        assert_eq!(n("post", "isbn"), Some(1));
    }
}
