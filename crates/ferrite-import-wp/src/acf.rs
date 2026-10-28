//! ACF field definitions from the export itself (P16).
//!
//! An ACF export holds its groups as `acf-field-group` items (`post_name` is the key,
//! `content:encoded` the PHP-serialised config whose `location` rules name the post
//! types) and its fields as `acf-field` items (`post_name` the key, `post_excerpt` the
//! name, `content:encoded` the serialised config with `type`; `post_parent` is the group,
//! or the repeater a sub-field belongs to). [`field_defs`] reads them from the IR and
//! [`to_field_def`] maps one to a FerriteCMS field, or says it is unmapped (its values
//! then stay as `wp.<name>` meta).

use std::collections::HashMap;

use ferrite_core::fields::{FieldDef, FieldKind, MAX_LABEL, MAX_NAME, check_schema};
use futures::TryStreamExt;

use crate::ImportError;
use crate::ir::Ir;
use crate::model::Item;
use crate::php::{PhpValue, unserialize};

/// The post type of ACF groups.
pub const GROUP_TYPE: &str = "acf-field-group";
/// The post type of ACF fields.
pub const FIELD_TYPE: &str = "acf-field";
/// ACF field types that only lay out the edit screen and hold no value.
const LAYOUT_KINDS: [&str; 3] = ["tab", "message", "accordion"];

/// Whether ACF field type `kind` only lays out the edit screen (no value).
pub fn is_layout(kind: &str) -> bool {
    LAYOUT_KINDS.contains(&kind)
}

/// Every ACF group of an export.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AcfModel {
    pub groups: Vec<AcfGroup>,
}

/// One field group.
#[derive(Debug, Clone, PartialEq)]
pub struct AcfGroup {
    pub key: String,
    pub title: String,
    /// The post types its location rules show it on (`post_type == …`).
    pub post_types: Vec<String>,
    /// In `menu_order`.
    pub fields: Vec<AcfField>,
}

/// One field and, for a repeater, its sub-fields.
#[derive(Debug, Clone, PartialEq)]
pub struct AcfField {
    pub key: String,
    /// The meta key its values are stored under.
    pub name: String,
    pub label: String,
    /// The ACF type (`text`, `repeater`, …); empty when the config is unreadable.
    pub kind: String,
    /// The serialised config (`Null` when unreadable).
    pub config: PhpValue,
    /// Sub-fields, in `menu_order`.
    pub sub: Vec<AcfField>,
}

/// A field that maps to no FerriteCMS field kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unmapped {
    pub field: String,
    pub kind: String,
}

impl AcfModel {
    /// The value-holding fields shown on `post_type`, group by group (layout fields such
    /// as tabs are left out).
    pub fn fields_for<'a>(&'a self, post_type: &'a str) -> impl Iterator<Item = &'a AcfField> {
        self.groups
            .iter()
            .filter(move |g| g.post_types.iter().any(|t| t == post_type))
            .flat_map(|g| &g.fields)
            .filter(|f| !is_layout(&f.kind))
    }

    /// Whether any group is shown on `post_type`.
    pub fn has_groups_for(&self, post_type: &str) -> bool {
        self.groups
            .iter()
            .any(|g| g.post_types.iter().any(|t| t == post_type))
    }
}

/// Reads the ACF groups and fields of the IR (see the module docs). Trashed groups are
/// left out; a field whose parent is neither a group nor a field is ignored.
pub async fn field_defs(ir: &Ir) -> Result<AcfModel, ImportError> {
    let items: Vec<Item> = ir.items(&[GROUP_TYPE, FIELD_TYPE]).try_collect().await?;
    let mut children: HashMap<i64, Vec<&Item>> = HashMap::new();
    for item in items.iter().filter(|i| i.post_type == FIELD_TYPE) {
        children.entry(item.parent_wp_id).or_default().push(item);
    }
    for list in children.values_mut() {
        // Stable: equal orders keep file order.
        list.sort_by_key(|i| i.menu_order);
    }
    let groups = items
        .iter()
        .filter(|i| i.post_type == GROUP_TYPE && i.status != "trash")
        .map(|g| AcfGroup {
            key: g.post_name.clone(),
            title: g.title.clone(),
            post_types: location_post_types(&unserialize(&g.content).unwrap_or(PhpValue::Null)),
            fields: fields_of(g.wp_id, &children, 0),
        })
        .collect();
    Ok(AcfModel { groups })
}

