//! Theme and plugin install/activation writes (design 04 §7–8, ruling P21).
//!
//! Each write takes the `extensions` lock (cluster-wide on Postgres, in process on
//! SQLite), then writes the artifact row, the theme or plugin row and its event in one
//! transaction. The artifact's bytes must already be in the media store: the caller
//! uploads them **before** (an orphaned upload is harmless and collected later, 04 §8),
//! so a node that sees the event can always fetch what the row names.
//!
//! **Two entry points over one set of writes.** [`ExtensionService`] is the system
//! (operator) path: no actor, no audit — the CLI (P26) and the job handlers call it.
//! [`ExtensionAdmin`] is its actor-checked facade for the admin API (M2a-2 P8, closing
//! M1.5 R11): it checks `manage_themes` / `manage_plugins` with the `extensions` scope
//! and writes an audit row in the same transaction (P22). Both reach the same private
//! `write_*` helpers, which take the optional audit row.
//!
//! **Plugin delete keeps the data (P9, R2, F13–F16).** Deleting a plugin removes its row
//! but keeps its KV and settings options for [`PLUGIN_DATA_RETENTION`]: the delete
//! transaction writes the marker option `plugin.<name>.deleted_at` (unix micros) and
//! enqueues a [`PLUGIN_PURGE_KIND`] job. The purge
//! ([`ExtensionService::purge_plugin_data`]) runs only when no row of that name exists
//! and the marker still names the job's deletion, so a reinstall (which removes the
//! marker) keeps the data, and only the latest deletion's job purges. Setting keys a
//! later version dropped from its schema are purged only by the daily sweep
//! ([`ExtensionService::enqueue_overdue_purges`]), which re-enqueues the purge for
//! every marker past the retention, so a purge job that went dead is retried.
//!
//! [`PLUGIN_DATA_RETENTION`]: crate::jobs::PLUGIN_DATA_RETENTION
//! [`PLUGIN_PURGE_KIND`]: crate::jobs::PLUGIN_PURGE_KIND

mod admin;
pub mod trust;

use std::{collections::BTreeMap, sync::Arc, time::Duration};

use ferrite_domain::plugin_registry::RegistryIndex;
use ferrite_domain::plugin_trust::{
    KeyId, KeySource, PluginOwner, PublisherKey, RegistryCacheRow, Revocation, TrustedKey,
};
use ferrite_domain::{
    Artifact, ArtifactKind, InstalledPlugin, InstalledTheme, NewAudit, Timestamp,
};
use ferrite_store::{LOCK_EXTENSIONS, LockGuard, Repos, Store, Tx, lock};
use serde_json::{Value, json};

use crate::{
    CoreError, events,
    jobs::{
        PLUGIN_DATA_RETENTION, PLUGIN_PURGE_GRACE, PLUGIN_PURGE_VERSION, PluginPurge,
        plugin_purge_job,
    },
    tx::finish,
};

pub use admin::{ExtensionAdmin, MAX_FAILURE_BUCKETS};
pub use trust::{
    InstallTrust, OwnerChange, RegistryContext, SignerInput, SignerSource, SignerView,
    TrustDecision, TrustWarning, owner_label,
};

/// What [`ExtensionService::store_registry`] did with a fetched index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistryStored {
    /// A newer serial: the cache now holds it.
    Stored,
    /// The same serial and bytes as the cache: only the check time moved.
    Unchanged,
}

/// [`ExtensionService::store_registry`]'s result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryRefresh {
    pub stored: RegistryStored,
    /// Plugins disabled by a newly compromised key the index revokes, or by a key
    /// collision.
    pub disabled: Vec<String>,
    /// Site keys whose id the index lists under other key bytes: their rows were removed
    /// and the plugins they signed disabled (M6-2 review).
    pub collisions: Vec<KeyId>,
}

/// KV keys deleted per statement by a purge.
const KV_PURGE_BATCH: u32 = 1_000;
/// Options per page read by the purge sweep.
const OPTION_PAGE: u32 = 200;
/// Prefix of every plugin-owned option.
const PLUGIN_OPTION_PREFIX: &str = "plugin.";
/// Last segment of the deletion marker option.
const DELETED_AT: &str = "deleted_at";
/// Last segment of the option the immediate erase of a `purge=true` delete writes in its
/// own transaction: that deletion's time. The deletion marker stays for the delete's
/// final sweep (F16); with this beside it the deletion is not listed as retained.
const ERASED_AT: &str = "erased_at";

/// A deleted plugin whose KV and settings are still kept
/// ([`ExtensionService::retained_plugins`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetainedPluginData {
    pub name: String,
    pub deleted_at: Timestamp,
    /// When the delete's purge job erases the data:
    /// [`PLUGIN_DATA_RETENTION`](crate::jobs::PLUGIN_DATA_RETENTION) after `deleted_at`.
    pub purge_after: Timestamp,
}

/// How long a write waits for the `extensions` lock by default.
pub const EXTENSIONS_LOCK_WAIT: Duration = Duration::from_secs(30);

/// Installs, activates and deletes themes and plugins. The system path: no actor and no
/// audit (see the module docs); the admin API goes through [`ExtensionAdmin`].
pub struct ExtensionService {
    store: Arc<dyn Store>,
    lock_wait: Duration,
}

impl ExtensionService {
    pub fn new(store: Arc<dyn Store>) -> Self {
        Self {
            store,
            lock_wait: EXTENSIONS_LOCK_WAIT,
        }
    }

    /// How long each write waits for the `extensions` lock before failing with
    /// `CoreError::Store(StoreError::Busy)` (default [`EXTENSIONS_LOCK_WAIT`]); a test
    /// seam (tests that hold the lock see `busy` quickly).
    #[cfg(feature = "test-seams")]
    pub fn with_lock_wait(mut self, wait: Duration) -> Self {
        self.lock_wait = wait;
        self
    }

