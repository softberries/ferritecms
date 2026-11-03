//! Languages and translation groups in a WXR file (M6-1, design 13 §8.1).
//!
//! Two shapes are read, neither confirmed against a real export (no WordPress with
//! Polylang or WPML was run; the tests use hand-written files):
//!
//! - **Polylang** keeps languages as taxonomies the exporter writes like any other: an
//!   item's `<category domain="language" nicename="de">` names its language, and a
//!   `post_translations` term, attached the same way, holds in its description a
//!   PHP-serialised map `language code → WP post id`.
//! - **WPML** with its "Export and Import" add-on writes post meta
//!   `_wpml_import_language_code` and `_wpml_import_translation_group`. A plain WPML
//!   export holds no language data at all: its items import in the default locale.

use std::collections::{BTreeMap, HashMap, HashSet};

use ferrite_domain::locale::{MAX_LOCALES, parse_tag};

use crate::model::{Item, Term};
use crate::pass::meta;

/// The taxonomies Polylang stores languages and translations in: language data, never
/// registered as taxonomies of the site.
pub const LANG_TAXONOMIES: [&str; 4] = [
    "language",
    "post_translations",
    "term_language",
    "term_translations",
];
/// WPML's language meta key.
pub const WPML_LANGUAGE: &str = "_wpml_import_language_code";
/// WPML's translation group meta key.
pub const WPML_GROUP: &str = "_wpml_import_translation_group";
/// The longest `post_translations` description read (bytes); a longer one is ignored.
const MAX_DESCRIPTION: usize = 16 * 1024;

/// The language an item names: `None` when it names none, `Some(Err(raw))` when it is
/// not a language tag.
pub fn item_language(item: &Item) -> Option<Result<String, String>> {
    let raw = item
        .terms
        .iter()
        .find(|t| t.taxonomy == "language")
        .map(|t| t.slug.as_str())
        .or_else(|| meta(item, WPML_LANGUAGE))?
        .trim();
    if raw.is_empty() {
        return None;
    }
    Some(parse_tag(raw).map_err(|_| raw.chars().take(40).collect()))
}

/// The translation groups the file describes, each a list of WP item ids (two or more,
/// in no particular order). An item is in at most one group (the first that names it);
/// a group naming more than [`MAX_LOCALES`] items, or two items for one language, keeps
/// the first of each language up to that limit. Malformed descriptions are skipped and
/// returned as notes.
pub fn groups<'a>(
    terms: &[Term],
    items: impl IntoIterator<Item = &'a Item>,
) -> (Vec<Vec<i64>>, Vec<String>) {
    let mut notes = Vec::new();
    let mut raw: Vec<Vec<(String, i64)>> = Vec::new();
    // Polylang: the `post_translations` terms the file holds.
    for t in terms.iter().filter(|t| t.taxonomy == "post_translations") {
        match polylang_map(&t.description) {
            Ok(members) => raw.push(members),
            Err(why) => notes.push(format!(
                "post_translations term {} skipped: {why}",
                t.slug.chars().take(60).collect::<String>()
            )),
        }
    }
    // WPML: items sharing a group value.
    let mut wpml: BTreeMap<String, Vec<(String, i64)>> = BTreeMap::new();
    for item in items {
        let Some(group) = meta(item, WPML_GROUP)
            .map(str::trim)
            .filter(|g| !g.is_empty())
        else {
            continue;
        };
        let lang = meta(item, WPML_LANGUAGE).unwrap_or("").trim().to_owned();
        wpml.entry(group.chars().take(200).collect())
            .or_default()
            .push((lang, item.wp_id));
    }
    raw.extend(wpml.into_values());

    let mut taken: HashSet<i64> = HashSet::new();
    let mut out = Vec::new();
    for members in raw {
        let mut langs: HashSet<String> = HashSet::new();
        let mut group = Vec::new();
        for (lang, id) in members {
            if group.len() == MAX_LOCALES || taken.contains(&id) || !langs.insert(lang) {
                continue;
            }
            taken.insert(id);
            group.push(id);
        }
        if group.len() > 1 {
            out.push(group);
        }
    }
    (out, notes)
}

