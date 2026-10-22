//! Shared helpers for integration tests: build and load the example plugins.

#[path = "../../src/test_support.rs"]
mod test_support;

pub use test_support::plugin_wasm;
use test_support::repo_root;

/// The plugin's `ferrite-plugin.toml` (from `examples/<name>` or `examples/hostile/<name>`).
pub fn plugin_manifest(name: &str) -> String {
    let examples = repo_root().join("plugins/examples");
    let dir = [examples.join(name), examples.join("hostile").join(name)]
        .into_iter()
        .find(|d| d.join("ferrite-plugin.toml").is_file())
        .unwrap_or_else(|| panic!("no ferrite-plugin.toml for plugin {name}"));
    let path = dir.join("ferrite-plugin.toml");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
}