    /// Records the theme artifact (kind `theme`, inserted if absent) and inserts or
    /// replaces the theme row (a replaced row keeps its active state); with `activate`
    /// the theme becomes the only active one. Emits `theme.changed` with the active
    /// theme's name only when the active theme changed (another theme became active, or
    /// the active one was replaced). `Invalid` when `artifact` is not a theme artifact
    /// or `theme.artifact_hash` is not its hash. `theme.active` is ignored.
    pub async fn install_theme(
        &self,
        artifact: &Artifact,
        theme: &InstalledTheme,
        activate: bool,
    ) -> Result<(), CoreError> {
        self.install_theme_with(artifact, theme, activate, None)
            .await
    }

    pub(crate) async fn install_theme_with(
        &self,
        artifact: &Artifact,
        theme: &InstalledTheme,
        activate: bool,
        audit: Option<NewAudit>,
    ) -> Result<(), CoreError> {
        check_artifact(artifact, ArtifactKind::Theme, &theme.artifact_hash)?;
        // Activation goes through `activate` only, so the one-active index never trips.
        let row = InstalledTheme {
            active: false,
            ..theme.clone()
        };
        let (guard, tx) = self.begin_locked().await?;
        let done = write_install_theme(tx.as_ref(), artifact, &row, activate, audit).await;
        finish_locked(guard, tx, done).await
    }

    /// Makes `name` the only active theme, if its row still names the artifact
    /// `expected` (the bundle the caller loaded and checked, M2a-2 P12, R10). A missing
    /// theme is `NotFound`; a row naming another artifact (replaced since it was
    /// checked) is [`CoreError::Changed`]; either way the active theme stays. The check
    /// runs under the `extensions` lock, in the write's `Tx`, so no install can replace
    /// the row between it and the activation. Emits `theme.changed` unless `name` was
    /// already active.
    pub async fn activate_theme(
        &self,
        name: &str,
        expected: &ferrite_domain::ArtifactHash,
    ) -> Result<(), CoreError> {
        self.activate_theme_with(name, expected, None).await
    }

    pub(crate) async fn activate_theme_with(
        &self,
        name: &str,
        expected: &ferrite_domain::ArtifactHash,
        audit: Option<NewAudit>,
    ) -> Result<(), CoreError> {
        let (guard, tx) = self.begin_locked().await?;
        let done = write_activate_theme(tx.as_ref(), name, expected, audit).await;
        finish_locked(guard, tx, done).await
    }

    /// Records the plugin artifact (kind `plugin`, inserted if absent) and inserts or
    /// replaces the plugin row; emits `plugins.changed` with its name. `Invalid` when
    /// `artifact` is not a plugin artifact or `row.artifact_hash` is not its hash.
    ///
    /// `row.enabled` and `row.disabled_reason` are ignored, as `install_theme` ignores
    /// `active`: a new plugin is installed disabled, a replaced one keeps its state.
    /// Enabling is [`set_plugin_enabled`](Self::set_plugin_enabled). A replacement with
    /// a different artifact clears the plugin's shared failure counters in the same
    /// transaction (P20).
    ///
    /// This is an unsigned install under policy `any` ([`InstallTrust::unsigned`]); a
    /// signed one is [`install_plugin_trusted`](Self::install_plugin_trusted).
    pub async fn install_plugin(
        &self,
        artifact: &Artifact,
        row: &InstalledPlugin,
    ) -> Result<(), CoreError> {
        let trust = InstallTrust::unsigned(manifest_publisher(&row.manifest));
        self.install_plugin_with(artifact, row, &trust, None)
            .await
            .map(|_| ())
    }

    /// [`install_plugin`](Self::install_plugin) with the upload's trust (M6-2, design 14
    /// §4.2): the decision is made again inside the transaction, under the `extensions`
    /// lock, and its writes (a newly approved key, the owner row) are part of it. A
    /// refusal (`InvalidField` on `signature` or `publisher`) writes nothing.
    pub async fn install_plugin_trusted(
        &self,
        artifact: &Artifact,
        row: &InstalledPlugin,
        trust: &InstallTrust,
    ) -> Result<TrustDecision, CoreError> {
        self.install_plugin_with(artifact, row, trust, None).await
    }

    pub(crate) async fn install_plugin_with(
        &self,
        artifact: &Artifact,
        row: &InstalledPlugin,
        trust: &InstallTrust,
        audit: Option<NewAudit>,
    ) -> Result<TrustDecision, CoreError> {
        check_artifact(artifact, ArtifactKind::Plugin, &row.artifact_hash)?;
        let (guard, tx) = self.begin_locked().await?;
        let done = write_install_plugin(tx.as_ref(), artifact, row, trust, audit).await;
        finish_locked(guard, tx, done).await
    }

    /// The trust decision an install of `name` with `trust` would make now (read
    /// connection; the install decides again in its transaction).
    pub async fn trust_preview(
        &self,
        name: &str,
        trust: &InstallTrust,
    ) -> Result<TrustDecision, CoreError> {
        trust::decide(self.store.read(), name, trust).await
    }

    /// Revokes `key` (design 14 §3.4), inserting its row (with `label_if_new` and
    /// `source_if_new`) when the site has none; with `compromised` every installed plugin
    /// whose current version it signed is disabled in the same transaction. Returns the
    /// disabled plugins.
    pub async fn revoke_key(
        &self,
        key: &PublisherKey,
        revocation: &Revocation,
        label_if_new: &str,
        source_if_new: KeySource,
    ) -> Result<Vec<String>, CoreError> {
        self.revoke_key_with(key, revocation, label_if_new, source_if_new, None)
            .await
    }

