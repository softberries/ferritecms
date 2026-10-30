//! Per-node minute metrics and their time series (M4-2 P35, design 09 §6): every node
//! writes one `node_metrics` row per minute; the series API sums them over nodes into
//! minute, hour or day buckets.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::{Timestamp, string_enum};

/// One minute of microseconds.
pub const MINUTE_MICROS: i64 = 60_000_000;

/// How long `node_metrics` rows are kept (the `node_metrics.prune` job).
pub const NODE_METRICS_RETENTION: Duration = Duration::from_secs(7 * 24 * 3600);

/// The figures of one node's minute, or of a series bucket (summed over minutes and
/// nodes, with `p95_ms` the largest).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MetricCounts {
    /// Responses of every route class but `other` (health checks, the comment form).
    pub requests: u64,
    /// Those of `requests` with a 5xx status.
    pub errors_5xx: u64,
    pub page_views: u64,
    pub cache_hits: u64,
    pub cache_misses: u64,
    pub plugin_calls: u64,
    pub plugin_failures: u64,
    pub jobs_done: u64,
    pub jobs_failed: u64,
    /// The upper bound (ms) of the latency bucket holding the 95th percentile of the
    /// page responses; 0 without page responses. A bucket's value is the largest of its
    /// minutes' (and nodes').
    pub p95_ms: u32,
}

impl MetricCounts {
    /// Adds `other` into `self` (counts saturate; `p95_ms` is the larger).
    pub fn merge(&mut self, other: &MetricCounts) {
        self.requests = self.requests.saturating_add(other.requests);
        self.errors_5xx = self.errors_5xx.saturating_add(other.errors_5xx);
        self.page_views = self.page_views.saturating_add(other.page_views);
        self.cache_hits = self.cache_hits.saturating_add(other.cache_hits);
        self.cache_misses = self.cache_misses.saturating_add(other.cache_misses);
        self.plugin_calls = self.plugin_calls.saturating_add(other.plugin_calls);
        self.plugin_failures = self.plugin_failures.saturating_add(other.plugin_failures);
        self.jobs_done = self.jobs_done.saturating_add(other.jobs_done);
        self.jobs_failed = self.jobs_failed.saturating_add(other.jobs_failed);
        self.p95_ms = self.p95_ms.max(other.p95_ms);
    }

    /// The counts in table column order (`requests` .. `jobs_failed`).
    pub fn counters(&self) -> [u64; 9] {
        [
            self.requests,
            self.errors_5xx,
            self.page_views,
            self.cache_hits,
            self.cache_misses,
            self.plugin_calls,
            self.plugin_failures,
            self.jobs_done,
            self.jobs_failed,
        ]
    }

    /// Counts in table column order (as [`counters`](Self::counters)) and a p95.
    pub fn from_counters(c: [u64; 9], p95_ms: u32) -> Self {
        Self {
            requests: c[0],
            errors_5xx: c[1],
            page_views: c[2],
            cache_hits: c[3],
            cache_misses: c[4],
            plugin_calls: c[5],
            plugin_failures: c[6],
            jobs_done: c[7],
            jobs_failed: c[8],
            p95_ms,
        }
    }
}

string_enum!(
    /// The bucket width of a metrics series. Buckets are aligned to the Unix epoch (UTC).
    SeriesStep {
        Minute => "minute",
        Hour => "hour",
        Day => "day",
    }
);

impl SeriesStep {
    /// The bucket width in microseconds.
    pub fn micros(self) -> i64 {
        match self {
            SeriesStep::Minute => MINUTE_MICROS,
            SeriesStep::Hour => 60 * MINUTE_MICROS,
            SeriesStep::Day => 24 * 60 * MINUTE_MICROS,
        }
    }

    /// The longest `to - from` a series of this step may span: 24 h for minutes, 7 days
    /// for hours and days.
    pub fn max_span(self) -> Duration {
        match self {
            SeriesStep::Minute => Duration::from_secs(24 * 3600),
            SeriesStep::Hour | SeriesStep::Day => Duration::from_secs(7 * 24 * 3600),
        }
    }

    /// `micros` rounded down to a bucket start.
    pub fn floor(self, micros: i64) -> i64 {
        micros.div_euclid(self.micros()) * self.micros()
    }
}

/// One bucket of a series: the figures of the minutes starting in
/// `[start, start + step)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeriesBucket {
    pub start: Timestamp,
    pub counts: MetricCounts,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_sums_and_takes_the_larger_p95() {
        let mut a = MetricCounts::from_counters([1, 2, 3, 4, 5, 6, 7, 8, 9], 50);
        a.merge(&MetricCounts::from_counters([10; 9], 25));
        assert_eq!(a.counters(), [11, 12, 13, 14, 15, 16, 17, 18, 19]);
        assert_eq!(a.p95_ms, 50);
        a.merge(&MetricCounts {
            requests: u64::MAX,
            p95_ms: 100,
            ..MetricCounts::default()
        });
        assert_eq!((a.requests, a.p95_ms), (u64::MAX, 100));
    }

    #[test]
    fn steps_floor_to_epoch_aligned_buckets() {
        let day = SeriesStep::Day.micros();
        assert_eq!(SeriesStep::Day.floor(day + 5), day);
        assert_eq!(SeriesStep::Hour.floor(-1), -SeriesStep::Hour.micros());
        assert_eq!(
            SeriesStep::Minute.floor(MINUTE_MICROS * 3),
            MINUTE_MICROS * 3
        );
        assert_eq!("hour".parse::<SeriesStep>().unwrap(), SeriesStep::Hour);
        assert!("week".parse::<SeriesStep>().is_err());
        assert_eq!(SeriesStep::Minute.max_span(), Duration::from_secs(86_400));
    }
}
