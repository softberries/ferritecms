//! Site settings tools (design 12 §5.2): `settings_get` and `settings_update`.
//!
//! Adapters over the REST layer's `api::settings::{read, write}` (the allowlist and the
//! validators of [`ferrite_core::settings`]; `settings:read` / `manage_settings` with
//! `settings:write` are checked in core). One setting is out of reach on purpose:
//! `mcp.enabled` is the switch of this very endpoint and stays a person's decision in
//! the admin (design 12 §5.4), so `settings_update` refuses it before anything is
//! written.

use ferrite_core::settings::MCP_ENABLED;
use rmcp::{handler::server::wrapper::Parameters, model::CallToolResult, tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::{
    api::settings::{read, write},
    mcp::{Caller, FerriteMcp, results},
};

/// The input of `settings_update`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateInput {
    /// The settings to change, as an object of setting keys to new values; settings you
    /// leave out are kept. `null` resets a setting to its default. Keys: `site.title`
    /// (text, at most 200 characters), `site.description` (text, at most 1000),
    /// `site.locale` (a language tag such as `en` or `pt-BR`), `reading.per_page`
    /// (integer 1 to 100), `reading.front_page` (the id of a page shown at `/`, or
    /// `null` for the post list), `reading.discourage_indexing` (boolean),
    /// `permalink.post` and `permalink.page` (permalink structures such as `/%slug%/`),
    /// `comments.enabled` (boolean), `comments.moderation` (`"all"` or `"known"`),
    /// `comments.per_page` (integer 1 to 100). All changes are checked together and
    /// written in one step; if any is refused, none is written.
    pub changes: Map<String, Value>,
}

#[tool_router(router = settings_tools, vis = "pub(crate)")]
impl FerriteMcp {
    /// Reads every site setting: title, description, language, posts per page, front
    /// page, search-engine indexing, permalink structures and comment rules. A `null`
    /// value means the setting uses its default. `mcp.enabled` (this connection's own
    /// switch) is shown but can only be changed in the admin.
    #[tool(annotations(title = "Get settings", read_only_hint = true, open_world_hint = false))]
    pub async fn settings_get(&self, caller: Caller) -> CallToolResult {
        match read(&self.api, caller.actor()).await {
            Ok(s) => results::ok(s, "Site settings."),
            Err(p) => results::from_problem(p),
        }
    }

    /// Changes site settings (see `changes` for the keys and their limits). This affects
    /// the whole public site at once: a permalink change moves every address of that
    /// kind, and `reading.discourage_indexing` asks search engines to drop the site, so
    /// confirm such changes with the user first. Returns every setting as stored. The
    /// switch `mcp.enabled` cannot be changed here.
    #[tool(annotations(
        title = "Update settings",
        read_only_hint = false,
        destructive_hint = true,
        idempotent_hint = true,
        open_world_hint = false
    ))]
    pub async fn settings_update(
        &self,
        caller: Caller,
        Parameters(input): Parameters<UpdateInput>,
    ) -> CallToolResult {
        if input.changes.is_empty() {
            return results::invalid(Some("changes"), "send at least one setting to change");
        }
        if let Err(p) = results::within_json_cap("changes", &input.changes) {
            return results::from_problem(p);
        }
        // Every `mcp.*` key stays a person's (design 17 §10, §12): the switch of this
        // endpoint, and the retention of the assistant's own change record.
        if let Some(key) = input.changes.keys().find(|k| k.starts_with("mcp.")) {
            let key = if key == MCP_ENABLED {
                MCP_ENABLED
            } else {
                "mcp"
            };
            return results::invalid(
                Some(key),
                "mcp.* settings cannot be changed through MCP: change them in the admin \
                 (Settings, AI assistants)",
            );
        }
        let keys: Vec<String> = input.changes.keys().cloned().collect();
        match write(&self.api, caller.actor(), input.changes).await {
            Ok(s) => {
                // The keys only: values can be long, and an audit row is not their copy.
                self.audit(
                    &caller,
                    "settings_update",
                    "settings",
                    None,
                    json!({"keys": keys}),
                )
                .await;
                results::ok(s, format!("Updated {}.", keys.join(", ")))
            }
            Err(p) => results::from_problem(p),
        }
    }
}
