//! Error types of the plugin host.

use std::fmt;

/// One validation problem, located by a dotted TOML path (e.g. `hooks.render.nope`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestProblem {
    pub path: String,
    pub message: String,
}

impl fmt::Display for ManifestProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.path.is_empty() {
            write!(f, "{}", self.message)
        } else {
            write!(f, "{}: {}", self.path, self.message)
        }
    }
}

/// Every problem found in a manifest; parsing never stops at the first one.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid plugin manifest ({} problem(s)):{}", .problems.len(), list(.problems))]
pub struct ManifestError {
    pub problems: Vec<ManifestProblem>,
}

fn list(problems: &[ManifestProblem]) -> String {
    problems.iter().map(|p| format!("\n  - {p}")).collect()
}

/// Host-level failures that are not tied to a single plugin.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PluginError {
    /// The wasmtime engine could not be configured.
    #[error("engine configuration failed: {0}")]
    Engine(String),
}

/// Why a component was rejected at install time.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InspectError {
    /// A plain core wasm module, not a component.
    #[error("not a WebAssembly component (core module)")]
    NotAComponent,
    /// The bytes do not compile as a component.
    #[error("invalid component: {0}")]
    Invalid(String),
    /// Imports a ferrite interface whose capability the manifest does not declare.
    #[error(
        "component imports `{import}` but the manifest does not declare capability `{capability}`"
    )]
    UndeclaredImport {
        import: String,
        capability: &'static str,
    },
    /// Imports something outside the allowed WASI / ferrite set.
    #[error("component imports forbidden interface `{0}`")]
    ForbiddenImport(String),
    /// No `ferrite:plugin/guest` export.
    #[error("component does not export `ferrite:plugin/guest`")]
    MissingGuestExport,
    /// Exports a guest interface of a version this host does not support.
    #[error("unsupported plugin world version `{0}`")]
    UnsupportedWorld(String),
}
