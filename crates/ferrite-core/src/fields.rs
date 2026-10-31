//! Field schemas of content types and the validation of an item's `fields` (P18).
//!
//! A type's stored `fields` value is `{"fields": [<field>, …]}`; each field is
//! `{"name", "label", "kind", "required", "options", "fields", "types"}` (see
//! [`FieldDef`]; `types` is stored only when non-empty). A
//! stored value **without** a `fields` key (`{}`, what the built-in types hold) means the
//! type has no schema: its items accept any JSON object (F17), so built-ins, `seed demo`
//! and plugins keep working until a type gets a schema.
//!
//! Schema rules ([`parse_schema`]): names match `[a-z][a-z0-9_]{0,63}` and are unique per
//! level; at most [`MAX_FIELDS`] fields per level; labels at most [`MAX_LABEL`]
//! characters; `select` has 1..=[`MAX_OPTIONS`] distinct, non-empty options (no other
//! kind has options); `repeater` has sub-fields (no other kind has any) and repeaters nest
//! at most [`MAX_REPEATER_DEPTH`] deep (a repeater inside a repeater is fine, a third one
//! inside those is not). The top-level name `featured_image` is reserved: it is a core
//! field every schema allows (a UUID string or `null`; the site reader shows it). Only
//! `relation` fields (at any level) have `types` (M2b-3 P52): distinct names of
//! registered content types other than `media`, checked against the registry by
//! [`check_relation_types`]; empty means any type.
//!
//! Value rules ([`validate_values`]): the value is an object; unknown keys and missing
//! `required` keys are refused; `null` is accepted for a field that is not required.
//! `text` is a string of at most [`MAX_TEXT`] characters, `rich_text` an object (a block
//! tree), `number` a JSON number, `bool` a boolean, `date` an RFC 3339 string, `select`
//! one of its options, `relation` and `media` a UUID string, and `repeater` an array of
//! at most [`MAX_ROWS`] objects, each validated against the sub-fields. Errors are
//! `InvalidField { field: "fields", .. }` whose message names the path, such as
//! `fields.chapters[2].title: required`. The items are not looked up here (one may be
//! deleted later anyway): content writes check the type of each item a relation with
//! `types` names ([`relation_refs`]); a missing or unreadable item is accepted (P77).

use ferrite_domain::{ContentId, MEDIA_KIND};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::CoreError;

/// Most fields per level of a schema.
pub const MAX_FIELDS: usize = 100;
/// Most options of a `select` field.
pub const MAX_OPTIONS: usize = 100;
/// Longest field name.
pub const MAX_NAME: usize = 64;
/// Longest label or option (characters).
pub const MAX_LABEL: usize = 200;
/// How deep repeaters nest.
pub const MAX_REPEATER_DEPTH: usize = 2;
/// Longest `text` value (characters).
pub const MAX_TEXT: usize = 65_536;
/// Most rows of a `repeater` value.
pub const MAX_ROWS: usize = 200;
/// The core field every schema allows at the top level.
pub const FEATURED_IMAGE: &str = "featured_image";

/// The input member every error of this module is about.
const FIELD: &str = "fields";

/// The kind of a field, spelled in snake case (`"text"`, `"rich_text"`, …).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldKind {
    Text,
    RichText,
    Number,
    Date,
    Bool,
    Select,
    Relation,
    Media,
    Repeater,
}

impl FieldKind {
    /// The kind's name as stored, e.g. `rich_text`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::RichText => "rich_text",
            Self::Number => "number",
            Self::Date => "date",
            Self::Bool => "bool",
            Self::Select => "select",
            Self::Relation => "relation",
            Self::Media => "media",
            Self::Repeater => "repeater",
        }
    }
}

/// One field of a type's schema.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FieldDef {
    pub name: String,
    #[serde(default)]
    pub label: String,
    pub kind: FieldKind,
    #[serde(default)]
    pub required: bool,
    /// The choices of a `select` field (empty for every other kind).
    #[serde(default)]
    pub options: Vec<String>,
    /// The sub-fields of a `repeater` (empty for every other kind).
    #[serde(default)]
    pub fields: Vec<FieldDef>,
    /// The content types a `relation` may point at (empty: any type; empty for every
    /// other kind).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub types: Vec<String>,
}

