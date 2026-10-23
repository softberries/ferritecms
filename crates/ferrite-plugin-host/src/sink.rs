//! Where the host reports each failed call besides its own log and counter (P20): the
//! node buffers these and shares them through the database, so a plugin that fails on
//! every node is disabled cluster-wide.

use crate::FailureKind;

/// Receives every recorded plugin failure. Called synchronously on the calling task,
/// from inside the failed hook call, so an implementation must never block or await:
/// buffer in memory and write elsewhere.
pub trait FailureSink: Send + Sync + 'static {
    fn record(&self, plugin: &str, hook: &str, kind: FailureKind);
}
