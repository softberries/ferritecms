//! Installed plugins and hook ordering. The registry is immutable; `install` builds a
//! new one and swaps it in (`ArcSwap`), so calls in flight keep their snapshot.

use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use crate::bindings::v0_1::PluginPre;
use crate::manifest::{Capabilities, Manifest};
use crate::services::HostServices;
use crate::state::HostState;

/// A compiled, linked plugin plus everything a call needs to build its store.
pub(crate) struct LoadedPlugin {
    pub(crate) name: Arc<str>,
    pub(crate) manifest: Manifest,
    pub(crate) pre: PluginPre<HostState>,
    pub(crate) grants: Arc<Capabilities>,
    pub(crate) services: Arc<dyn HostServices>,
    /// Effective limits: `min(manifest [limits], host default)`.
    pub(crate) memory_bytes: usize,
    pub(crate) fuel: u64,
    pub(crate) epoch_tick: Duration,
    /// Set once when the plugin is auto-disabled; holds the reason. A reinstall builds a
    /// new `LoadedPlugin`, so it starts enabled again.
    pub(crate) disabled: OnceLock<String>,
}

#[derive(Clone, Default)]
pub(crate) struct Registry {
    plugins: BTreeMap<String, Arc<LoadedPlugin>>,
}

impl Registry {
    /// A copy of this registry with `plugin` added or replacing a same-named one.
    pub(crate) fn with(&self, plugin: Arc<LoadedPlugin>) -> Self {
        let mut next = self.clone();
        next.plugins.insert(plugin.name.to_string(), plugin);
        next
    }

    pub(crate) fn get(&self, name: &str) -> Option<&Arc<LoadedPlugin>> {
        self.plugins.get(name)
    }

    /// Plugins subscribed to `hook`, by ascending priority, ties by plugin name.
    pub(crate) fn subscribers(&self, hook: &str) -> Vec<Arc<LoadedPlugin>> {
        let mut subs: Vec<(i32, &Arc<LoadedPlugin>)> = self
            .plugins
            .values()
            .filter_map(|p| p.manifest.hooks.get(hook).map(|prio| (*prio, p)))
            .collect();
        // `plugins` iterates in name order and the sort is stable, so ties keep it.
        subs.sort_by_key(|(prio, _)| *prio);
        subs.into_iter().map(|(_, p)| Arc::clone(p)).collect()
    }
}