/// A relation value whose field lists target `types` ([`relation_refs`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelationRef {
    /// Where the value is, e.g. `fields.chapters[1].related`.
    pub path: String,
    /// The item the value names.
    pub id: ContentId,
    /// The types the field allows (never empty).
    pub types: Vec<String>,
}

fn invalid(message: impl Into<String>) -> CoreError {
    CoreError::invalid_field(FIELD, message)
}

/// The schema stored in a type's `fields` value: `None` when it has no `fields` key (no
/// schema), else the parsed and checked fields.
pub fn schema_of(stored: &Value) -> Result<Option<Vec<FieldDef>>, CoreError> {
    match stored.as_object() {
        Some(o) if !o.contains_key(FIELD) => Ok(None),
        _ => parse_schema(stored).map(Some),
    }
}

/// Parses a stored schema `{"fields": [ … ]}` and checks the rules in the module docs.
/// `{}` is an empty list (no schema).
pub fn parse_schema(stored: &Value) -> Result<Vec<FieldDef>, CoreError> {
    let Some(obj) = stored.as_object() else {
        return Err(invalid("a field schema must be an object"));
    };
    if let Some(extra) = obj.keys().find(|k| *k != FIELD) {
        return Err(invalid(format!("unknown schema member {extra:?}")));
    }
    let Some(list) = obj.get(FIELD) else {
        return Ok(Vec::new());
    };
    let defs: Vec<FieldDef> = serde_json::from_value(list.clone())
        .map_err(|e| invalid(format!("invalid field schema: {e}")))?;
    check_schema(&defs)?;
    Ok(defs)
}

/// The stored form of `defs`: `{"fields": [ … ]}`.
pub fn schema_value(defs: &[FieldDef]) -> Value {
    let list = serde_json::to_value(defs).unwrap_or_else(|_| Value::Array(Vec::new()));
    let mut obj = Map::new();
    obj.insert(FIELD.to_owned(), list);
    Value::Object(obj)
}

/// Checks `defs` against the rules in the module docs.
pub fn check_schema(defs: &[FieldDef]) -> Result<(), CoreError> {
    check_level(defs, FIELD, 0)
}

fn check_level(defs: &[FieldDef], path: &str, depth: usize) -> Result<(), CoreError> {
    if defs.len() > MAX_FIELDS {
        return Err(invalid(format!(
            "{path}: more than {MAX_FIELDS} fields on one level"
        )));
    }
    let mut seen: Vec<&str> = Vec::with_capacity(defs.len());
    for def in defs {
        let at = format!("{path}.{}", def.name);
        if !valid_name(&def.name) {
            return Err(invalid(format!(
                "{path}: field name {:?} must match [a-z][a-z0-9_]{{0,63}}",
                def.name
            )));
        }
        if seen.contains(&def.name.as_str()) {
            return Err(invalid(format!("{at}: duplicate field name")));
        }
        seen.push(&def.name);
        if depth == 0 && def.name == FEATURED_IMAGE {
            return Err(invalid(format!(
                "{at}: {FEATURED_IMAGE} is a core field and cannot be redefined"
            )));
        }
        if def.label.chars().count() > MAX_LABEL {
            return Err(invalid(format!(
                "{at}: label longer than {MAX_LABEL} characters"
            )));
        }
        if def.kind == FieldKind::Select {
            check_options(&def.options, &at)?;
        } else if !def.options.is_empty() {
            return Err(invalid(format!("{at}: only select fields have options")));
        }
        if def.kind == FieldKind::Relation {
            if let Some((i, t)) = def
                .types
                .iter()
                .enumerate()
                .find(|(i, t)| def.types[..*i].contains(t))
            {
                return Err(invalid(format!(
                    "{at}: type {t:?} is listed twice (entry {i})"
                )));
            }
        } else if !def.types.is_empty() {
            return Err(invalid(format!("{at}: only relation fields have types")));
        }
        if def.kind == FieldKind::Repeater {
            if def.fields.is_empty() {
                return Err(invalid(format!("{at}: a repeater needs sub-fields")));
            }
            if depth + 1 > MAX_REPEATER_DEPTH {
                return Err(invalid(format!(
                    "{at}: repeaters nest at most {MAX_REPEATER_DEPTH} deep"
                )));
            }
            check_level(&def.fields, &at, depth + 1)?;
        } else if !def.fields.is_empty() {
            return Err(invalid(format!(
                "{at}: only repeater fields have sub-fields"
            )));
        }
    }
    Ok(())
}

