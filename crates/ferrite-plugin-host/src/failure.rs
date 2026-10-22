//! Failure records of plugin calls and the sliding-window tracker behind auto-disable
//! (design 05 §8).

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

/// Most failure records kept for [`crate::PluginHost::failures`]; the oldest are dropped.
pub(crate) const MAX_HISTORY: usize = 1_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    Trap,
    OutOfFuel,
    Deadline,
    Memory,
    Instantiate,
    GuestError,
    InvalidOutput,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailureRecord {
    pub plugin: String,
    pub hook: String,
    pub kind: FailureKind,
    pub at: Instant,
    pub duration: Duration,
}

#[derive(Default)]
struct Inner {
    /// Per plugin: timestamps of its most recent failures, at most `threshold` of them
    /// (older ones cannot influence whether the threshold is crossed).
    windows: BTreeMap<String, VecDeque<Instant>>,
    history: VecDeque<FailureRecord>,
}

/// Records failures and says when a plugin has failed too often within the window.
/// Time is passed in by the caller, so tests need no sleeping.
pub(crate) struct FailureTracker {
    threshold: usize,
    window: Duration,
    inner: Mutex<Inner>,
}

impl FailureTracker {
    pub(crate) fn new(threshold: u32, window: Duration) -> Self {
        Self {
            threshold: usize::try_from(threshold).unwrap_or(usize::MAX),
            window,
            inner: Mutex::new(Inner::default()),
        }
    }

    /// Store `record`; `true` when its plugin now has at least `threshold` failures
    /// within `window` ending at `record.at`.
    pub(crate) fn record(&self, record: FailureRecord) -> bool {
        let mut inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        let now = record.at;
        let times = inner.windows.entry(record.plugin.clone()).or_default();
        times.push_back(now);
        while times
            .front()
            .is_some_and(|t| now.saturating_duration_since(*t) > self.window)
        {
            times.pop_front();
        }
        while times.len() > self.threshold {
            times.pop_front();
        }
        let crossed = times.len() >= self.threshold;
        if inner.history.len() == MAX_HISTORY {
            inner.history.pop_front();
        }
        inner.history.push_back(record);
        crossed
    }

    /// Forget the window of `plugin` (a fresh install starts with a clean slate).
    pub(crate) fn reset(&self, plugin: &str) {
        let mut inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        inner.windows.remove(plugin);
    }

    pub(crate) fn history(&self) -> Vec<FailureRecord> {
        let inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        inner.history.iter().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(plugin: &str, at: Instant) -> FailureRecord {
        FailureRecord {
            plugin: plugin.into(),
            hook: "render.head".into(),
            kind: FailureKind::Trap,
            at,
            duration: Duration::ZERO,
        }
    }

    #[test]
    fn crosses_threshold_inside_window() {
        let t = FailureTracker::new(3, Duration::from_secs(60));
        let t0 = Instant::now();
        assert!(!t.record(rec("a", t0)));
        assert!(!t.record(rec("a", t0 + Duration::from_secs(10))));
        assert!(t.record(rec("a", t0 + Duration::from_secs(20))));
    }

    #[test]
    fn old_failures_expire() {
        let t = FailureTracker::new(3, Duration::from_secs(60));
        let t0 = Instant::now();
        t.record(rec("a", t0));
        t.record(rec("a", t0 + Duration::from_secs(1)));
        // The first two are older than the window by now.
        assert!(!t.record(rec("a", t0 + Duration::from_secs(100))));
        assert!(!t.record(rec("a", t0 + Duration::from_secs(101))));
        assert!(t.record(rec("a", t0 + Duration::from_secs(102))));
    }

    #[test]
    fn plugins_are_independent() {
        let t = FailureTracker::new(2, Duration::from_secs(60));
        let t0 = Instant::now();
        assert!(!t.record(rec("a", t0)));
        assert!(!t.record(rec("b", t0)));
        assert!(t.record(rec("a", t0)));
    }

    #[test]
    fn reset_clears_window_not_history() {
        let t = FailureTracker::new(2, Duration::from_secs(60));
        let t0 = Instant::now();
        t.record(rec("a", t0));
        t.reset("a");
        assert!(!t.record(rec("a", t0)));
        assert_eq!(t.history().len(), 2);
    }

    #[test]
    fn history_is_bounded() {
        let t = FailureTracker::new(u32::MAX, Duration::from_secs(60));
        let t0 = Instant::now();
        for _ in 0..MAX_HISTORY + 10 {
            t.record(rec("a", t0));
        }
        assert_eq!(t.history().len(), MAX_HISTORY);
    }
}
