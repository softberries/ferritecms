//! Appearance tools (design 12 §5.2): `themes_list`, `theme_activate`,
//! `theme_settings_get` and `theme_settings_update`.
//!
//! Adapters over the REST layer's theme functions (`api::themes::{entries, activate,
//! theme_settings, set_theme_settings}`) and [`ExtensionAdmin`](ferrite_core::ExtensionAdmin):
//! `manage_themes` with `extensions:read` / `extensions:write`, checked in core. There
//! is no install, upload or delete: bringing a theme in is a person's job (design 12
//! §5.4). Activating one that is already installed (or embedded) is a switch between
//! reviewed themes.

use rmcp::{handler::server::wrapper::Parameters, model::CallToolResult, tool, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::{
    api::{
        dto::extensions::ThemeDto,
        themes::{activate, entries, set_theme_settings, theme_settings},
    },
    mcp::{Caller, FerriteMcp, results},
};

/// A theme named by its name.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NameInput {
    /// The theme's name, for example `blog` (from themes_list).
    pub name: String,
}

/// The input of `theme_settings_update`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SettingsInput {
    /// The theme's name (from themes_list).
    pub name: String,
    /// An object of the theme's setting keys to new values. Read the keys, types and
    /// options with theme_settings_get first. Settings you leave out are kept; `null`
    /// resets a setting to its default. A key the theme does not declare, or a value of
    /// the wrong type, is refused and nothing is written.
    pub changes: Map<String, Value>,
}

/// The answer of `themes_list`.
#[derive(Debug, Serialize)]
pub struct ListOut {
    pub items: Vec<ThemeDto>,
}

#[tool_router(router = appearance_tools, vis = "pub(crate)")]
impl FerriteMcp {
    /// Lists the themes the site can use: the installed ones and the starter themes that
    /// come with FerriteCMS. Each has its name, version, description, whether it is the
    /// active one, whether it has settings, and for the active theme what each server
    /// node is serving. Installing a new theme is done in the admin.
    #[tool(annotations(title = "List themes", read_only_hint = true, open_world_hint = false))]
    pub async fn themes_list(&self, caller: Caller) -> CallToolResult {
        match entries(&self.api, caller.actor()).await {
            Ok(items) => {
                let active = items
                    .iter()
                    .find(|t| t.active)
                    .map_or("none", |t| t.name.as_str())
                    .to_owned();
                let summary = format!("{} themes; active: {active}.", items.len());
                results::ok(ListOut { items }, summary)
            }
            Err(p) => results::from_problem(p),
        }
    }

    /// Makes an installed or built-in theme the site's active theme. The whole public
    /// site changes its look at once, so confirm with the user first. A theme that does
    /// not load is refused and the current one stays. Returns the theme's entry.
    #[tool(annotations(
        title = "Activate theme",
        read_only_hint = false,
        destructive_hint = false,
        idempotent_hint = true,
        open_world_hint = false
    ))]
    pub async fn theme_activate(
        &self,
        caller: Caller,
        Parameters(input): Parameters<NameInput>,
    ) -> CallToolResult {
        match activate(&self.api, caller.actor(), &input.name).await {
            Ok(theme) => {
                self.audit(
                    &caller,
                    "theme_activate",
                    "theme",
                    Some(theme.name.clone()),
                    json!({"version": theme.version}),
                )
                .await;
                let summary = format!("{} is now the active theme.", theme.name);
                results::ok(theme, summary)
            }
            Err(p) => results::from_problem(p),
        }
    }

    /// Reads a theme's settings: the schema (each setting's key, type, label, default
    /// and, for a choice, its options) and the current value of every setting as the
    /// theme's templates see it.
    #[tool(annotations(
        title = "Get theme settings",
        read_only_hint = true,
        open_world_hint = false
    ))]
    pub async fn theme_settings_get(
        &self,
        caller: Caller,
        Parameters(input): Parameters<NameInput>,
    ) -> CallToolResult {
        match theme_settings(&self.api, caller.actor(), &input.name).await {
            Ok(s) => {
                let summary = format!("{} settings of {}.", s.schema.len(), input.name);
                results::ok(s, summary)
            }
            Err(p) => results::from_problem(p),
        }
    }

    /// Changes a theme's settings (colours, toggles, a logo's media id...). The public
    /// pages that show them are refreshed. Returns the schema and every value as
    /// stored.
    #[tool(annotations(
        title = "Update theme settings",
        read_only_hint = false,
        destructive_hint = true,
        idempotent_hint = true,
        open_world_hint = false
    ))]
    pub async fn theme_settings_update(
        &self,
        caller: Caller,
        Parameters(input): Parameters<SettingsInput>,
    ) -> CallToolResult {
        if input.changes.is_empty() {
            return results::invalid(Some("changes"), "send at least one setting to change");
        }
        let keys: Vec<String> = input.changes.keys().cloned().collect();
        match set_theme_settings(&self.api, caller.actor(), &input.name, input.changes).await {
            Ok(s) => {
                self.audit(
                    &caller,
                    "theme_settings_update",
                    "theme",
                    Some(input.name.clone()),
                    json!({"keys": keys}),
                )
                .await;
                let summary = format!("Updated {} of {}.", keys.join(", "), input.name);
                results::ok(s, summary)
            }
            Err(p) => results::from_problem(p),
        }
    }
}
