//! Contract tests for the per-node minute metrics and their series (M4-2 P35).

use ferrite_domain::{
    MINUTE_MICROS, MetricCounts, NODE_METRICS_RETENTION, NodeId, PAGE_LATENCY_SLOTS, SeriesStep,
    Timestamp,
};
use ferrite_store::{MAX_PRUNE_BATCH, StoreError};

use crate::TestDb;

const HOUR: i64 = 60 * MINUTE_MICROS;
const DAY: i64 = 24 * HOUR;

fn at(micros: i64) -> Timestamp {
    Timestamp::from_micros(micros).unwrap()
}

/// Counts `n` in every counter, with `n` page responses in latency bucket `slot`.
fn counts(n: u64, slot: usize) -> MetricCounts {
    let mut h = [0; PAGE_LATENCY_SLOTS];
    h[slot] = n;
    MetricCounts::from_counters([n; 9], h)
}

/// A whole day, so the minutes below are in one day bucket and in known hours.
fn day_start() -> i64 {
    Timestamp::now().as_micros() / DAY * DAY - DAY
}

/// Rows of two nodes: an insert into an existing (node, minute) adds to it; the series
/// sums over nodes per minute, hour and day bucket, filters by node and by
/// `[from, to)`, and leaves out buckets without rows. Bad input is `Invalid`.
pub async fn node_metrics_insert_and_series_sum_across_nodes(db: &TestDb) {
    let m = db.store().read().node_metrics();
    let (a, b) = (NodeId::new(), NodeId::new());
    let t0 = day_start();

    m.insert(a, at(t0), &counts(1, 5)).await.unwrap();
    // A second write of the same minute (a shutdown flush) adds to the row.
    m.insert(a, at(t0), &counts(2, 5)).await.unwrap();
    m.insert(b, at(t0), &counts(10, 5)).await.unwrap();
    m.insert(a, at(t0 + MINUTE_MICROS), &counts(4, 5))
        .await
        .unwrap();
    m.insert(b, at(t0 + HOUR + 2 * MINUTE_MICROS), &counts(100, 5))
        .await
        .unwrap();
    // The next day: outside a one-day range.
    m.insert(a, at(t0 + DAY), &counts(1000, 5)).await.unwrap();

    let minutes = m
        .series(at(t0), at(t0 + DAY), SeriesStep::Minute, None)
        .await
        .unwrap();
    let got: Vec<(i64, u64)> = minutes
        .iter()
        .map(|b| (b.start.as_micros(), b.counts.requests))
        .collect();
    assert_eq!(
        got,
        [
            (t0, 13),
            (t0 + MINUTE_MICROS, 4),
            (t0 + HOUR + 2 * MINUTE_MICROS, 100)
        ],
        "summed over nodes, only minutes with rows, ascending"
    );
    assert_eq!(minutes[0].counts, counts(13, 5), "every counter summed");

    let hours = m
        .series(at(t0), at(t0 + DAY), SeriesStep::Hour, None)
        .await
        .unwrap();
    let got: Vec<(i64, u64)> = hours
        .iter()
        .map(|b| (b.start.as_micros(), b.counts.jobs_failed))
        .collect();
    assert_eq!(got, [(t0, 17), (t0 + HOUR, 100)]);

    let days = m
        .series(at(t0), at(t0 + 2 * DAY), SeriesStep::Day, None)
        .await
        .unwrap();
    let got: Vec<(i64, u64)> = days
        .iter()
        .map(|b| (b.start.as_micros(), b.counts.page_views))
        .collect();
    assert_eq!(got, [(t0, 117), (t0 + DAY, 1000)]);

    // One node only.
    let only_b = m
        .series(at(t0), at(t0 + DAY), SeriesStep::Day, Some(b))
        .await
        .unwrap();
    assert_eq!(only_b.len(), 1);
    assert_eq!(only_b[0].counts.requests, 110);
    let nobody = m
        .series(at(t0), at(t0 + DAY), SeriesStep::Day, Some(NodeId::new()))
        .await
        .unwrap();
    assert!(nobody.is_empty());

    // `to` is exclusive, `from` inclusive.
    let edge = m
        .series(
            at(t0 + MINUTE_MICROS),
            at(t0 + HOUR + 2 * MINUTE_MICROS),
            SeriesStep::Minute,
            None,
        )
        .await
        .unwrap();
    assert_eq!(edge.len(), 1);
    assert_eq!(edge[0].start.as_micros(), t0 + MINUTE_MICROS);

    // Counts beyond u32 survive.
    let big = NodeId::new();
    m.insert(big, at(t0 - DAY), &counts(u64::from(u32::MAX), 0))
        .await
        .unwrap();
    m.insert(big, at(t0 - DAY), &counts(u64::from(u32::MAX), 0))
        .await
        .unwrap();
    let s = m
        .series(at(t0 - DAY), at(t0), SeriesStep::Day, None)
        .await
        .unwrap();
    assert_eq!(s[0].counts.requests, 2 * u64::from(u32::MAX));

    // Validation.
    let invalid = |r: Result<_, StoreError>| matches!(r, Err(StoreError::Invalid(_)));
    assert!(invalid(m.insert(a, at(t0 + 1), &counts(1, 0)).await));
    // Before 1970 the backends would bucket differently: refused on both.
    assert!(invalid(
        m.insert(a, at(-MINUTE_MICROS), &counts(1, 0)).await
    ));
    assert!(invalid(m.insert(a, at(t0), &counts(u64::MAX, 0)).await));
    let mut too_many = counts(1, 0);
    too_many.page_latency[3] = u64::MAX;
    assert!(invalid(m.insert(a, at(t0), &too_many).await));
    assert!(invalid(
        m.series(at(t0), at(t0), SeriesStep::Hour, None)
            .await
            .map(|_| ())
    ));
}