    pub(crate) async fn revoke_key_with(
        &self,
        key: &PublisherKey,
        revocation: &Revocation,
        label_if_new: &str,
        source_if_new: KeySource,
        audit: Option<NewAudit>,
    ) -> Result<Vec<String>, CoreError> {
        let (guard, tx) = self.begin_locked().await?;
        let done = trust::write_revocation(
            tx.as_ref(),
            key,
            revocation,
            label_if_new,
            source_if_new,
            audit,
        )
        .await;
        finish_locked(guard, tx, done).await
    }

    /// Trusts `key` as an operator key (`ferrite plugin keys trust`), clearing a local
    /// revocation. A key whose id the cached registry `index` lists under other key bytes
    /// is refused (`InvalidField { field: "key" }`; a key id is 8 bytes anyone can choose).
    pub async fn trust_key(
        &self,
        key: &PublisherKey,
        label: &str,
        index: Option<&RegistryIndex>,
    ) -> Result<(), CoreError> {
        if let Some(p) = index.and_then(|i| i.collides(key)) {
            return Err(CoreError::invalid_field(
                "key",
                format!(
                    "key {} has the id of a key the registry lists for {}, but other key bytes",
                    key.key_id, p.name
                ),
            ));
        }
        let (guard, tx) = self.begin_locked().await?;
        let done = tx
            .plugin_trust()
            .upsert_key(&TrustedKey {
                key: key.clone(),
                label: label.to_owned(),
                source: KeySource::Operator,
                trusted_at: Timestamp::now(),
                revoked: None,
            })
            .await
            .map_err(CoreError::from);
        finish_locked(guard, tx, done).await
    }

    /// Deletes the owner row of `name` and keeps its data (`ferrite plugin unbind`,
    /// operator only): the next install binds the name again. `false` when it had none.
    pub async fn unbind(&self, name: &str) -> Result<bool, CoreError> {
        let (guard, tx) = self.begin_locked().await?;
        let done = tx
            .plugin_trust()
            .delete_owner(name)
            .await
            .map_err(CoreError::from);
        finish_locked(guard, tx, done).await
    }

    /// Stores a fetched and verified index of registry `row.url` (signed by
    /// `registry_key`), then applies its revocations, in one transaction under the
    /// `extensions` lock: a lower serial than the cached one, or the same serial with
    /// other bytes, is refused (`InvalidField { field: "serial" }`, a rollback) and the
    /// cache stays. Every revoked key of the index gets a revocation row; a newly
    /// compromised one disables the plugins it signed.
    ///
    /// `stale`: the cached index text the caller found not to verify under the current
    /// registry key (the key changed, or the row was tampered with); a cached row with
    /// exactly that text is replaced without the serial rule.
    ///
    /// A site key whose id the index lists under other key bytes (a collision, possible
    /// for a key approved before the index was fetched) is removed, every installed plugin
    /// it signed is disabled, and the collision is audited (`plugin.publisher.collision`).
    pub async fn store_registry(
        &self,
        row: &RegistryCacheRow,
        index: &RegistryIndex,
        stale: Option<&str>,
    ) -> Result<RegistryRefresh, CoreError> {
        let (guard, tx) = self.begin_locked().await?;
        let done = write_registry(tx.as_ref(), row, index, stale).await;
        finish_locked(guard, tx, done).await
    }

    /// Records a failed check of registry `url` (the cache, if any, stays).
    pub async fn registry_failed(&self, url: &str, error: &str) -> Result<(), CoreError> {
        let error: String = error.chars().take(500).collect();
        self.store
            .read()
            .plugin_trust()
            .registry_checked(url, Timestamp::now(), Some(&error))
            .await?;
        Ok(())
    }

    /// The cached index row of registry `url`.
    pub async fn registry_cache(&self, url: &str) -> Result<Option<RegistryCacheRow>, CoreError> {
        Ok(self.store.read().plugin_trust().get_registry(url).await?)
    }

    /// The site's key rows (trusted and revoked), by key id.
    pub async fn list_keys(&self) -> Result<Vec<TrustedKey>, CoreError> {
        Ok(self.store.read().plugin_trust().list_keys().await?)
    }

    /// One key row.
    pub async fn get_key(&self, id: &KeyId) -> Result<Option<TrustedKey>, CoreError> {
        Ok(self.store.read().plugin_trust().get_key(id).await?)
    }

    /// The owner row of `name`.
    pub async fn owner(&self, name: &str) -> Result<Option<PluginOwner>, CoreError> {
        Ok(self.store.read().plugin_trust().get_owner(name).await?)
    }

    /// Every owner row, by plugin name.
    pub async fn owners(&self) -> Result<Vec<PluginOwner>, CoreError> {
        Ok(self.store.read().plugin_trust().list_owners().await?)
    }

    /// Enables or disables the plugin `name` (missing: `NotFound`) and clears its
    /// `disabled_reason` either way (an admin decision replaces an automatic one);
    /// enabling also deletes its shared failure counters in the same transaction (P20),
    /// so old failures cannot disable it again at once. Emits `plugins.changed` with its
    /// name.
    pub async fn set_plugin_enabled(&self, name: &str, enabled: bool) -> Result<(), CoreError> {
        self.set_plugin_enabled_with(name, enabled, None).await
    }

    pub(crate) async fn set_plugin_enabled_with(
        &self,
        name: &str,
        enabled: bool,
        audit: Option<NewAudit>,
    ) -> Result<(), CoreError> {
        let (guard, tx) = self.begin_locked().await?;
        let done = write_plugin_enabled(tx.as_ref(), name, enabled, audit).await;
        finish_locked(guard, tx, done).await
    }

