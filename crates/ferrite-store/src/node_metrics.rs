//! Shared validation and decoding of `node_metrics` rows (M4-2 P35) for both backends.

use ferrite_domain::{MINUTE_MICROS, MetricCounts, PAGE_LATENCY_SLOTS, SeriesBucket, Timestamp};

use crate::StoreError;

/// The bound values of one `node_metrics` row: the nine counts in column order and the
/// twelve page latency bucket counts (M4-2 R7). `Invalid`: `minute` before 1970 or not a
/// whole minute, or a count above `i64::MAX`.
pub fn check_node_metrics(
    minute: Timestamp,
    counts: &MetricCounts,
) -> Result<([i64; 9], [i64; PAGE_LATENCY_SLOTS]), StoreError> {
    // Before 1970 SQLite's integer division and Postgres' `date_bin` would bucket
    // differently; the writer never writes such a minute.
    if minute.as_micros() < 0 {
        return Err(StoreError::Invalid(
            "node metrics minute must not be before 1970".into(),
        ));
    }
    if minute.as_micros().rem_euclid(MINUTE_MICROS) != 0 {
        return Err(StoreError::Invalid(
            "node metrics minute must be a whole minute".into(),
        ));
    }
    let to_db = |n: u64| {
        i64::try_from(n)
            .map_err(|_| StoreError::Invalid(format!("node metrics count {n} is too large")))
    };
    let mut out = [0i64; 9];
    for (slot, n) in out.iter_mut().zip(counts.counters()) {
        *slot = to_db(n)?;
    }
    let mut latency = [0i64; PAGE_LATENCY_SLOTS];
    for (slot, n) in latency.iter_mut().zip(counts.page_latency) {
        *slot = to_db(n)?;
    }
    Ok((out, latency))
}

/// Validates a series range: `from < to`.
pub fn check_series_range(from: Timestamp, to: Timestamp) -> Result<(), StoreError> {
    if from >= to {
        return Err(StoreError::Invalid(
            "series range must have from before to".into(),
        ));
    }
    Ok(())
}

/// A series bucket from its summed database values: the nine counts and the twelve
/// page latency bucket counts (sums are never negative).
pub fn series_bucket_from_db(
    start: Timestamp,
    sums: [i64; 9],
    latency: [i64; PAGE_LATENCY_SLOTS],
) -> Result<SeriesBucket, StoreError> {
    let from_db = |v: i64| {
        u64::try_from(v)
            .map_err(|_| StoreError::Database(format!("corrupt node metrics value {v}")))
    };
    let mut counts = [0u64; 9];
    for (slot, v) in counts.iter_mut().zip(sums) {
        *slot = from_db(v)?;
    }
    let mut page_latency = [0u64; PAGE_LATENCY_SLOTS];
    for (slot, v) in page_latency.iter_mut().zip(latency) {
        *slot = from_db(v)?;
    }
    Ok(SeriesBucket {
        start,
        counts: MetricCounts::from_counters(counts, page_latency),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(micros: i64) -> Timestamp {
        Timestamp::from_micros(micros).unwrap()
    }

    #[test]
    fn rows_are_checked() {
        let mut latency = [0u64; PAGE_LATENCY_SLOTS];
        latency[2] = 25;
        let counts = MetricCounts::from_counters([1, 2, 3, 4, 5, 6, 7, 8, 9], latency);
        let (c, l) = check_node_metrics(at(MINUTE_MICROS * 5), &counts).unwrap();
        assert_eq!(c, [1, 2, 3, 4, 5, 6, 7, 8, 9]);
        assert_eq!(l, [0, 0, 25, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        assert!(check_node_metrics(at(MINUTE_MICROS * 5 + 1), &counts).is_err());
        assert!(
            check_node_metrics(at(-MINUTE_MICROS), &counts).is_err(),
            "before 1970"
        );
        let big = MetricCounts {
            jobs_failed: u64::MAX,
            ..MetricCounts::default()
        };
        assert!(check_node_metrics(at(0), &big).is_err());
        let mut huge = [0u64; PAGE_LATENCY_SLOTS];
        huge[11] = u64::MAX;
        let slow = MetricCounts {
            page_latency: huge,
            ..MetricCounts::default()
        };
        assert!(check_node_metrics(at(0), &slow).is_err());
        assert!(check_series_range(at(1), at(1)).is_err());
        assert!(check_series_range(at(1), at(2)).is_ok());
        assert!(series_bucket_from_db(at(0), [-1; 9], [0; PAGE_LATENCY_SLOTS]).is_err());
        assert!(series_bucket_from_db(at(0), [0; 9], [-1; PAGE_LATENCY_SLOTS]).is_err());
        let b = series_bucket_from_db(at(0), [1; 9], [2; PAGE_LATENCY_SLOTS]).unwrap();
        assert_eq!(b.counts.page_latency, [2; PAGE_LATENCY_SLOTS]);
    }
}
