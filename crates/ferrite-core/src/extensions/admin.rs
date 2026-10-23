//! The admin API's extension service: capability checks and audit over the
//! [`ExtensionService`] writes (M2a-2 P8, P10; closes M1.5 R11).

use std::{sync::Arc, time::Duration};

use ferrite_domain::{Artifact, InstalledPlugin, InstalledTheme, NewAudit, Timestamp};
use ferrite_store::Store;
use serde_json::{Value, json};

use super::{ExtensionService, setting_keys};
use crate::{
    Actor, CoreError,
    actor::{Capability, require, require_reader},
};

/// Installs, activates, lists and deletes themes and plugins for an actor. Writes need
/// `manage_themes` / `manage_plugins` and `extensions:write` ([`require`]); reads need
/// the same role capability and `extensions:read` ([`require_reader`]), so editors see
/// no plugin list. Every write appends one audit row in its transaction (P22).
pub struct ExtensionAdmin {
    store: Arc<dyn Store>,
    service: ExtensionService,
}

impl ExtensionAdmin {
    pub fn new(store: Arc<dyn Store>) -> Self {
        Self {
            service: ExtensionService::new(store.clone()),
            store,
        }
    }

    /// How long each write waits for the `extensions` lock (see
    /// [`ExtensionService::with_lock_wait`]).
    pub fn with_lock_wait(mut self, wait: Duration) -> Self {
        self.service = self.service.with_lock_wait(wait);
        self
    }

    /// Whether `actor` may write plugins; handlers call it before reading or compiling
    /// an upload (every write checks again).
    pub fn authorize_plugins(&self, actor: &Actor) -> Result<(), CoreError> {
        require(actor, Capability::ManagePlugins)
    }

    /// Whether `actor` may write themes; handlers call it before unpacking an upload.
    pub fn authorize_themes(&self, actor: &Actor) -> Result<(), CoreError> {
        require(actor, Capability::ManageThemes)
    }

    /// The installed plugins.
    pub async fn list_plugins(&self, actor: &Actor) -> Result<Vec<InstalledPlugin>, CoreError> {
        require_reader(actor, Capability::ManagePlugins)?;
        Ok(self.store.read().plugins().list().await?)
    }

    /// One installed plugin; missing: `NotFound`.
    pub async fn get_plugin(
        &self,
        actor: &Actor,
        name: &str,
    ) -> Result<InstalledPlugin, CoreError> {
        require_reader(actor, Capability::ManagePlugins)?;
        self.store
            .read()
            .plugins()
            .get(name)
            .await?
            .ok_or(CoreError::NotFound)
    }

    /// The installed themes.
    pub async fn list_themes(&self, actor: &Actor) -> Result<Vec<InstalledTheme>, CoreError> {
        require_reader(actor, Capability::ManageThemes)?;
        Ok(self.store.read().themes().list().await?)
    }

    /// [`ExtensionService::install_plugin`] with audit `plugin.install`
    /// `{name, version, artifact, granted}`. `row.granted` must be a JSON object (the
    /// approved grant, P3): anything else is `InvalidField { field: "approve" }` and
    /// nothing is written.
    pub async fn install_plugin(
        &self,
        actor: &Actor,
        artifact: &Artifact,
        row: &InstalledPlugin,
    ) -> Result<(), CoreError> {
        require(actor, Capability::ManagePlugins)?;
        if !row.granted.is_object() {
            return Err(CoreError::invalid_field("approve", "a grant is required"));
        }
        let details = json!({
            "name": row.name,
            "version": row.version,
            "artifact": artifact.hash.as_str(),
            "granted": row.granted,
        });
        let audit = audit(actor, "plugin.install", "plugin", &row.name, details);
        self.service
            .install_plugin_with(artifact, row, Some(audit))
            .await
    }

    /// [`ExtensionService::set_plugin_enabled`] with audit `plugin.enable` or
    /// `plugin.disable` `{name}`.
    pub async fn set_plugin_enabled(
        &self,
        actor: &Actor,
        name: &str,
        enabled: bool,
    ) -> Result<(), CoreError> {
        require(actor, Capability::ManagePlugins)?;
        let action = if enabled {
            "plugin.enable"
        } else {
            "plugin.disable"
        };
        let audit = audit(actor, action, "plugin", name, json!({ "name": name }));
        self.service
            .set_plugin_enabled_with(name, enabled, Some(audit))
            .await
    }

    /// [`ExtensionService::delete_plugin`] with audit `plugin.delete {name, purge}`.
    /// Without `purge` the KV and settings stay for the retention (P9). With `purge`
    /// they are deleted right after the commit, keeping the deletion marker, and the
    /// delete's purge job runs [`PLUGIN_PURGE_GRACE`](crate::jobs::PLUGIN_PURGE_GRACE)
    /// later as the final sweep: it removes KV a node wrote before it reloaded, finishes
    /// the purge if the immediate one failed (F16), and deletes the marker.
    pub async fn delete_plugin(
        &self,
        actor: &Actor,
        name: &str,
        purge: bool,
    ) -> Result<(), CoreError> {
        require(actor, Capability::ManagePlugins)?;
        let deleted_at = Timestamp::now();
        let details = json!({ "name": name, "purge": purge });
        let audit = audit(actor, "plugin.delete", "plugin", name, details);
        let row = self
            .service
            .delete_plugin_with(name, deleted_at, purge, Some(audit))
            .await?;
        if purge {
            let settings = setting_keys(&row.manifest);
            // The marker stays: the delete's job sweeps again after the grace.
            if let Err(e) = self
                .service
                .purge_plugin_data_with(name, deleted_at, &settings, true)
                .await
            {
                tracing::warn!(plugin = name, error = %e, "purge after delete failed; the purge job retries");
            }
        }
        Ok(())
    }

    /// [`ExtensionService::install_theme`] with audit `theme.install`
    /// `{name, version, artifact, activate}`.
    pub async fn install_theme(
        &self,
        actor: &Actor,
        artifact: &Artifact,
        row: &InstalledTheme,
        activate: bool,
    ) -> Result<(), CoreError> {
        require(actor, Capability::ManageThemes)?;
        let details = json!({
            "name": row.name,
            "version": row.version,
            "artifact": artifact.hash.as_str(),
            "activate": activate,
        });
        let audit = audit(actor, "theme.install", "theme", &row.name, details);
        self.service
            .install_theme_with(artifact, row, activate, Some(audit))
            .await
    }

    /// [`ExtensionService::activate_theme`] with audit `theme.activate {name}`.
    pub async fn activate_theme(&self, actor: &Actor, name: &str) -> Result<(), CoreError> {
        require(actor, Capability::ManageThemes)?;
        let audit = audit(
            actor,
            "theme.activate",
            "theme",
            name,
            json!({ "name": name }),
        );
        self.service.activate_theme_with(name, Some(audit)).await
    }
}

fn audit(actor: &Actor, action: &str, kind: &str, name: &str, details: Value) -> NewAudit {
    NewAudit {
        actor_id: Some(actor.user),
        action: action.to_owned(),
        target_kind: kind.to_owned(),
        target_id: Some(name.to_owned()),
        details,
    }
}