    /// Deletes the plugin `name` (missing: `NotFound`) and returns the deleted row. In
    /// one transaction under the `extensions` lock: the row is deleted, its shared
    /// failure counters cleared, the marker option `plugin.<name>.deleted_at` written,
    /// a [`PLUGIN_PURGE_KIND`](crate::jobs::PLUGIN_PURGE_KIND) job enqueued for
    /// [`PLUGIN_DATA_RETENTION`](crate::jobs::PLUGIN_DATA_RETENTION) later (F13), and
    /// `plugins.changed` emitted. KV and settings stay until that job (see the module
    /// docs); [`purge_plugin_data`](Self::purge_plugin_data) removes them earlier.
    pub async fn delete_plugin(&self, name: &str) -> Result<InstalledPlugin, CoreError> {
        self.delete_plugin_with(name, Timestamp::now(), false, None)
            .await
    }

    /// The operator form of delete (`ferrite plugin remove`, P26): [`delete_plugin`](Self::delete_plugin)
    /// (Self::delete_plugin), or with `purge` the immediate purge of the KV and settings
    /// after the commit, exactly as [`ExtensionAdmin::delete_plugin`] does it (the
    /// delete's purge job, due [`PLUGIN_PURGE_GRACE`] later, is the final sweep and the
    /// retry). Returns the deleted row.
    pub async fn remove_plugin(
        &self,
        name: &str,
        purge: bool,
    ) -> Result<InstalledPlugin, CoreError> {
        let deleted_at = Timestamp::now();
        let row = self
            .delete_plugin_with(name, deleted_at, purge, None)
            .await?;
        if purge {
            self.purge_after_delete(&row, deleted_at).await;
        }
        Ok(row)
    }

    /// The immediate purge after a delete with `purge`; the marker stays (the delete's
    /// job sweeps again after the grace). A failure is logged: the job retries.
    pub(crate) async fn purge_after_delete(&self, row: &InstalledPlugin, deleted_at: Timestamp) {
        let settings = setting_keys(&row.manifest);
        if let Err(e) = self
            .purge_plugin_data_with(&row.name, deleted_at, &settings, true)
            .await
        {
            tracing::warn!(plugin = %row.name, error = %e, "purge after delete failed; the purge job retries");
        }
    }

    /// A `purge=true` delete whose immediate erase never ran (it failed), as
    /// [`remove_plugin`](Self::remove_plugin) leaves it then; a test seam.
    #[cfg(feature = "test-seams")]
    pub async fn delete_plugin_without_erase(
        &self,
        name: &str,
        deleted_at: Timestamp,
    ) -> Result<InstalledPlugin, CoreError> {
        self.delete_plugin_with(name, deleted_at, true, None).await
    }

    /// [`delete_plugin`](Self::delete_plugin) at `deleted_at`; with `purge_now` the
    /// purge job is due [`PLUGIN_PURGE_GRACE`] after the deletion instead of the
    /// retention: it sweeps KV a node wrote before it reloaded, and finishes the purge
    /// the caller starts after the commit if that one failed (F16, review round 1).
    pub(crate) async fn delete_plugin_with(
        &self,
        name: &str,
        deleted_at: Timestamp,
        purge_now: bool,
        audit: Option<NewAudit>,
    ) -> Result<InstalledPlugin, CoreError> {
        let (guard, tx) = self.begin_locked().await?;
        let done = write_delete_plugin(tx.as_ref(), name, deleted_at, purge_now, audit).await;
        finish_locked(guard, tx, done).await
    }

    /// Deletes the KV and the settings options (`plugin.<name>.settings.<key>` for each
    /// of `settings`) of the plugin deleted at `deleted_at`, and its deletion marker.
    /// Returns `false` and deletes nothing when a plugin `name` is installed again or
    /// the marker does not name `deleted_at` (a reinstall, a later deletion, or already
    /// purged; F14).
    ///
    /// Holds the `extensions` lock throughout (F15), so an install cannot slip between
    /// the check and the deletes: the KV goes in batches outside any transaction, then
    /// one transaction deletes the options, each with `options.changed`. A crash in
    /// between leaves the marker, so a retry finishes the job. `Busy` when the lock is
    /// not free within the lock wait.
    pub async fn purge_plugin_data(
        &self,
        name: &str,
        deleted_at: Timestamp,
        settings: &[String],
    ) -> Result<bool, CoreError> {
        self.purge_plugin_data_with(name, deleted_at, settings, false)
            .await
    }

    /// [`purge_plugin_data`](Self::purge_plugin_data); with `keep_marker` the deletion
    /// marker stays, so the delete's own job (the final sweep) still runs.
    pub(crate) async fn purge_plugin_data_with(
        &self,
        name: &str,
        deleted_at: Timestamp,
        settings: &[String],
        keep_marker: bool,
    ) -> Result<bool, CoreError> {
        let guard = lock(self.store.as_ref(), LOCK_EXTENSIONS, self.lock_wait).await?;
        let done = self
            .purge_locked(name, deleted_at, settings, keep_marker)
            .await;
        release(guard).await;
        done
    }

