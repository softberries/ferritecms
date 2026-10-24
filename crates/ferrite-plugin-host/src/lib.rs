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
mod skips;
mod state;
mod stats;
#[cfg(test)]
mod test_support;
mod trace;
mod types;
mod validate;

use std::collections::BTreeSet;
use std::sync::{Arc, OnceLock, PoisonError, RwLock};
use std::time::{Duration, Instant};

use arc_swap::ArcSwap;
use tokio::sync::{Semaphore, SemaphorePermit};
use wasmtime::Engine;

pub use capability::{
    DeadlineClass, FilterType, HookKind, HookSpec, Iface, granted_interfaces, hook_catalogue,
};
pub use config::HostConfig;
pub use error::{InspectError, ManifestError, ManifestProblem, PluginError};
pub use failure::{FailureKind, FailureRecord};
pub use grant::{
    ApprovedLimits, DEFAULT_ROUTE_RATE_LIMIT, Grant, LIMIT_CEILING_FUEL_FACTOR,
    LIMIT_CEILING_MEMORY, RoutePolicy,
};
pub use inspect::{ComponentReport, inspect};
pub use manifest::{
    Capabilities, HtmlMode, LimitRequest, MAX_MANIFEST_BYTES, MailScope, Manifest, PluginMeta,
    RESERVED_PLUGIN_NAMES, RouteSpec, SettingKind, SettingSpec,
};
pub use report::{LimitItem, PermissionItem, PermissionReport, Risk};
pub use services::{HostServices, InMemoryServices, KindOk, MAIL_NOT_CONFIGURED, SentMail};
pub use sink::FailureSink;
pub use state::{MAX_MAIL_BODY_BYTES, MAX_MAIL_SUBJECT_CHARS};
pub use stats::{DURATION_BUCKETS_MS, HookStats, MAX_STATS_HOOKS};
pub use trace::{FilterOutcome, dep_keys};
pub use types::{
    ActionEvent, FilterValue, Head, LinkTag, MetaTag, Post, RequestInfo, RouteRequest,
    RouteResponse, ScriptTag, TermRef,
};
pub use validate::{MAX_ROUTE_BODY_BYTES, MAX_ROUTE_HEADER_BYTES, MAX_ROUTE_HEADERS};

use bindings::v0_1::PluginPre;
use call::{CallDeadline, CallFailure, call_action, call_filter, call_route};
use engine::{EpochTicker, build_engine};
use failure::FailureTracker;
use grant::LimitBounds;
use registry::{LoadedPlugin, Registry};
use skips::{SKIP_WARN_EVERY, Skip, SkipLog};
use stats::{CallStats, Outcome};
use trace::CallTrace;
use validate::{matches_type, validate_output, validate_route_response};

/// Runs installed plugins with capability-scoped imports and per-call limits.
pub struct PluginHost {
    cfg: HostConfig,
    engine: Engine,
    services: Arc<dyn HostServices>,
    registry: ArcSwap<Registry>,
    failures: FailureTracker,
    /// Also told about every failure (P20); see [`PluginHost::set_failure_sink`].
    failure_sink: RwLock<Option<Arc<dyn FailureSink>>>,
    /// One permit per running call, [`HostConfig::max_concurrent_calls`] in all (P22).
    calls: Semaphore,
    /// Rate limit for the skip warnings (review m7).
    skips: SkipLog,
    /// Call outcomes per plugin and hook since the host was built (M2a-2b P20).
    stats: CallStats,
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
        ("max_concurrent_calls", cfg.max_concurrent_calls == 0),
    ];
    if let Some((name, _)) = zero.iter().find(|(_, is_zero)| *is_zero) {
        return Err(PluginError::InvalidConfig(format!("{name} must be > 0")));
    }
    // A pooling engine has slots for this many instances; more calls at once would
    // fail to instantiate (and count as failures).
    let most = if cfg.pooling {
        usize::try_from(engine::POOL_SLOTS).unwrap_or(usize::MAX)
    } else {
        Semaphore::MAX_PERMITS
    };
    if cfg.max_concurrent_calls > most {
        return Err(PluginError::InvalidConfig(format!(
            "max_concurrent_calls must be at most {most}"
        )));
    }
    Ok(())
}

