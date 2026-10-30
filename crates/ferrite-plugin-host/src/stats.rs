//! Per-plugin, per-hook call statistics of one host (M2a-2b P20): node-local, counted
//! since the host was built, read by `GET /plugins/{name}/metrics`. A time series and a
//! metrics registry are M4.
//!
//! Counted where the host decides each outcome: a call that ran is a success, a
//! failure (the ones counted toward auto-disable) or a budget cut (cut short by the
//! render budget or a slot wait, not a failure); a call that never ran is `busy` (no
//! call slot) or a budget skip (the page's render budget was spent).

use std::{
    collections::HashMap,
    sync::{
        Mutex, PoisonError,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

/// Most hooks tracked per plugin; calls of further hooks (route paths are the
/// plugin's to choose) count only toward `CallStats::totals`.
pub const MAX_STATS_HOOKS: usize = 64;

/// Upper bounds of the duration buckets, in milliseconds; the last bucket is "more".
pub const DURATION_BUCKETS_MS: [u64; 5] = [1, 5, 25, 100, 500];

/// The statistics of one plugin hook.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HookStats {
    pub hook: String,
    /// Calls that ran (successes, failures and budget cuts).
    pub calls: u64,
    /// Calls that failed (counted toward auto-disable).
    pub failures: u64,
    /// Calls skipped because no call slot freed up in time.
    pub busy: u64,
    /// Calls the render budget (or a slot wait) cut short, and calls skipped because the
    /// page's render budget was spent.
    pub budget_cut: u64,
    /// Summed duration of the calls that ran, in microseconds.
    pub total_micros: u64,
    /// The longest call, in microseconds.
    pub max_micros: u64,
    /// Calls that ran by duration: at most 1, 5, 25, 100, 500 ms, and longer.
    pub buckets: [u64; 6],
}

/// How one call ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Outcome {
    Success(Duration),
    Failure(Duration),
    /// Ran, then was cut by the budget.
    Cut(Duration),
    /// Never ran: the budget was spent.
    Skipped,
    /// Never ran: no call slot.
    Busy,
}

/// One plugin's counters, by hook name.
type PluginHooks = HashMap<Box<str>, HookStats>;

/// The host's counters: per plugin, per hook. A call allocates nothing once its
/// plugin and hook have an entry (two `&str` lookups under the lock); the first call
/// of a new plugin or hook allocates its entry.
#[derive(Debug, Default)]
pub(crate) struct CallStats {
    plugins: Mutex<HashMap<Box<str>, PluginHooks>>,
    calls: AtomicU64,
    failures: AtomicU64,
}

impl CallStats {
    pub(crate) fn record(&self, plugin: &str, hook: &str, outcome: Outcome) {
        let ran = match outcome {
            Outcome::Success(d) | Outcome::Failure(d) | Outcome::Cut(d) => Some(d),
            Outcome::Skipped | Outcome::Busy => None,
        };
        if ran.is_some() {
            self.calls.fetch_add(1, Ordering::Relaxed);
        }
        if matches!(outcome, Outcome::Failure(_)) {
            self.failures.fetch_add(1, Ordering::Relaxed);
        }
        let mut plugins = self.plugins.lock().unwrap_or_else(PoisonError::into_inner);
        if !plugins.contains_key(plugin) {
            plugins.insert(plugin.into(), HashMap::new());
        }
        let Some(hooks) = plugins.get_mut(plugin) else {
            return;
        };
        if !hooks.contains_key(hook) {
            if hooks.len() >= MAX_STATS_HOOKS {
                return;
            }
            let stats = HookStats {
                hook: hook.to_owned(),
                ..HookStats::default()
            };
            hooks.insert(hook.into(), stats);
        }
        let Some(stats) = hooks.get_mut(hook) else {
            return;
        };
        match outcome {
            Outcome::Success(_) => {}
            Outcome::Failure(_) => stats.failures += 1,
            Outcome::Cut(_) | Outcome::Skipped => stats.budget_cut += 1,
            Outcome::Busy => stats.busy += 1,
        }
        if let Some(d) = ran {
            let micros = u64::try_from(d.as_micros()).unwrap_or(u64::MAX);
            stats.calls += 1;
            stats.total_micros = stats.total_micros.saturating_add(micros);
            stats.max_micros = stats.max_micros.max(micros);
            stats.buckets[bucket(d)] += 1;
        }
    }

    /// `plugin`'s hooks, by name.
    pub(crate) fn of(&self, plugin: &str) -> Vec<HookStats> {
        let plugins = self.plugins.lock().unwrap_or_else(PoisonError::into_inner);
        let mut out: Vec<HookStats> = plugins
            .get(plugin)
            .map(|hooks| hooks.values().cloned().collect())
            .unwrap_or_default();
        out.sort_by(|a, b| a.hook.cmp(&b.hook));
        out
    }

