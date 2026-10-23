//! Wasmtime-based host for FerriteCMS plugins.

mod bindings;
mod call;
mod capability;
mod config;
pub mod engine;
mod error;
mod failure;
mod grant;
mod inspect;
mod linker;
mod manifest;
mod output;
mod registry;
mod report;
mod services;
mod sink;
mod state;
#[cfg(test)]
mod test_support;
mod trace;
mod types;
mod validate;

use std::collections::BTreeSet;
use std::sync::{Arc, OnceLock, PoisonError, RwLock};
use std::time::{Duration, Instant};

use arc_swap::ArcSwap;
use wasmtime::Engine;

pub use capability::{
    DeadlineClass, FilterType, HookKind, HookSpec, Iface, granted_interfaces, hook_catalogue,
};
pub use config::HostConfig;
pub use error::{InspectError, ManifestError, ManifestProblem, PluginError};
pub use failure::{FailureKind, FailureRecord};
pub use grant::{ApprovedLimits, Grant, LIMIT_CEILING_FUEL_FACTOR, LIMIT_CEILING_MEMORY};
pub use inspect::{ComponentReport, inspect};
pub use manifest::{
    Capabilities, HtmlMode, LimitRequest, MAX_MANIFEST_BYTES, MailScope, Manifest, PluginMeta,
    RouteSpec, SettingKind, SettingSpec,
};
pub use report::{LimitItem, PermissionItem, PermissionReport};
pub use services::{HostServices, InMemoryServices, KindOk};
pub use sink::FailureSink;
pub use trace::{FilterOutcome, dep_keys};
pub use types::{
    ActionEvent, FilterValue, Head, LinkTag, MetaTag, Post, RequestInfo, RouteRequest,
    RouteResponse, ScriptTag, TermRef,
};

use bindings::v0_1::PluginPre;
use call::{CallFailure, call_action, call_filter, call_route};
use engine::{EpochTicker, build_engine};
use failure::FailureTracker;
use grant::LimitBounds;
use registry::{LoadedPlugin, Registry};
use trace::CallTrace;
use validate::{matches_type, validate_output};

/// Runs installed plugins with capability-scoped imports and per-call limits.
pub struct PluginHost {
    cfg: HostConfig,
    engine: Engine,
    services: Arc<dyn HostServices>,
    registry: ArcSwap<Registry>,
    failures: FailureTracker,
    /// Also told about every failure (P20); see [`PluginHost::set_failure_sink`].
    failure_sink: RwLock<Option<Arc<dyn FailureSink>>>,
    /// Advances the engine epoch; joined when the host is dropped.
    _ticker: EpochTicker,
}

/// A compiled, linked plugin from [`PluginHost::prepare`], not callable until
/// [`PluginHost::commit`] swaps it in.
pub struct PreparedPlugin {
    loaded: Arc<LoadedPlugin>,
}

impl PreparedPlugin {
    /// The plugin's name (from its manifest).
    pub fn name(&self) -> &str {
        &self.loaded.name
    }

    /// The effective memory limit, in bytes (plan ruling P5).
    pub fn memory_bytes(&self) -> u64 {
        u64::try_from(self.loaded.memory_bytes).unwrap_or(u64::MAX)
    }

    /// The effective fuel per call (plan ruling P5).
    pub fn fuel(&self) -> u64 {
        self.loaded.fuel
    }
}

impl std::fmt::Debug for PreparedPlugin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreparedPlugin")
            .field("name", &self.name())
            .finish_non_exhaustive()
    }
}

/// Reject settings that would make the host unusable: a zero epoch tick never advances
/// deadlines, a zero deadline fails every call, and a zero failure threshold disables a
/// plugin on its first failure.
fn validate_config(cfg: &HostConfig) -> Result<(), PluginError> {
    let zero = [
        ("epoch_tick", cfg.epoch_tick.is_zero()),
        ("render_deadline", cfg.render_deadline.is_zero()),
        ("action_deadline", cfg.action_deadline.is_zero()),
        ("route_deadline", cfg.route_deadline.is_zero()),
        ("disable_after_failures", cfg.disable_after_failures == 0),
    ];
    match zero.iter().find(|(_, is_zero)| *is_zero) {
        Some((name, _)) => Err(PluginError::InvalidConfig(format!("{name} must be > 0"))),
        None => Ok(()),
    }
}

