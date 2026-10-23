//! Installed plugins and hook ordering. The registry is immutable; `commit` builds a
//! new one and swaps it in (`ArcSwap`), so calls in flight keep their snapshot (and the
//! instances in it) until they finish.

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
    /// Effective limits (plan ruling P5; see `PluginHost::prepare_with`).
    pub(crate) memory_bytes: usize,
    pub(crate) fuel: u64,
    pub(crate) epoch_tick: Duration,
    /// Set once when the plugin is disabled (automatically after repeated failures, or
    /// through `PluginHost::disable`); holds the reason. A new `prepare` builds a new
    /// `LoadedPlugin`, so it starts enabled again.
    pub(crate) disabled: OnceLock<String>,
}

#[derive(Default)]
pub(crate) struct Registry {
    plugins: BTreeMap<String, Arc<LoadedPlugin>>,
}

impl Registry {
    /// A registry with the plugins of `self` whose name passes `keep`, plus `prepared`
    /// (a prepared plugin replaces a kept one of the same name; among prepared plugins of
    /// one name the last wins). Kept plugins are the same instances, disabled state
    /// included.
    pub(crate) fn rebuilt(
        &self,
        prepared: &[Arc<LoadedPlugin>],
        keep: impl Fn(&str) -> bool,
    ) -> Self {
        let mut plugins: BTreeMap<String, Arc<LoadedPlugin>> = self
            .plugins
            .iter()
            .filter(|(name, _)| keep(name))
            .map(|(name, p)| (name.clone(), Arc::clone(p)))
            .collect();
        for p in prepared {
            plugins.insert(p.name.to_string(), Arc::clone(p));
        }
        Self { plugins }
    }

    /// Names of the plugins in this registry, in name order.
    pub(crate) fn names(&self) -> Vec<String> {
        self.plugins.keys().cloned().collect()
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