    /// Enqueues a [`PLUGIN_PURGE_KIND`](crate::jobs::PLUGIN_PURGE_KIND) job, due at
    /// once, for every deletion marker older than
    /// [`PLUGIN_DATA_RETENTION`](crate::jobs::PLUGIN_DATA_RETENTION) at `now` whose
    /// plugin is not installed (the daily sweep, review round 1): a purge job that went
    /// dead is retried. The job's settings are the plugin's stored
    /// `plugin.<name>.settings.*` options (so keys a later version dropped go too); its
    /// unique key is the deletion's plus `:sweep:<sweep_at micros>`, one per sweep run.
    /// Returns how many jobs were enqueued.
    pub async fn enqueue_overdue_purges(
        &self,
        now: Timestamp,
        sweep_at: Timestamp,
    ) -> Result<u64, CoreError> {
        let repos = self.store.read();
        let mut markers: BTreeMap<String, i64> = BTreeMap::new();
        let mut settings: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut after = String::new();
        loop {
            let page = repos
                .options()
                .list_prefix(PLUGIN_OPTION_PREFIX, &after, OPTION_PAGE)
                .await?;
            for entry in &page {
                match parse_plugin_option(&entry.key) {
                    Some((name, PluginOption::DeletedAt)) => {
                        if let Some(micros) = entry.value.as_i64() {
                            markers.insert(name.to_owned(), micros);
                        }
                    }
                    Some((name, PluginOption::Setting(key))) => settings
                        .entry(name.to_owned())
                        .or_default()
                        .push(key.to_owned()),
                    None => {}
                }
            }
            match page.last() {
                Some(last) if page.len() >= OPTION_PAGE as usize => after = last.key.clone(),
                _ => break,
            }
        }
        let mut enqueued = 0;
        for (name, micros) in markers {
            let deleted_at = Timestamp::from_micros(micros)
                .map_err(|e| CoreError::Invalid(format!("deletion marker of {name}: {e}")))?;
            if crate::jobs::after(deleted_at, PLUGIN_DATA_RETENTION) > now
                || repos.plugins().get(&name).await?.is_some()
            {
                continue;
            }
            let mut job = plugin_purge_job(&PluginPurge {
                v: PLUGIN_PURGE_VERSION,
                settings: settings.remove(&name).unwrap_or_default(),
                name,
                deleted_at,
            });
            job.run_at = None;
            job.unique_key = job
                .unique_key
                .map(|key| format!("{key}:sweep:{}", sweep_at.as_micros()));
            if repos.jobs().enqueue(&job).await?.is_some() {
                enqueued += 1;
            }
        }
        Ok(enqueued)
    }

    async fn purge_locked(
        &self,
        name: &str,
        deleted_at: Timestamp,
        settings: &[String],
        keep_marker: bool,
    ) -> Result<bool, CoreError> {
        let repos = self.store.read();
        if repos.plugins().get(name).await?.is_some() {
            return Ok(false);
        }
        let marker = repos.options().get(&deleted_marker(name)).await?;
        if marker.and_then(|m| m.value.as_i64()) != Some(deleted_at.as_micros()) {
            return Ok(false);
        }
        // Keeping the marker (the immediate erase of a `purge=true` delete), the erase
        // records itself in its own transaction: only then is the deletion no longer
        // listed as retained.
        self.erase_locked(name, settings, keep_marker.then_some(deleted_at), None)
            .await?;
        Ok(true)
    }

    /// The deleted plugins whose KV and settings are still kept, by name, with the day
    /// the purge job erases them (M2b-3 P47).
    pub async fn retained_plugins(&self) -> Result<Vec<RetainedPluginData>, CoreError> {
        let repos = self.store.read();
        let retained = repos.plugins().retained().await?;
        let mut out = Vec::with_capacity(retained.len());
        for r in retained {
            // Deleted with `purge=true`: erased already, only the marker is left.
            if erased_with(repos, &r.name, r.deleted_at.as_micros()).await? {
                continue;
            }
            out.push(RetainedPluginData {
                purge_after: crate::jobs::after(r.deleted_at, PLUGIN_DATA_RETENTION),
                name: r.name,
                deleted_at: r.deleted_at,
            });
        }
        Ok(out)
    }

    /// Erases the retained data of the deleted plugin `name` now, instead of at the end
    /// of the retention (M2b-3 P47): its KV, every stored `plugin.<name>.settings.*`
    /// option and its deletion marker, under the `extensions` lock, with `audit` in the
    /// options' transaction. A plugin `name` installed is `InvalidField { field: "name"
    /// }`; no deletion marker (nothing retained, or already purged) is `NotFound`. The
    /// delete's purge job later finds no marker and does nothing.
    pub(crate) async fn purge_retained_with(
        &self,
        name: &str,
        audit: Option<NewAudit>,
    ) -> Result<(), CoreError> {
        // Plugin names have no `.`: such a name would address another plugin's options.
        if name.is_empty() || name.contains('.') {
            return Err(CoreError::NotFound);
        }
        let guard = lock(self.store.as_ref(), LOCK_EXTENSIONS, self.lock_wait).await?;
        let done = self.purge_retained_locked(name, audit).await;
        release(guard).await;
        done
    }

    async fn purge_retained_locked(
        &self,
        name: &str,
        audit: Option<NewAudit>,
    ) -> Result<(), CoreError> {
        let repos = self.store.read();
        if repos.plugins().get(name).await?.is_some() {
            return Err(CoreError::invalid_field(
                "name",
                "a plugin of this name is installed; delete it to erase its data",
            ));
        }
        let marker = repos.options().get(&deleted_marker(name)).await?;
        let Some(deleted_at) = marker.and_then(|m| m.value.as_i64()) else {
            return Err(CoreError::NotFound);
        };
        // Deleted with `purge=true`: nothing retained (the delete's sweep removes the marker).
        if erased_with(repos, name, deleted_at).await? {
            return Err(CoreError::NotFound);
        }
        let settings = stored_setting_keys(repos, name).await?;
        self.erase_locked(name, &settings, None, audit).await
    }

