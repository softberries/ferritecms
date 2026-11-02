//! The admin API's extension service: capability checks and audit over the
//! [`ExtensionService`] writes (M2a-2 P8, P10; closes M1.5 R11).

use std::{collections::BTreeMap, sync::Arc};

use ferrite_domain::{
    Artifact, ArtifactHash, FailureBucket, InstalledPlugin, InstalledTheme, NewAudit, Timestamp,
};
use ferrite_store::{Repos, Store};
use serde_json::{Map, Value, json};

use ferrite_domain::plugin_trust::{KeySource, PluginOwner, PublisherKey, Revocation, TrustedKey};

use super::{
    ExtensionService, InstallTrust, RetainedPluginData, TrustDecision, append_audit, finish_locked,
};
use crate::{
    Actor, CoreError,
    actor::{Capability, require, require_reader},
    auth::SiteKey,
    events,
    ext_settings::{
        PluginSettingsView, SettingType, SettingsSchema, open_secret, plugin_option,
        resolve_plugin_value, seal_secret, theme_option, validate_value,
    },
};

/// Most rows [`ExtensionAdmin::failure_buckets`] returns: 24 hours of the 64 hooks a
/// host tracks per plugin.
pub const MAX_FAILURE_BUCKETS: u32 = 24 * 64;

/// Installs, activates, lists and deletes themes and plugins for an actor. Writes need
/// `manage_themes` / `manage_plugins` and `extensions:write` ([`require`]); reads need
/// the same role capability and `extensions:read` ([`require_reader`]), so editors see
/// no plugin list. Every write appends one audit row in its transaction (P22).
///
/// Settings (M2a-2 P13; see [`ext_settings`](crate::ext_settings)) are extension
/// writes too: under the `extensions` lock, one transaction with its `options.changed`
/// events and one audit row listing the keys, never the values. Plugin secrets are
/// sealed with `key`.
pub struct ExtensionAdmin {
    store: Arc<dyn Store>,
    service: ExtensionService,
    key: SiteKey,
}

impl ExtensionAdmin {
    /// `key` is the site key: plugin `secret` settings are sealed with it.
    pub fn new(store: Arc<dyn Store>, key: SiteKey) -> Self {
        Self {
            service: ExtensionService::new(store.clone()),
            store,
            key,
        }
    }

    /// How long each write waits for the `extensions` lock (see
    /// [`ExtensionService::with_lock_wait`]); a test seam.
    #[cfg(feature = "test-seams")]
    pub fn with_lock_wait(mut self, wait: std::time::Duration) -> Self {
        self.service = self.service.with_lock_wait(wait);
        self
    }

    /// Whether `actor` may write plugins; handlers call it before reading or compiling
    /// an upload (every write checks again).
    pub fn authorize_plugins(&self, actor: &Actor) -> Result<(), CoreError> {
        require(actor, Capability::ManagePlugins)
    }

    /// Whether `actor` may read plugins (`manage_plugins` with `extensions:read`): the
    /// registry view and the publisher keys (M6-2).
    pub fn authorize_plugin_reads(&self, actor: &Actor) -> Result<(), CoreError> {
        require_reader(actor, Capability::ManagePlugins)
    }

