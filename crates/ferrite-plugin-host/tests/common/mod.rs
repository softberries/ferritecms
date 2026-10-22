//! Shared helpers for integration tests: build and load the example plugins.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

const TARGET: &str = "wasm32-wasip2";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .unwrap_or_else(|| panic!("crate lives two levels below the repo root"))
        .to_path_buf()
}

/// Build `plugins/` once per test binary; panics with cargo's stderr on failure.
fn build_plugins() {
    static BUILT: OnceLock<()> = OnceLock::new();
    BUILT.get_or_init(|| {
        let manifest = repo_root().join("plugins/Cargo.toml");
        let out = Command::new(env!("CARGO"))
            .args(["build", "--release", "--target", TARGET, "--manifest-path"])
            .arg(&manifest)
            .output()
            .unwrap_or_else(|e| panic!("failed to spawn cargo: {e}"));
        assert!(
            out.status.success(),
            "building plugins failed:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
    });
}

/// Bytes of the built component for the plugin crate `name`.
pub fn plugin_wasm(name: &str) -> Vec<u8> {
    build_plugins();
    let file = format!("{}.wasm", name.replace('-', "_"));
    let path = repo_root()
        .join("plugins/target")
        .join(TARGET)
        .join("release")
        .join(file);
    std::fs::read(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
}

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
