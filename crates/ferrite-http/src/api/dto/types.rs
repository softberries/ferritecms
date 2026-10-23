//! Bodies of the `/types`, `/taxonomies` and `/terms` endpoints.
//!
//! Input bodies ignore members they do not know; in a `PATCH` an absent member keeps the
//! stored value and `null` is refused (`400 malformed-json`), except `parent_id` of a
//! term, where `null` moves it to the top level.

use ferrite_core::{
    NewType, TypePatch,
    fields::{self, FieldDef, FieldKind},
};
use ferrite_domain::{ContentType, Taxonomy, Term};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use super::content::{nullable, present};

/// The kind of a custom field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum FieldKindDto {
    /// A string of at most 65 536 characters.
    Text,
    /// A block tree (an object).
    RichText,
    /// A JSON number.
    Number,
    /// An RFC 3339 date-time string.
    Date,
    Bool,
    /// One of `options`.
    Select,
    /// The id of a content item.
    Relation,
    /// The id of a media item.
    Media,
    /// An array of at most 200 objects with the sub-`fields`.
    Repeater,
}

impl From<FieldKind> for FieldKindDto {
    fn from(k: FieldKind) -> Self {
        match k {
            FieldKind::Text => Self::Text,
            FieldKind::RichText => Self::RichText,
            FieldKind::Number => Self::Number,
            FieldKind::Date => Self::Date,
            FieldKind::Bool => Self::Bool,
            FieldKind::Select => Self::Select,
            FieldKind::Relation => Self::Relation,
            FieldKind::Media => Self::Media,
            FieldKind::Repeater => Self::Repeater,
        }
    }
}

impl From<FieldKindDto> for FieldKind {
    fn from(k: FieldKindDto) -> Self {
        match k {
            FieldKindDto::Text => Self::Text,
            FieldKindDto::RichText => Self::RichText,
            FieldKindDto::Number => Self::Number,
            FieldKindDto::Date => Self::Date,
            FieldKindDto::Bool => Self::Bool,
            FieldKindDto::Select => Self::Select,
            FieldKindDto::Relation => Self::Relation,
            FieldKindDto::Media => Self::Media,
            FieldKindDto::Repeater => Self::Repeater,
        }
    }
}

/// One field of a content type's schema. Names match `[a-z][a-z0-9_]{0,63}` and are
/// unique per level; at most 100 fields per level; `featured_image` is a core field that
/// every schema allows and none may define.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct FieldDefDto {
    pub name: String,
    #[serde(default)]
    pub label: String,
    pub kind: FieldKindDto,
    #[serde(default)]
    pub required: bool,
    /// The choices of a `select` (1 to 100); empty for every other kind.
    #[serde(default)]
    pub options: Vec<String>,
    /// The sub-fields of a `repeater` (repeaters nest at most 2 deep); empty for every
    /// other kind.
    #[serde(default)]
    #[schema(no_recursion)]
    pub fields: Vec<FieldDefDto>,
}

impl From<FieldDef> for FieldDefDto {
    fn from(d: FieldDef) -> Self {
        Self {
            name: d.name,
            label: d.label,
            kind: d.kind.into(),
            required: d.required,
            options: d.options,
            fields: d.fields.into_iter().map(Self::from).collect(),
        }
    }
}

impl From<FieldDefDto> for FieldDef {
    fn from(d: FieldDefDto) -> Self {
        Self {
            name: d.name,
            label: d.label,
            kind: d.kind.into(),
            required: d.required,
            options: d.options,
            fields: d.fields.into_iter().map(Self::from).collect(),
        }
    }
}

fn defs(dtos: Vec<FieldDefDto>) -> Vec<FieldDef> {
    dtos.into_iter().map(FieldDef::from).collect()
}

