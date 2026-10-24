//! What this node reports in `nodes.meta` (ruling P14, 04 §8): the theme, the plugins
//! and the permalink rules it runs, with the last reload error of each
//! (`theme.error`, `plugins.<name>.error` / `plugins_error`, `permalinks_error`), and the
//! job kinds its runner handles (`job_kinds`, sorted; M2a-2b P19). The registry
//! adds `site_id`, `site_url`, `mode` and `clock_skew_ms` itself.
//!
//! Error texts are scrubbed before they are stored ([`scrub`], M2a-2b P18): URLs,
//! absolute paths, IP addresses and credentials are replaced and the text is cut to
//! 1 KiB ([`MAX_DIAGNOSTIC`](ferrite_core::diagnostics::MAX_DIAGNOSTIC)); the full
//! text is logged where the error is produced (the reloader and startup).

use std::sync::{Arc, Mutex, PoisonError};

use ferrite_cluster::ClusterMode as NodeMode;
use ferrite_config::ClusterMode as ConfigMode;
use ferrite_core::diagnostics::scrub;
use ferrite_http::{PluginStatus, ReloadReport};
use serde_json::{Map, Value, json};

use crate::site::{LoadedTheme, ThemeOrigin};

/// The node's mode for the registry. `mode` must be resolved
/// ([`ferrite_config::Config::effective_cluster_mode`]); an unresolved `Auto` counts as
/// cluster (fail closed, as `load_theme` does).
pub fn node_mode(mode: ConfigMode) -> NodeMode {
    match mode {
        ConfigMode::Solo => NodeMode::Solo,
        ConfigMode::Cluster | ConfigMode::Auto => NodeMode::Cluster,
    }
}

/// The node-local state the heartbeat publishes; updated by the reloader.
#[derive(Default)]
pub struct NodeStatus {
    inner: Mutex<Status>,
}

#[derive(Default)]
struct Status {
    theme: Value,
    plugins: Value,
    plugins_error: Option<String>,
    permalinks_error: Option<String>,
    job_kinds: Vec<String>,
}

impl NodeStatus {
    /// `nodes.meta.theme`: the served theme, whether it is a `fallback` (there is an
    /// active row and this is not its artifact: a startup fallback or a failed hot swap,
    /// pre-flight F12) and, if set, why the active one is not served.
    pub fn set_theme(&self, loaded: &LoadedTheme, error: Option<&str>, fallback: bool) {
        let origin = match &loaded.origin {
            ThemeOrigin::Artifact(hash) => json!({"artifact": hash.as_str()}),
            ThemeOrigin::Folder => json!("folder"),
            ThemeOrigin::Embedded => json!("embedded"),
        };
        self.lock().theme = json!({
            "name": loaded.theme.name(),
            "hash": loaded.theme.hash(),
            "origin": origin,
            "error": error.map(scrub),
            "fallback": fallback,
        });
    }

    /// `nodes.meta.plugins`: one entry per row of the last plugin reload.
    pub fn set_plugins(&self, report: &ReloadReport) {
        let mut map = Map::new();
        for (name, state) in &report.states {
            let (status, error) = match &state.status {
                PluginStatus::Loaded => ("loaded", None),
                PluginStatus::Failed(reason) => ("failed", Some(scrub(reason))),
                PluginStatus::Disabled(reason) => ("disabled", Some(scrub(reason))),
            };
            map.insert(
                name.clone(),
                json!({
                    "version": state.version,
                    "hash": state.hash.as_str(),
                    "status": status,
                    "error": error,
                }),
            );
        }
        self.lock().plugins = Value::Object(map);
    }

    /// `nodes.meta.plugins_error`: why the plugin rows could not be read at the last
    /// reload (the registry and `plugins` stay as they were).
    pub fn set_plugins_error(&self, error: Option<&str>) {
        self.lock().plugins_error = error.map(scrub);
    }

    /// `nodes.meta.permalinks_error`: why the current option values are not in effect.
    pub fn set_permalinks_error(&self, error: Option<&str>) {
        self.lock().permalinks_error = error.map(scrub);
    }

    /// `nodes.meta.job_kinds`: the kinds this node's job runner handles (stored sorted,
    /// without duplicates), so the job list can tell a job no live node runs.
    pub fn set_job_kinds(&self, mut kinds: Vec<String>) {
        kinds.sort();
        kinds.dedup();
        self.lock().job_kinds = kinds;
    }

    /// The node's part of `nodes.meta`.
    pub fn meta(&self) -> Value {
        let s = self.lock();
        json!({
            "theme": s.theme,
            "plugins": s.plugins,
            "plugins_error": s.plugins_error,
            "permalinks_error": s.permalinks_error,
            "job_kinds": s.job_kinds,
        })
    }

