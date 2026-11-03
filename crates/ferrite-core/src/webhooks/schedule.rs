//! The retry schedule (design 18 §4.3): Standard Webhooks' first 8 attempts.

use std::time::Duration;

use crate::{CoreError, auth::random_bytes};

const MIN: u64 = 60;
const HOUR: u64 = 3600;

/// The delay before each attempt: attempt 1 at once, then 5 s, 5 min, 30 min, 2 h, 5 h,
/// 10 h and 14 h.
pub const DELAYS: [Duration; 8] = [
    Duration::ZERO,
    Duration::from_secs(5),
    Duration::from_secs(5 * MIN),
    Duration::from_secs(30 * MIN),
    Duration::from_secs(2 * HOUR),
    Duration::from_secs(5 * HOUR),
    Duration::from_secs(10 * HOUR),
    Duration::from_secs(14 * HOUR),
];

/// The jitter added to each delay is at most this fraction of it.
pub const MAX_JITTER: f64 = 0.1;

/// A `Retry-After` never pushes the next attempt further than this.
pub const MAX_RETRY_AFTER: Duration = Duration::from_secs(24 * HOUR);

/// The delay after attempt `attempt` (1-based) failed, from `schedule` (one entry per
/// attempt, the first being attempt 1's): the next entry plus `jitter` (clamped to
/// 0..=[`MAX_JITTER`]) of it, raised to `retry_after` (capped at [`MAX_RETRY_AFTER`]).
/// `None` when `attempt` was the last.
pub fn next_delay_in(
    schedule: &[Duration],
    attempt: u32,
    retry_after: Option<Duration>,
    jitter: f64,
) -> Option<Duration> {
    let base = *schedule.get(attempt as usize)?;
    let j = if jitter.is_finite() {
        jitter.clamp(0.0, MAX_JITTER)
    } else {
        0.0
    };
    let delay = base + base.mul_f64(j);
    Some(match retry_after {
        Some(r) => delay.max(r.min(MAX_RETRY_AFTER)),
        None => delay,
    })
}

/// [`next_delay_in`] over [`DELAYS`].
pub fn next_delay(attempt: u32, retry_after: Option<Duration>, jitter: f64) -> Option<Duration> {
    next_delay_in(&DELAYS, attempt, retry_after, jitter)
}

/// A random jitter fraction in 0..[`MAX_JITTER`].
pub fn random_jitter() -> Result<f64, CoreError> {
    let bytes: [u8; 4] = random_bytes()?;
    Ok(f64::from(u32::from_le_bytes(bytes)) / f64::from(u32::MAX) * MAX_JITTER)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn schedule_and_jitter_bounds() {
        assert_eq!(next_delay(1, None, 0.0), Some(Duration::from_secs(5)));
        assert_eq!(next_delay(2, None, 0.0), Some(Duration::from_secs(300)));
        assert_eq!(
            next_delay(7, None, 0.0),
            Some(Duration::from_secs(14 * 3600))
        );
        assert_eq!(next_delay(8, None, 0.0), None);
        assert_eq!(next_delay(9, None, 0.0), None);
        assert_eq!(
            next_delay(3, None, 0.1),
            Some(Duration::from_secs(30 * 60 + 180))
        );
        // Jitter is clamped.
        assert_eq!(next_delay(1, None, 5.0), Some(Duration::from_millis(5500)));
        assert_eq!(next_delay(1, None, -1.0), Some(Duration::from_secs(5)));
        assert_eq!(next_delay(1, None, f64::NAN), Some(Duration::from_secs(5)));
        for _ in 0..100 {
            let j = random_jitter().unwrap();
            assert!((0.0..=MAX_JITTER).contains(&j), "{j}");
        }
        let total: Duration = DELAYS.iter().sum();
        assert_eq!(total, Duration::from_secs(31 * 3600 + 35 * 60 + 5));
    }

    #[test]
    fn retry_after_raises_but_caps_24h() {
        assert_eq!(
            next_delay(1, Some(Duration::from_secs(120)), 0.0),
            Some(Duration::from_secs(120))
        );
        // Shorter than the schedule: the schedule wins.
        assert_eq!(
            next_delay(2, Some(Duration::from_secs(1)), 0.0),
            Some(Duration::from_secs(300))
        );
        assert_eq!(
            next_delay(1, Some(Duration::from_secs(7 * 24 * 3600)), 0.0),
            Some(MAX_RETRY_AFTER)
        );
        assert_eq!(next_delay(8, Some(Duration::from_secs(5)), 0.0), None);
    }
}
