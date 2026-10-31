//! The periodic scheduler (04 §6, ruling P11): every node ticks and inserts one job per
//! period, keyed by the period start on the database clock, so the unique key admits
//! exactly one row per period cluster-wide without a leader.

use std::{sync::Arc, time::Duration};

use ferrite_domain::{NewJob, Timestamp};
use ferrite_store::Store;
use serde_json::Value;
use tokio::{task::JoinHandle, time::MissedTickBehavior};
use tokio_util::sync::CancellationToken;

/// How often each node's scheduler tries to insert the current period's jobs.
pub const SCHEDULER_TICK: Duration = Duration::from_secs(10);

/// A job inserted once per period.
#[derive(Debug, Clone, PartialEq)]
pub struct Periodic {
    pub kind: &'static str,
    /// The period; whole seconds (rounded down, at least 1 s).
    pub every: Duration,
    /// The job payload (a JSON object).
    pub payload: Value,
    /// Attempts each period's job gets (usually [`ferrite_domain::DEFAULT_MAX_ATTEMPTS`]).
    pub max_attempts: i32,
    /// When in its period the job is due: `run_at` is the period start plus this (keep it
    /// below `every`). The key stays the period start, so the offset only moves the time
    /// (M4-4 P75: `artifacts.gc` at 03:00 UTC, apart from a daily backup at 00:00).
    pub offset: Duration,
}

impl Periodic {
    /// The job of the period holding `db_now`: keyed by [`period_key`], due at the period
    /// start plus [`Periodic::offset`] (in the past when a node ticks after it: due at once).
    pub fn job_at(&self, db_now: Timestamp) -> NewJob {
        let (key, start) = period_key(self.kind, self.every, db_now);
        let offset = i64::try_from(self.offset.as_micros()).unwrap_or(i64::MAX);
        let run_at = start
            .as_micros()
            .checked_add(offset)
            .and_then(|us| Timestamp::from_micros(us).ok())
            .unwrap_or(start);
        NewJob {
            kind: self.kind.to_owned(),
            payload: self.payload.clone(),
            unique_key: Some(key),
            run_at: Some(run_at),
            priority: 0,
            max_attempts: self.max_attempts,
        }
    }
}

/// `(unique_key, period start)` for `kind` at `db_now`: the start is
/// `floor(db_now / every) × every` and the key is `"<kind>:<start, unix seconds>"`.
/// `every` counts in whole seconds, at least 1.
pub fn period_key(kind: &str, every: Duration, db_now: Timestamp) -> (String, Timestamp) {
    const MICROS: i64 = 1_000_000;
    let secs = i64::try_from(every.as_secs()).unwrap_or(i64::MAX).max(1);
    let now_secs = db_now.as_micros().div_euclid(MICROS);
    let start_secs = now_secs.div_euclid(secs) * secs;
    let start = start_secs
        .checked_mul(MICROS)
        .and_then(|us| Timestamp::from_micros(us).ok())
        .unwrap_or(db_now);
    (format!("{kind}:{start_secs}"), start)
}

/// Inserts each [`Periodic`] job once per period (on any node).
pub struct Scheduler {
    store: Arc<dyn Store>,
    periodics: Vec<Periodic>,
    tick: Duration,
}

impl Scheduler {
    /// A scheduler ticking every `tick` ([`SCHEDULER_TICK`] in production).
    pub fn new(store: Arc<dyn Store>, periodics: Vec<Periodic>, tick: Duration) -> Self {
        Self {
            store,
            periodics,
            tick,
        }
    }

    /// Ticks on the current runtime until `stop`; the first tick is immediate.
    pub fn spawn(self, stop: CancellationToken) -> JoinHandle<()> {
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(self.tick);
            ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
            loop {
                tokio::select! {
                    biased;
                    () = stop.cancelled() => return,
                    _ = ticker.tick() => self.tick_once().await,
                }
            }
        })
    }

    /// Reads the database clock and inserts the current period's job of every periodic;
    /// a key already taken (by any node) is the normal case. Errors are logged; the next
    /// tick tries again.
    async fn tick_once(&self) {
        let repos = self.store.read();
        let now = match repos.nodes().db_now().await {
            Ok(now) => now,
            Err(e) => {
                tracing::warn!(error = %e, "scheduler could not read the database clock");
                return;
            }
        };
        for periodic in &self.periodics {
            let job = periodic.job_at(now);
            match repos.jobs().enqueue(&job).await {
                Ok(Some(id)) => tracing::debug!(kind = periodic.kind, %id, "periodic job inserted"),
                Ok(None) => {}
                Err(e) => tracing::warn!(
                    kind = periodic.kind, error = %e,
                    "could not insert periodic job; retrying next tick"
                ),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-10-23T00:00:00Z in unix seconds.
    const DAY: i64 = 1_792_713_600;

    /// That day at `h:m` plus `micros`.
    fn at(h: i64, m: i64, micros: i64) -> Timestamp {
        Timestamp::from_micros((DAY + h * 3600 + m * 60) * 1_000_000 + micros).unwrap()
    }

    #[test]
    fn period_key_is_floor_of_db_time() {
        let hour = Duration::from_secs(3600);
        let ten = DAY + 10 * 3600;
        let eleven = ten + 3600;
        let (key, start) = period_key("events.prune", hour, at(10, 59, 59_900_000));
        assert_eq!(key, format!("events.prune:{ten}"));
        assert_eq!(start.to_rfc3339(), "2026-10-23T10:00:00+00:00");
        let (key, start) = period_key("events.prune", hour, at(11, 0, 0));
        assert_eq!(key, format!("events.prune:{eleven}"));
        assert_eq!(start.to_rfc3339(), "2026-10-23T11:00:00+00:00");

        // Sub-second periods count as 1 s; 30 s periods floor to :00 / :30.
        let (key, _) = period_key("k", Duration::from_millis(10), at(10, 0, 5_500_000));
        assert_eq!(key, format!("k:{}", ten + 5));
        let (key, _) = period_key("k", Duration::from_secs(30), at(10, 0, 59_000_000));
        assert_eq!(key, format!("k:{}", ten + 30));
    }

    /// P75: an offset moves the job's `run_at` within its period; the key stays the
    /// period start, so a node ticking before or after the offset inserts the same row.
    #[test]
    fn periodic_offset_shifts_run_at() {
        let mut periodic = Periodic {
            kind: "artifacts.gc",
            every: Duration::from_secs(86_400),
            payload: serde_json::json!({"v": 1}),
            max_attempts: 3,
            offset: Duration::ZERO,
        };
        let plain = periodic.job_at(at(1, 30, 0));
        assert_eq!(
            plain.unique_key.as_deref(),
            Some(&*format!("artifacts.gc:{DAY}"))
        );
        assert_eq!(plain.run_at, Some(at(0, 0, 0)));
        assert_eq!(plain.max_attempts, 3);

        periodic.offset = Duration::from_secs(3 * 3600);
        for now in [at(1, 30, 0), at(3, 0, 0), at(23, 59, 0)] {
            let job = periodic.job_at(now);
            assert_eq!(job.unique_key, plain.unique_key, "{now:?}");
            assert_eq!(job.run_at, Some(at(3, 0, 0)), "{now:?}");
            assert_eq!(job.kind, "artifacts.gc");
            assert_eq!(job.payload, periodic.payload);
        }
    }
}