impl PluginHost {
    /// Build the engine and start the epoch ticker (stopped when the host is dropped).
    pub fn new(cfg: HostConfig, services: Arc<dyn HostServices>) -> Result<Self, PluginError> {
        validate_config(&cfg)?;
        let engine = build_engine(&cfg)?;
        let ticker = EpochTicker::start(engine.clone(), cfg.epoch_tick)?;
        let failures = FailureTracker::new(cfg.disable_after_failures, cfg.failure_window);
        Ok(Self {
            cfg,
            engine,
            services,
            registry: ArcSwap::from_pointee(Registry::default()),
            failures,
            failure_sink: RwLock::new(None),
            _ticker: ticker,
        })
    }

    /// Inspect, compile and link `wasm`, then make it callable: [`prepare`](Self::prepare)
    /// plus a [`commit`](Self::commit) that keeps every other plugin. Replaces an
    /// installed plugin of the same name. The grants are the manifest's capabilities.
    pub fn install(&self, manifest: Manifest, wasm: &[u8]) -> Result<(), PluginError> {
        let prepared = self.prepare(manifest, wasm)?;
        self.swap(vec![prepared], |_| true);
        Ok(())
    }

    /// [`prepare_with`](Self::prepare_with) with nothing approved: the manifest's
    /// `[limits]` may only lower the host defaults.
    pub fn prepare(&self, manifest: Manifest, wasm: &[u8]) -> Result<PreparedPlugin, PluginError> {
        self.prepare_with(manifest, wasm, ApprovedLimits::default())
    }

    /// Inspect, compile and link `wasm` for `manifest` without making it callable. This
    /// is the slow part of an install (milliseconds to seconds of CPU per component), so
    /// async callers run it under `spawn_blocking`. The grants are the manifest's
    /// capabilities: callers check them against what the admin approved first.
    ///
    /// Limits (plan ruling P5): per resource, the manifest's request applies when it is
    /// at most the host default, or at most both the ceiling ([`LIMIT_CEILING_MEMORY`],
    /// [`LIMIT_CEILING_FUEL_FACTOR`] x the default fuel) and the `approved` value;
    /// otherwise `LimitAboveDefault` (not approved) or `LimitAboveCeiling`. A pooling
    /// host's memory ceiling is its default: the pool's slots are that size.
    pub fn prepare_with(
        &self,
        manifest: Manifest,
        wasm: &[u8],
        approved: ApprovedLimits,
    ) -> Result<PreparedPlugin, PluginError> {
        let memory_default = u64::try_from(self.cfg.memory_bytes).unwrap_or(u64::MAX);
        let memory_bounds = if self.cfg.pooling {
            LimitBounds::fixed(memory_default)
        } else {
            LimitBounds::memory(memory_default)
        };
        let memory = memory_bounds.effective("memory", manifest.limits.memory, approved.memory)?;
        let fuel = LimitBounds::fuel(self.cfg.fuel).effective(
            "fuel",
            manifest.limits.fuel,
            approved.fuel,
        )?;
        let memory_bytes = usize::try_from(memory).map_err(|_| PluginError::LimitAboveCeiling {
            limit: "memory",
            requested: memory,
            ceiling: u64::try_from(usize::MAX).unwrap_or(u64::MAX),
        })?;

        let (component, report) = inspect::inspect_component(&self.engine, wasm, &manifest)?;
        let name = manifest.plugin.name.clone();
        let link_err = |e: wasmtime::Error| PluginError::Link {
            plugin: name.clone(),
            reason: output::sanitize_guest_text(&format!("{e:#}")),
        };
        let grants = manifest.capabilities.clone();
        let linker = linker::build_linker(&self.engine, &granted_interfaces(&grants))?;
        let pre = PluginPre::new(linker.instantiate_pre(&component).map_err(link_err)?)
            .map_err(link_err)?;
        tracing::debug!(
            plugin = %name,
            imports = ?report.ferrite_imports,
            memory_bytes,
            fuel,
            "plugin prepared"
        );
        Ok(PreparedPlugin {
            loaded: Arc::new(LoadedPlugin {
                name: Arc::from(name.as_str()),
                manifest,
                pre,
                grants: Arc::new(grants),
                services: Arc::clone(&self.services),
                memory_bytes,
                fuel,
                epoch_tick: self.cfg.epoch_tick,
                disabled: OnceLock::new(),
            }),
        })
    }