impl PluginHost {
    /// Build the engine and start the epoch ticker (stopped when the host is dropped).
    pub fn new(cfg: HostConfig, services: Arc<dyn HostServices>) -> Result<Self, PluginError> {
        validate_config(&cfg)?;
        let engine = build_engine(&cfg)?;
        let ticker = EpochTicker::start(engine.clone(), cfg.epoch_tick)?;
        let failures = FailureTracker::new(cfg.disable_after_failures, cfg.failure_window);
        let calls = Semaphore::new(cfg.max_concurrent_calls);
        Ok(Self {
            cfg,
            engine,
            services,
            registry: ArcSwap::from_pointee(Registry::default()),
            failures,
            failure_sink: RwLock::new(None),
            calls,
            skips: SkipLog::new(SKIP_WARN_EVERY),
            stats: CallStats::default(),
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
        let (memory_bounds, fuel_bounds) = LimitBounds::of_host(&self.cfg);
        let memory = memory_bounds.effective("memory", manifest.limits.memory, approved.memory)?;
        let fuel = fuel_bounds.effective("fuel", manifest.limits.fuel, approved.fuel)?;
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
        let settings = Arc::new(registry::setting_defaults(&manifest));
        Ok(PreparedPlugin {
            loaded: Arc::new(LoadedPlugin {
                name: Arc::from(name.as_str()),
                manifest,
                pre,
                grants: Arc::new(grants),
                settings,
                services: Arc::clone(&self.services),
                memory_bytes,
                fuel,
                epoch_tick: self.cfg.epoch_tick,
                disabled: OnceLock::new(),
            }),
        })
    }

    /// The host's configuration (defaults and limits).
    pub fn config(&self) -> &HostConfig {
        &self.cfg
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
        self.stats
            .record(&plugin.name, hook, Outcome::Failure(duration));
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

    /// A call slot, waiting at most `wait` (P22), and how long it waited (zero when one
    /// was free at once); `None` when none freed up. The permit is released when
    /// dropped, also when the call's future is dropped mid-call.
    async fn permit(&self, wait: Duration) -> Option<(SemaphorePermit<'_>, Duration)> {
        if let Ok(permit) = self.calls.try_acquire() {
            return Some((permit, Duration::ZERO));
        }
        let start = Instant::now();
        match tokio::time::timeout(wait, self.calls.acquire()).await {
            Ok(Ok(permit)) => Some((permit, start.elapsed())),
            // Timed out; the semaphore is never closed.
            Ok(Err(_)) | Err(_) => None,
        }
    }

    /// Log a call skipped for want of a call slot. Not a failure. Warned at most once
    /// per plugin per [`SKIP_WARN_EVERY`] (with the skips since), else debug.
    fn log_busy(&self, plugin: &str, hook: &str) {
        let max = self.cfg.max_concurrent_calls;
        match self.skips.admit(Skip::Busy, plugin, Instant::now()) {
            Some(suppressed) => tracing::warn!(
                plugin,
                hook,
                max_concurrent_calls = max,
                skipped_since_last_warning = suppressed,
                "no plugin call slot free; call skipped"
            ),
            None => tracing::debug!(plugin, hook, "no plugin call slot free; call skipped"),
        }
    }

    /// Log that the render budget ended a filter chain (or one call of it, `cut`):
    /// names `spent_by`, the plugin called last before it ran out (the most recent
    /// caller, not necessarily the slowest; review m4). Warned at most once per hook per
    /// [`SKIP_WARN_EVERY`], else debug.
    fn log_budget(&self, hook: &str, spent_by: Option<&str>, cut: Option<&str>) {
        let spent_by = spent_by.unwrap_or("an earlier filter of the page");
        match self.skips.admit(Skip::Budget, hook, Instant::now()) {
            Some(suppressed) => tracing::warn!(
                hook,
                spent_by,
                cut,
                skipped_since_last_warning = suppressed,
                "render budget spent; remaining plugins skipped (not counted as failures)"
            ),
            None => tracing::debug!(hook, spent_by, cut, "render budget spent; plugins skipped"),
        }
    }

    /// `Busy` for `plugin`, logged.
    fn busy(&self, plugin: &str, hook: &str) -> PluginError {
        self.stats.record(plugin, hook, Outcome::Busy);
        self.log_busy(plugin, hook);
        PluginError::Busy {
            plugin: plugin.to_owned(),
        }
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
    ///
    /// [`apply_filter_until`](Self::apply_filter_until) without a time budget: each
    /// call waits at most its deadline for a call slot, then gets its full deadline.
    pub async fn apply_filter_traced(
        &self,
        hook: &str,
        value: FilterValue,
        req: &RequestInfo,
    ) -> FilterOutcome {
        self.run_filters(hook, value, req, None).await
    }

    /// [`apply_filter_traced`](Self::apply_filter_traced) within a time budget that
    /// ends at `until` (P22, the page's render budget). Each call's deadline is
    /// `min(hook deadline, until − now)` and its hard deadline `min(1.5 × hook
    /// deadline, until − now)` (F37); waiting for a call slot counts against it. Once
    /// `until` has passed, the remaining plugins are skipped. A skip (budget spent, no
    /// slot in time) and a deadline failure of a call the budget cut short are not
    /// plugin failures; each makes the outcome volatile.
    pub async fn apply_filter_until(
        &self,
        hook: &str,
        value: FilterValue,
        req: &RequestInfo,
        until: Instant,
    ) -> FilterOutcome {
        self.run_filters(hook, value, req, Some(until)).await
    }

    async fn run_filters(
        &self,
        hook: &str,
        value: FilterValue,
        req: &RequestInfo,
        until: Option<Instant>,
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
        let left = |until: Instant| until.saturating_duration_since(Instant::now());
        let registry = self.registry.load_full();
        let mut current = value;
        // The plugin called last in this chain: named when the budget runs out.
        let mut last_called: Option<Arc<str>> = None;
        let subscribers = registry.subscribers(hook);
        for (i, plugin) in subscribers.iter().enumerate() {
            if plugin.disabled.get().is_some() {
                continue;
            }
            if !matches_type(ty, &current) {
                tracing::debug!(plugin = %plugin.name, hook, "filter value of wrong type; skipped");
                continue;
            }
            if until.is_some_and(|u| left(u).is_zero()) {
                self.log_budget(hook, last_called.as_deref(), None);
                self.count_skipped(&subscribers[i..], hook);
                volatile = true;
                break;
            }
            let wait = until.map_or(deadline, |u| deadline.min(left(u)));
            let Some((_permit, _)) = self.permit(wait).await else {
                self.stats.record(&plugin.name, hook, Outcome::Busy);
                self.log_busy(&plugin.name, hook);
                volatile = true;
                continue;
            };
            let limit = match until {
                None => CallDeadline::full(deadline),
                Some(u) if left(u).is_zero() => {
                    self.log_budget(hook, last_called.as_deref(), None);
                    self.count_skipped(&subscribers[i..], hook);
                    volatile = true;
                    break;
                }
                Some(u) => CallDeadline::within(deadline, left(u)),
            };
            let started = Instant::now();
            last_called = Some(plugin.name.clone());
            let trace = CallTrace::default();
            let render_path = spec.deadline == DeadlineClass::Render;
            let result = call_filter(plugin, hook, &current, req, limit, &trace, render_path).await;
            let html_mode = plugin.manifest.capabilities.render_html;
            match result {
                Ok(out) => match validate_output(ty, &current, out, html_mode) {
                    Ok(valid) => {
                        self.stats
                            .record(&plugin.name, hook, Outcome::Success(started.elapsed()));
                        current = valid;
                    }
                    Err(reason) => {
                        trace.volatile();
                        self.record_failure(
                            plugin,
                            hook,
                            FailureKind::InvalidOutput,
                            &reason,
                            started,
                        );
                    }
                },
                Err(failure) if limit.ended_by_budget(&failure) => {
                    trace.volatile();
                    self.stats
                        .record(&plugin.name, hook, Outcome::Cut(started.elapsed()));
                    self.log_budget(hook, Some(&plugin.name), Some(&plugin.name));
                }
                Err(failure) => {
                    trace.volatile();
                    self.record_call_failure(plugin, hook, &failure, started);
                }
            }
            trace.drain_into(&mut deps, &mut volatile);
        }
        outcome(current, deps, volatile)
    }

    /// Names of the enabled loaded plugins whose manifest subscribes to `hook`, by
    /// ascending priority, ties by name (the order filters run in).
    pub fn subscribers(&self, hook: &str) -> Vec<String> {
        self.registry
            .load()
            .subscribers(hook)
            .iter()
            .filter(|p| p.disabled.get().is_none())
            .map(|p| p.name.to_string())
            .collect()
    }

    /// Deliver action `hook` to one plugin that subscribes to it. The plugin is looked
    /// up once and that instance serves the whole call, so a reload committed meanwhile
    /// does not affect it (R13). `UnknownPlugin` when it is not loaded, `Disabled` when
    /// this host disabled it, `NotSubscribed` when `hook` is not an action or not in its
    /// `[hooks]`, `Busy` (not a failure) when no call slot freed up within the action
    /// deadline, `CallFailed` (recorded towards auto-disable) when the call failed. The
    /// wait for the slot counts toward the deadline (final review M5): soft and hard
    /// deadlines run from when the call began waiting, so wait and call together take at
    /// most the hard deadline (1.5 × the action deadline). A call whose deadline the wait
    /// shortened and that then ran out of time is `Busy` too, not a failure.
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
        let deadline = self.deadline(spec.deadline);
        let (_permit, waited) = self
            .permit(deadline)
            .await
            .ok_or_else(|| self.busy(plugin, hook))?;
        let limit = CallDeadline::after_wait(deadline, waited);
        let started = Instant::now();
        let done = call_action(&loaded, hook, ev, limit).await;
        let elapsed = started.elapsed();
        if done.is_ok() {
            self.stats.record(plugin, hook, Outcome::Success(elapsed));
        }
        done.map_err(|failure| {
            if limit.ended_by_budget(&failure) {
                self.stats.record(plugin, hook, Outcome::Cut(elapsed));
                self.log_busy(plugin, hook);
                return PluginError::Busy {
                    plugin: plugin.to_owned(),
                };
            }
            self.record_call_failure(&loaded, hook, &failure, started);
            PluginError::CallFailed {
                plugin: plugin.to_string(),
                reason: failure.to_string(),
            }
        })
    }

    /// The `[routes]` entry for `path` of the enabled plugin `plugin` (05 §4b): what the
    /// HTTP layer checks (methods, origin rule, rate limit) before calling
    /// [`handle_route`](Self::handle_route). `UnknownPlugin` / `Disabled` as for
    /// actions; `RouteNotFound` (method `*`) when the plugin has no `routes` grant or no
    /// such path.
    pub fn route(&self, plugin: &str, path: &str) -> Result<RouteSpec, PluginError> {
        let loaded = self.enabled_plugin(plugin)?;
        route_spec(&loaded, path)
            .cloned()
            .ok_or_else(|| PluginError::RouteNotFound {
                plugin: plugin.to_string(),
                method: "*".to_owned(),
                path: path.to_owned(),
            })
    }

    /// Serve a request under `/_p/<plugin>/`; `req.path` is relative to that prefix and
    /// must match a `[routes]` entry (path and method) of a plugin with `routes = true`.
    /// The call is bounded like every other (fuel, memory, the route deadline) and takes
    /// a call slot like every other (Task 8 m7: a flood of route requests cannot exhaust
    /// instances); `Busy`, not a failure, when none freed up within the route deadline.
    /// The wait counts toward the deadline as for actions (final review M5): wait and
    /// call together take at most the hard deadline (1.5 × the route deadline), and a
    /// call the wait shortened that then ran out of time is `Busy`.
    /// A response outside the route checks (status 200..=599, at most
    /// [`MAX_ROUTE_HEADERS`] headers whose names and values are valid header text of at
    /// most [`MAX_ROUTE_HEADER_BYTES`], a body of at most [`MAX_ROUTE_BODY_BYTES`]) is a
    /// `CallFailed`, recorded as `InvalidOutput`.
    pub async fn handle_route(
        &self,
        plugin: &str,
        req: RouteRequest,
    ) -> Result<RouteResponse, PluginError> {
        let loaded = self.enabled_plugin(plugin)?;
        let routed =
            route_spec(&loaded, &req.path).is_some_and(|r| r.methods.contains(&req.method));
        if !routed {
            return Err(PluginError::RouteNotFound {
                plugin: plugin.to_string(),
                method: req.method,
                path: req.path,
            });
        }
        let hook = format!("route {} {}", req.method, req.path);
        let deadline = self.deadline(DeadlineClass::Route);
        let (_permit, waited) = self
            .permit(deadline)
            .await
            .ok_or_else(|| self.busy(plugin, &hook))?;
        let limit = CallDeadline::after_wait(deadline, waited);
        let started = Instant::now();
        let failed = |reason: String| PluginError::CallFailed {
            plugin: plugin.to_string(),
            reason,
        };
        let res = call_route(&loaded, req, limit).await.map_err(|failure| {
            if limit.ended_by_budget(&failure) {
                self.stats
                    .record(plugin, &hook, Outcome::Cut(started.elapsed()));
                self.log_busy(plugin, &hook);
                return PluginError::Busy {
                    plugin: plugin.to_owned(),
                };
            }
            self.record_call_failure(&loaded, &hook, &failure, started);
            failed(failure.to_string())
        })?;
        if let Err(reason) = validate_route_response(&res) {
            self.record_failure(&loaded, &hook, FailureKind::InvalidOutput, &reason, started);
            return Err(failed(format!("invalid response: {reason}")));
        }
        self.stats
            .record(plugin, &hook, Outcome::Success(started.elapsed()));
        Ok(res)
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

    /// `plugin`'s call statistics per hook (by hook name), counted on this host since
    /// it was built: at most [`MAX_STATS_HOOKS`] hooks per plugin. Empty for a plugin
    /// that was never called here.
    pub fn call_stats(&self, plugin: &str) -> Vec<HookStats> {
        self.stats.of(plugin)
    }

    /// Calls that ran and calls that failed on this host since it was built, every
    /// plugin and hook.
    pub fn totals(&self) -> (u64, u64) {
        self.stats.totals()
    }

    /// Counts a budget skip for each enabled plugin of `rest` (the chain's remainder).
    fn count_skipped(&self, rest: &[Arc<LoadedPlugin>], hook: &str) {
        for p in rest.iter().filter(|p| p.disabled.get().is_none()) {
            self.stats.record(&p.name, hook, Outcome::Skipped);
        }
    }

    /// Recorded call failures, oldest first. Keeps the most recent 1_000 across all
    /// plugins; older records are dropped.
    pub fn failures(&self) -> Vec<FailureRecord> {
        self.failures.history()
    }
}

/// `path`'s `[routes]` entry when the plugin holds the `routes` grant.
fn route_spec<'a>(loaded: &'a LoadedPlugin, path: &str) -> Option<&'a RouteSpec> {
    if !loaded.manifest.capabilities.routes {
        return None;
    }
    loaded.manifest.routes.get(path)
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
            HostConfig {
                max_concurrent_calls: 0,
                ..base()
            },
            HostConfig {
                max_concurrent_calls: Semaphore::MAX_PERMITS + 1,
                ..base()
            },
            HostConfig {
                pooling: true,
                max_concurrent_calls: 257,
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
        let pooled = HostConfig {
            pooling: true,
            max_concurrent_calls: 256,
            ..base()
        };
        assert!(new_host(pooled).is_ok());
    }
}