/// A content type.
#[derive(Debug, Serialize, ToSchema)]
pub struct ContentTypeDto {
    pub name: String,
    pub label: String,
    /// Whether items can have a parent (cannot change).
    pub hierarchical: bool,
    /// Whether `/{name}/` lists its published items.
    pub has_archive: bool,
    /// `post`, `page` and `media`: cannot be removed.
    pub built_in: bool,
    /// The field schema items are validated against. Empty for a type without a schema
    /// (the built-in types until one is set), whose items take any `fields` object.
    pub fields: Vec<FieldDefDto>,
}

impl From<ContentType> for ContentTypeDto {
    fn from(t: ContentType) -> Self {
        let fields = match fields::schema_of(&t.fields) {
            Ok(schema) => schema.unwrap_or_default(),
            Err(e) => {
                tracing::warn!(r#type = %t.name, error = %e, "stored field schema unreadable");
                Vec::new()
            }
        };
        Self {
            name: t.name,
            label: t.label,
            hierarchical: t.hierarchical,
            has_archive: t.has_archive,
            built_in: t.built_in,
            fields: fields.into_iter().map(FieldDefDto::from).collect(),
        }
    }
}

/// `GET /types`: every type, by name.
#[derive(Debug, Serialize, ToSchema)]
pub struct ContentTypeList {
    pub items: Vec<ContentTypeDto>,
    /// Always `null`: one page holds every type.
    pub next_cursor: Option<String>,
}

/// `POST /types`.
#[derive(Debug, Deserialize, ToSchema)]
pub struct ContentTypeInput {
    /// `[a-z][a-z0-9_-]{0,31}`; not a router word, type, taxonomy or top-level post or
    /// page slug, and not a first segment of a permalink structure.
    pub name: String,
    /// 1 to 100 characters.
    pub label: String,
    #[serde(default)]
    pub hierarchical: bool,
    #[serde(default)]
    pub has_archive: bool,
    #[serde(default)]
    pub fields: Vec<FieldDefDto>,
}

impl ContentTypeInput {
    pub fn into_new(self) -> NewType {
        NewType {
            name: self.name,
            label: self.label,
            hierarchical: self.hierarchical,
            has_archive: self.has_archive,
            fields: defs(self.fields),
        }
    }
}

/// `PATCH /types/{name}`: `name` and `hierarchical` cannot change.
#[derive(Debug, Deserialize, ToSchema)]
pub struct ContentTypePatch {
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = String, required = false)]
    pub label: Option<String>,
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = bool, required = false)]
    pub has_archive: Option<bool>,
    /// Replaces the schema; stored items are validated against it on their next save.
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = Vec<FieldDefDto>, required = false)]
    pub fields: Option<Vec<FieldDefDto>>,
}

impl ContentTypePatch {
    pub fn into_patch(self) -> TypePatch {
        TypePatch {
            label: self.label,
            has_archive: self.has_archive,
            fields: self.fields.map(defs),
        }
    }
}

/// A taxonomy.
#[derive(Debug, Serialize, ToSchema)]
pub struct TaxonomyDto {
    pub name: String,
    pub label: String,
    /// Whether terms can have a parent (cannot change).
    pub hierarchical: bool,
    /// The content types its terms apply to.
    pub content_types: Vec<String>,
    /// `category` and `tag`: cannot be removed.
    pub built_in: bool,
}

impl From<Taxonomy> for TaxonomyDto {
    fn from(t: Taxonomy) -> Self {
        Self {
            name: t.name,
            label: t.label,
            hierarchical: t.hierarchical,
            content_types: t.content_types,
            built_in: t.built_in,
        }
    }
}

/// `GET /taxonomies`: every taxonomy, by name.
#[derive(Debug, Serialize, ToSchema)]
pub struct TaxonomyList {
    pub items: Vec<TaxonomyDto>,
    /// Always `null`: one page holds every taxonomy.
    pub next_cursor: Option<String>,
}

/// `POST /taxonomies`.
#[derive(Debug, Deserialize, ToSchema)]
pub struct TaxonomyInput {
    /// The same rules as a type name.
    pub name: String,
    /// 1 to 100 characters.
    pub label: String,
    #[serde(default)]
    pub hierarchical: bool,
    /// Registered content types (duplicates are dropped).
    #[serde(default)]
    pub content_types: Vec<String>,
}

