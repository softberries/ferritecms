//! What one filter call touched (design 05 §7a): the dependency keys of the data it read
//! and whether its output is volatile.
//!
//! The host records while the guest runs, inside the import implementations
//! (`state.rs`), so a plugin cannot under-report:
//!
//! * `content.get-post`: `post:<id>` of the requested id, found or not (publishing it
//!   later must refresh the page); `get-post-by-slug`: `archive:<kind>` plus `post:<id>`
//!   of the post returned; `list-posts`: `archive:<kind>` plus `post:<id>` of each post
//!   returned.
//! * `meta.get`: `post:<id>` of the item read. `options.get`: `option:<key>`.
//! * Volatile: any `kv` or `http` call, a read of the wall clock
//!   (`wasi:clocks/wall-clock.now`), and a failed call (its output was dropped, and the
//!   failure may be transient). The monotonic clock is not volatile: it measures
//!   intervals and reveals no date.
//!
//! Only calls that pass their capability and budget checks are recorded: a refused call
//! returns nothing, so nothing in the output depends on it.
//!
//! Key formats follow the grammar in `ferrite-core`'s `events` module (this crate does
//! not depend on core; a test in `ferrite-http` pins the two together).

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use wasmtime_wasi::HostWallClock;

use crate::types::FilterValue;

/// Dependency key formats (the `ferrite-core` events grammar).
pub mod dep_keys {
    /// `post:<id>`: one content item.
    pub fn post(id: &str) -> String {
        format!("post:{id}")
    }

    /// `archive:<kind>`: the listing of a content type.
    pub fn archive(kind: &str) -> String {
        format!("archive:{kind}")
    }

    /// `option:<key>`: one option value.
    pub fn option(key: &str) -> String {
        format!("option:{key}")
    }
}

/// Longest id or kind text recorded as part of a key. Ids are UUIDs and kinds short
/// names; a guest asking for a longer one gets nothing recorded (and nothing found).
const MAX_KEY_PART: usize = 128;

/// The result of [`PluginHost::apply_filter_traced`](crate::PluginHost::apply_filter_traced).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilterOutcome {
    pub value: FilterValue,
    /// Dependency keys recorded by every plugin call of the chain, sorted, deduplicated.
    pub deps: Vec<String>,
    /// Some call used `kv`, `http` or the wall clock, or failed.
    pub volatile: bool,
}

#[derive(Debug, Default)]
struct Recorded {
    deps: BTreeSet<String>,
    volatile: bool,
}

/// Shared recorder for one call: the store state writes to it, the caller reads it
/// after the call, even when the call timed out and its store was dropped.
#[derive(Debug, Clone, Default)]
pub(crate) struct CallTrace(Arc<Mutex<Recorded>>);

impl CallTrace {
    fn lock(&self) -> MutexGuard<'_, Recorded> {
        // Every update is a single insert or flag write, so a poisoned lock still holds
        // consistent data.
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn post(&self, id: &str) {
        if id.len() <= MAX_KEY_PART {
            self.lock().deps.insert(dep_keys::post(id));
        }
    }

    pub(crate) fn archive(&self, kind: &str) {
        if kind.len() <= MAX_KEY_PART {
            self.lock().deps.insert(dep_keys::archive(kind));
        }
    }

    /// `key` already matched the plugin's `options.read` grants.
    pub(crate) fn option(&self, key: &str) {
        self.lock().deps.insert(dep_keys::option(key));
    }

    pub(crate) fn volatile(&self) {
        self.lock().volatile = true;
    }

    /// Fold this call's record into a chain total.
    pub(crate) fn drain_into(&self, deps: &mut BTreeSet<String>, volatile: &mut bool) {
        let mut r = self.lock();
        deps.append(&mut r.deps);
        *volatile |= r.volatile;
    }
}

/// The host wall clock, marking the call volatile whenever the guest reads it.
pub(crate) struct TracingWallClock {
    inner: Box<dyn HostWallClock + Send>,
    trace: CallTrace,
}

impl TracingWallClock {
    pub(crate) fn new(trace: CallTrace) -> Self {
        Self {
            inner: wasmtime_wasi::clocks::wall_clock(),
            trace,
        }
    }
}

impl HostWallClock for TracingWallClock {
    fn resolution(&self) -> Duration {
        self.inner.resolution()
    }

    fn now(&self) -> Duration {
        self.trace.volatile();
        self.inner.now()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_parts_are_not_recorded() {
        let t = CallTrace::default();
        t.post(&"x".repeat(MAX_KEY_PART + 1));
        t.archive(&"k".repeat(MAX_KEY_PART + 1));
        t.post("p");
        let (mut deps, mut volatile) = (BTreeSet::new(), false);
        t.drain_into(&mut deps, &mut volatile);
        assert_eq!(deps.into_iter().collect::<Vec<_>>(), ["post:p"]);
        assert!(!volatile);
    }

    #[test]
    fn wall_clock_reads_mark_volatile() {
        let t = CallTrace::default();
        let clock = TracingWallClock::new(t.clone());
        let _ = clock.resolution();
        let (mut deps, mut volatile) = (BTreeSet::new(), false);
        t.drain_into(&mut deps, &mut volatile);
        assert!(!volatile, "resolution reveals no time");
        assert!(clock.now() > Duration::ZERO);
        t.drain_into(&mut deps, &mut volatile);
        assert!(volatile);
    }
}