/// A page latency histogram: `pages` responses in each `(slot, pages)`.
fn latency(slots: &[(usize, u64)]) -> [u64; PAGE_LATENCY_SLOTS] {
    let mut h = [0; PAGE_LATENCY_SLOTS];
    for &(slot, pages) in slots {
        h[slot] += pages;
    }
    h
}

/// One request per page response, with the given histogram.
fn pages(slots: &[(usize, u64)]) -> MetricCounts {
    let h = latency(slots);
    let n = h.iter().sum();
    MetricCounts::from_counters([n; 9], h)
}

/// A bucket's p95 comes from the histogram summed over its minutes and nodes (M4-2 R7),
/// never the largest of their p95s: 19 fast page responses and 1 slow one, spread over two
/// nodes and two minutes, have a fast p95 over the hour and over the shared minute; the
/// minute that holds the slow one alone (fewer than 20 responses) is slow. A second write
/// of a minute adds to its histogram.
pub async fn series_p95_from_combined_histogram(db: &TestDb) {
    let m = db.store().read().node_metrics();
    let (a, b) = (NodeId::new(), NodeId::new());
    let t0 = day_start();
    // Slot 0: at most 5 ms; slot 7: at most 1000 ms.
    m.insert(a, at(t0), &pages(&[(0, 6)])).await.unwrap();
    m.insert(a, at(t0), &pages(&[(0, 4)])).await.unwrap();
    m.insert(b, at(t0), &pages(&[(0, 5)])).await.unwrap();
    m.insert(b, at(t0 + MINUTE_MICROS), &pages(&[(0, 4), (7, 1)]))
        .await
        .unwrap();

    let hour = m
        .series(at(t0), at(t0 + HOUR), SeriesStep::Hour, None)
        .await
        .unwrap();
    assert_eq!(hour.len(), 1);
    assert_eq!(hour[0].counts.page_latency, latency(&[(0, 19), (7, 1)]));
    assert_eq!(
        hour[0].counts.p95_ms(),
        5,
        "19 fast + 1 slow: the p95 is fast"
    );
    let day = m
        .series(at(t0), at(t0 + DAY), SeriesStep::Day, None)
        .await
        .unwrap();
    assert_eq!(day[0].counts.p95_ms(), 5);

    let minutes = m
        .series(at(t0), at(t0 + HOUR), SeriesStep::Minute, None)
        .await
        .unwrap();
    let p95: Vec<u32> = minutes.iter().map(|b| b.counts.p95_ms()).collect();
    assert_eq!(p95, [5, 1000], "the second minute holds 4 fast and 1 slow");
    assert_eq!(minutes[0].counts.page_latency, latency(&[(0, 15)]));

    // One node: its own histogram.
    let only_b = m
        .series(at(t0), at(t0 + HOUR), SeriesStep::Hour, Some(b))
        .await
        .unwrap();
    assert_eq!(only_b[0].counts.page_latency, latency(&[(0, 9), (7, 1)]));
    assert_eq!(only_b[0].counts.p95_ms(), 1000);

    // Two slow responses of 20: the p95 is slow, over the hour and every bucket width.
    m.insert(a, at(t0 + 2 * MINUTE_MICROS), &pages(&[(7, 1)]))
        .await
        .unwrap();
    let hour = m
        .series(at(t0), at(t0 + HOUR), SeriesStep::Hour, None)
        .await
        .unwrap();
    assert_eq!(hour[0].counts.p95_ms(), 1000);

    // Histogram counts beyond u32 survive the sums.
    let big = NodeId::new();
    let huge = pages(&[(11, u64::from(u32::MAX))]);
    m.insert(big, at(t0 - DAY), &huge).await.unwrap();
    m.insert(big, at(t0 - DAY), &huge).await.unwrap();
    let s = m
        .series(at(t0 - DAY), at(t0), SeriesStep::Day, None)
        .await
        .unwrap();
    assert_eq!(s[0].counts.page_latency[11], 2 * u64::from(u32::MAX));
    assert_eq!(s[0].counts.p95_ms(), 10_000);
}

/// The 7-day prune: rows older than the retention go in batches, newer ones stay.
pub async fn prune_older_than_7_days(db: &TestDb) {
    let m = db.store().read().node_metrics();
    let node = NodeId::new();
    let now = Timestamp::now().as_micros() / MINUTE_MICROS * MINUTE_MICROS;
    let retention = i64::try_from(NODE_METRICS_RETENTION.as_micros()).unwrap();
    let cutoff = now - retention;
    for i in 1..=5 {
        m.insert(node, at(cutoff - i * MINUTE_MICROS), &counts(1, 0))
            .await
            .unwrap();
    }
    m.insert(node, at(cutoff), &counts(1, 0)).await.unwrap();
    m.insert(node, at(now - DAY), &counts(1, 0)).await.unwrap();

    assert!(matches!(
        m.prune(at(cutoff), 0).await,
        Err(StoreError::Invalid(_))
    ));
    assert!(matches!(
        m.prune(at(cutoff), MAX_PRUNE_BATCH + 1).await,
        Err(StoreError::Invalid(_))
    ));
    assert_eq!(m.prune(at(cutoff), 3).await.unwrap(), 3, "one batch");
    assert_eq!(m.prune(at(cutoff), 3).await.unwrap(), 2, "a short batch");
    assert_eq!(m.prune(at(cutoff), 3).await.unwrap(), 0);
    let left = m
        .series(at(cutoff - DAY), at(now + 1), SeriesStep::Day, None)
        .await
        .unwrap();
    let total: u64 = left.iter().map(|b| b.counts.requests).sum();
    assert_eq!(total, 2, "the row at the cutoff and the recent one stay");
}