    /// Swap the registry, once, to exactly `prepared` plus the currently loaded plugins
    /// named in `keep` (a prepared plugin replaces a kept one of the same name). Every
    /// other plugin is dropped. Kept plugins stay the same instances, so a disabled one
    /// stays disabled; each prepared plugin starts enabled with an empty failure window.
    ///
    /// Calls already running keep the registry snapshot they loaded and finish on the
    /// old instances; calls that start afterwards see the new registry.
    pub fn commit(&self, prepared: Vec<PreparedPlugin>, keep: &[&str]) {
        self.swap(prepared, |name| keep.contains(&name));
    }

    fn swap(&self, prepared: Vec<PreparedPlugin>, keep: impl Fn(&str) -> bool) {
        let prepared: Vec<Arc<LoadedPlugin>> = prepared.into_iter().map(|p| p.loaded).collect();
        for p in &prepared {
            self.failures.reset(&p.name);
        }
        self.registry.rcu(|r| r.rebuilt(&prepared, &keep));
        for p in &prepared {
            tracing::info!(
                plugin = %p.name,
                version = %p.manifest.plugin.version,
                memory_bytes = p.memory_bytes,
                fuel = p.fuel,
                "plugin installed"
            );
        }
    }

    /// Disable the loaded plugin `name` at once: calls that start afterwards skip it
    /// (later hooks of a request already running included). `true` only when this call
    /// disabled it; `false` when it is not loaded or already disabled (the first reason
    /// is kept). Lasts until the plugin is prepared and committed again.
    pub fn disable(&self, name: &str, reason: &str) -> bool {
        let Some(plugin) = self.registry.load().get(name).cloned() else {
            return false;
        };
        let done = plugin.disabled.set(reason.to_owned()).is_ok();
        if done {
            tracing::warn!(plugin = %name, reason, "plugin disabled");
        }
        done
    }

    /// Names of the loaded plugins (enabled or disabled), in name order.
    pub fn loaded(&self) -> Vec<String> {
        self.registry.load().names()
    }

    /// Report every later failure to `sink` as well (replacing an earlier sink).
    /// [`FailureSink::record`] runs synchronously inside the failed call.
    pub fn set_failure_sink(&self, sink: Arc<dyn FailureSink>) {
        *self
            .failure_sink
            .write()
            .unwrap_or_else(PoisonError::into_inner) = Some(sink);
    }

    fn deadline(&self, class: DeadlineClass) -> Duration {
        match class {
            DeadlineClass::Render => self.cfg.render_deadline,
            DeadlineClass::Action => self.cfg.action_deadline,
            DeadlineClass::Route => self.cfg.route_deadline,
        }
    }

    /// Log, store and count one failed call; disables the plugin when this failure
    /// pushes it over `disable_after_failures` within `failure_window`.
    fn record_failure(
        &self,
        plugin: &LoadedPlugin,
        hook: &str,
        kind: FailureKind,
        detail: &str,
        started: Instant,
    ) {
        let duration = started.elapsed();
        tracing::warn!(
            plugin = %plugin.name,
            hook,
            kind = ?kind,
            error = detail,
            duration_ms = duration.as_secs_f64() * 1000.0,
            "plugin hook failed"
        );
        let crossed = self.failures.record(FailureRecord {
            plugin: plugin.name.to_string(),
            hook: hook.to_string(),
            kind,
            at: Instant::now(),
            duration,
        });
        let sink = self
            .failure_sink
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        if let Some(sink) = sink {
            sink.record(&plugin.name, hook, kind);
        }
        if crossed {
            let reason = format!(
                "auto: {} failures in window",
                self.cfg.disable_after_failures
            );
            if plugin.disabled.set(reason).is_ok() {
                tracing::error!(
                    plugin = %plugin.name,
                    failures = self.cfg.disable_after_failures,
                    window_s = self.cfg.failure_window.as_secs(),
                    "plugin disabled after repeated failures"
                );
            }
        }
    }

