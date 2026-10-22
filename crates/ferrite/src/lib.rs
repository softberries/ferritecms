//! The `ferrite` binary's logic, split from `main.rs` so tests (and later `serve`) can
//! call it directly.

mod cli;
mod init;
mod migrate;
mod open;

pub use cli::{Cli, Command, MigrateCommand, run};
pub use open::{Decision, decide, open_store, startup_check};

/// The version of this binary, parsed from `Cargo.toml`.
pub fn binary_version() -> anyhow::Result<semver::Version> {
    Ok(semver::Version::parse(env!("CARGO_PKG_VERSION"))?)
}

/// The process environment as UTF-8 pairs; entries that are not valid UTF-8 are skipped.
pub(crate) fn utf8_env() -> impl Iterator<Item = (String, String)> {
    std::env::vars_os().filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?)))
}

/// Loads `ferrite.toml` + environment from `data_dir`.
pub(crate) fn load_config(data_dir: &std::path::Path) -> anyhow::Result<ferrite_config::Config> {
    Ok(ferrite_config::load(data_dir, utf8_env())?)
}