    /// [`meta`](Self::meta) as the heartbeat takes it.
    pub fn meta_fn(self: &Arc<Self>) -> Arc<dyn Fn() -> Value + Send + Sync> {
        let this = self.clone();
        Arc::new(move || this.meta())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Status> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use ferrite_core::diagnostics::MAX_DIAGNOSTIC;
    use ferrite_domain::ArtifactHash;
    use ferrite_http::PluginState;

    use super::*;

    #[test]
    fn modes_map_fail_closed() {
        assert_eq!(node_mode(ConfigMode::Solo), NodeMode::Solo);
        assert_eq!(node_mode(ConfigMode::Cluster), NodeMode::Cluster);
        assert_eq!(node_mode(ConfigMode::Auto), NodeMode::Cluster);
    }

    /// Review Focus 5 (P18): every error text is scrubbed before it reaches
    /// `nodes.meta`; the full text is only in the node's log.
    #[test]
    fn node_meta_errors_scrubbed_at_source() {
        let status = NodeStatus::default();
        let loaded = LoadedTheme {
            theme: Arc::new(ferrite_theme::Theme::load_embedded("blog").unwrap()),
            origin: ThemeOrigin::Embedded,
            error: None,
            fallback: true,
        };
        let leak = "storage backend error: Unable to open file \
                    /srv/ferrite/data/media/artifacts/ab/abcd: Permission denied; \
                    https://AKIAKEY:hunter2@minio.internal:9000/bucket 10.0.0.5:5432";
        let want = "storage backend error: Unable to open file <path>: Permission denied; \
                    <url> <addr>";
        status.set_theme(&loaded, Some(leak), true);
        let h = ArtifactHash::parse(&"a".repeat(64)).unwrap();
        let mut states = BTreeMap::new();
        states.insert(
            "p".to_owned(),
            PluginState {
                version: "1.0.0".into(),
                hash: h.clone(),
                status: PluginStatus::Failed(leak.into()),
            },
        );
        states.insert(
            "q".to_owned(),
            PluginState {
                version: "1.0.0".into(),
                hash: h,
                status: PluginStatus::Disabled(leak.into()),
            },
        );
        status.set_plugins(&ReloadReport { states });
        status.set_plugins_error(Some(leak));
        status.set_permalinks_error(Some(leak));
        let meta = status.meta();
        for secret in ["/srv/ferrite", "AKIAKEY", "hunter2", "minio", "10.0.0.5"] {
            assert!(!meta.to_string().contains(secret), "{secret}: {meta}");
        }
        assert_eq!(meta["theme"]["error"], want);
        assert_eq!(meta["plugins"]["p"]["error"], want);
        assert_eq!(meta["plugins"]["q"]["error"], want);
        assert_eq!(meta["plugins_error"], want);
        assert_eq!(meta["permalinks_error"], want);
    }

    #[test]
    fn meta_shape_and_caps() {
        let status = NodeStatus::default();
        let theme = ferrite_theme::Theme::load_embedded("blog").unwrap();
        let hash = theme.hash().to_owned();
        let loaded = LoadedTheme {
            theme: Arc::new(theme),
            origin: ThemeOrigin::Embedded,
            error: None,
            fallback: true,
        };
        let long = "é".repeat(MAX_DIAGNOSTIC);
        status.set_theme(&loaded, Some(&long), loaded.fallback);
        let h = ArtifactHash::parse(&"a".repeat(64)).unwrap();
        let mut states = BTreeMap::new();
        for (name, s) in [
            ("a", PluginStatus::Loaded),
            ("b", PluginStatus::Failed("boom".into())),
            ("c", PluginStatus::Disabled("auto".into())),
        ] {
            states.insert(
                name.to_owned(),
                PluginState {
                    version: "1.0.0".into(),
                    hash: h.clone(),
                    status: s,
                },
            );
        }
        status.set_plugins(&ReloadReport { states });
        status.set_permalinks_error(Some("bad"));
        assert!(status.meta()["plugins_error"].is_null());
        status.set_plugins_error(Some("list failed"));
        assert_eq!(status.meta()["plugins_error"], "list failed");
        let meta = status.meta();
        assert_eq!(meta["theme"]["name"], "blog");
        assert_eq!(meta["theme"]["hash"], hash.as_str());
        assert_eq!(meta["theme"]["origin"], "embedded");
        assert_eq!(meta["theme"]["fallback"], true);
        let err = meta["theme"]["error"].as_str().unwrap();
        assert!(err.len() <= MAX_DIAGNOSTIC && err.len() > MAX_DIAGNOSTIC - 2);
        assert_eq!(meta["plugins"]["a"]["status"], "loaded");
        assert!(meta["plugins"]["a"]["error"].is_null());
        assert_eq!(meta["plugins"]["b"]["status"], "failed");
        assert_eq!(meta["plugins"]["b"]["error"], "boom");
        assert_eq!(meta["plugins"]["c"]["status"], "disabled");
        assert_eq!(meta["plugins"]["a"]["hash"], h.as_str());
        assert_eq!(meta["permalinks_error"], "bad");
        status.set_permalinks_error(None);
        assert!(status.meta()["permalinks_error"].is_null());
        assert_eq!(meta["job_kinds"], json!([]));
        status.set_job_kinds(vec!["b.x".into(), "a.y".into(), "b.x".into()]);
        assert_eq!(status.meta()["job_kinds"], json!(["a.y", "b.x"]));
    }
}
