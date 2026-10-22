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
