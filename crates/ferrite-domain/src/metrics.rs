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

/// Upper bounds (ms) of the page latency buckets; one more bucket holds the rest (over
/// 10 s).
pub const PAGE_LATENCY_BOUNDS_MS: [u32; 11] =
    [5, 10, 25, 50, 100, 250, 500, 1000, 2500, 5000, 10_000];

/// Buckets of a page latency histogram: [`PAGE_LATENCY_BOUNDS_MS`] plus the overflow one.
pub const PAGE_LATENCY_SLOTS: usize = PAGE_LATENCY_BOUNDS_MS.len() + 1;

/// The upper bound (ms) of the latency bucket holding the 95th percentile of `counts`
/// (per-bucket counts as in [`MetricCounts::page_latency`]): the first bucket at which
/// the running total reaches `ceil(0.95 × total)`. 0 without responses; the overflow
/// bucket (over 10 s) reports the largest bound, 10 000.
pub fn page_p95_ms(counts: &[u64; PAGE_LATENCY_SLOTS]) -> u32 {
    let last = PAGE_LATENCY_BOUNDS_MS[PAGE_LATENCY_BOUNDS_MS.len() - 1];
    let total: u64 = counts.iter().fold(0, |a, n| a.saturating_add(*n));
    if total == 0 {
        return 0;
    }
    // ceil(0.95 × total) without overflow.
    let rank = total - total / 20;
    let mut seen = 0u64;
    for (i, n) in counts.iter().enumerate() {
        seen = seen.saturating_add(*n);
        if seen >= rank {
            return PAGE_LATENCY_BOUNDS_MS.get(i).copied().unwrap_or(last);
        }
    }
    last
}

/// The figures of one node's minute, or of a series bucket (summed over minutes and
/// nodes, the latency histogram too).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MetricCounts {
    /// Responses of every route class but `other` (health checks).
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
    /// Page-class responses by latency bucket: index `i < 11` counts the responses that
    /// took at most [`PAGE_LATENCY_BOUNDS_MS`]`[i]` ms and more than the bound before
    /// it; the last index counts those over 10 s. Not cumulative over the bounds, so the
    /// histograms of several minutes and nodes sum (M4-2 R7).
    pub page_latency: [u64; PAGE_LATENCY_SLOTS],
}

impl MetricCounts {
    /// Adds `other` into `self` (counts saturate; the histograms are summed per bucket).
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
        for (sum, n) in self.page_latency.iter_mut().zip(other.page_latency) {
            *sum = sum.saturating_add(n);
        }
    }

    /// The page p95 of these counts ([`page_p95_ms`] of the histogram): over a series
    /// bucket it is the percentile of all the bucket's page responses, every minute's
    /// and node's together.
    pub fn p95_ms(&self) -> u32 {
        page_p95_ms(&self.page_latency)
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

    /// Counts in table column order (as [`counters`](Self::counters)) and a page latency
    /// histogram.
    pub fn from_counters(c: [u64; 9], page_latency: [u64; PAGE_LATENCY_SLOTS]) -> Self {
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
            page_latency,
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
    fn merge_sums_counts_and_histograms() {
        let mut a = MetricCounts::from_counters([1, 2, 3, 4, 5, 6, 7, 8, 9], {
            let mut h = [0; PAGE_LATENCY_SLOTS];
            h[0] = 10;
            h
        });
        let mut slow = [0; PAGE_LATENCY_SLOTS];
        slow[0] = 9;
        slow[7] = 1;
        a.merge(&MetricCounts::from_counters([10; 9], slow));
        assert_eq!(a.counters(), [11, 12, 13, 14, 15, 16, 17, 18, 19]);
        assert_eq!(a.page_latency[0], 19);
        assert_eq!(a.page_latency[7], 1);
        // 19 fast and 1 slow: the p95 is the fast bucket, not the slowest response.
        assert_eq!(a.p95_ms(), 5);
        a.merge(&MetricCounts {
            requests: u64::MAX,
            ..MetricCounts::default()
        });
        assert_eq!(a.requests, u64::MAX);
    }

    #[test]
    fn p95_is_the_bound_of_the_bucket_holding_it() {
        let mut c = [0u64; PAGE_LATENCY_SLOTS];
        assert_eq!(page_p95_ms(&c), 0, "no responses");
        c[0] = 1;
        assert_eq!(page_p95_ms(&c), 5);
        // 19 fast, 1 slow: rank 19 is still fast.
        c[0] = 19;
        c[7] = 1;
        assert_eq!(page_p95_ms(&c), 5);
        // 18 fast, 2 slow: rank 19 is slow.
        c[0] = 18;
        c[7] = 2;
        assert_eq!(page_p95_ms(&c), 1000);
        // Fewer than 20 responses: the rank is the slowest one.
        let mut few = [0u64; PAGE_LATENCY_SLOTS];
        few[0] = 9;
        few[7] = 1;
        assert_eq!(page_p95_ms(&few), 1000);
        let mut slow = [0u64; PAGE_LATENCY_SLOTS];
        slow[PAGE_LATENCY_SLOTS - 1] = 3;
        assert_eq!(page_p95_ms(&slow), 10_000);
        let huge = [u64::MAX; PAGE_LATENCY_SLOTS];
        // The total saturates (no overflow panic); the first bucket already reaches it.
        assert_eq!(page_p95_ms(&huge), 5);
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