    fn record_call_failure(
        &self,
        plugin: &LoadedPlugin,
        hook: &str,
        failure: &CallFailure,
        started: Instant,
    ) {
        self.record_failure(plugin, hook, failure.kind(), &failure.to_string(), started);
    }

    /// Enabled plugin `name`, or the error a caller should see.
    fn enabled_plugin(&self, name: &str) -> Result<Arc<LoadedPlugin>, PluginError> {
        let loaded = self.plugin(name)?;
        match loaded.disabled.get() {
            Some(reason) => Err(PluginError::Disabled {
                plugin: name.to_string(),
                reason: reason.clone(),
            }),
            None => Ok(loaded),
        }
    }

    fn plugin(&self, name: &str) -> Result<Arc<LoadedPlugin>, PluginError> {
        self.registry
            .load()
            .get(name)
            .cloned()
            .ok_or_else(|| PluginError::UnknownPlugin(name.to_string()))
    }

    /// [`apply_filter_traced`](Self::apply_filter_traced) without the trace.
    pub async fn apply_filter(
        &self,
        hook: &str,
        value: FilterValue,
        req: &RequestInfo,
    ) -> FilterValue {
        self.apply_filter_traced(hook, value, req).await.value
    }

    /// Run every plugin subscribed to filter `hook`, in priority order, each receiving
    /// the previous output. A failing plugin is skipped (its input is passed on), so
    /// this never fails. Unknown hooks, action hooks and a `value` of the wrong variant
    /// return `value` unchanged.
    ///
    /// `html` output is sanitised with `ammonia` unless the plugin declares
    /// `render.html = "raw"` (design 05 §7). The outcome also carries the dependency
    /// keys and volatility of every call made (design 05 §7a; see the `trace` module):
    /// a failed call counts as volatile.
    pub async fn apply_filter_traced(
        &self,
        hook: &str,
        value: FilterValue,
        req: &RequestInfo,
    ) -> FilterOutcome {
        let mut deps = BTreeSet::new();
        let mut volatile = false;
        let Some(spec) = hook_catalogue().iter().find(|h| h.name == hook) else {
            return outcome(value, deps, volatile);
        };
        let HookKind::Filter(ty) = spec.kind else {
            return outcome(value, deps, volatile);
        };
        let deadline = self.deadline(spec.deadline);
        let registry = self.registry.load_full();
        let mut current = value;
        for plugin in registry.subscribers(hook) {
            if plugin.disabled.get().is_some() {
                continue;
            }
            if !matches_type(ty, &current) {
                tracing::debug!(plugin = %plugin.name, hook, "filter value of wrong type; skipped");
                continue;
            }
            let started = Instant::now();
            let trace = CallTrace::default();
            let render_path = spec.deadline == DeadlineClass::Render;
            let result =
                call_filter(&plugin, hook, &current, req, deadline, &trace, render_path).await;
            let html_mode = plugin.manifest.capabilities.render_html;
            match result {
                Ok(out) => match validate_output(ty, &current, out, html_mode) {
                    Ok(valid) => current = valid,
                    Err(reason) => {
                        trace.volatile();
                        self.record_failure(
                            &plugin,
                            hook,
                            FailureKind::InvalidOutput,
                            &reason,
                            started,
                        );
                    }
                },
                Err(failure) => {
                    trace.volatile();
                    self.record_call_failure(&plugin, hook, &failure, started);
                }
            }
            trace.drain_into(&mut deps, &mut volatile);
        }
        outcome(current, deps, volatile)
    }

