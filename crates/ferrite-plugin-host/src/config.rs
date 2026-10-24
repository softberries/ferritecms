//! Host-wide default limits (design 05 §6).

use std::time::Duration;

/// Limits and tuning for the plugin host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostConfig {
    pub memory_bytes: usize,
    pub fuel: u64,
    pub render_deadline: Duration,
    pub action_deadline: Duration,
    pub route_deadline: Duration,
    pub epoch_tick: Duration,
    pub disable_after_failures: u32,
    pub failure_window: Duration,
    /// Use wasmtime's pooling instance allocator (design 05 §6).
    pub pooling: bool,
    /// Most plugin calls (filters, actions and routes together) running at once on this
    /// host (P22, `plugins.max_concurrent_calls`). A call waits at most its deadline for
    /// a slot, then is skipped without counting as a plugin failure.
    pub max_concurrent_calls: usize,
}

/// `max(8, 4 × available_parallelism)`: the default of
/// [`HostConfig::max_concurrent_calls`] (and of `plugins.max_concurrent_calls`).
fn default_max_concurrent_calls() -> usize {
    std::thread::available_parallelism()
        .map_or(1, std::num::NonZeroUsize::get)
        .saturating_mul(4)
        .max(8)
}

impl Default for HostConfig {
    fn default() -> Self {
        Self {
            memory_bytes: 32 * 1024 * 1024,
            fuel: 50_000_000,
            // Ruling R18: render filters may make store-backed imports (a few content
            // reads are several round trips each), so 50 ms was too tight.
            render_deadline: Duration::from_millis(200),
            action_deadline: Duration::from_millis(5_000),
            route_deadline: Duration::from_millis(2_000),
            epoch_tick: Duration::from_millis(5),
            disable_after_failures: 20,
            failure_window: Duration::from_secs(10 * 60),
            pooling: false,
            max_concurrent_calls: default_max_concurrent_calls(),
        }
    }
}
