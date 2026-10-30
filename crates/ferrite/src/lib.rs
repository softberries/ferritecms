//! The `ferrite` binary's logic, split from `main.rs` so tests can call it directly.

mod archive;
pub mod backup;
mod cli;
mod config_cmd;
mod export_cmd;
mod ext_cmd;
pub mod extensions;
pub mod gc;
mod healthcheck;
mod import_cmd;
mod init;
pub mod jobs;
pub mod keys;
mod media_cmd;
mod media_url;
mod migrate;
mod node;
mod open;
pub mod reload;
mod seed;
pub mod serve;
mod site;
mod site_copy;
#[cfg(test)]
#[path = "../../ferrite-plugin-host/src/test_support.rs"]
mod test_support;
pub mod update;
pub mod upgrade;
mod user_cmd;

pub use cli::{Cli, Command, MigrateCommand, run};
pub use open::{Decision, OpenMode, decide, open_store, startup_check};
#[cfg(feature = "test-seams")]
pub use serve::RunnerOverrides;
pub use serve::{NodeOptions, RUNTIME_SHUTDOWN, RunningNode, start};
pub use site::{LoadedTheme, ThemeOrigin, load_theme, reload_theme};

/// The version of this binary, parsed from `Cargo.toml`.
pub fn binary_version() -> anyhow::Result<semver::Version> {
    Ok(semver::Version::parse(env!("CARGO_PKG_VERSION"))?)
}

/// The target triple this binary was built for (`FERRITE_TARGET` in M4-2 P27), embedded
/// by `build.rs`; `self-update` picks its release asset by it.
pub fn binary_target() -> &'static str {
    include_str!(concat!(env!("OUT_DIR"), "/target-triple"))
}

/// The process environment as UTF-8 pairs. Non-UTF-8 variables are ignored, except a
/// `FERRITE_*` variable with a non-UTF-8 value, which is an error naming the key only.
pub(crate) fn utf8_env() -> anyhow::Result<Vec<(String, String)>> {
    let mut out = Vec::new();
    for (k, v) in std::env::vars_os() {
        let Ok(key) = k.into_string() else { continue };
        match v.into_string() {
            Ok(value) => out.push((key, value)),
            Err(_) if key.starts_with("FERRITE_") => {
                anyhow::bail!("environment variable {key} is not valid UTF-8")
            }
            Err(_) => {}
        }
    }
    Ok(out)
}

/// Loads `ferrite.toml` + environment from `data_dir`.
pub(crate) fn load_config(data_dir: &std::path::Path) -> anyhow::Result<ferrite_config::Config> {
    Ok(ferrite_config::load(data_dir, utf8_env()?)?)
}
