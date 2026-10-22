//! Wasmtime engine construction and the epoch ticker.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use wasmtime::{Config, Engine, InstanceAllocationStrategy, PoolingAllocationConfig};

use crate::config::HostConfig;
use crate::error::PluginError;

/// Concurrent component instances the pooling allocator reserves slots for. Calls beyond
/// this fail to instantiate (counted as a plugin failure), so it is sized well above
/// realistic request concurrency.
const POOL_SLOTS: u32 = 256;

/// Build an engine with fuel metering and epoch interruption enabled; with
/// `cfg.pooling` the pooling allocator, sized to match the per-store limits.
pub fn build_engine(cfg: &HostConfig) -> Result<Engine, PluginError> {
    let mut config = Config::new();
    config.consume_fuel(true).epoch_interruption(true);
    if cfg.pooling {
        let mut pool = PoolingAllocationConfig::new();
        pool.total_component_instances(POOL_SLOTS)
            .total_core_instances(POOL_SLOTS * 3)
            .total_memories(POOL_SLOTS)
            .total_tables(POOL_SLOTS * 2)
            .max_core_instances_per_component(3)
            .max_memories_per_component(1)
            .max_tables_per_component(2)
            .max_memory_size(cfg.memory_bytes)
            .table_elements(10_000);
        config.allocation_strategy(InstanceAllocationStrategy::Pooling(pool));
    }
    Engine::new(&config).map_err(|e| PluginError::Engine(format!("{e:#}")))
}

/// Background thread that advances the engine epoch every `tick`; stopped on drop.
pub(crate) struct EpochTicker {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl EpochTicker {
    pub(crate) fn start(engine: Engine, tick: Duration) -> Result<Self, PluginError> {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let thread = std::thread::Builder::new()
            .name("ferrite-epoch".into())
            .spawn(move || {
                while !flag.load(Ordering::Relaxed) {
                    std::thread::sleep(tick);
                    engine.increment_epoch();
                }
            })
            .map_err(|e| PluginError::Engine(format!("starting epoch ticker: {e}")))?;
        Ok(Self {
            stop,
            thread: Some(thread),
        })
    }
}

impl Drop for EpochTicker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            // The thread only sleeps and bumps an atomic, so it cannot panic; a join
            // error would carry nothing actionable.
            let _ = thread.join();
        }
    }
}
