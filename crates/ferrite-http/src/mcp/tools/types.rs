//! Content type tools (design 12 §5.2): `content_types_list`, `content_type_get`,
//! `content_type_create` and `content_type_update`. There is no delete (design 12 §5.4:
//! removing a type is site structure for a person).
//!
//! Adapters over [`ContentTypeService`](ferrite_core::ContentTypeService) and the REST
//! layer's DTOs ([`crate::api::dto::types`]): the field schema is the REST
//! [`FieldDefDto`] list. Core checks every permission (`content:read` to read,
//! `manage_settings` with `settings:write` to write) and the name, label and schema
//! rules.

use rmcp::{handler::server::wrapper::Parameters, model::CallToolResult, tool, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{
    api::dto::{
        content::{nullable, present},
        types::{ContentTypeDto, ContentTypeInput, ContentTypePatch, FieldDefDto},
    },
    mcp::{Caller, FerriteMcp, results},
};

// ---- inputs ------------------------------------------------------------------------------

/// A content type named by its name (`content_type_get`).
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NameInput {
    /// The content type's name: `post`, `page` or a custom type from content_types_list.
    pub name: String,
}

/// The input of `content_type_create`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateInput {
    /// The type's name, which is also its URL prefix (`/{name}/`): lowercase letters,
    /// digits, `-` and `_`, starting with a letter, at most 32 characters, and not a
    /// word the site already uses (such as `post`, `page`, `admin`, `api`).
    pub name: String,
    /// The human label, 1 to 100 characters (for example `Case study`).
    pub label: String,
    /// Whether items can have a parent, like pages. Cannot be changed later.
    #[serde(default)]
    pub hierarchical: bool,
    /// Whether `/{name}/` lists the published items.
    #[serde(default)]
    pub has_archive: bool,
    /// The custom fields items of this type have. Leave out (or `[]`) for none: items
    /// then take no custom fields; `null` registers the type without a schema, so items
    /// take any fields object.
    #[serde(default, deserialize_with = "nullable")]
    #[schemars(with = "Option<Vec<FieldDefDto>>")]
    pub fields: Option<Option<Vec<FieldDefDto>>>,
}

/// The input of `content_type_update`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateInput {
    /// The content type's name. The name and `hierarchical` cannot be changed.
    pub name: String,
    /// A new label, 1 to 100 characters. Leave out to keep it.
    #[serde(default, deserialize_with = "present")]
    #[schemars(with = "Option<String>")]
    pub label: Option<String>,
    /// Whether `/{name}/` lists the published items. Leave out to keep it.
    #[serde(default, deserialize_with = "present")]
    #[schemars(with = "Option<bool>")]
    pub has_archive: Option<bool>,
    /// The complete new field list, replacing the old one (send the existing fields you
    /// want to keep, as content_type_get returns them): `[]` leaves no custom fields,
    /// `null` removes the schema so items take any fields object. Leave out to keep it.
    /// Values already stored in removed fields stay in the items but are refused on
    /// their next save.
    #[serde(default, deserialize_with = "nullable")]
    #[schemars(with = "Option<Vec<FieldDefDto>>")]
    pub fields: Option<Option<Vec<FieldDefDto>>>,
}

// ---- outputs -----------------------------------------------------------------------------

/// `content_types_list`.
#[derive(Debug, Serialize)]
pub struct TypesOut {
    pub items: Vec<ContentTypeDto>,
}

// ---- tools -------------------------------------------------------------------------------

