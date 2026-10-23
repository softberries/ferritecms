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
//! later version dropped from its schema are not purged.
//!
//! [`PLUGIN_DATA_RETENTION`]: crate::jobs::PLUGIN_DATA_RETENTION
//! [`PLUGIN_PURGE_KIND`]: crate::jobs::PLUGIN_PURGE_KIND

mod admin;

use std::{sync::Arc, time::Duration};

use ferrite_domain::{
    Artifact, ArtifactKind, InstalledPlugin, InstalledTheme, NewAudit, Timestamp,
};
use ferrite_store::{LOCK_EXTENSIONS, LockGuard, Repos, Store, Tx, lock};
use serde_json::{Value, json};

use crate::{
    CoreError, events,
    jobs::{PLUGIN_PURGE_VERSION, PluginPurge, plugin_purge_job},
    tx::finish,
};

pub use admin::ExtensionAdmin;

/// KV keys deleted per statement by a purge.
const KV_PURGE_BATCH: u32 = 1_000;

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
    /// `CoreError::Store(StoreError::Busy)` (default [`EXTENSIONS_LOCK_WAIT`]).
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

    /// Makes `name` the only active theme; a missing theme is `NotFound` and the active
    /// theme stays. Emits `theme.changed` unless `name` was already active.
    pub async fn activate_theme(&self, name: &str) -> Result<(), CoreError> {
        self.activate_theme_with(name, None).await
    }

    pub(crate) async fn activate_theme_with(
        &self,
        name: &str,
        audit: Option<NewAudit>,
    ) -> Result<(), CoreError> {
        let (guard, tx) = self.begin_locked().await?;
        let done = write_activate_theme(tx.as_ref(), name, audit).await;
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
    pub async fn install_plugin(
        &self,
        artifact: &Artifact,
        row: &InstalledPlugin,
    ) -> Result<(), CoreError> {
        self.install_plugin_with(artifact, row, None).await
    }

    pub(crate) async fn install_plugin_with(
        &self,
        artifact: &Artifact,
        row: &InstalledPlugin,
        audit: Option<NewAudit>,
    ) -> Result<(), CoreError> {
        check_artifact(artifact, ArtifactKind::Plugin, &row.artifact_hash)?;
        let (guard, tx) = self.begin_locked().await?;
        let done = write_install_plugin(tx.as_ref(), artifact, row, audit).await;
        finish_locked(guard, tx, done).await
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

    /// [`delete_plugin`](Self::delete_plugin) at `deleted_at`; with `purge_now` the
    /// purge job is due at once (F16: it finishes a purge the caller starts after the
    /// commit if that one fails).
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
        let guard = lock(self.store.as_ref(), LOCK_EXTENSIONS, self.lock_wait).await?;
        let done = self.purge_locked(name, deleted_at, settings).await;
        release(guard).await;
        done
    }

    async fn purge_locked(
        &self,
        name: &str,
        deleted_at: Timestamp,
        settings: &[String],
    ) -> Result<bool, CoreError> {
        let repos = self.store.read();
        if repos.plugins().get(name).await?.is_some() {
            return Ok(false);
        }
        let marker = repos.options().get(&deleted_marker(name)).await?;
        if marker.and_then(|m| m.value.as_i64()) != Some(deleted_at.as_micros()) {
            return Ok(false);
        }
        // Batches outside any `Tx`, so no transaction holds the writer for long.
        loop {
            let deleted = repos.plugin_kv().delete_all(name, KV_PURGE_BATCH).await?;
            if deleted < u64::from(KV_PURGE_BATCH) {
                break;
            }
        }
        let tx = self.store.begin().await?;
        let done = delete_options(tx.as_ref(), name, settings).await;
        finish(tx, done).await?;
        Ok(true)
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
    audit: Option<NewAudit>,
) -> Result<(), CoreError> {
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
    audit: Option<NewAudit>,
) -> Result<(), CoreError> {
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
    let marker = deleted_marker(&row.name);
    if repos.options().get(&marker).await?.is_some() {
        delete_option(repos, &marker).await?;
    }
    repos
        .events()
        .append(&events::plugins_changed(&row.name))
        .await?;
    append_audit(repos, audit).await
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
    let mut job = plugin_purge_job(&PluginPurge {
        v: PLUGIN_PURGE_VERSION,
        name: name.to_owned(),
        deleted_at,
        settings: setting_keys(&row.manifest),
    });
    if purge_now {
        // Due at once (the database clock).
        job.run_at = None;
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

/// Deletes the purged plugin's settings options and its deletion marker.
async fn delete_options(
    repos: &dyn Repos,
    name: &str,
    settings: &[String],
) -> Result<(), CoreError> {
    for key in settings {
        delete_option(repos, &format!("plugin.{name}.settings.{key}")).await?;
    }
    delete_option(repos, &deleted_marker(name)).await
}

async fn delete_option(repos: &dyn Repos, key: &str) -> Result<(), CoreError> {
    repos.options().delete(key).await?;
    repos.events().append(&events::options_changed(key)).await?;
    Ok(())
}

/// The option recording when plugin `name` was last deleted (unix micros). Hidden from
/// plugins by the `plugin.` rule of `options.get`.
fn deleted_marker(name: &str) -> String {
    format!("plugin.{name}.deleted_at")
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
