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
    /// A `HostConfig` value makes the host unusable (zero deadline, tick or threshold).
    #[error("invalid host configuration: {0}")]
    InvalidConfig(String),
    /// The component failed install-time inspection.
    #[error("component rejected: {0}")]
    Inspect(#[from] InspectError),
    /// `[limits]` asks for more than the host default (admin approval is post-M0).
    #[error("plugin requests {limit} = {requested}, above the host default {default}")]
    LimitAboveDefault {
        limit: &'static str,
        requested: u64,
        default: u64,
    },
    /// The component cannot be linked against its granted interfaces.
    #[error("linking plugin `{plugin}` failed: {reason}")]
    Link { plugin: String, reason: String },
    /// No installed (enabled) plugin with this name.
    #[error("unknown plugin `{0}`")]
    UnknownPlugin(String),
    /// The plugin does not subscribe to this action hook, or it is not an action.
    #[error("plugin `{plugin}` does not handle action `{hook}`")]
    NotSubscribed { plugin: String, hook: String },
    /// No `[routes]` entry of the plugin matches the request.
    #[error("plugin `{plugin}` has no route {method} {path}")]
    RouteNotFound {
        plugin: String,
        method: String,
        path: String,
    },
    /// The plugin call was stopped or failed (trap, timeout, guest error, …).
    #[error("plugin `{plugin}` call failed: {reason}")]
    CallFailed { plugin: String, reason: String },
    /// The plugin was disabled (automatically after repeated failures).
    #[error("plugin `{plugin}` is disabled: {reason}")]
    Disabled { plugin: String, reason: String },
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