    /// Every plugin's hooks, plugins and hooks by name (one lock, for a metrics scrape).
    pub(crate) fn all(&self) -> Vec<(String, Vec<HookStats>)> {
        let plugins = self.plugins.lock().unwrap_or_else(PoisonError::into_inner);
        let mut out: Vec<(String, Vec<HookStats>)> = plugins
            .iter()
            .map(|(name, hooks)| {
                let mut hooks: Vec<HookStats> = hooks.values().cloned().collect();
                hooks.sort_by(|a, b| a.hook.cmp(&b.hook));
                (name.to_string(), hooks)
            })
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    /// Drops the counters of every plugin `loaded` rejects (uninstalled from the host).
    /// The totals keep counting their calls.
    pub(crate) fn retain(&self, loaded: impl Fn(&str) -> bool) {
        let mut plugins = self.plugins.lock().unwrap_or_else(PoisonError::into_inner);
        plugins.retain(|name, _| loaded(name));
    }

    /// Calls that ran and failures, every plugin and hook.
    pub(crate) fn totals(&self) -> (u64, u64) {
        (
            self.calls.load(Ordering::Relaxed),
            self.failures.load(Ordering::Relaxed),
        )
    }
}

/// The index of `d`'s duration bucket.
fn bucket(d: Duration) -> usize {
    let micros = d.as_micros();
    DURATION_BUCKETS_MS
        .iter()
        .position(|ms| micros <= u128::from(*ms) * 1000)
        .unwrap_or(DURATION_BUCKETS_MS.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_are_bucketed() {
        let ms = Duration::from_millis;
        assert_eq!(bucket(Duration::ZERO), 0);
        assert_eq!(bucket(ms(1)), 0);
        assert_eq!(bucket(ms(1) + Duration::from_micros(1)), 1);
        assert_eq!(bucket(ms(5)), 1);
        assert_eq!(bucket(ms(25)), 2);
        assert_eq!(bucket(ms(100)), 3);
        assert_eq!(bucket(ms(500)), 4);
        assert_eq!(bucket(ms(501)), 5);
        assert_eq!(bucket(Duration::from_secs(3600)), 5);
    }

    #[test]
    fn outcomes_land_in_their_counters() {
        let stats = CallStats::default();
        let ms = Duration::from_millis;
        stats.record("p", "render.head", Outcome::Success(ms(2)));
        stats.record("p", "render.head", Outcome::Failure(ms(30)));
        stats.record("p", "render.head", Outcome::Cut(ms(600)));
        stats.record("p", "render.head", Outcome::Skipped);
        stats.record("p", "post.saved", Outcome::Busy);
        stats.record("q", "render.head", Outcome::Success(ms(0)));
        let p = stats.of("p");
        assert_eq!(
            p,
            [
                HookStats {
                    hook: "post.saved".into(),
                    busy: 1,
                    ..HookStats::default()
                },
                HookStats {
                    hook: "render.head".into(),
                    calls: 3,
                    failures: 1,
                    busy: 0,
                    budget_cut: 2,
                    total_micros: 632_000,
                    max_micros: 600_000,
                    buckets: [0, 1, 0, 1, 0, 1],
                },
            ]
        );
        assert_eq!(stats.totals(), (4, 1));
        assert!(stats.of("nobody").is_empty());
        let all = stats.all();
        let names: Vec<&str> = all.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["p", "q"]);
        assert_eq!(all[0].1, p);
    }

    #[test]
    fn hooks_per_plugin_are_capped() {
        let stats = CallStats::default();
        for i in 0..MAX_STATS_HOOKS + 10 {
            stats.record(
                "p",
                &format!("route GET /{i}"),
                Outcome::Success(Duration::ZERO),
            );
        }
        stats.record("q", "render.head", Outcome::Success(Duration::ZERO));
        assert_eq!(stats.of("p").len(), MAX_STATS_HOOKS);
        assert_eq!(stats.of("q").len(), 1, "another plugin has its own cap");
        // Untracked hooks still count toward the totals.
        assert_eq!(stats.totals(), (MAX_STATS_HOOKS as u64 + 11, 0));
        // A tracked hook keeps counting.
        stats.record("p", "route GET /0", Outcome::Success(Duration::ZERO));
        let first = stats
            .of("p")
            .into_iter()
            .find(|s| s.hook == "route GET /0")
            .unwrap();
        assert_eq!(first.calls, 2);
    }

    #[test]
    fn retain_drops_uninstalled_plugins() {
        let stats = CallStats::default();
        stats.record("p", "render.head", Outcome::Success(Duration::ZERO));
        stats.record("q", "render.head", Outcome::Failure(Duration::ZERO));
        stats.retain(|name| name == "q");
        assert!(stats.of("p").is_empty());
        assert_eq!(stats.of("q").len(), 1);
        assert_eq!(
            stats.totals(),
            (2, 1),
            "totals are since the host was built"
        );
    }
}