    /// Whether `actor` may refresh the registry now: a plugin write, and at most one
    /// refresh per minute for the whole site (`RateLimited` otherwise; M6-2, design 14
    /// §7.3). Counts the attempt.
    pub async fn authorize_registry_refresh(&self, actor: &Actor) -> Result<(), CoreError> {
        require(actor, Capability::ManagePlugins)?;
        crate::auth::limits::hit(
            self.store.as_ref(),
            &crate::auth::limits::registry_refresh(),
            crate::auth::limits::REGISTRY_REFRESH_PER_SITE,
            crate::auth::limits::WINDOW,
        )
        .await
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

    /// Plugin `name`'s failures per hook and hour from the shared counters (every
    /// node's, P20), from `since` on, newest hour first: at most
    /// [`MAX_FAILURE_BUCKETS`] rows. Unknown plugin: `NotFound`.
    pub async fn failure_buckets(
        &self,
        actor: &Actor,
        name: &str,
        since: Timestamp,
    ) -> Result<Vec<FailureBucket>, CoreError> {
        require_reader(actor, Capability::ManagePlugins)?;
        let read = self.store.read();
        if read.plugins().get(name).await?.is_none() {
            return Err(CoreError::NotFound);
        }
        Ok(read
            .plugin_failures()
            .buckets(name, since, MAX_FAILURE_BUCKETS)
            .await?)
    }

    /// The installed themes.
    pub async fn list_themes(&self, actor: &Actor) -> Result<Vec<InstalledTheme>, CoreError> {
        require_reader(actor, Capability::ManageThemes)?;
        Ok(self.store.read().themes().list().await?)
    }

    /// One installed theme, if there is a row named `name`.
    pub async fn get_theme(
        &self,
        actor: &Actor,
        name: &str,
    ) -> Result<Option<InstalledTheme>, CoreError> {
        require_reader(actor, Capability::ManageThemes)?;
        Ok(self.store.read().themes().get(name).await?)
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
        let trust = InstallTrust::unsigned(super::manifest_publisher(&row.manifest));
        self.service
            .install_plugin_with(artifact, row, &trust, Some(audit))
            .await
            .map(|_| ())
    }

    /// [`ExtensionService::install_plugin_trusted`] with audit `plugin.install` `{name,
    /// version, artifact, granted, signer, owner, source}`.
    pub async fn install_plugin_trusted(
        &self,
        actor: &Actor,
        artifact: &Artifact,
        row: &InstalledPlugin,
        trust: &InstallTrust,
    ) -> Result<TrustDecision, CoreError> {
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
            .install_plugin_with(artifact, row, trust, Some(audit))
            .await
    }

    /// The trust decision for a report ([`ExtensionService::trust_preview`]).
    pub async fn trust_preview(
        &self,
        actor: &Actor,
        name: &str,
        trust: &InstallTrust,
    ) -> Result<TrustDecision, CoreError> {
        require(actor, Capability::ManagePlugins)?;
        self.service.trust_preview(name, trust).await
    }

    /// [`ExtensionService::revoke_key`] with audit `plugin.publisher.revoke` `{key_id,
    /// reason, compromised, disabled}`.
    pub async fn revoke_key(
        &self,
        actor: &Actor,
        key: &PublisherKey,
        revocation: &Revocation,
        label_if_new: &str,
        source_if_new: KeySource,
    ) -> Result<Vec<String>, CoreError> {
        require(actor, Capability::ManagePlugins)?;
        let reason = revocation.reason.trim();
        if reason.is_empty() || reason.chars().count() > 500 {
            return Err(CoreError::invalid_field(
                "reason",
                "a reason of 1 to 500 characters is required",
            ));
        }
        let details = json!({
            "key_id": key.key_id.to_string(),
            "reason": revocation.reason,
            "compromised": revocation.compromised,
        });
        let audit = audit(
            actor,
            "plugin.publisher.revoke",
            "publisher",
            &key.key_id.to_string(),
            details,
        );
        self.service
            .revoke_key_with(key, revocation, label_if_new, source_if_new, Some(audit))
            .await
    }

    /// The site's publisher key rows.
    pub async fn publishers(&self, actor: &Actor) -> Result<Vec<TrustedKey>, CoreError> {
        require_reader(actor, Capability::ManagePlugins)?;
        self.service.list_keys().await
    }

    /// The plugin owner rows.
    pub async fn owners(&self, actor: &Actor) -> Result<Vec<PluginOwner>, CoreError> {
        require_reader(actor, Capability::ManagePlugins)?;
        self.service.owners().await
    }

    /// The owner row of `name`.
    pub async fn owner(&self, actor: &Actor, name: &str) -> Result<Option<PluginOwner>, CoreError> {
        require_reader(actor, Capability::ManagePlugins)?;
        self.service.owner(name).await
    }

    /// The system service behind this facade (registry refreshes are system writes).
    pub fn service(&self) -> &ExtensionService {
        &self.service
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
            self.service.purge_after_delete(&row, deleted_at).await;
        }
        Ok(())
    }

    /// The deleted plugins whose KV and settings are still kept
    /// ([`ExtensionService::retained_plugins`]); `manage_plugins` + `extensions:read`.
    pub async fn retained_plugins(
        &self,
        actor: &Actor,
    ) -> Result<Vec<RetainedPluginData>, CoreError> {
        require_reader(actor, Capability::ManagePlugins)?;
        self.service.retained_plugins().await
    }