impl TaxonomyInput {
    pub fn into_taxonomy(self) -> Taxonomy {
        Taxonomy {
            name: self.name,
            label: self.label,
            hierarchical: self.hierarchical,
            content_types: self.content_types,
            built_in: false,
        }
    }
}

/// `PATCH /taxonomies/{name}`: `name` and `hierarchical` cannot change.
#[derive(Debug, Deserialize, ToSchema)]
pub struct TaxonomyPatch {
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = String, required = false)]
    pub label: Option<String>,
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = Vec<String>, required = false)]
    pub content_types: Option<Vec<String>>,
}

/// A term of a taxonomy.
#[derive(Debug, Serialize, ToSchema)]
pub struct TermDto {
    pub id: Uuid,
    pub taxonomy: String,
    /// Unique in the taxonomy; the archive is `/{taxonomy}/{slug}/`.
    pub slug: String,
    pub name: String,
    pub description: String,
    pub parent_id: Option<Uuid>,
}

impl From<Term> for TermDto {
    fn from(t: Term) -> Self {
        Self {
            id: t.id.as_uuid(),
            taxonomy: t.taxonomy,
            slug: t.slug,
            name: t.name,
            description: t.description,
            parent_id: t.parent_id.map(|p| p.as_uuid()),
        }
    }
}

/// `GET /taxonomies/{taxonomy}/terms`: every term of the taxonomy.
#[derive(Debug, Serialize, ToSchema)]
pub struct TermList {
    pub items: Vec<TermDto>,
    /// Always `null`: one page holds every term.
    pub next_cursor: Option<String>,
}

/// `POST /taxonomies/{taxonomy}/terms`.
#[derive(Debug, Deserialize, ToSchema)]
pub struct TermInput {
    /// Not empty after trimming.
    pub name: String,
    /// Derived from the name when absent; a taken slug gets `-2`, `-3`, ….
    #[serde(default)]
    pub slug: Option<String>,
    /// A term of the same (hierarchical) taxonomy.
    #[serde(default)]
    pub parent_id: Option<Uuid>,
    #[serde(default)]
    pub description: String,
}

/// `PATCH /terms/{id}`: merged into the current term.
#[derive(Debug, Deserialize, ToSchema)]
pub struct TermPatch {
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = String, required = false)]
    pub name: Option<String>,
    /// A taken slug gets `-2`, `-3`, ….
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = String, required = false)]
    pub slug: Option<String>,
    /// `null` moves the term to the top level.
    #[serde(default, deserialize_with = "nullable")]
    #[schema(value_type = Option<Uuid>, required = false)]
    pub parent_id: Option<Option<Uuid>>,
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = String, required = false)]
    pub description: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn field_defs_round_trip_and_patches_refuse_null() {
        let dto: FieldDefDto = serde_json::from_value(json!({
            "name": "chapters", "kind": "repeater",
            "fields": [{"name": "title", "kind": "rich_text", "required": true}]
        }))
        .unwrap();
        let def = FieldDef::from(dto.clone());
        assert_eq!(def.fields[0].kind, FieldKind::RichText);
        assert_eq!(FieldDefDto::from(def), dto);
        assert!(
            serde_json::from_value::<FieldDefDto>(json!({"name": "x", "kind": "nope"})).is_err()
        );

        for (field, v) in [
            ("label", json!(null)),
            ("fields", json!(null)),
            ("has_archive", json!(null)),
        ] {
            assert!(
                serde_json::from_value::<ContentTypePatch>(json!({ field: v })).is_err(),
                "{field}"
            );
        }
        let p: TermPatch = serde_json::from_value(json!({"parent_id": null})).unwrap();
        assert_eq!(p.parent_id, Some(None));
        assert!(serde_json::from_value::<TermPatch>(json!({"slug": null})).is_err());
        let p: TermPatch = serde_json::from_value(json!({})).unwrap();
        assert!(p.name.is_none() && p.parent_id.is_none());
    }
}
