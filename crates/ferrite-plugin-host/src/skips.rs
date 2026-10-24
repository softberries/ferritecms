//! Rate-limited warnings for skipped plugin calls (P22, review m7): under sustained
//! load every page skips calls, so each kind of skip of each subject is warned at most
//! once per [`SKIP_WARN_EVERY`], with the number of skips logged only at debug since.

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

/// Shortest interval between two warnings for the same skip kind and subject.
pub(crate) const SKIP_WARN_EVERY: Duration = Duration::from_secs(60);

/// Why calls were skipped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Skip {
    /// No call slot free within the call's deadline (subject: the plugin).
    Busy,
    /// A page's render budget ran out (subject: the hook).
    Budget,
}

struct Window {
    warned_at: Instant,
    suppressed: u64,
}

/// The last warning per (skip kind, subject). Holds one entry per plugin or hook.
pub(crate) struct SkipLog {
    every: Duration,
    seen: Mutex<HashMap<(Skip, String), Window>>,
}

impl SkipLog {
    pub(crate) fn new(every: Duration) -> Self {
        Self {
            every,
            seen: Mutex::new(HashMap::new()),
        }
    }

    /// `Some(n)`: warn now, `n` skips of this kind and subject went unwarned since the
    /// last warning; `None`: counted, log at debug only.
    pub(crate) fn admit(&self, skip: Skip, subject: &str, now: Instant) -> Option<u64> {
        // Each update is one insert or two field writes: a poisoned map is still usable.
        let mut seen = self.seen.lock().unwrap_or_else(PoisonError::into_inner);
        match seen.get_mut(&(skip, subject.to_owned())) {
            Some(w) if now.saturating_duration_since(w.warned_at) < self.every => {
                w.suppressed += 1;
                None
            }
            Some(w) => {
                let n = w.suppressed;
                *w = Window {
                    warned_at: now,
                    suppressed: 0,
                };
                Some(n)
            }
            None => {
                seen.insert(
                    (skip, subject.to_owned()),
                    Window {
                        warned_at: now,
                        suppressed: 0,
                    },
                );
                Some(0)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn warns_once_per_window_with_the_suppressed_count() {
        let log = SkipLog::new(Duration::from_secs(60));
        let t0 = Instant::now();
        assert_eq!(log.admit(Skip::Busy, "a", t0), Some(0));
        assert_eq!(
            log.admit(Skip::Busy, "a", t0 + Duration::from_secs(1)),
            None
        );
        assert_eq!(
            log.admit(Skip::Busy, "a", t0 + Duration::from_secs(2)),
            None
        );
        // Other subjects and kinds have their own windows.
        assert_eq!(log.admit(Skip::Busy, "b", t0), Some(0));
        assert_eq!(log.admit(Skip::Budget, "a", t0), Some(0));
        assert_eq!(
            log.admit(Skip::Busy, "a", t0 + Duration::from_secs(60)),
            Some(2)
        );
        assert_eq!(
            log.admit(Skip::Busy, "a", t0 + Duration::from_secs(61)),
            None
        );
    }
}
