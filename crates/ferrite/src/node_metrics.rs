//! The per-node minute writer (M4-2 P35, design 09 §6): a `serve` background task (not a
//! job) that writes one `node_metrics` row per minute from the deltas of the node's
//! metrics registry, solo included.
//!
//! **Periods.** Each period runs from the previous write (or the task's start) to the
//! next whole UTC minute of the wall clock; its delta ([`MetricsSnapshot::since`]) is
//! written under the minute its period started in. So the first, partial minute after a
//! boot has a row of its own, and every later row covers one whole minute.
//!
//! **Shutdown.** When the background tasks stop (after the request drain and the job
//! runner, so their last responses and jobs are counted) the partial minute is flushed
//! once more, bounded by [`FINAL_FLUSH`]; the shutdown's own bound cuts it if the database
//! is slow.
//!
//! **Failures.** A write that fails (or takes over [`WRITE_TIMEOUT`]) is logged and its
//! delta kept: the next write carries it, still under the failed period's minute. A write
//! adds to an existing row of the same (node, minute) (`NodeMetricsRepo::insert`), so a
//! retried or repeated minute never conflicts or double-counts what was not written.

use std::{sync::Arc, time::Duration};

use ferrite_domain::{MINUTE_MICROS, NodeId, Timestamp};
use ferrite_http::metrics::{Metrics, MetricsSnapshot};
use ferrite_store::Store;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

/// The longest one periodic write may take.
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);
/// The longest the final flush at shutdown may take.
pub const FINAL_FLUSH: Duration = Duration::from_millis(500);

/// The writer of one node.
pub struct MinuteWriter {
    store: Arc<dyn Store>,
    node: NodeId,
    metrics: Arc<Metrics>,
    /// `None`: at every whole minute of the wall clock (production); `Some(d)` (test
    /// seam): every `d` from the start, rows still under the minute their period
    /// started in, so several periods may add to one row.
    every: Option<Duration>,
}

impl MinuteWriter {
    /// A writer of `node`'s rows from `metrics`, at the times `every` gives (the field docs).
    pub fn new(
        store: Arc<dyn Store>,
        node: NodeId,
        metrics: Arc<Metrics>,
        every: Option<Duration>,
    ) -> Self {
        Self {
            store,
            node,
            metrics,
            every,
        }
    }

    /// Runs the writer until `stop`, then flushes the partial minute.
    pub fn spawn(self, stop: CancellationToken) -> JoinHandle<()> {
        tokio::spawn(async move { self.run(stop).await })
    }

    async fn run(self, stop: CancellationToken) {
        let mut last = self.metrics.snapshot();
        let mut period_start = Timestamp::now();
        loop {
            let wait = self
                .every
                .unwrap_or_else(|| until_next_minute(Timestamp::now()));
            tokio::select! {
                () = stop.cancelled() => break,
                () = tokio::time::sleep(wait) => {}
            }
            let now = self.metrics.snapshot();
            if self.write(&now, &last, period_start, WRITE_TIMEOUT).await {
                last = now;
                period_start = Timestamp::now();
            }
        }
        let now = self.metrics.snapshot();
        self.write(&now, &last, period_start, FINAL_FLUSH).await;
    }

    /// Writes `now - last` under the minute of `period_start`; whether it was written.
    async fn write(
        &self,
        now: &MetricsSnapshot,
        last: &MetricsSnapshot,
        period_start: Timestamp,
        limit: Duration,
    ) -> bool {
        let counts = now.since(last).counts();
        let minute = minute_of(period_start);
        let insert = self
            .store
            .read()
            .node_metrics()
            .insert(self.node, minute, &counts);
        match tokio::time::timeout(limit, insert).await {
            Ok(Ok(())) => true,
            Ok(Err(e)) => {
                tracing::warn!(error = %e, "cannot write the node's minute metrics; kept for the next write");
                false
            }
            Err(_) => {
                tracing::warn!(
                    "writing the node's minute metrics timed out; kept for the next write"
                );
                false
            }
        }
    }
}

/// The whole minute `t` falls in.
fn minute_of(t: Timestamp) -> Timestamp {
    let m = t.as_micros();
    // A clock in range gives a minute in range; fall back to `t` itself otherwise.
    Timestamp::from_micros(m - m.rem_euclid(MINUTE_MICROS)).unwrap_or(t)
}

/// How long from `t` to the next whole minute (a full minute at a boundary).
fn until_next_minute(t: Timestamp) -> Duration {
    let into = t.as_micros().rem_euclid(MINUTE_MICROS);
    let left = u64::try_from(MINUTE_MICROS - into).unwrap_or(0);
    Duration::from_micros(left)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(micros: i64) -> Timestamp {
        Timestamp::from_micros(micros).unwrap()
    }

    #[test]
    fn minutes_and_waits() {
        let m = 1_000 * MINUTE_MICROS;
        assert_eq!(minute_of(at(m)), at(m));
        assert_eq!(minute_of(at(m + MINUTE_MICROS - 1)), at(m));
        assert_eq!(until_next_minute(at(m)), Duration::from_secs(60));
        assert_eq!(
            until_next_minute(at(m + 59_500_000)),
            Duration::from_millis(500)
        );
    }
}
