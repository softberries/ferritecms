//! Hot reload of the plugin registry from the `plugins` rows (plan rulings P18, P19).
//!
//! [`PluginReloader::reload`] re-reads every row and brings the [`PluginHost`] to it in
//! one registry swap ([`PluginHost::commit`]):
//!
//! * an enabled row whose artifact, manifest and grant equal what this reloader loaded,
//!   and that the host has not disabled, is **kept** without fetching or recompiling
//!   (P19: reuse is in memory, by artifact);
//! * a new or changed row, or a row enabled again after a host-side disable (its
//!   `updated_at` moved), is fetched (SHA-256 checked), its manifest parsed, its grant
//!   checked (R21) and the component prepared on a blocking thread;
//! * if any of that fails, the previously loaded version keeps running (reported as
//!   [`PluginStatus::Failed`]); a plugin with no previous version stays absent;
//! * disabled and deleted rows are dropped.
//!
//! Calls already running finish on the registry snapshot they loaded (old instances
//! included); calls that start after the swap use the new one. Reloads are serialised.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use ferrite_domain::{ArtifactHash, InstalledPlugin, Timestamp};
use ferrite_media::MediaStore;
use ferrite_plugin_host::{Manifest, PluginError, PluginHost, PreparedPlugin};
use ferrite_store::{Store, StoreError};
use serde_json::Value;
use tokio::sync::Mutex;

use super::SharedFailures;
use super::load::{check_row, hash_of};

/// Compiles one plugin; [`PluginHost::prepare`] unless replaced in tests.
type PrepareFn =
    Arc<dyn Fn(&PluginHost, Manifest, &[u8]) -> Result<PreparedPlugin, PluginError> + Send + Sync>;

/// What a [`PluginReloader::reload`] left each plugin row at.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReloadReport {
    /// One entry per row (deleted rows are absent), by name.
    pub states: BTreeMap<String, PluginState>,
}

/// One row's outcome; `version` and `hash` are the row's (what was asked for).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginState {
    pub version: String,
    pub hash: ArtifactHash,
    pub status: PluginStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginStatus {
    /// The row's version is loaded and enabled.
    Loaded,
    /// The row's version could not be loaded (the reason says whether an older version
    /// keeps running).
    Failed(String),
    /// Not running: the row is disabled (its reason, or `"disabled"`), or the host
    /// disabled the loaded instance (the host's reason) and the row has not changed
    /// since.
    Disabled(String),
}

/// What identifies a loaded plugin: the row fields it was prepared from.
#[derive(Debug, Clone, PartialEq)]
struct Loaded {
    hash: ArtifactHash,
    manifest: Value,
    granted: Value,
    updated_at: Timestamp,
}

impl Loaded {
    fn of(row: &InstalledPlugin) -> Self {
        Self {
            hash: row.artifact_hash.clone(),
            manifest: row.manifest.clone(),
            granted: row.granted.clone(),
            updated_at: row.updated_at,
        }
    }

    fn same_artifact(&self, row: &InstalledPlugin) -> bool {
        self.hash == row.artifact_hash
            && self.manifest == row.manifest
            && self.granted == row.granted
    }
}

/// Keeps a [`PluginHost`] in step with the `plugins` rows (see the module docs).
pub struct PluginReloader {
    store: Arc<dyn Store>,
    media: Arc<MediaStore>,
    host: Arc<PluginHost>,
    prepare: PrepareFn,
    /// Told the row version of every committed plugin (stale failure counts are dropped).
    failures: Option<Arc<SharedFailures>>,
    /// What the last reload committed, per plugin; the lock serialises reloads.
    loaded: Mutex<BTreeMap<String, Loaded>>,
}

impl PluginReloader {
    pub fn new(store: Arc<dyn Store>, media: Arc<MediaStore>, host: Arc<PluginHost>) -> Self {
        Self {
            store,
            media,
            host,
            prepare: Arc::new(|host, manifest, wasm| host.prepare(manifest, wasm)),
            failures: None,
            loaded: Mutex::new(BTreeMap::new()),
        }
    }

    /// After each commit, tell `failures` the row version (`updated_at`) of every plugin
    /// in the host, so failure counts recorded before a row changed (for example before
    /// it was enabled again, which cleared its shared counters) are dropped (R12).
    pub fn with_failures(mut self, failures: Arc<SharedFailures>) -> Self {
        self.failures = Some(failures);
        self
    }