    /// Deletes `name`'s KV, its `settings` options and its deletion marker, appending
    /// `audit` with the options; with `keep_marker` (the deletion's time) the marker
    /// stays and the erase is recorded as `plugin.<name>.erased_at` instead. The caller
    /// holds the `extensions` lock and has checked that no plugin `name` is installed.
    async fn erase_locked(
        &self,
        name: &str,
        settings: &[String],
        keep_marker: Option<Timestamp>,
        audit: Option<NewAudit>,
    ) -> Result<(), CoreError> {
        let repos = self.store.read();
        // Batches outside any `Tx`, so no transaction holds the writer for long.
        loop {
            let deleted = repos.plugin_kv().delete_all(name, KV_PURGE_BATCH).await?;
            if deleted < u64::from(KV_PURGE_BATCH) {
                break;
            }
        }
        let tx = self.store.begin().await?;
        let done = match delete_options(tx.as_ref(), name, settings, keep_marker).await {
            Ok(()) => append_audit(tx.as_ref(), audit).await,
            Err(e) => Err(e),
        };
        finish(tx, done).await
    }

    /// Takes the `extensions` lock, then opens the write transaction. The lock is
    /// released by [`finish_locked`] after the transaction ends, so the next holder sees
    /// the committed rows. (Plain async fns rather than closure helpers: rustc cannot
    /// prove an `AsyncFnOnce(&dyn Repos)` future `Send`.)
    async fn begin_locked(&self) -> Result<(Box<dyn LockGuard>, Box<dyn Tx>), CoreError> {
        let guard = lock(self.store.as_ref(), LOCK_EXTENSIONS, self.lock_wait).await?;
        match self.store.begin().await {
            Ok(tx) => Ok((guard, tx)),
            Err(e) => {
                release(guard).await;
                Err(e.into())
            }
        }
    }
}

/// Commits `tx` on `Ok` (rolls back on `Err`), then releases the lock.
async fn finish_locked<T>(
    guard: Box<dyn LockGuard>,
    tx: Box<dyn Tx>,
    result: Result<T, CoreError>,
) -> Result<T, CoreError> {
    let done = finish(tx, result).await;
    release(guard).await;
    done
}

async fn write_install_theme(
    repos: &dyn Repos,
    artifact: &Artifact,
    row: &InstalledTheme,
    activate: bool,
    audit: Option<NewAudit>,
) -> Result<(), CoreError> {
    let before = active_theme(repos).await?;
    record_artifact(repos, artifact).await?;
    repos.themes().upsert(row).await?;
    if activate {
        repos.themes().activate(&row.name).await?;
    }
    emit_if_active_changed(repos, before).await?;
    append_audit(repos, audit).await
}

async fn write_activate_theme(
    repos: &dyn Repos,
    name: &str,
    expected: &ferrite_domain::ArtifactHash,
    audit: Option<NewAudit>,
) -> Result<(), CoreError> {
    let row = repos.themes().get(name).await?.ok_or(CoreError::NotFound)?;
    if &row.artifact_hash != expected {
        return Err(CoreError::Changed(
            "the theme was replaced after it was checked",
        ));
    }
    let before = active_theme(repos).await?;
    repos.themes().activate(name).await?;
    emit_if_active_changed(repos, before).await?;
    append_audit(repos, audit).await
}

/// Also removes a deletion marker left by an earlier delete: the reinstalled plugin
/// keeps its data and no pending purge job may remove it.
async fn write_install_plugin(
    repos: &dyn Repos,
    artifact: &Artifact,
    row: &InstalledPlugin,
    trust: &InstallTrust,
    audit: Option<NewAudit>,
) -> Result<TrustDecision, CoreError> {
    // Decided on committed state, before anything is written (design 14 §4.2).
    let decision = trust::decide(repos, &row.name, trust).await?;
    trust::write_trust(repos, &row.name, trust, &decision, Timestamp::now()).await?;
    let audit = audit.map(|mut a| {
        if let Some(details) = a.details.as_object_mut() {
            details.insert(
                "signer".into(),
                json!(trust.signer.as_ref().map(|s| s.key.key_id.to_string())),
            );
            details.insert(
                "owner".into(),
                json!(decision.next_owner.as_ref().map(ToString::to_string)),
            );
            details.insert("source".into(), json!(trust.source.as_str()));
        }
        a
    });
    record_artifact(repos, artifact).await?;
    let (enabled, disabled_reason) = match repos.plugins().get(&row.name).await? {
        Some(existing) => {
            // A new artifact starts with clean failure counters (P20).
            if existing.artifact_hash != row.artifact_hash {
                repos.plugin_failures().clear(&row.name).await?;
            }
            (existing.enabled, existing.disabled_reason)
        }
        None => (false, None),
    };
    let row = InstalledPlugin {
        enabled,
        disabled_reason,
        ..row.clone()
    };
    repos.plugins().upsert(&row).await?;
    for marker in [deleted_marker(&row.name), erased_marker(&row.name)] {
        if repos.options().get(&marker).await?.is_some() {
            delete_option(repos, &marker).await?;
        }
    }
    repos
        .events()
        .append(&events::plugins_changed(&row.name))
        .await?;
    append_audit(repos, audit).await?;
    Ok(decision)
}