/// A Polylang `post_translations` description: `language code → positive WP id`.
fn polylang_map(description: &str) -> Result<Vec<(String, i64)>, String> {
    if description.len() > MAX_DESCRIPTION {
        return Err(format!("its description is over {MAX_DESCRIPTION} bytes"));
    }
    let value = crate::php::unserialize(description.trim()).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for (k, v) in value.entries() {
        let (Some(lang), Some(id)) = (k.as_text(), v.as_text()) else {
            return Err("an entry is not `code => id`".to_owned());
        };
        let id: i64 = id
            .parse()
            .ok()
            .filter(|id| *id > 0)
            .ok_or_else(|| format!("{id:?} is not a post id"))?;
        out.push((lang, id));
    }
    if out.is_empty() {
        return Err("it names no posts".to_owned());
    }
    Ok(out)
}

/// The languages of the file's items: code (as parsed) → number of items.
pub fn census<'a>(items: impl IntoIterator<Item = &'a Item>) -> HashMap<String, u64> {
    let mut out = HashMap::new();
    for item in items {
        if let Some(Ok(tag)) = item_language(item) {
            *out.entry(tag).or_default() += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::model::ItemTerm;

    fn item(id: i64, lang: Option<&str>, meta: &[(&str, &str)]) -> Item {
        Item {
            wp_id: id,
            terms: lang
                .map(|l| ItemTerm {
                    taxonomy: "language".into(),
                    slug: l.into(),
                    name: l.into(),
                })
                .into_iter()
                .collect(),
            meta: meta
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect(),
            ..Item::default()
        }
    }

    fn term(slug: &str, description: &str) -> Term {
        Term {
            taxonomy: "post_translations".into(),
            slug: slug.into(),
            description: description.into(),
            ..Term::default()
        }
    }

    #[test]
    fn languages_from_polylang_terms_or_wpml_meta() {
        assert_eq!(
            item_language(&item(1, Some("de"), &[])),
            Some(Ok("de".into()))
        );
        assert_eq!(
            item_language(&item(1, None, &[(WPML_LANGUAGE, "pt-br")])),
            Some(Ok("pt-BR".into()))
        );
        assert_eq!(item_language(&item(1, None, &[])), None);
        assert_eq!(
            item_language(&item(1, Some("not a tag"), &[])),
            Some(Err("not a tag".into()))
        );
    }

    #[test]
    fn polylang_groups() {
        let terms = [term("pll_1", r#"a:2:{s:2:"en";i:10;s:2:"de";i:11;}"#)];
        let (groups, notes) = groups(&terms, []);
        assert_eq!(groups, [vec![10, 11]]);
        assert!(notes.is_empty());
    }

    #[test]
    fn post_translations_hostile_descriptions() {
        let big = format!(
            r#"a:1:{{s:2:"en";s:{n}:"{s}";}}"#,
            n = 20_000,
            s = "x".repeat(20_000)
        );
        let nested = format!("{}i:1;{}", "a:1:{i:0;".repeat(40), "}".repeat(40));
        let terms = [
            term("big", &big),
            term("nested", &nested),
            term("str-id", r#"a:2:{s:2:"en";s:3:"abc";s:2:"de";i:2;}"#),
            term("neg", r#"a:2:{s:2:"en";i:-1;s:2:"de";i:2;}"#),
            term("object", r#"O:8:"stdClass":0:{}"#),
            // An integer key reads as text: three members.
            term("dup", r#"a:3:{s:2:"en";i:5;s:2:"de";i:6;i:0;i:7;}"#),
            // 5 and 7 are taken by "dup": one member left, no group.
            term("again", r#"a:3:{s:2:"en";i:5;s:2:"fr";i:7;s:2:"it";i:8;}"#),
        ];
        let (groups, notes) = groups(&terms, []);
        assert_eq!(groups, [vec![5, 6, 7]]);
        assert_eq!(notes.len(), 5, "{notes:?}");
    }

    #[test]
    fn wpml_groups_by_meta() {
        let items = [
            item(1, None, &[(WPML_LANGUAGE, "en"), (WPML_GROUP, "g1")]),
            item(2, None, &[(WPML_LANGUAGE, "de"), (WPML_GROUP, "g1")]),
            item(3, None, &[(WPML_LANGUAGE, "de"), (WPML_GROUP, "g1")]),
            item(4, None, &[(WPML_LANGUAGE, "en"), (WPML_GROUP, "g2")]),
        ];
        let (groups, _) = groups(&[], &items);
        // Item 3 repeats `de` in g1; g2 has one member: no group.
        assert_eq!(groups, [vec![1, 2]]);
    }
}