/// Fields under `parent`, with their sub-fields; nesting stops at a depth WordPress
/// itself never reaches (a cycle in a broken export ends there).
fn fields_of(parent: i64, children: &HashMap<i64, Vec<&Item>>, depth: usize) -> Vec<AcfField> {
    const MAX_DEPTH: usize = 8;
    if depth > MAX_DEPTH {
        return Vec::new();
    }
    children
        .get(&parent)
        .map(|list| {
            list.iter()
                .map(|f| {
                    let config = unserialize(&f.content).unwrap_or(PhpValue::Null);
                    AcfField {
                        key: f.post_name.clone(),
                        name: f.excerpt.clone(),
                        label: f.title.clone(),
                        kind: config
                            .get("type")
                            .and_then(PhpValue::as_text)
                            .unwrap_or_default(),
                        config,
                        sub: fields_of(f.wp_id, children, depth + 1),
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The post types of `post_type == X` rules in a group's `location` (OR groups of AND
/// rules), each once, in order.
fn location_post_types(config: &PhpValue) -> Vec<String> {
    let mut types = Vec::new();
    let rules = config
        .get("location")
        .map(PhpValue::entries)
        .unwrap_or_default()
        .iter()
        .flat_map(|(_, and)| and.entries())
        .map(|(_, rule)| rule);
    for rule in rules {
        let text = |k| rule.get(k).and_then(PhpValue::as_text);
        if text("param").as_deref() == Some("post_type")
            && text("operator").as_deref() == Some("==")
            && let Some(value) = text("value")
            && !types.contains(&value)
        {
            types.push(value);
        }
    }
    types
}

/// Maps one ACF field (P16):
///
/// - text, textarea, email, url, password → `text`; wysiwyg → `rich_text`;
///   number, range → `number`; true_false → `bool`; date_picker, date_time_picker → `date`;
/// - select, radio, button_group without `multiple` → `select` with the choices' values;
/// - image, file → `media`; post_object, page_link without `multiple` → `relation`;
/// - relationship → a `repeater` of one `relation` `item`; gallery → a `repeater` of one
///   `media` `image`; checkbox → a `repeater` of one `text` `value`;
/// - repeater → a `repeater` of its mapped sub-fields (all must map, and there must be
///   some: an export without sub-field rows is unmapped, R3).
///
/// Everything else is [`Unmapped`], and so is a field whose name normalises to nothing or
/// whose result breaks the schema rules. Fields are never `required`: WordPress items
/// written before a field became required lack its value.
pub fn to_field_def(f: &AcfField) -> Result<FieldDef, Unmapped> {
    let unmapped = || Unmapped {
        field: f.name.clone(),
        kind: f.kind.clone(),
    };
    let name = field_name(&f.name).ok_or_else(unmapped)?;
    let label = field_label(&f.label, &name);
    let multiple = f.config.get("multiple").is_some_and(PhpValue::truthy);
    let leaf = |kind| FieldDef {
        name: name.clone(),
        label: label.clone(),
        kind,
        required: false,
        options: Vec::new(),
        fields: Vec::new(),
        types: Vec::new(),
    };
    let rows_of = |sub_name: &str, kind| FieldDef {
        fields: vec![FieldDef {
            name: sub_name.to_owned(),
            label: field_label("", sub_name),
            ..leaf(kind)
        }],
        ..leaf(FieldKind::Repeater)
    };
    let def = match f.kind.as_str() {
        "text" | "textarea" | "email" | "url" | "password" => leaf(FieldKind::Text),
        "wysiwyg" => leaf(FieldKind::RichText),
        "number" | "range" => leaf(FieldKind::Number),
        "true_false" => leaf(FieldKind::Bool),
        "date_picker" | "date_time_picker" => leaf(FieldKind::Date),
        "select" | "radio" | "button_group" if !multiple => {
            let options = f
                .config
                .get("choices")
                .map(PhpValue::entries)
                .unwrap_or_default()
                .iter()
                .filter_map(|(value, _)| value.as_text())
                .collect();
            FieldDef {
                options,
                ..leaf(FieldKind::Select)
            }
        }
        "image" | "file" => leaf(FieldKind::Media),
        "post_object" | "page_link" if !multiple => leaf(FieldKind::Relation),
        "relationship" => rows_of("item", FieldKind::Relation),
        "gallery" => rows_of("image", FieldKind::Media),
        "checkbox" => rows_of("value", FieldKind::Text),
        "repeater" if !f.sub.is_empty() => {
            let fields = f
                .sub
                .iter()
                .filter(|s| !is_layout(&s.kind))
                .map(to_field_def)
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| unmapped())?;
            FieldDef {
                fields,
                ..leaf(FieldKind::Repeater)
            }
        }
        _ => return Err(unmapped()),
    };
    check_schema(std::slice::from_ref(&def)).map_err(|_| unmapped())?;
    Ok(def)
}

/// An ACF name in the field-name rules (`[a-z][a-z0-9_]{0,63}`): lower-cased, other
/// characters as `_`, a leading non-letter prefixed with `f_`, cut to 64. `None` when
/// nothing is left.
pub fn field_name(acf_name: &str) -> Option<String> {
    let mut name: String = acf_name
        .trim()
        .chars()
        .map(|c| {
            let c = c.to_ascii_lowercase();
            if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if name.trim_matches('_').is_empty() {
        return None;
    }
    if !name.starts_with(|c: char| c.is_ascii_lowercase()) {
        name.insert_str(0, "f_");
    }
    name.truncate(MAX_NAME);
    Some(name)
}

/// The label, cut to the schema's limit; the name when it is blank.
fn field_label(label: &str, name: &str) -> String {
    let label = label.trim();
    let label = if label.is_empty() { name } else { label };
    label
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_LABEL)
        .collect()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn acf(name: &str, kind: &str, config: &str) -> AcfField {
        AcfField {
            key: format!("field_{name}"),
            name: name.into(),
            label: String::new(),
            kind: kind.into(),
            config: unserialize(config).unwrap(),
            sub: Vec::new(),
        }
    }

    fn kind_of(f: &AcfField) -> Option<(FieldKind, Vec<(String, FieldKind)>)> {
        to_field_def(f).ok().map(|d| {
            let sub = d.fields.iter().map(|s| (s.name.clone(), s.kind)).collect();
            (d.kind, sub)
        })
    }

    #[test]
    fn kinds_map_as_p16_says() {
        let simple = [
            ("text", FieldKind::Text),
            ("textarea", FieldKind::Text),
            ("email", FieldKind::Text),
            ("url", FieldKind::Text),
            ("password", FieldKind::Text),
            ("wysiwyg", FieldKind::RichText),
            ("number", FieldKind::Number),
            ("range", FieldKind::Number),
            ("true_false", FieldKind::Bool),
            ("date_picker", FieldKind::Date),
            ("date_time_picker", FieldKind::Date),
            ("image", FieldKind::Media),
            ("file", FieldKind::Media),
            ("post_object", FieldKind::Relation),
            ("page_link", FieldKind::Relation),
        ];
        for (kind, want) in simple {
            assert_eq!(
                kind_of(&acf("f", kind, "a:0:{}")),
                Some((want, vec![])),
                "{kind}"
            );
        }
        for (kind, sub, want) in [
            ("relationship", "item", FieldKind::Relation),
            ("gallery", "image", FieldKind::Media),
            ("checkbox", "value", FieldKind::Text),
        ] {
            assert_eq!(
                kind_of(&acf("f", kind, "a:0:{}")),
                Some((FieldKind::Repeater, vec![(sub.to_owned(), want)])),
                "{kind}"
            );
        }
        for kind in [
            "group",
            "flexible_content",
            "clone",
            "google_map",
            "oembed",
            "",
        ] {
            assert_eq!(
                to_field_def(&acf("f", kind, "a:0:{}")),
                Err(Unmapped {
                    field: "f".into(),
                    kind: kind.into(),
                }),
                "{kind}"
            );
        }
    }

    #[test]
    fn choices_and_multiple() {
        let choices = r#"a:2:{s:7:"choices";a:3:{s:1:"a";s:1:"A";i:2;s:3:"Two";s:1:"c";s:1:"C";}s:8:"multiple";i:0;}"#;
        for kind in ["select", "radio", "button_group"] {
            let def = to_field_def(&acf("pick", kind, choices)).unwrap();
            assert_eq!(def.kind, FieldKind::Select);
            assert_eq!(def.options, ["a", "2", "c"]);
        }
        let multi = r#"a:2:{s:7:"choices";a:1:{s:1:"a";s:1:"A";}s:8:"multiple";i:1;}"#;
        assert!(to_field_def(&acf("pick", "select", multi)).is_err());
        assert!(to_field_def(&acf("p", "post_object", r#"a:1:{s:8:"multiple";b:1;}"#)).is_err());
        // A select needs choices.
        assert!(to_field_def(&acf("pick", "select", "a:0:{}")).is_err());
    }

    #[test]
    fn repeaters_need_mapped_sub_fields() {
        let mut quotes = acf("quotes", "repeater", "a:0:{}");
        assert!(to_field_def(&quotes).is_err(), "no sub-field rows (R3)");
        quotes.sub = vec![acf("text", "text", "a:0:{}"), acf("by", "url", "a:0:{}")];
        let def = to_field_def(&quotes).unwrap();
        assert_eq!(def.kind, FieldKind::Repeater);
        assert_eq!(def.fields.len(), 2);
        quotes.sub.push(acf("where", "google_map", "a:0:{}"));
        assert!(to_field_def(&quotes).is_err());
    }

    #[test]
    fn names_and_labels_are_normalised() {
        assert_eq!(field_name("ISBN").as_deref(), Some("isbn"));
        assert_eq!(field_name("release-date").as_deref(), Some("release_date"));
        assert_eq!(field_name("2nd").as_deref(), Some("f_2nd"));
        assert_eq!(field_name("_hidden").as_deref(), Some("f__hidden"));
        assert_eq!(field_name("--"), None);
        assert_eq!(field_name(&"x".repeat(80)).unwrap().len(), MAX_NAME);
        let mut f = acf("Big Name", "text", "a:0:{}");
        f.label = "  Big\u{7}  ".into();
        let def = to_field_def(&f).unwrap();
        assert_eq!((def.name.as_str(), def.label.as_str()), ("big_name", "Big"));
        // The reserved core field cannot come from ACF.
        assert!(to_field_def(&acf("featured_image", "image", "a:0:{}")).is_err());
    }

    #[test]
    fn location_rules_name_post_types() {
        let config = unserialize(
            r#"a:1:{s:8:"location";a:2:{i:0;a:2:{i:0;a:3:{s:5:"param";s:9:"post_type";s:8:"operator";s:2:"==";s:5:"value";s:4:"book";}i:1;a:3:{s:5:"param";s:9:"post_type";s:8:"operator";s:2:"!=";s:5:"value";s:4:"page";}}i:1;a:1:{i:0;a:3:{s:5:"param";s:9:"post_type";s:8:"operator";s:2:"==";s:5:"value";s:6:"author";}}}}"#,
        )
        .unwrap();
        assert_eq!(location_post_types(&config), ["book", "author"]);
        assert!(location_post_types(&PhpValue::Null).is_empty());
    }

    #[test]
    fn php_truthiness() {
        for (v, want) in [
            (PhpValue::Null, false),
            (PhpValue::Bool(true), true),
            (PhpValue::Int(0), false),
            (PhpValue::Str("0".into()), false),
            (PhpValue::Str("no".into()), true),
            (PhpValue::Array(Vec::new()), false),
        ] {
            assert_eq!(v.truthy(), want, "{v:?}");
        }
    }
}
