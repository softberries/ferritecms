//! Wasmtime engine construction and the epoch ticker.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use wasmtime::{Config, Engine};

use crate::config::HostConfig;
use crate::error::PluginError;

/// Build an engine with fuel metering and epoch interruption enabled.
///
/// `cfg` is not consulted yet: all limits in `HostConfig` are per store. The pooling
/// allocator switch (Task 9) will be read here.
pub fn build_engine(_cfg: &HostConfig) -> Result<Engine, PluginError> {
    let mut config = Config::new();
    config.consume_fuel(true).epoch_interruption(true);
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