/// The serial rule, the cache row and the index's revocations (see
/// [`ExtensionService::store_registry`]).
async fn write_registry(
    repos: &dyn Repos,
    row: &RegistryCacheRow,
    index: &RegistryIndex,
    stale: Option<&str>,
) -> Result<RegistryRefresh, CoreError> {
    let trust = repos.plugin_trust();
    let cached = trust
        .get_registry(&row.url)
        .await?
        .filter(|c| stale != Some(c.index_json.as_str()));
    let stored = match cached {
        Some(cached) if cached.serial > row.serial => {
            return Err(CoreError::invalid_field(
                "serial",
                format!(
                    "the registry served serial {}, older than the cached {} (a rollback)",
                    row.serial, cached.serial
                ),
            ));
        }
        Some(cached) if cached.serial == row.serial => {
            if cached.index_json != row.index_json {
                return Err(CoreError::invalid_field(
                    "serial",
                    format!(
                        "the registry served serial {} with other bytes than the cached copy",
                        row.serial
                    ),
                ));
            }
            trust
                .registry_checked(&row.url, row.checked_at, None)
                .await?;
            RegistryStored::Unchanged
        }
        _ => {
            trust.put_registry(row).await?;
            RegistryStored::Stored
        }
    };
    let mut disabled = Vec::new();
    let mut collisions = Vec::new();
    for site_key in trust.list_keys().await? {
        let Some(p) = index.collides(&site_key.key) else {
            continue;
        };
        let id = site_key.key.key_id;
        let reason = format!(
            "publisher key {id} has the id of the registry's key for {} but other key bytes",
            p.name
        );
        let mut off = Vec::new();
        for owner in trust.owners_signed_by(&id).await? {
            let Some(plugin) = repos.plugins().get_for_update(&owner.plugin).await? else {
                continue;
            };
            if plugin.enabled {
                repos
                    .plugins()
                    .set_enabled(&owner.plugin, false, Some(&reason))
                    .await?;
                repos
                    .events()
                    .append(&events::plugins_changed(&owner.plugin))
                    .await?;
                off.push(owner.plugin);
            }
        }
        trust.delete_key(&id).await?;
        append_audit(
            repos,
            Some(NewAudit {
                actor_id: None,
                action: "plugin.publisher.collision".to_owned(),
                target_kind: "publisher".to_owned(),
                target_id: Some(id.to_string()),
                details: json!({
                    "key_id": id.to_string(),
                    "registry_publisher": p.id,
                    "removed_label": site_key.label,
                    "disabled": off,
                }),
            }),
        )
        .await?;
        tracing::warn!(key = %id, publisher = %p.id, disabled = ?off, "{reason}; the site key was removed");
        disabled.extend(off);
        collisions.push(id);
    }
    for publisher in &index.publishers {
        for revoked in &publisher.revoked_keys {
            let current = trust
                .get_key(&revoked.key.key_id)
                .await?
                .filter(|k| k.key == revoked.key);
            let same = current
                .as_ref()
                .and_then(|k| k.revoked.as_ref())
                .is_some_and(|r| r.compromised || !revoked.compromised);
            if same {
                continue;
            }
            let revocation = Revocation {
                at: revoked.revoked_at,
                reason: revoked.reason.clone(),
                compromised: revoked.compromised,
            };
            disabled.extend(
                trust::write_revocation(
                    repos,
                    &revoked.key,
                    &revocation,
                    &publisher.name,
                    KeySource::Registry,
                    None,
                )
                .await?,
            );
        }
    }
    Ok(RegistryRefresh {
        stored,
        disabled,
        collisions,
    })
}

