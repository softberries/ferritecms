//! Theme and plugin install/activation writes (design 04 §7–8, ruling P21).
//!
//! Each write takes the `extensions` lock (cluster-wide on Postgres, in process on
//! SQLite), then writes the artifact row, the theme or plugin row and its event in one
//! transaction. The artifact's bytes must already be in the media store: the caller
//! uploads them **before** (an orphaned upload is harmless and collected later, 04 §8),
//! so a node that sees the event can always fetch what the row names.

use std::{sync::Arc, time::Duration};

use ferrite_domain::{Artifact, ArtifactKind, InstalledPlugin, InstalledTheme};
use ferrite_store::{LOCK_EXTENSIONS, LockGuard, Repos, Store, lock};

use crate::{CoreError, events, tx::finish};

/// How long a write waits for the `extensions` lock by default.
pub const EXTENSIONS_LOCK_WAIT: Duration = Duration::from_secs(30);

/// Installs and activates themes and plugins.
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
        check_artifact(artifact, ArtifactKind::Theme, &theme.artifact_hash)?;
        // Activation goes through `activate` only, so the one-active index never trips.
        let row = InstalledTheme {
            active: false,
            ..theme.clone()
        };
        self.locked(async |repos: &dyn Repos| {
            let before = active_theme(repos).await?;
            repos.artifacts().insert_if_absent(artifact).await?;
            repos.themes().upsert(&row).await?;
            if activate {
                repos.themes().activate(&row.name).await?;
            }
            emit_if_active_changed(repos, before).await
        })
        .await
    }

    /// Makes `name` the only active theme; a missing theme is `NotFound` and the active
    /// theme stays. Emits `theme.changed` unless `name` was already active.
    pub async fn activate_theme(&self, name: &str) -> Result<(), CoreError> {
        self.locked(async |repos: &dyn Repos| {
            let before = active_theme(repos).await?;
            repos.themes().activate(name).await?;
            emit_if_active_changed(repos, before).await
        })
        .await
    }

    /// Records the plugin artifact (kind `plugin`, inserted if absent) and inserts or
    /// replaces the plugin row as given; emits `plugins.changed` with its name. `Invalid`
    /// when `artifact` is not a plugin artifact or `row.artifact_hash` is not its hash.
    pub async fn install_plugin(
        &self,
        artifact: &Artifact,
        row: &InstalledPlugin,
    ) -> Result<(), CoreError> {
        check_artifact(artifact, ArtifactKind::Plugin, &row.artifact_hash)?;
        self.locked(async |repos: &dyn Repos| {
            repos.artifacts().insert_if_absent(artifact).await?;
            repos.plugins().upsert(row).await?;
            repos
                .events()
                .append(&events::plugins_changed(&row.name))
                .await?;
            Ok(())
        })
        .await
    }

    /// Enables or disables the plugin `name` (missing: `NotFound`) and clears its
    /// `disabled_reason` either way (an admin decision replaces an automatic one);
    /// emits `plugins.changed` with its name.
    pub async fn set_plugin_enabled(&self, name: &str, enabled: bool) -> Result<(), CoreError> {
        self.locked(async |repos: &dyn Repos| {
            repos.plugins().set_enabled(name, enabled, None).await?;
            repos
                .events()
                .append(&events::plugins_changed(name))
                .await?;
            Ok(())
        })
        .await
    }

    /// Runs `write` in one transaction while holding the `extensions` lock. The lock is
    /// released after the transaction ends, so the next holder sees the committed rows.
    async fn locked(
        &self,
        write: impl AsyncFnOnce(&dyn Repos) -> Result<(), CoreError>,
    ) -> Result<(), CoreError> {
        let guard = lock(self.store.as_ref(), LOCK_EXTENSIONS, self.lock_wait).await?;
        let done = self.in_tx(write).await;
        release(guard).await;
        done
    }

    async fn in_tx(
        &self,
        write: impl AsyncFnOnce(&dyn Repos) -> Result<(), CoreError>,
    ) -> Result<(), CoreError> {
        let tx = self.store.begin().await?;
        let done = write(tx.as_ref()).await;
        finish(tx, done).await
    }
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