    /// Erases the retained KV and settings of the deleted plugin `name` now, with audit
    /// `plugin.purge {name}` (M2b-3 P47; see [`ExtensionService`]'s purge of retained
    /// data): a plugin `name` installed is `InvalidField { field: "name" }`, nothing
    /// retained `NotFound`.
    pub async fn purge_plugin(&self, actor: &Actor, name: &str) -> Result<(), CoreError> {
        require(actor, Capability::ManagePlugins)?;
        let entry = audit(
            actor,
            "plugin.purge",
            "plugin",
            name,
            json!({ "name": name }),
        );
        self.service.purge_retained_with(name, Some(entry)).await
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

    /// [`ExtensionService::activate_theme`] (only if the row still names `expected`)
    /// with audit `theme.activate {name, artifact}`.
    pub async fn activate_theme(
        &self,
        actor: &Actor,
        name: &str,
        expected: &ArtifactHash,
    ) -> Result<(), CoreError> {
        require(actor, Capability::ManageThemes)?;
        let audit = audit(
            actor,
            "theme.activate",
            "theme",
            name,
            json!({ "name": name, "artifact": expected.as_str() }),
        );
        self.service
            .activate_theme_with(name, expected, Some(audit))
            .await
    }
}

impl ExtensionAdmin {
    /// The settings of theme `name` as its templates see them
    /// ([`SettingsSchema::values`] of the stored `theme.<name>.settings` object).
    /// `schema` is the theme's, derived by the caller from its row or files.
    pub async fn theme_settings(
        &self,
        actor: &Actor,
        name: &str,
        schema: &SettingsSchema,
    ) -> Result<Map<String, Value>, CoreError> {
        require_reader(actor, Capability::ManageThemes)?;
        let stored = stored_object(self.store.read(), &theme_option(name)).await?;
        Ok(schema.values(&stored))
    }

    /// Applies `patch` (`key: value`, `null` resets a key to its default) to theme
    /// `name`'s settings and returns them as [`theme_settings`](Self::theme_settings)
    /// does. Every entry is checked first: an undeclared key is `Invalid`, a bad value
    /// [`CoreError::InvalidKey`], and a schema declaring a `secret` is refused (themes
    /// have none). Then, under the `extensions` lock in one transaction: the option
    /// `theme.<name>.settings` (one object of the set keys), `options.changed` (its
    /// `option:` key is what every page of the theme records, so every node's cache
    /// drops them), and audit `theme.settings {name, keys}`. An empty patch writes
    /// nothing.
    pub async fn set_theme_settings(
        &self,
        actor: &Actor,
        name: &str,
        schema: &SettingsSchema,
        patch: Map<String, Value>,
    ) -> Result<Map<String, Value>, CoreError> {
        require(actor, Capability::ManageThemes)?;
        if schema.0.iter().any(|d| d.kind == SettingType::Secret) {
            return Err(CoreError::Invalid(
                "themes cannot have secret settings".into(),
            ));
        }
        check_patch(schema, &patch)?;
        if patch.is_empty() {
            return self.theme_settings(actor, name, schema).await;
        }
        let keys: Vec<&String> = patch.keys().collect();
        let entry = audit(
            actor,
            "theme.settings",
            "theme",
            name,
            json!({ "name": name, "keys": keys }),
        );
        let (guard, tx) = self.service.begin_locked().await?;
        let done = write_theme_settings(tx.as_ref(), name, &patch, entry).await;
        let stored = finish_locked(guard, tx, done).await?;
        Ok(schema.values(&stored))
    }

    /// The settings of plugin `name` (missing: `NotFound`), its schema read from the
    /// row's manifest `[settings]` table: non-secret values as its `settings.get`
    /// answers them, and for each secret only whether it is set.
    pub async fn plugin_settings(
        &self,
        actor: &Actor,
        name: &str,
    ) -> Result<PluginSettingsView, CoreError> {
        require_reader(actor, Capability::ManagePlugins)?;
        let repos = self.store.read();
        let row = repos
            .plugins()
            .get(name)
            .await?
            .ok_or(CoreError::NotFound)?;
        plugin_view(repos, &self.key, name, plugin_schema(&row)?).await
    }