    /// Test seam: compile through `prepare` instead of [`PluginHost::prepare`] (for
    /// example a wrapper that counts compiles).
    #[doc(hidden)]
    pub fn with_prepare(
        mut self,
        prepare: impl Fn(&PluginHost, Manifest, &[u8]) -> Result<PreparedPlugin, PluginError>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        self.prepare = Arc::new(prepare);
        self
    }

    /// Bring the host to the current rows (see the module docs) with one registry swap.
    /// Only failing to list the rows is an error, and then nothing changes.
    pub async fn reload(&self) -> Result<ReloadReport, StoreError> {
        let mut loaded = self.loaded.lock().await;
        let mut rows = self.store.read().plugins().list().await?;
        rows.sort_by(|a, b| a.name.cmp(&b.name));
        let in_host: BTreeSet<String> = self.host.loaded().into_iter().collect();

        let mut report = ReloadReport::default();
        let mut prepared = Vec::new();
        let mut keep = Vec::new();
        let mut next = BTreeMap::new();
        // Row version of every plugin left in the host (kept, prepared or kept after a
        // failed update: the row's current `updated_at` in each case).
        let mut versions = HashMap::new();
        for row in rows {
            let state = |status| PluginState {
                version: row.version.clone(),
                hash: row.artifact_hash.clone(),
                status,
            };
            if !row.enabled {
                let reason = row
                    .disabled_reason
                    .clone()
                    .unwrap_or_else(|| "disabled".to_owned());
                report
                    .states
                    .insert(row.name.clone(), state(PluginStatus::Disabled(reason)));
                continue;
            }
            let current = loaded
                .get(&row.name)
                .filter(|_| in_host.contains(&row.name));
            let unchanged = current.filter(|c| c.same_artifact(&row));
            let host_disabled = self.host.disabled_reason(&row.name);
            let status = match (unchanged, host_disabled) {
                (Some(cur), None) => {
                    keep.push(row.name.clone());
                    next.insert(row.name.clone(), cur.clone());
                    PluginStatus::Loaded
                }
                // Disabled on this host and the row has not been touched since.
                (Some(cur), Some(reason)) if cur.updated_at == row.updated_at => {
                    keep.push(row.name.clone());
                    next.insert(row.name.clone(), cur.clone());
                    PluginStatus::Disabled(reason)
                }
                _ => match self.prepare_row(&row).await {
                    Ok(p) => {
                        prepared.push(p);
                        next.insert(row.name.clone(), Loaded::of(&row));
                        PluginStatus::Loaded
                    }
                    Err(reason) => {
                        tracing::warn!(
                            plugin = %row.name,
                            hash = %row.artifact_hash,
                            %reason,
                            "plugin not loaded"
                        );
                        match current {
                            Some(cur) => {
                                keep.push(row.name.clone());
                                next.insert(row.name.clone(), cur.clone());
                                PluginStatus::Failed(format!(
                                    "{reason} (still running {})",
                                    cur.hash
                                ))
                            }
                            None => PluginStatus::Failed(reason),
                        }
                    }
                },
            };
            if next.contains_key(&row.name) {
                versions.insert(row.name.clone(), row.updated_at);
            }
            report.states.insert(row.name.clone(), state(status));
        }

        let compiled = prepared.len();
        let keep: Vec<&str> = keep.iter().map(String::as_str).collect();
        self.host.commit(prepared, &keep);
        if let Some(failures) = &self.failures {
            failures.set_versions(versions);
        }
        *loaded = next;
        tracing::info!(
            compiled,
            kept = keep.len(),
            running = loaded.len(),
            "plugin registry reloaded"
        );
        Ok(report)
    }

    /// Fetch, check and compile `row`'s plugin (the slow part on a blocking thread).
    async fn prepare_row(&self, row: &InstalledPlugin) -> Result<PreparedPlugin, String> {
        let manifest = check_row(row)?;
        let wasm = self
            .media
            .get(&row.artifact_hash)
            .await
            .map_err(|e| format!("artifact {}: {e}", row.artifact_hash))?;
        if hash_of(&wasm).as_ref() != Some(&row.artifact_hash) {
            return Err(format!(
                "artifact {} does not match its hash",
                row.artifact_hash
            ));
        }
        let host = self.host.clone();
        let prepare = self.prepare.clone();
        tokio::task::spawn_blocking(move || prepare(&host, manifest, &wasm))
            .await
            .map_err(|e| format!("the compile task failed: {e}"))?
            .map_err(|e| e.to_string())
    }
}
