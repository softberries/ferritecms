//! Wasmtime engine construction.

use wasmtime::{Config, Engine};

use crate::config::HostConfig;
use crate::error::PluginError;

/// Build an engine with fuel metering and epoch interruption enabled.
///
/// `cfg` is not consulted yet; limits are applied per store in later tasks.
pub fn build_engine(_cfg: &HostConfig) -> Result<Engine, PluginError> {
    let mut config = Config::new();
    config.consume_fuel(true).epoch_interruption(true);
    Engine::new(&config).map_err(|e| PluginError::Engine(format!("{e:#}")))
}