    /// Deliver action `hook` to one plugin that subscribes to it.
    pub async fn dispatch_action(
        &self,
        plugin: &str,
        hook: &str,
        ev: ActionEvent,
    ) -> Result<(), PluginError> {
        let loaded = self.enabled_plugin(plugin)?;
        let spec = hook_catalogue()
            .iter()
            .find(|h| h.name == hook && h.kind == HookKind::Action)
            .filter(|_| loaded.manifest.hooks.contains_key(hook))
            .ok_or_else(|| PluginError::NotSubscribed {
                plugin: plugin.to_string(),
                hook: hook.to_string(),
            })?;
        let started = Instant::now();
        call_action(&loaded, hook, ev, self.deadline(spec.deadline))
            .await
            .map_err(|failure| {
                self.record_call_failure(&loaded, hook, &failure, started);
                PluginError::CallFailed {
                    plugin: plugin.to_string(),
                    reason: failure.to_string(),
                }
            })
    }

    /// Serve a request under `/_p/<plugin>/`; `req.path` is relative to that prefix and
    /// must match a `[routes]` entry (path and method) of a plugin with `routes = true`.
    pub async fn handle_route(
        &self,
        plugin: &str,
        req: RouteRequest,
    ) -> Result<RouteResponse, PluginError> {
        let loaded = self.enabled_plugin(plugin)?;
        let routed = loaded.manifest.capabilities.routes
            && loaded
                .manifest
                .routes
                .get(&req.path)
                .is_some_and(|r| r.methods.contains(&req.method));
        if !routed {
            return Err(PluginError::RouteNotFound {
                plugin: plugin.to_string(),
                method: req.method,
                path: req.path,
            });
        }
        let hook = format!("route {} {}", req.method, req.path);
        let started = Instant::now();
        call_route(&loaded, req, self.deadline(DeadlineClass::Route))
            .await
            .map_err(|failure| {
                self.record_call_failure(&loaded, &hook, &failure, started);
                PluginError::CallFailed {
                    plugin: plugin.to_string(),
                    reason: failure.to_string(),
                }
            })
    }

    /// Whether `plugin` is installed and enabled.
    pub fn is_enabled(&self, plugin: &str) -> bool {
        self.registry
            .load()
            .get(plugin)
            .is_some_and(|p| p.disabled.get().is_none())
    }

    /// Why `plugin` was disabled on this host, if it was. This lives in memory only: a
    /// restart or a new prepare + commit of the plugin enables it again (the shared,
    /// persistent disable is the `plugins` row, P20).
    pub fn disabled_reason(&self, plugin: &str) -> Option<String> {
        self.registry
            .load()
            .get(plugin)
            .and_then(|p| p.disabled.get().cloned())
    }

    /// Recorded call failures, oldest first. Keeps the most recent 1_000 across all
    /// plugins; older records are dropped.
    pub fn failures(&self) -> Vec<FailureRecord> {
        self.failures.history()
    }
}

fn outcome(value: FilterValue, deps: BTreeSet<String>, volatile: bool) -> FilterOutcome {
    FilterOutcome {
        value,
        deps: deps.into_iter().collect(),
        volatile,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new_host(cfg: HostConfig) -> Result<PluginHost, PluginError> {
        PluginHost::new(cfg, Arc::new(InMemoryServices::new()))
    }

    #[test]
    fn degenerate_configs_are_rejected() {
        let base = HostConfig::default;
        let bad = [
            HostConfig {
                disable_after_failures: 0,
                ..base()
            },
            HostConfig {
                render_deadline: Duration::ZERO,
                ..base()
            },
            HostConfig {
                action_deadline: Duration::ZERO,
                ..base()
            },
            HostConfig {
                route_deadline: Duration::ZERO,
                ..base()
            },
            HostConfig {
                epoch_tick: Duration::ZERO,
                ..base()
            },
        ];
        for cfg in bad {
            let shown = format!("{cfg:?}");
            assert!(
                matches!(new_host(cfg), Err(PluginError::InvalidConfig(_))),
                "{shown}"
            );
        }
        assert!(new_host(base()).is_ok());
    }
}
