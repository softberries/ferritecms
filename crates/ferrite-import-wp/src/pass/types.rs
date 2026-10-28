//! Custom post types → content types, custom taxonomies → taxonomies (pass 1).
//!
//! `plan` decides what to register from the item census and the ACF export; `ensure`
//! registers it in a schema batch. The names are derived again by later passes with
//! [`type_name`] and [`taxonomy_name`].

use std::collections::BTreeSet;

use ferrite_core::NewType;
use ferrite_core::fields::FieldDef;
use ferrite_core::import::{ImportBatch, TypeWritten};
use ferrite_core::slug;
use ferrite_domain::Taxonomy;

use super::Census;
use crate::ImportError;
use crate::acf::{AcfField, AcfModel, Unmapped, to_field_def};
use crate::run::{Report, UnmappedAcf};

/// WordPress post types that are not content types of their own: built-ins core already
/// has, attachments (pass 2), menus (pass 4) and WordPress's internal ones. Names
/// starting with `acf-` or `wp_` are internal too.
const NOT_TYPES: [&str; 9] = [
    "post",
    "page",
    "attachment",
    "nav_menu_item",
    "revision",
    "custom_css",
    "customize_changeset",
    "oembed_cache",
    "user_request",
];
/// The built-in types a custom type may hold terms for beside the custom ones.
const BUILT_IN_TYPES: [&str; 2] = ["post", "page"];
/// WP taxonomies with a built-in twin in core, and its name.
const BUILT_IN_TAXONOMIES: [(&str, &str); 2] = [("category", "category"), ("post_tag", "tag")];
/// WordPress taxonomies that are not imported as taxonomies: menus (pass 4), link
/// categories and post formats. Names starting with `wp_` are internal too.
const NOT_TAXONOMIES: [&str; 3] = ["nav_menu", "link_category", "post_format"];
/// Longest type or taxonomy name (`MAX_TYPE_NAME`).
const MAX_NAME: usize = ferrite_core::MAX_TYPE_NAME;
/// Names the router, the API or a built-in type already use; a WP name equal to one
/// gets the prefix `wp_`. `refs` is `ferrite_core`'s `API_WORDS`, `media` its built-in
/// media type.
const TAKEN: [&str; 2] = ["refs", "media"];

/// What pass 1 registers.
#[derive(Debug, Clone, Default)]
pub(crate) struct Schema {
    pub types: Vec<NewType>,
    pub taxonomies: Vec<Taxonomy>,
}

/// Whether the WP post type `post_type` becomes a content type of its own.
pub fn is_custom(post_type: &str) -> bool {
    !(NOT_TYPES.contains(&post_type)
        || post_type.starts_with("acf-")
        || post_type.starts_with("wp_"))
}

/// The content type the items of WP post type `post_type` go to: `post` and `page` as
/// they are, a custom type by [`schema_name`]; `None` for the rest.
pub fn type_name(post_type: &str) -> Option<String> {
    if BUILT_IN_TYPES.contains(&post_type) {
        return Some(post_type.to_owned());
    }
    is_custom(post_type).then(|| schema_name(post_type))
}

/// The taxonomy terms of WP taxonomy `taxonomy` go to: `category` and `tag` (WP
/// `post_tag`) are core's own, others by [`schema_name`]; `None` when it is not imported
/// as a taxonomy (menus, link categories, post formats, `wp_*`).
pub fn taxonomy_name(taxonomy: &str) -> Option<String> {
    if let Some((_, core)) = BUILT_IN_TAXONOMIES.iter().find(|(wp, _)| *wp == taxonomy) {
        return Some((*core).to_owned());
    }
    let internal = NOT_TAXONOMIES.contains(&taxonomy) || taxonomy.starts_with("wp_");
    (!internal).then(|| schema_name(taxonomy))
}