/// Checks that every relation's `types` (at any level) names one of `registered`, the
/// names of the content types, and not `media` (media is picked by `media` fields).
pub fn check_relation_types(defs: &[FieldDef], registered: &[&str]) -> Result<(), CoreError> {
    relation_types_at(defs, registered, FIELD)
}

fn relation_types_at(defs: &[FieldDef], registered: &[&str], path: &str) -> Result<(), CoreError> {
    for def in defs {
        let at = format!("{path}.{}", def.name);
        for t in &def.types {
            if t == MEDIA_KIND {
                return Err(invalid(format!(
                    "{at}: a relation cannot point at media (use a media field)"
                )));
            }
            if !registered.contains(&t.as_str()) {
                return Err(invalid(format!("{at}: type {t:?} is not a content type")));
            }
        }
        relation_types_at(&def.fields, registered, &at)?;
    }
    Ok(())
}

/// Whether `defs` hold a relation (at any level) that lists `name` in its `types`.
pub fn names_type(defs: &[FieldDef], name: &str) -> bool {
    defs.iter()
        .any(|d| d.types.iter().any(|t| t == name) || names_type(&d.fields, name))
}

fn check_options(options: &[String], at: &str) -> Result<(), CoreError> {
    if options.is_empty() || options.len() > MAX_OPTIONS {
        return Err(invalid(format!(
            "{at}: a select needs 1 to {MAX_OPTIONS} options"
        )));
    }
    for (i, o) in options.iter().enumerate() {
        if o.is_empty() || o.chars().count() > MAX_LABEL || o.contains('\0') {
            return Err(invalid(format!(
                "{at}: option {i} must be 1 to {MAX_LABEL} characters"
            )));
        }
        if options[..i].contains(o) {
            return Err(invalid(format!("{at}: duplicate option {o:?}")));
        }
    }
    Ok(())
}

/// `[a-z][a-z0-9_]{0,63}`.
fn valid_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    matches!(bytes.next(), Some(b'a'..=b'z'))
        && name.len() <= MAX_NAME
        && bytes.all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'_'))
}

/// Checks an item's `fields` value against `defs` (see the module docs). The top-level
/// `featured_image` (a UUID string or `null`) is always allowed.
pub fn validate_values(defs: &[FieldDef], value: &Value) -> Result<(), CoreError> {
    let Some(obj) = value.as_object() else {
        return Err(invalid("fields must be an object"));
    };
    if let Some(image) = obj.get(FEATURED_IMAGE)
        && !image.is_null()
        && !is_uuid(image)
    {
        return Err(invalid(format!(
            "{FIELD}.{FEATURED_IMAGE}: must be a UUID string or null"
        )));
    }
    check_object(defs, obj, FIELD, true)
}

/// Checks the `fields` of an item of a type whose stored schema is `stored`: no schema
/// accepts any object.
pub fn validate_item(stored: &Value, value: &Value) -> Result<(), CoreError> {
    match schema_of(stored)? {
        Some(defs) => validate_values(&defs, value),
        None if value.is_object() => Ok(()),
        None => Err(invalid("fields must be an object")),
    }
}

/// The relation values of an item's `fields` (already accepted by [`validate_item`])
/// whose field lists target `types`, in schema order (rows in order), with their
/// paths in the format of the validation errors. A type without a schema has none.
pub fn relation_refs(stored: &Value, value: &Value) -> Result<Vec<RelationRef>, CoreError> {
    let mut out = Vec::new();
    if let (Some(defs), Some(obj)) = (schema_of(stored)?, value.as_object()) {
        collect_refs(&defs, obj, FIELD, &mut out);
    }
    Ok(out)
}