/// The manifest's `plugin.publisher` text of a stored row (empty when absent).
pub(crate) fn manifest_publisher(manifest: &Value) -> String {
    manifest
        .pointer("/plugin/publisher")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

async fn write_plugin_enabled(
    repos: &dyn Repos,
    name: &str,
    enabled: bool,
    audit: Option<NewAudit>,
) -> Result<(), CoreError> {
    repos.plugins().set_enabled(name, enabled, None).await?;
    if enabled {
        repos.plugin_failures().clear(name).await?;
    }
    repos
        .events()
        .append(&events::plugins_changed(name))
        .await?;
    append_audit(repos, audit).await
}

async fn write_delete_plugin(
    repos: &dyn Repos,
    name: &str,
    deleted_at: Timestamp,
    purge_now: bool,
    audit: Option<NewAudit>,
) -> Result<InstalledPlugin, CoreError> {
    let row = repos
        .plugins()
        .get_for_update(name)
        .await?
        .ok_or(CoreError::NotFound)?;
    repos.plugins().delete(name).await?;
    repos.plugin_failures().clear(name).await?;
    let marker = deleted_marker(name);
    repos
        .options()
        .set(&marker, &json!(deleted_at.as_micros()), false)
        .await?;
    repos
        .events()
        .append(&events::options_changed(&marker))
        .await?;
    // An earlier `purge=true` deletion's record; this deletion's is written by its
    // own erase, once that has run.
    let erased = erased_marker(name);
    if repos.options().get(&erased).await?.is_some() {
        delete_option(repos, &erased).await?;
    }
    let mut job = plugin_purge_job(&PluginPurge {
        v: PLUGIN_PURGE_VERSION,
        name: name.to_owned(),
        deleted_at,
        settings: setting_keys(&row.manifest),
    });
    if purge_now {
        job.run_at = Some(crate::jobs::after(deleted_at, PLUGIN_PURGE_GRACE));
    }
    // `None` (key taken) needs a second delete of the same name in the same
    // microsecond; that deletion's job is the one already queued.
    repos.jobs().enqueue(&job).await?;
    repos
        .events()
        .append(&events::plugins_changed(name))
        .await?;
    append_audit(repos, audit).await?;
    Ok(row)
}

/// Deletes the purged plugin's settings options and its deletion marker; with
/// `keep_marker` (the deletion's time) the marker stays and `erased_at` records it.
async fn delete_options(
    repos: &dyn Repos,
    name: &str,
    settings: &[String],
    keep_marker: Option<Timestamp>,
) -> Result<(), CoreError> {
    for key in settings {
        delete_option(repos, &crate::ext_settings::plugin_option(name, key)).await?;
    }
    let erased = erased_marker(name);
    if let Some(deleted_at) = keep_marker {
        repos
            .options()
            .set(&erased, &json!(deleted_at.as_micros()), false)
            .await?;
        repos
            .events()
            .append(&events::options_changed(&erased))
            .await?;
        return Ok(());
    }
    if repos.options().get(&erased).await?.is_some() {
        delete_option(repos, &erased).await?;
    }
    // The data is gone: the name is free for any publisher (design 14 §4.3).
    repos.plugin_trust().delete_owner(name).await?;
    delete_option(repos, &deleted_marker(name)).await
}

/// Whether plugin `name`'s deletion at `deleted_at` (unix micros) was a `purge=true`
/// one, its data already erased.
async fn erased_with(repos: &dyn Repos, name: &str, deleted_at: i64) -> Result<bool, CoreError> {
    let erased = repos.options().get(&erased_marker(name)).await?;
    Ok(erased.and_then(|o| o.value.as_i64()) == Some(deleted_at))
}

/// A plugin-owned option the sweep reads.
enum PluginOption<'a> {
    DeletedAt,
    Setting(&'a str),
}

/// `plugin.<name>.deleted_at` or `plugin.<name>.settings.<key>` (names have no `.`).
fn parse_plugin_option(key: &str) -> Option<(&str, PluginOption<'_>)> {
    let (name, rest) = key.strip_prefix(PLUGIN_OPTION_PREFIX)?.split_once('.')?;
    if rest == DELETED_AT {
        return Some((name, PluginOption::DeletedAt));
    }
    let setting = rest.strip_prefix("settings.")?;
    Some((name, PluginOption::Setting(setting)))
}

async fn delete_option(repos: &dyn Repos, key: &str) -> Result<(), CoreError> {
    repos.options().delete(key).await?;
    repos.events().append(&events::options_changed(key)).await?;
    Ok(())
}

/// The keys of plugin `name`'s stored settings options (`plugin.<name>.settings.<key>`),
/// whatever its last manifest declared.
async fn stored_setting_keys(repos: &dyn Repos, name: &str) -> Result<Vec<String>, CoreError> {
    let prefix = crate::ext_settings::plugin_option(name, "");
    let mut keys = Vec::new();
    let mut after = String::new();
    loop {
        let page = repos
            .options()
            .list_prefix(&prefix, &after, OPTION_PAGE)
            .await?;
        keys.extend(
            page.iter()
                .filter_map(|e| e.key.strip_prefix(&prefix).map(str::to_owned)),
        );
        match page.last() {
            Some(last) if page.len() >= OPTION_PAGE as usize => after = last.key.clone(),
            _ => break,
        }
    }
    Ok(keys)
}

/// The option recording when plugin `name` was last deleted (unix micros). Hidden from
/// plugins by the `plugin.` rule of `options.get`.
fn deleted_marker(name: &str) -> String {
    format!("{PLUGIN_OPTION_PREFIX}{name}.{DELETED_AT}")
}

/// The option recording that plugin `name`'s last deletion erased its data at once
/// (`purge=true`; unix micros of that deletion). Hidden from plugins like the marker.
fn erased_marker(name: &str) -> String {
    format!("{PLUGIN_OPTION_PREFIX}{name}.{ERASED_AT}")
}

/// The keys of a row manifest's `[settings]` table (sorted).
pub(crate) fn setting_keys(manifest: &Value) -> Vec<String> {
    manifest
        .get("settings")
        .and_then(Value::as_object)
        .map(|table| table.keys().cloned().collect())
        .unwrap_or_default()
}

async fn append_audit(repos: &dyn Repos, audit: Option<NewAudit>) -> Result<(), CoreError> {
    if let Some(entry) = audit {
        repos.audit().append(&entry).await?;
    }
    Ok(())
}

/// `Invalid` unless `artifact` has `kind` and the row names its hash.
fn check_artifact(
    artifact: &Artifact,
    kind: ArtifactKind,
    row_hash: &ferrite_domain::ArtifactHash,
) -> Result<(), CoreError> {
    if artifact.kind != kind {
        return Err(CoreError::Invalid(format!(
            "artifact kind is {}, expected {}",
            artifact.kind.as_str(),
            kind.as_str()
        )));
    }
    if &artifact.hash != row_hash {
        return Err(CoreError::Invalid(format!(
            "row names artifact {row_hash}, given {}",
            artifact.hash
        )));
    }
    Ok(())
}

/// Inserts `artifact` unless its hash exists, then checks the stored row's kind: the
/// same bytes recorded earlier as another kind (say, a media upload) are `Invalid`.
async fn record_artifact(repos: &dyn Repos, artifact: &Artifact) -> Result<(), CoreError> {
    repos.artifacts().insert_if_absent(artifact).await?;
    let stored = repos
        .artifacts()
        .get(&artifact.hash)
        .await?
        .ok_or(CoreError::NotFound)?;
    if stored.kind != artifact.kind {
        return Err(CoreError::Invalid(format!(
            "artifact {} is already stored as kind {}, not {}",
            artifact.hash,
            stored.kind.as_str(),
            artifact.kind.as_str()
        )));
    }
    Ok(())
}

/// What identifies the active theme's files: its name and artifact.
type ActiveTheme = Option<(String, ferrite_domain::ArtifactHash)>;

async fn active_theme(repos: &dyn Repos) -> Result<ActiveTheme, CoreError> {
    Ok(repos
        .themes()
        .active()
        .await?
        .map(|t| (t.name, t.artifact_hash)))
}

/// Appends `theme.changed` if the active theme differs from `before`.
async fn emit_if_active_changed(repos: &dyn Repos, before: ActiveTheme) -> Result<(), CoreError> {
    let after = active_theme(repos).await?;
    if let Some((name, _)) = after.as_ref().filter(|now| Some(*now) != before.as_ref()) {
        repos.events().append(&events::theme_changed(name)).await?;
    }
    Ok(())
}

/// Releases the lock; a failure is logged (the lock goes away with its connection).
async fn release(guard: Box<dyn LockGuard>) {
    if let Err(e) = guard.release().await {
        tracing::warn!(error = %e, lock = LOCK_EXTENSIONS, "releasing the lock failed");
    }
}
