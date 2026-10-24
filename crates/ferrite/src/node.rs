//! What this node reports in `nodes.meta` (ruling P14, 04 §8): the theme, the plugins
//! and the permalink rules it runs, with the last reload error of each
//! (`theme.error`, `plugins.<name>.error` / `plugins_error`, `permalinks_error`). The registry
//! adds `site_id`, `site_url`, `mode` and `clock_skew_ms` itself.

use std::sync::{Arc, Mutex, PoisonError};

use ferrite_cluster::ClusterMode as NodeMode;
use ferrite_config::ClusterMode as ConfigMode;
use ferrite_http::{PluginStatus, ReloadReport};
use serde_json::{Map, Value, json};

use crate::site::{LoadedTheme, ThemeOrigin};

/// Longest error text per entry in `nodes.meta` (bytes).
pub const MAX_META_ERROR: usize = 1024;

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
            "error": error.map(cap),
            "fallback": fallback,
        });
    }

    /// `nodes.meta.plugins`: one entry per row of the last plugin reload.
    pub fn set_plugins(&self, report: &ReloadReport) {
        let mut map = Map::new();
        for (name, state) in &report.states {
            let (status, error) = match &state.status {
                PluginStatus::Loaded => ("loaded", None),
                PluginStatus::Failed(reason) => ("failed", Some(cap(reason))),
                PluginStatus::Disabled(reason) => ("disabled", Some(cap(reason))),
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
        self.lock().plugins_error = error.map(cap);
    }

    /// `nodes.meta.permalinks_error`: why the current option values are not in effect.
    pub fn set_permalinks_error(&self, error: Option<&str>) {
        self.lock().permalinks_error = error.map(cap);
    }

    /// The node's part of `nodes.meta`.
    pub fn meta(&self) -> Value {
        let s = self.lock();
        json!({
            "theme": s.theme,
            "plugins": s.plugins,
            "plugins_error": s.plugins_error,
            "permalinks_error": s.permalinks_error,
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

/// `s` cut to [`MAX_META_ERROR`] bytes on a character boundary.
fn cap(s: &str) -> String {
    let mut end = s.len().min(MAX_META_ERROR);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_owned()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use ferrite_domain::ArtifactHash;
    use ferrite_http::PluginState;

    use super::*;

    #[test]
    fn modes_map_fail_closed() {
        assert_eq!(node_mode(ConfigMode::Solo), NodeMode::Solo);
        assert_eq!(node_mode(ConfigMode::Cluster), NodeMode::Cluster);
        assert_eq!(node_mode(ConfigMode::Auto), NodeMode::Cluster);
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
        let long = "é".repeat(MAX_META_ERROR);
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
        assert!(err.len() <= MAX_META_ERROR && err.len() > MAX_META_ERROR - 2);
        assert_eq!(meta["plugins"]["a"]["status"], "loaded");
        assert!(meta["plugins"]["a"]["error"].is_null());
        assert_eq!(meta["plugins"]["b"]["status"], "failed");
        assert_eq!(meta["plugins"]["b"]["error"], "boom");
        assert_eq!(meta["plugins"]["c"]["status"], "disabled");
        assert_eq!(meta["plugins"]["a"]["hash"], h.as_str());
        assert_eq!(meta["permalinks_error"], "bad");
        status.set_permalinks_error(None);
        assert!(status.meta()["permalinks_error"].is_null());
    }
}
