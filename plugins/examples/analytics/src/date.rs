//! UTC calendar days, from event timestamps and storage keys, without a date library.
//!
//! The plugin has no clock: "today" is the newest event day it has seen, so the retention
//! cutoff and the stats window depend on the events alone.

/// A calendar day, counted from 1970-01-01. Ordering is chronological.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Day(i64);

impl Day {
    /// The day `year-month-day`, or `None` when no such date exists (30 February, month 13).
    fn from_ymd(year: i64, month: u32, day: u32) -> Option<Self> {
        if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
            return None;
        }
        let found = Self(days_from_civil(year, month, day));
        // 31 April rolls over into May: the round trip tells.
        (found.ymd() == (year, month, day)).then_some(found)
    }

    /// `YYYY-MM-DD`, the form of the storage keys.
    pub fn parse_key(s: &str) -> Option<Self> {
        Self::parse_date(s.as_bytes())
    }

    /// The day of an RFC 3339 UTC timestamp such as `2026-10-30T12:34:56.123456Z`. Any other
    /// shape (an offset, a missing `Z`, an impossible date or time) is `None`.
    pub fn from_ts(ts: &str) -> Option<Self> {
        let b = ts.as_bytes();
        if b.len() < 20 || b[10] != b'T' || b[13] != b':' || b[16] != b':' {
            return None;
        }
        let hour = number(&b[11..13])?;
        let minute = number(&b[14..16])?;
        let second = number(&b[17..19])?;
        if hour > 23 || minute > 59 || second > 60 {
            return None;
        }
        let mut rest = &b[19..];
        if let Some(fraction) = rest.strip_prefix(b".") {
            let digits = fraction.iter().take_while(|c| c.is_ascii_digit()).count();
            if digits == 0 {
                return None;
            }
            rest = &fraction[digits..];
        }
        if rest != b"Z" {
            return None;
        }
        Self::parse_date(&b[..10])
    }

    fn parse_date(b: &[u8]) -> Option<Self> {
        if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
            return None;
        }
        Self::from_ymd(
            i64::from(number(&b[..4])?),
            number(&b[5..7])?,
            number(&b[8..])?,
        )
    }

    /// `YYYY-MM-DD`.
    pub fn key(self) -> String {
        let (y, m, d) = self.ymd();
        format!("{y:04}-{m:02}-{d:02}")
    }

    /// The day `n` days earlier.
    pub fn before(self, n: u32) -> Self {
        Self(self.0 - i64::from(n))
    }

    fn ymd(self) -> (i64, u32, u32) {
        civil_from_days(self.0)
    }
}

/// An unsigned number from exactly these ASCII digits.
fn number(digits: &[u8]) -> Option<u32> {
    digits.iter().try_fold(0u32, |n, c| {
        c.is_ascii_digit().then(|| n * 10 + u32::from(c - b'0'))
    })
}

/// Days since 1970-01-01 of a proleptic Gregorian date (Hinnant's `days_from_civil`).
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let yoe = year - era * 400;
    let month = i64::from(month);
    let doy = (153 * ((month + 9) % 12) + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The date `days` after 1970-01-01 (Hinnant's `civil_from_days`).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    // Both are in 1..=31 by construction.
    (
        yoe + era * 400 + i64::from(month <= 2),
        month as u32,
        day as u32,
    )
}