fn collect_refs(
    defs: &[FieldDef],
    obj: &Map<String, Value>,
    path: &str,
    out: &mut Vec<RelationRef>,
) {
    for def in defs {
        let Some(v) = obj.get(&def.name) else {
            continue;
        };
        let at = format!("{path}.{}", def.name);
        match def.kind {
            FieldKind::Relation if !def.types.is_empty() => {
                if let Some(id) = v.as_str().and_then(|s| s.parse().ok()) {
                    out.push(RelationRef {
                        path: at,
                        id,
                        types: def.types.clone(),
                    });
                }
            }
            FieldKind::Repeater => {
                for (i, row) in v.as_array().into_iter().flatten().enumerate() {
                    if let Some(row) = row.as_object() {
                        collect_refs(&def.fields, row, &format!("{at}[{i}]"), out);
                    }
                }
            }
            _ => {}
        }
    }
}

fn check_object(
    defs: &[FieldDef],
    obj: &Map<String, Value>,
    path: &str,
    top: bool,
) -> Result<(), CoreError> {
    for key in obj.keys() {
        let known = defs.iter().any(|d| d.name == *key) || (top && key == FEATURED_IMAGE);
        if !known {
            return Err(invalid(format!("{path}.{key}: unknown field")));
        }
    }
    for def in defs {
        let at = format!("{path}.{}", def.name);
        match obj.get(&def.name) {
            None | Some(Value::Null) if def.required => {
                return Err(invalid(format!("{at}: required")));
            }
            None | Some(Value::Null) => {}
            Some(v) => check_value(def, v, &at)?,
        }
    }
    Ok(())
}

fn check_value(def: &FieldDef, v: &Value, at: &str) -> Result<(), CoreError> {
    let wrong = |what: &str| invalid(format!("{at}: must be {what}"));
    match def.kind {
        FieldKind::Text => match v.as_str() {
            Some(s) if s.chars().count() <= MAX_TEXT => Ok(()),
            Some(_) => Err(invalid(format!("{at}: longer than {MAX_TEXT} characters"))),
            None => Err(wrong("a string")),
        },
        FieldKind::RichText if v.is_object() => Ok(()),
        FieldKind::RichText => Err(wrong("an object (a block tree)")),
        FieldKind::Number if v.is_number() => Ok(()),
        FieldKind::Number => Err(wrong("a number")),
        FieldKind::Bool if v.is_boolean() => Ok(()),
        FieldKind::Bool => Err(wrong("true or false")),
        FieldKind::Date => match v.as_str() {
            Some(s) if chrono::DateTime::parse_from_rfc3339(s).is_ok() => Ok(()),
            _ => Err(wrong("an RFC 3339 date-time string")),
        },
        FieldKind::Select => match v.as_str() {
            Some(s) if def.options.iter().any(|o| o == s) => Ok(()),
            _ => Err(wrong("one of the field's options")),
        },
        FieldKind::Relation | FieldKind::Media if is_uuid(v) => Ok(()),
        FieldKind::Relation | FieldKind::Media => Err(wrong("a UUID string")),
        FieldKind::Repeater => {
            let Some(rows) = v.as_array() else {
                return Err(wrong("an array"));
            };
            if rows.len() > MAX_ROWS {
                return Err(invalid(format!("{at}: more than {MAX_ROWS} rows")));
            }
            for (i, row) in rows.iter().enumerate() {
                let row_at = format!("{at}[{i}]");
                let Some(obj) = row.as_object() else {
                    return Err(invalid(format!("{row_at}: must be an object")));
                };
                check_object(&def.fields, obj, &row_at, false)?;
            }
            Ok(())
        }
    }
}