    /// Applies `patch` (`key: value`, `null` deletes the key's option) to plugin
    /// `name`'s settings (missing: `NotFound`) and returns the new view. Every entry is
    /// checked against the row's schema first (undeclared key `Invalid`, bad value
    /// [`CoreError::InvalidKey`]). Then, under the `extensions` lock in one transaction:
    /// one option `plugin.<name>.settings.<key>` per entry (a secret sealed with the
    /// site key for this plugin and key), `options.changed` per key (pages whose plugin
    /// calls read it record that key; plugins read settings per call, P14), and audit
    /// `plugin.settings {name, keys}`. An empty patch writes nothing.
    pub async fn set_plugin_settings(
        &self,
        actor: &Actor,
        name: &str,
        patch: Map<String, Value>,
    ) -> Result<PluginSettingsView, CoreError> {
        require(actor, Capability::ManagePlugins)?;
        if !patch.is_empty() {
            let keys: Vec<&String> = patch.keys().collect();
            let entry = audit(
                actor,
                "plugin.settings",
                "plugin",
                name,
                json!({ "name": name, "keys": keys }),
            );
            let (guard, tx) = self.service.begin_locked().await?;
            let done = self
                .write_plugin_settings(tx.as_ref(), name, &patch, entry)
                .await;
            finish_locked(guard, tx, done).await?;
        }
        self.plugin_settings(actor, name).await
    }

    async fn write_plugin_settings(
        &self,
        repos: &dyn Repos,
        name: &str,
        patch: &Map<String, Value>,
        entry: NewAudit,
    ) -> Result<(), CoreError> {
        let row = repos
            .plugins()
            .get(name)
            .await?
            .ok_or(CoreError::NotFound)?;
        let schema = plugin_schema(&row)?;
        check_patch(&schema, patch)?;
        for (key, value) in patch {
            let option = plugin_option(name, key);
            if value.is_null() {
                repos.options().delete(&option).await?;
            } else {
                let secret = schema
                    .get(key)
                    .is_some_and(|d| d.kind == SettingType::Secret);
                let stored = match value.as_str() {
                    Some(plaintext) if secret => seal_secret(&self.key, name, key, plaintext)?,
                    _ => value.clone(),
                };
                repos.options().set(&option, &stored, false).await?;
            }
            repos
                .events()
                .append(&events::options_changed(&option))
                .await?;
        }
        append_audit(repos, Some(entry)).await
    }
}

/// `InvalidKey` (the key as the field) for a key `schema` does not declare; each
/// non-null value validated.
fn check_patch(schema: &SettingsSchema, patch: &Map<String, Value>) -> Result<(), CoreError> {
    for (key, value) in patch {
        let Some(def) = schema.get(key) else {
            return Err(CoreError::invalid_key(key, "unknown setting"));
        };
        if !value.is_null() {
            validate_value(def, value)?;
        }
    }
    Ok(())
}

fn plugin_schema(row: &InstalledPlugin) -> Result<SettingsSchema, CoreError> {
    SettingsSchema::from_plugin_table(row.manifest.get("settings"))
}

/// The option `key` as an object (unset or not an object: empty).
async fn stored_object(repos: &dyn Repos, key: &str) -> Result<Map<String, Value>, CoreError> {
    Ok(repos
        .options()
        .get(key)
        .await?
        .and_then(|e| match e.value {
            Value::Object(map) => Some(map),
            _ => None,
        })
        .unwrap_or_default())
}

async fn write_theme_settings(
    repos: &dyn Repos,
    name: &str,
    patch: &Map<String, Value>,
    entry: NewAudit,
) -> Result<Map<String, Value>, CoreError> {
    let option = theme_option(name);
    let mut stored = stored_object(repos, &option).await?;
    for (key, value) in patch {
        if value.is_null() {
            stored.remove(key);
        } else {
            stored.insert(key.clone(), value.clone());
        }
    }
    repos
        .options()
        .set(&option, &Value::Object(stored.clone()), false)
        .await?;
    repos
        .events()
        .append(&events::options_changed(&option))
        .await?;
    append_audit(repos, Some(entry)).await?;
    Ok(stored)
}

async fn plugin_view(
    repos: &dyn Repos,
    site_key: &SiteKey,
    name: &str,
    schema: SettingsSchema,
) -> Result<PluginSettingsView, CoreError> {
    let mut values = Map::new();
    let mut secrets = BTreeMap::new();
    let mut invalid = Vec::new();
    for def in &schema.0 {
        let stored = repos.options().get(&plugin_option(name, &def.key)).await?;
        let resolved = resolve_plugin_value(def, stored.as_ref().map(|e| &e.value), |sealed| {
            open_secret(site_key, name, &def.key, sealed).ok()
        });
        if resolved.fell_back {
            invalid.push(def.key.clone());
        }
        if def.kind == SettingType::Secret {
            secrets.insert(def.key.clone(), resolved.value.is_some());
        } else {
            values.insert(def.key.clone(), resolved.value.unwrap_or(Value::Null));
        }
    }
    Ok(PluginSettingsView {
        schema,
        values,
        secrets,
        invalid,
    })
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
