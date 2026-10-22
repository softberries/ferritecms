//! Wasmtime-based host for FerriteCMS plugins.

mod bindings;
mod call;
mod capability;
mod config;
pub mod engine;
mod error;
mod failure;
mod inspect;
mod linker;
mod manifest;
mod registry;
mod services;
mod state;
#[cfg(test)]
mod test_support;
mod types;
mod validate;

use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use arc_swap::ArcSwap;
use wasmtime::Engine;

pub use capability::{
    DeadlineClass, FilterType, HookKind, HookSpec, Iface, granted_interfaces, hook_catalogue,
};
pub use config::HostConfig;
pub use error::{InspectError, ManifestError, ManifestProblem, PluginError};
pub use failure::{FailureKind, FailureRecord};
pub use inspect::{ComponentReport, inspect};
pub use manifest::{
    Capabilities, HtmlMode, LimitRequest, MailScope, Manifest, PluginMeta, RouteSpec, SettingKind,
    SettingSpec,
};
pub use services::{HostServices, InMemoryServices};
pub use types::{
    ActionEvent, FilterValue, Head, LinkTag, MetaTag, Post, RequestInfo, RouteRequest,
    RouteResponse, ScriptTag, TermRef,
};

use bindings::v0_1::PluginPre;
use call::{CallFailure, call_action, call_filter, call_route};
use engine::{EpochTicker, build_engine};
use failure::FailureTracker;
use registry::{LoadedPlugin, Registry};
use validate::{matches_type, validate_output};

/// Runs installed plugins with capability-scoped imports and per-call limits.
pub struct PluginHost {
    cfg: HostConfig,
    engine: Engine,
    services: Arc<dyn HostServices>,
    registry: ArcSwap<Registry>,
    failures: FailureTracker,
    /// Advances the engine epoch; joined when the host is dropped.
    _ticker: EpochTicker,
}

/// Effective limit (ruling R3): the manifest may lower the host default, not raise it.
fn effective_limit(
    limit: &'static str,
    requested: Option<u64>,
    default: u64,
) -> Result<u64, PluginError> {
    match requested {
        Some(requested) if requested > default => Err(PluginError::LimitAboveDefault {
            limit,
            requested,
            default,
        }),
        Some(requested) => Ok(requested),
        None => Ok(default),
    }
}

impl PluginHost {
    /// Build the engine and start the epoch ticker (stopped when the host is dropped).
    pub fn new(cfg: HostConfig, services: Arc<dyn HostServices>) -> Result<Self, PluginError> {
        if cfg.epoch_tick.is_zero() {
            return Err(PluginError::Engine("epoch_tick must be > 0".into()));
        }
        let engine = build_engine(&cfg)?;
        let ticker = EpochTicker::start(engine.clone(), cfg.epoch_tick)?;
        let failures = FailureTracker::new(cfg.disable_after_failures, cfg.failure_window);
        Ok(Self {
            cfg,
            engine,
            services,
            registry: ArcSwap::from_pointee(Registry::default()),
            failures,
            _ticker: ticker,
        })
    }

    /// Inspect, compile and link `wasm`, then make it callable. Replaces an installed
    /// plugin of the same name. In M0 the grants are the manifest's capabilities.
    pub fn install(&self, manifest: Manifest, wasm: &[u8]) -> Result<(), PluginError> {
        let memory_default = u64::try_from(self.cfg.memory_bytes).unwrap_or(u64::MAX);
        let memory = effective_limit("memory", manifest.limits.memory, memory_default)?;
        let fuel = effective_limit("fuel", manifest.limits.fuel, self.cfg.fuel)?;
        // `memory <= memory_default`, which came from a usize.
        let memory_bytes = usize::try_from(memory).unwrap_or(self.cfg.memory_bytes);

        let (component, report) = inspect::inspect_component(&self.engine, wasm, &manifest)?;
        let name = manifest.plugin.name.clone();
        let link_err = |e: wasmtime::Error| PluginError::Link {
            plugin: name.clone(),
            reason: format!("{e:#}"),
        };
        let grants = manifest.capabilities.clone();
        let linker = linker::build_linker(&self.engine, &granted_interfaces(&grants))?;
        let pre = PluginPre::new(linker.instantiate_pre(&component).map_err(link_err)?)
            .map_err(link_err)?;

        let loaded = Arc::new(LoadedPlugin {
            name: Arc::from(name.as_str()),
            manifest,
            pre,
            grants: Arc::new(grants),
            services: Arc::clone(&self.services),
            memory_bytes,
            fuel,
            epoch_tick: self.cfg.epoch_tick,
            disabled: OnceLock::new(),
        });
        self.failures.reset(&name);
        self.registry.rcu(|r| r.with(Arc::clone(&loaded)));
        tracing::info!(
            plugin = %name,
            imports = ?report.ferrite_imports,
            memory_bytes,
            fuel,
            "plugin installed"
        );
        Ok(())
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

    /// Run every plugin subscribed to filter `hook`, in priority order, each receiving
    /// the previous output. A failing plugin is skipped (its input is passed on), so
    /// this never fails. Unknown hooks, action hooks and a `value` of the wrong variant
    /// return `value` unchanged.
    pub async fn apply_filter(
        &self,
        hook: &str,
        value: FilterValue,
        req: &RequestInfo,
    ) -> FilterValue {
        let Some(spec) = hook_catalogue().iter().find(|h| h.name == hook) else {
            return value;
        };
        let HookKind::Filter(ty) = spec.kind else {
            return value;
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
            match call_filter(&plugin, hook, &current, req, deadline).await {
                Ok(out) => match validate_output(ty, &current, out) {
                    Ok(valid) => current = valid,
                    Err(reason) => {
                        self.record_failure(
                            &plugin,
                            hook,
                            FailureKind::InvalidOutput,
                            &reason,
                            started,
                        );
                    }
                },
                Err(failure) => self.record_call_failure(&plugin, hook, &failure, started),
            }
        }
        current
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

    /// Why `plugin` was disabled, if it was. In M0 this lives in memory only: a restart
    /// or a reinstall of the plugin enables it again.
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