/// A WP name in the type-name rules (`[a-z][a-z0-9_-]{0,31}`, not a reserved word, not
/// four digits): lower-cased, other characters as `_`, a leading non-letter or a
/// reserved name prefixed with `wp_`, cut to 32.
pub fn schema_name(wp: &str) -> String {
    let mut name: String = wp
        .trim()
        .chars()
        .map(|c| {
            let c = c.to_ascii_lowercase();
            if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let reserved = slug::is_reserved(&name)
        || slug::ROUTE_WORDS.contains(&name.as_str())
        || slug::is_year(&name)
        || TAKEN.contains(&name.as_str());
    if reserved || !name.starts_with(|c: char| c.is_ascii_lowercase()) {
        name.insert_str(0, "wp_");
    }
    name.truncate(MAX_NAME);
    name
}

/// The label of a name: its words (split at `_` and `-`) capitalised.
pub fn label(name: &str) -> String {
    name.split(['_', '-'])
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut c = w.chars();
            c.next()
                .map(|f| f.to_uppercase().chain(c).collect::<String>())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The types and taxonomies to register. A custom type is hierarchical when any of its
/// items has a parent, has an archive, and gets the mapped ACF fields of the groups
/// shown on it (none: no schema); unmapped ones go to the report. A taxonomy (other
/// than the built-ins) is hierarchical when any of its terms has a parent and applies to
/// the types of the items that use it.
pub(crate) fn plan(
    census: &Census,
    acf: &AcfModel,
    terms: &[crate::model::Term],
    report: &mut Report,
) -> Schema {
    let mut schema = Schema::default();
    for (post_type, has_parent) in &census.post_types {
        if !is_custom(post_type) {
            continue;
        }
        let name = schema_name(post_type);
        let fields = acf
            .has_groups_for(post_type)
            .then(|| fields_of(post_type, acf, census, report));
        schema.types.push(NewType {
            label: label(&name),
            name,
            hierarchical: *has_parent,
            has_archive: true,
            fields,
        });
    }
    let registered: BTreeSet<&str> = schema
        .types
        .iter()
        .map(|t| t.name.as_str())
        .chain(BUILT_IN_TYPES)
        .collect();
    let mut taxonomies: Vec<&str> = terms.iter().map(|t| t.taxonomy.as_str()).collect();
    taxonomies.extend(census.taxonomy_users.keys().map(String::as_str));
    let mut seen = BTreeSet::new();
    for wp in taxonomies {
        if BUILT_IN_TAXONOMIES.iter().any(|(b, _)| *b == wp) || !seen.insert(wp) {
            continue;
        }
        let Some(name) = taxonomy_name(wp) else {
            continue;
        };
        let content_types: BTreeSet<String> = census
            .taxonomy_users
            .get(wp)
            .into_iter()
            .flatten()
            .filter_map(|t| type_name(t))
            .filter(|t| registered.contains(t.as_str()))
            .collect();
        schema.taxonomies.push(Taxonomy {
            label: label(&name),
            name,
            hierarchical: terms.iter().any(|t| {
                t.taxonomy == wp && t.parent_slug.as_deref().is_some_and(|p| !p.is_empty())
            }),
            content_types: content_types.into_iter().collect(),
            built_in: false,
        });
    }
    schema
}

/// The ACF fields shown on `post_type`, split: those that map (each name once, with
/// their definition), those that do not, and those whose name repeats a mapped one.
/// Pass 1 registers the first; pass 3 maps values with them.
pub(crate) struct TypeFields<'a> {
    pub mapped: Vec<(&'a AcfField, FieldDef)>,
    pub unmapped: Vec<Unmapped>,
    pub repeated: Vec<(&'a AcfField, String)>,
}

pub(crate) fn type_fields<'a>(post_type: &'a str, acf: &'a AcfModel) -> TypeFields<'a> {
    let mut out = TypeFields {
        mapped: Vec::new(),
        unmapped: Vec::new(),
        repeated: Vec::new(),
    };
    for field in acf.fields_for(post_type) {
        match to_field_def(field) {
            Ok(def) if !out.mapped.iter().any(|(_, d)| d.name == def.name) => {
                out.mapped.push((field, def));
            }
            Ok(def) => out.repeated.push((field, def.name)),
            Err(unmapped) => out.unmapped.push(unmapped),
        }
    }
    out
}

/// The mapped ACF fields shown on `post_type`; the rest is reported.
fn fields_of(
    post_type: &str,
    acf: &AcfModel,
    census: &Census,
    report: &mut Report,
) -> Vec<FieldDef> {
    let fields = type_fields(post_type, acf);
    for (field, name) in &fields.repeated {
        report.note(format!(
            "ACF field {:?} of {post_type} repeats the name {name:?}; its values stay as meta",
            field.name
        ));
    }
    for unmapped in fields.unmapped {
        report.unmapped_acf.push(UnmappedAcf {
            items: census
                .meta_counts
                .get(&(post_type.to_owned(), unmapped.field.clone()))
                .copied()
                .unwrap_or(0),
            field: unmapped.field,
            kind: unmapped.kind,
        });
    }
    fields.mapped.into_iter().map(|(_, def)| def).collect()
}

/// Registers `schema` in a schema batch: each type and taxonomy that does not exist yet
/// (an existing one is kept, a type with a schema gaining the fields it lacks). When
/// `report` is given the outcomes are counted (`types`, `taxonomies`) and a taxonomy
/// that exists with another `hierarchical` is noted.
pub(crate) async fn ensure(
    b: &mut ImportBatch<'_>,
    schema: &Schema,
    mut report: Option<&mut Report>,
) -> Result<(), ImportError> {
    for ty in &schema.types {
        let written = b.content_type(ty.clone()).await?;
        if let Some(report) = report.as_deref_mut() {
            let counts = report.counts_of("types");
            match written {
                TypeWritten::Created => counts.created += 1,
                TypeWritten::Updated => counts.updated += 1,
                TypeWritten::Unchanged => counts.unchanged += 1,
            }
        }
    }
    for tax in &schema.taxonomies {
        let before = b.len();
        let differs = b.taxonomy(tax.clone()).await?;
        if let Some(report) = report.as_deref_mut() {
            let counts = report.counts_of("taxonomies");
            if b.len() > before {
                counts.created += 1;
            } else {
                counts.unchanged += 1;
            }
            if differs {
                report.note(format!(
                    "the site's taxonomy {:?} is kept, though WordPress has it {}hierarchical",
                    tax.name,
                    if tax.hierarchical { "" } else { "not " }
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_follow_the_type_rules() {
        assert_eq!(schema_name("book"), "book");
        assert_eq!(schema_name("Event Item"), "event_item");
        assert_eq!(schema_name("author"), "wp_author");
        assert_eq!(schema_name("media"), "wp_media");
        assert_eq!(schema_name("2024"), "wp_2024");
        assert_eq!(schema_name("_x"), "wp__x");
        assert_eq!(schema_name(&"a".repeat(40)).len(), MAX_NAME);
        assert_eq!(label("book"), "Book");
        assert_eq!(label("event_item-x"), "Event Item X");
    }

    #[test]
    fn which_types_and_taxonomies_are_imported() {
        for t in ["book", "product", "event"] {
            assert!(is_custom(t), "{t}");
        }
        for t in [
            "post",
            "page",
            "attachment",
            "nav_menu_item",
            "acf-field",
            "wp_block",
            "revision",
        ] {
            assert!(!is_custom(t), "{t}");
        }
        assert_eq!(type_name("page").as_deref(), Some("page"));
        assert_eq!(type_name("attachment"), None);
        assert_eq!(taxonomy_name("category").as_deref(), Some("category"));
        assert_eq!(taxonomy_name("post_tag").as_deref(), Some("tag"));
        assert_eq!(taxonomy_name("genre").as_deref(), Some("genre"));
        for t in ["nav_menu", "post_format", "link_category", "wp_theme"] {
            assert_eq!(taxonomy_name(t), None, "{t}");
        }
    }
}
