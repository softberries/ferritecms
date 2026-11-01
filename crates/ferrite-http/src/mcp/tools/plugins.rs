//! Plugin tools (design 12 §5.2): `plugins_list`, `plugin_enable`, `plugin_disable`,
//! `plugin_settings_get` and `plugin_settings_update`.
//!
//! Adapters over the REST layer's plugin functions (`api::plugins::{plugin_list,
//! set_enabled, plugin_settings, set_plugin_settings}`) and
//! [`ExtensionAdmin`](ferrite_core::ExtensionAdmin): `manage_plugins` with
//! `extensions:read` / `extensions:write`, checked in core. There is no install,
//! update, remove or purge: installing needs a person to read the permission report and
//! approve its fingerprint (design 12 §5.4). Enabling an installed plugin only turns on
//! a grant that person already approved. A secret setting is write-only here as in the
//! API: reads say whether it is set, never its value, and the audit row names keys only.

use rmcp::{handler::server::wrapper::Parameters, model::CallToolResult, tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::{
    api::plugins::{plugin_list, plugin_settings, set_enabled, set_plugin_settings},
    mcp::{Caller, FerriteMcp, results},
};

/// A plugin named by its name.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NameInput {
    /// The plugin's name, for example `seo` (from plugins_list).
    pub name: String,
}

/// The input of `plugin_settings_update`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SettingsInput {
    /// The plugin's name (from plugins_list).
    pub name: String,
    /// An object of the plugin's setting keys to new values. Read the keys and types
    /// with plugin_settings_get first. Settings you leave out are kept; `null` deletes a
    /// setting's value (the plugin then uses its default). A setting of type `secret`
    /// takes a string, is stored sealed and is never shown again. A key the plugin does
    /// not declare, or a value of the wrong type, is refused and nothing is written.
    pub changes: Map<String, Value>,
}

#[tool_router(router = plugin_tools, vis = "pub(crate)")]
impl FerriteMcp {
    /// Lists the installed plugins: name, version, whether it is enabled (and why it was
    /// disabled, if the site switched it off after repeated failures), description,
    /// publisher, the permissions it was approved with (`report`) and its state on each
    /// server node. Installing or removing a plugin is done in the admin.
    #[tool(annotations(title = "List plugins", read_only_hint = true, open_world_hint = false))]
    pub async fn plugins_list(&self, caller: Caller) -> CallToolResult {
        match plugin_list(&self.api, caller.actor()).await {
            Ok(list) => {
                let on = list.items.iter().filter(|p| p.enabled).count();
                let summary = format!("{} plugins, {on} enabled.", list.items.len());
                results::ok(list, summary)
            }
            Err(p) => results::from_problem(p),
        }
    }

    /// Turns on an installed plugin, with the permissions a person approved when it was
    /// installed. It starts running on every node within moments and can change the
    /// public pages. Returns the plugin's entry.
    #[tool(annotations(
        title = "Enable plugin",
        read_only_hint = false,
        destructive_hint = false,
        idempotent_hint = true,
        open_world_hint = false
    ))]
    pub async fn plugin_enable(
        &self,
        caller: Caller,
        Parameters(input): Parameters<NameInput>,
    ) -> CallToolResult {
        self.switch(&caller, &input.name, true).await
    }

    /// Turns off an installed plugin. Its settings and data are kept, and it can be
    /// enabled again. Returns the plugin's entry.
    #[tool(annotations(
        title = "Disable plugin",
        read_only_hint = false,
        destructive_hint = true,
        idempotent_hint = true,
        open_world_hint = false
    ))]
    pub async fn plugin_disable(
        &self,
        caller: Caller,
        Parameters(input): Parameters<NameInput>,
    ) -> CallToolResult {
        self.switch(&caller, &input.name, false).await
    }

    /// Reads a plugin's settings: the schema (each setting's key, type, label, default
    /// and options) and the current value of every setting that is not a secret. For a
    /// `secret` setting you only learn whether it is set (`secrets`), never its value.
    #[tool(annotations(
        title = "Get plugin settings",
        read_only_hint = true,
        open_world_hint = false
    ))]
    pub async fn plugin_settings_get(
        &self,
        caller: Caller,
        Parameters(input): Parameters<NameInput>,
    ) -> CallToolResult {
        match plugin_settings(&self.api, caller.actor(), &input.name).await {
            Ok(s) => {
                let summary = format!("{} settings of {}.", s.schema.len(), input.name);
                results::ok(s, summary)
            }
            Err(p) => results::from_problem(p),
        }
    }

    /// Changes a plugin's settings. Secrets (API keys and the like) can be set or
    /// cleared but never read back, so do not echo them in your answer. Returns the same
    /// view as plugin_settings_get.
    #[tool(annotations(
        title = "Update plugin settings",
        read_only_hint = false,
        destructive_hint = true,
        idempotent_hint = true,
        open_world_hint = false
    ))]
    pub async fn plugin_settings_update(
        &self,
        caller: Caller,
        Parameters(input): Parameters<SettingsInput>,
    ) -> CallToolResult {
        if input.changes.is_empty() {
            return results::invalid(Some("changes"), "send at least one setting to change");
        }
        let keys: Vec<String> = input.changes.keys().cloned().collect();
        match set_plugin_settings(&self.api, caller.actor(), &input.name, input.changes).await {
            Ok(s) => {
                // Keys only: never a value, a secret least of all.
                self.audit(
                    &caller,
                    "plugin_settings_update",
                    "plugin",
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

impl FerriteMcp {
    /// `plugin_enable` and `plugin_disable`.
    async fn switch(&self, caller: &Caller, name: &str, enabled: bool) -> CallToolResult {
        match set_enabled(&self.api, caller.actor(), name, enabled).await {
            Ok(plugin) => {
                let (tool, verb) = if enabled {
                    ("plugin_enable", "enabled")
                } else {
                    ("plugin_disable", "disabled")
                };
                self.audit(
                    caller,
                    tool,
                    "plugin",
                    Some(plugin.name.clone()),
                    json!({"version": plugin.version}),
                )
                .await;
                let summary = format!("{} is {verb}.", plugin.name);
                results::ok(plugin, summary)
            }
            Err(p) => results::from_problem(p),
        }
    }
}