#[tool_router(router = type_tools, vis = "pub(crate)")]
impl FerriteMcp {
    /// Lists the site's content types (`post`, `page` and custom ones such as
    /// `case_study`) with their label, whether items nest (`hierarchical`), whether
    /// `/{name}/` lists them (`has_archive`) and their custom field schema (`fields`;
    /// `null` means any fields are accepted). Read the schema before creating items of a
    /// custom type.
    #[tool(annotations(
        title = "List content types",
        read_only_hint = true,
        open_world_hint = false
    ))]
    pub async fn content_types_list(&self, caller: Caller) -> CallToolResult {
        match self.api.types.list(caller.actor()).await {
            Ok(items) => {
                let items: Vec<ContentTypeDto> = items.into_iter().map(Into::into).collect();
                let names: Vec<&str> = items.iter().map(|t| t.name.as_str()).collect();
                let summary = format!("{} content types: {}.", items.len(), names.join(", "));
                results::ok(TypesOut { items }, summary)
            }
            Err(e) => results::from_core(e),
        }
    }

    /// Returns one content type with its label, `hierarchical`, `has_archive` and the
    /// custom field schema (`fields`: name, label, kind, required, options, nested
    /// fields, allowed relation types).
    #[tool(annotations(
        title = "Get content type",
        read_only_hint = true,
        open_world_hint = false
    ))]
    pub async fn content_type_get(
        &self,
        caller: Caller,
        Parameters(input): Parameters<NameInput>,
    ) -> CallToolResult {
        match self.api.types.get(caller.actor(), &input.name).await {
            Ok(t) => {
                let dto = ContentTypeDto::from(t);
                let summary = format!("{:?} ({}): {}.", dto.label, dto.name, fields_phrase(&dto));
                results::ok(dto, summary)
            }
            Err(e) => results::from_core(e),
        }
    }

    /// Registers a new content type (for example `case_study`, `event`, `product`),
    /// optionally with custom fields: each has a `name` (lowercase letters, digits and
    /// `_`), a `label`, a `kind` (`text`, `rich_text`, `number`, `date`, `bool`,
    /// `select` with `options`, `relation` with allowed `types`, `media`, `repeater`
    /// with nested `fields`) and `required`. The type gets its own address prefix
    /// `/{name}/` and shows up in the admin at once. This changes the site's structure:
    /// confirm the plan with the user. Returns the type.
    #[tool(annotations(
        title = "Create content type",
        read_only_hint = false,
        destructive_hint = false,
        idempotent_hint = false,
        open_world_hint = false
    ))]
    pub async fn content_type_create(
        &self,
        caller: Caller,
        Parameters(input): Parameters<CreateInput>,
    ) -> CallToolResult {
        let new = ContentTypeInput {
            name: input.name,
            label: input.label,
            hierarchical: input.hierarchical,
            has_archive: input.has_archive,
            fields: input.fields,
        }
        .into_new();
        match self.api.types.create(caller.actor(), new).await {
            Ok(t) => {
                self.audit(
                    &caller,
                    "content_type_create",
                    "content_type",
                    Some(t.name.clone()),
                    json!({"label": t.label}),
                )
                .await;
                let dto = ContentTypeDto::from(t);
                let summary = format!(
                    "Created {:?} ({}): {}.",
                    dto.label,
                    dto.name,
                    fields_phrase(&dto)
                );
                results::ok(dto, summary)
            }
            Err(e) => results::from_core(e),
        }
    }

    /// Changes a content type's label, archive flag or field schema; what you leave out
    /// is kept. `fields` replaces the whole list, so start from content_type_get and
    /// send the full new list. Stored items are checked against the new schema on their
    /// next save. This changes the site's structure: confirm with the user. Returns the
    /// type.
    #[tool(annotations(
        title = "Update content type",
        read_only_hint = false,
        destructive_hint = true,
        idempotent_hint = true,
        open_world_hint = false
    ))]
    pub async fn content_type_update(
        &self,
        caller: Caller,
        Parameters(input): Parameters<UpdateInput>,
    ) -> CallToolResult {
        if input.label.is_none() && input.has_archive.is_none() && input.fields.is_none() {
            return results::invalid(None, "send at least one of label, has_archive, fields");
        }
        let patch = ContentTypePatch {
            label: input.label,
            has_archive: input.has_archive,
            fields: input.fields,
        }
        .into_patch();
        match self
            .api
            .types
            .update(caller.actor(), &input.name, patch)
            .await
        {
            Ok(t) => {
                self.audit(
                    &caller,
                    "content_type_update",
                    "content_type",
                    Some(t.name.clone()),
                    json!({"label": t.label}),
                )
                .await;
                let dto = ContentTypeDto::from(t);
                let summary = format!(
                    "Updated {:?} ({}): {}.",
                    dto.label,
                    dto.name,
                    fields_phrase(&dto)
                );
                results::ok(dto, summary)
            }
            Err(e) => results::from_core(e),
        }
    }
}

/// How many custom fields `t` has, for a summary.
fn fields_phrase(t: &ContentTypeDto) -> String {
    match &t.fields {
        None => "any fields accepted (no schema)".to_owned(),
        Some(f) if f.is_empty() => "no custom fields".to_owned(),
        Some(f) => format!(
            "{} custom field{}",
            f.len(),
            if f.len() == 1 { "" } else { "s" }
        ),
    }
}
