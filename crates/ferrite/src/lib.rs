//! The `ferrite` binary's logic, split from `main.rs` so tests can call it directly.

mod cli;
pub mod extensions;
mod init;
pub mod jobs;
mod migrate;
mod node;
mod open;
pub mod reload;
mod seed;
pub mod serve;
mod site;

pub use cli::{Cli, Command, MigrateCommand, run};
pub use open::{Decision, OpenMode, decide, open_store, startup_check};
pub use serve::{NodeOptions, RUNTIME_SHUTDOWN, RunnerOverrides, RunningNode, start};
pub use site::{LoadedTheme, ThemeOrigin, load_theme, reload_theme};

/// The version of this binary, parsed from `Cargo.toml`.
pub fn binary_version() -> anyhow::Result<semver::Version> {
    Ok(semver::Version::parse(env!("CARGO_PKG_VERSION"))?)
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