fn is_uuid(v: &Value) -> bool {
    v.as_str().is_some_and(|s| uuid::Uuid::parse_str(s).is_ok())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use serde_json::json;

    fn field(name: &str, kind: FieldKind) -> FieldDef {
        FieldDef {
            name: name.into(),
            label: String::new(),
            kind,
            required: false,
            options: Vec::new(),
            fields: Vec::new(),
            types: Vec::new(),
        }
    }

    fn message(r: Result<Vec<FieldDef>, CoreError>) -> String {
        match r {
            Err(CoreError::InvalidField {
                field: "fields",
                message,
            }) => message,
            other => panic!("expected InvalidField(fields), got {other:?}"),
        }
    }

    fn schema(fields: Value) -> Value {
        json!({ "fields": fields })
    }

    #[test]
    fn field_schema_rules() {
        // `{}` is no schema; an empty list is an empty schema.
        assert_eq!(schema_of(&json!({})).unwrap(), None);
        assert_eq!(schema_of(&schema(json!([]))).unwrap(), Some(Vec::new()));
        assert!(parse_schema(&json!({})).unwrap().is_empty());

        // A schema round-trips through its stored form.
        let mut select = field("genre", FieldKind::Select);
        select.options = vec!["a".into(), "b".into()];
        let mut rep = field("chapters", FieldKind::Repeater);
        rep.fields = vec![field("title", FieldKind::Text)];
        let defs = vec![
            field("subtitle", FieldKind::Text),
            select.clone(),
            rep.clone(),
        ];
        assert_eq!(parse_schema(&schema_value(&defs)).unwrap(), defs);
        let stored = schema_value(&[field("x", FieldKind::RichText)]);
        assert_eq!(stored["fields"][0]["kind"], "rich_text");

        // Not an object; unknown schema member; malformed list; unknown kind; unknown
        // member of a field.
        assert!(message(parse_schema(&json!([]))).contains("object"));
        assert!(message(parse_schema(&json!({"fields": [], "x": 1}))).contains("\"x\""));
        message(parse_schema(&json!({"fields": {}})));
        message(parse_schema(&schema(
            json!([{"name": "a", "kind": "colour"}]),
        )));
        message(parse_schema(&schema(
            json!([{"name": "a", "kind": "text", "max": 3}]),
        )));
        // Names: pattern and length.
        for bad in ["", "A", "1a", "_a", "a-b", "a b", "ä"] {
            let m = message(parse_schema(&schema(
                json!([{"name": bad, "kind": "text"}]),
            )));
            assert!(m.contains("must match"), "{bad:?}: {m}");
        }
        let long = "a".repeat(65);
        message(parse_schema(&schema(
            json!([{"name": long, "kind": "text"}]),
        )));
        let max = "a".repeat(64);
        parse_schema(&schema(json!([{"name": max, "kind": "text"}]))).unwrap();
        // Unique per level (but the same name may appear on another level).
        let m = message(parse_schema(&schema(json!([
            {"name": "a", "kind": "text"}, {"name": "a", "kind": "bool"}
        ]))));
        assert!(m.contains("duplicate"), "{m}");
        parse_schema(&schema(json!([
            {"name": "a", "kind": "text"},
            {"name": "r", "kind": "repeater", "fields": [{"name": "a", "kind": "text"}]}
        ])))
        .unwrap();
        // At most 100 per level.
        let many: Vec<Value> = (0..=MAX_FIELDS)
            .map(|i| json!({"name": format!("f{i}"), "kind": "text"}))
            .collect();
        assert!(message(parse_schema(&schema(json!(many)))).contains("more than 100"));
        parse_schema(&schema(json!(many[..MAX_FIELDS]))).unwrap();
        // Labels at most 200 characters.
        let label = "x".repeat(201);
        message(parse_schema(&schema(
            json!([{"name": "a", "kind": "text", "label": label}]),
        )));
        // featured_image is reserved at the top level.
        assert!(
            message(parse_schema(&schema(
                json!([{"name": "featured_image", "kind": "media"}])
            )))
            .contains("core field")
        );
        // Select: 1..=100 distinct non-empty options; no options elsewhere.
        message(parse_schema(&schema(
            json!([{"name": "s", "kind": "select"}]),
        )));
        let opts: Vec<String> = (0..=MAX_OPTIONS).map(|i| format!("o{i}")).collect();
        message(parse_schema(&schema(
            json!([{"name": "s", "kind": "select", "options": opts}]),
        )));
        parse_schema(&schema(
            json!([{"name": "s", "kind": "select", "options": opts[..MAX_OPTIONS]}]),
        ))
        .unwrap();
        message(parse_schema(&schema(
            json!([{"name": "s", "kind": "select", "options": ["a", "a"]}]),
        )));
        message(parse_schema(&schema(
            json!([{"name": "s", "kind": "select", "options": [""]}]),
        )));
        assert!(
            message(parse_schema(&schema(
                json!([{"name": "t", "kind": "text", "options": ["a"]}])
            )))
            .contains("only select")
        );
        // Repeater: needs sub-fields; nobody else has any; nests at most 2 deep.
        assert!(
            message(parse_schema(&schema(
                json!([{"name": "r", "kind": "repeater"}])
            )))
            .contains("sub-fields")
        );
        assert!(
            message(parse_schema(&schema(json!([
                {"name": "t", "kind": "text", "fields": [{"name": "a", "kind": "text"}]}
            ]))))
            .contains("only repeater")
        );
        let leaf = json!([{"name": "x", "kind": "text"}]);
        let two = json!([{"name": "r1", "kind": "repeater", "fields": [
            {"name": "r2", "kind": "repeater", "fields": leaf}
        ]}]);
        parse_schema(&schema(two)).unwrap();
        let three = json!([{"name": "r1", "kind": "repeater", "fields": [
            {"name": "r2", "kind": "repeater", "fields": [
                {"name": "r3", "kind": "repeater", "fields": leaf}
            ]}
        ]}]);
        let m = message(parse_schema(&schema(three)));
        assert!(m.contains("fields.r1.r2.r3") && m.contains("nest"), "{m}");
    }

    #[test]
    fn values_by_kind() {
        let defs = parse_schema(&schema(json!([
            {"name": "t", "kind": "text"},
            {"name": "rt", "kind": "rich_text"},
            {"name": "n", "kind": "number"},
            {"name": "d", "kind": "date"},
            {"name": "b", "kind": "bool"},
            {"name": "s", "kind": "select", "options": ["x", "y"]},
            {"name": "rel", "kind": "relation"},
            {"name": "m", "kind": "media"},
            {"name": "req", "kind": "text", "required": true},
        ])))
        .unwrap();
        let id = uuid::Uuid::now_v7().to_string();
        let ok = json!({
            "t": "hi", "rt": {"type": "doc"}, "n": 1.5, "d": "2026-10-23T10:00:00Z",
            "b": true, "s": "y", "rel": id, "m": id, "req": "r", "featured_image": id,
        });
        validate_values(&defs, &ok).unwrap();
        validate_values(
            &defs,
            &json!({"req": "r", "t": null, "featured_image": null}),
        )
        .unwrap();
        let bad = |patch: Value| {
            let mut v = ok.clone();
            for (k, val) in patch.as_object().unwrap() {
                v[k] = val.clone();
            }
            match validate_values(&defs, &v) {
                Err(CoreError::InvalidField {
                    field: "fields",
                    message,
                }) => message,
                other => panic!("{patch}: {other:?}"),
            }
        };
        assert!(bad(json!({"t": 1})).starts_with("fields.t:"));
        assert!(bad(json!({"t": "x".repeat(MAX_TEXT + 1)})).contains("longer"));
        bad(json!({"rt": "text"}));
        bad(json!({"n": "1"}));
        bad(json!({"d": "2026-10-23"}));
        bad(json!({"b": "true"}));
        bad(json!({"s": "z"}));
        bad(json!({"rel": "not-a-uuid"}));
        bad(json!({"m": 7}));
        assert_eq!(bad(json!({"req": null})), "fields.req: required");
        bad(json!({"featured_image": "nope"}));
        assert!(validate_values(&defs, &json!([])).is_err());
        let mut text = String::new();
        text.extend(std::iter::repeat_n('ż', MAX_TEXT));
        let mut v = ok.clone();
        v["t"] = json!(text);
        validate_values(&defs, &v).unwrap();
    }
}
