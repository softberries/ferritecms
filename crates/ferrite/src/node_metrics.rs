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
//! **Failures.** A periodic write that fails is logged and its delta kept: the next write
//! carries it, still under the failed period's minute. A periodic write has no timeout of
//! its own (a slow write only delays the next one): a statement abandoned by a timeout
//! could still commit, and the kept delta would then be added twice, since a write adds to
//! an existing row of the same (node, minute) (`NodeMetricsRepo::insert`). Only the final
//! flush is bounded; losing that partial minute is the lesser evil there.

use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use ferrite_domain::{MINUTE_MICROS, MetricCounts, NodeId, Timestamp};
use ferrite_http::metrics::{Metrics, MetricsSnapshot};
use ferrite_store::{Store, StoreError};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

/// The longest the final flush at shutdown may take.
pub const FINAL_FLUSH: Duration = Duration::from_millis(500);

/// Where the writer's rows go: the store's `node_metrics` (tests substitute a sink).
#[async_trait]
pub trait MinuteSink: Send + Sync {
    /// Adds `counts` to `node`'s row of `minute`.
    async fn insert(
        &self,
        node: NodeId,
        minute: Timestamp,
        counts: &MetricCounts,
    ) -> Result<(), StoreError>;
}

#[async_trait]
impl MinuteSink for Arc<dyn Store> {
    async fn insert(
        &self,
        node: NodeId,
        minute: Timestamp,
        counts: &MetricCounts,
    ) -> Result<(), StoreError> {
        self.read()
            .node_metrics()
            .insert(node, minute, counts)
            .await
    }
}

/// The writer of one node.
pub struct MinuteWriter {
    sink: Arc<dyn MinuteSink>,
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
        sink: Arc<dyn MinuteSink>,
        node: NodeId,
        metrics: Arc<Metrics>,
        every: Option<Duration>,
    ) -> Self {
        Self {
            sink,
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
            if self.write(&now, &last, period_start, None).await {
                last = now;
                period_start = Timestamp::now();
            }
        }
        let now = self.metrics.snapshot();
        self.write(&now, &last, period_start, Some(FINAL_FLUSH))
            .await;
    }

    /// Writes `now - last` under the minute of `period_start`, within `limit` when given;
    /// whether it was written.
    async fn write(
        &self,
        now: &MetricsSnapshot,
        last: &MetricsSnapshot,
        period_start: Timestamp,
        limit: Option<Duration>,
    ) -> bool {
        let counts = now.since(last).counts();
        let minute = minute_of(period_start);
        let insert = self.sink.insert(self.node, minute, &counts);
        let done = match limit {
            Some(limit) => match tokio::time::timeout(limit, insert).await {
                Ok(done) => done,
                Err(_) => {
                    tracing::warn!("the final flush of the node's minute metrics timed out");
                    return false;
                }
            },
            None => insert.await,
        };
        match done {
            Ok(()) => true,
            Err(e) => {
                tracing::warn!(error = %e, "cannot write the node's minute metrics; kept for the next write");
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
    use std::sync::Mutex;

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

    /// Records every attempt (minute, page views, accepted); refuses the first.
    #[derive(Default)]
    struct FailOnce {
        calls: Mutex<Vec<(Timestamp, u64, bool)>>,
    }

    #[async_trait]
    impl MinuteSink for FailOnce {
        async fn insert(
            &self,
            _node: NodeId,
            minute: Timestamp,
            counts: &MetricCounts,
        ) -> Result<(), StoreError> {
            let mut calls = self.calls.lock().unwrap();
            let ok = !calls.is_empty();
            calls.push((minute, counts.page_views, ok));
            if ok {
                Ok(())
            } else {
                Err(StoreError::Database("down".into()))
            }
        }
    }

    /// A failed write keeps its delta and its period: the next write carries both
    /// (under the failed period's minute); after a success the next period starts anew,
    /// and the final flush writes the rest.
    #[tokio::test(start_paused = true)]
    async fn a_failed_write_is_carried_by_the_next() {
        let sink = Arc::new(FailOnce::default());
        let metrics = Arc::new(Metrics::new("0.0.0", "test"));
        let stop = CancellationToken::new();
        let start = minute_of(Timestamp::now());
        let task = MinuteWriter::new(
            sink.clone(),
            NodeId::new(),
            metrics.clone(),
            Some(Duration::from_secs(1)),
        )
        .spawn(stop.clone());
        // The writer takes its first snapshot.
        tokio::time::sleep(Duration::from_millis(500)).await;
        metrics.count_page_view();
        metrics.count_page_view();
        // Tick at 1 s: refused.
        tokio::time::sleep(Duration::from_secs(1)).await;
        metrics.count_page_view();
        // Tick at 2 s: the carried 2 plus 1.
        tokio::time::sleep(Duration::from_secs(1)).await;
        metrics.count_page_view();
        stop.cancel();
        task.await.unwrap();

        let calls = sink.calls.lock().unwrap().clone();
        let views: Vec<(u64, bool)> = calls.iter().map(|c| (c.1, c.2)).collect();
        assert_eq!(views, [(2, false), (3, true), (1, true)], "{calls:?}");
        // The paused clock does not move the wall clock far: the carried write is under
        // the failed period's minute (the start's, unless the wall clock just crossed a
        // minute).
        assert_eq!(calls[0].0, calls[1].0, "carried under the old minute");
        assert!(calls[0].0 >= start);
    }
}
