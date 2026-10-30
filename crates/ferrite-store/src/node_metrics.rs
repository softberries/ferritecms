//! Shared validation and decoding of `node_metrics` rows (M4-2 P35) for both backends.

use ferrite_domain::{MINUTE_MICROS, MetricCounts, SeriesBucket, Timestamp};

use crate::StoreError;

/// The bound values of one `node_metrics` row: the nine counts in column order and
/// `p95_ms`. `Invalid`: `minute` not a whole minute, a count above `i64::MAX`, or `p95_ms`
/// above `i32::MAX`.
pub fn check_node_metrics(
    minute: Timestamp,
    counts: &MetricCounts,
) -> Result<([i64; 9], i32), StoreError> {
    if minute.as_micros().rem_euclid(MINUTE_MICROS) != 0 {
        return Err(StoreError::Invalid(
            "node metrics minute must be a whole minute".into(),
        ));
    }
    let mut out = [0i64; 9];
    for (slot, n) in out.iter_mut().zip(counts.counters()) {
        *slot = i64::try_from(n)
            .map_err(|_| StoreError::Invalid(format!("node metrics count {n} is too large")))?;
    }
    let p95 = i32::try_from(counts.p95_ms).map_err(|_| {
        StoreError::Invalid(format!("node metrics p95 {} is too large", counts.p95_ms))
    })?;
    Ok((out, p95))
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

/// A series bucket from its summed database values (sums are never negative).
pub fn series_bucket_from_db(
    start: Timestamp,
    sums: [i64; 9],
    p95: i64,
) -> Result<SeriesBucket, StoreError> {
    let corrupt = |v: i64| StoreError::Database(format!("corrupt node metrics value {v}"));
    let mut counts = [0u64; 9];
    for (slot, v) in counts.iter_mut().zip(sums) {
        *slot = u64::try_from(v).map_err(|_| corrupt(v))?;
    }
    let p95 = u32::try_from(p95).map_err(|_| corrupt(p95))?;
    Ok(SeriesBucket {
        start,
        counts: MetricCounts::from_counters(counts, p95),
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
        let counts = MetricCounts::from_counters([1, 2, 3, 4, 5, 6, 7, 8, 9], 25);
        let (c, p) = check_node_metrics(at(MINUTE_MICROS * 5), &counts).unwrap();
        assert_eq!((c, p), ([1, 2, 3, 4, 5, 6, 7, 8, 9], 25));
        assert!(check_node_metrics(at(MINUTE_MICROS * 5 + 1), &counts).is_err());
        let big = MetricCounts {
            jobs_failed: u64::MAX,
            ..MetricCounts::default()
        };
        assert!(check_node_metrics(at(0), &big).is_err());
        let slow = MetricCounts {
            p95_ms: u32::MAX,
            ..MetricCounts::default()
        };
        assert!(check_node_metrics(at(0), &slow).is_err());
        assert!(check_series_range(at(1), at(1)).is_err());
        assert!(check_series_range(at(1), at(2)).is_ok());
        assert!(series_bucket_from_db(at(0), [-1; 9], 0).is_err());
    }
}
