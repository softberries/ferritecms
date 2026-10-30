//! UTC calendar days, from event timestamps and storage keys, on the SDK's calendar
//! arithmetic ([`ferrite_sdk::time`]).
//!
//! "Today" is the wall clock's UTC day ([`Day::today`]).

use ferrite_sdk::time::{civil_from_days, days_from_civil, unix_now};

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

    /// `YYYY-MM`, the month of the day: a key prefix that lists one month of days.
    pub fn month(self) -> String {
        let (y, m, _) = self.ymd();
        format!("{y:04}-{m:02}")
    }

    /// The current UTC day, from the wall clock.
    pub fn today() -> Self {
        Self::from_unix_secs(unix_now())
    }

    fn from_unix_secs(secs: u64) -> Self {
        Self(i64::try_from(secs / 86_400).unwrap_or(0))
    }

    /// The day `n` days later.
    pub fn after(self, n: u32) -> Self {
        Self(self.0 + i64::from(n))
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

#[cfg(test)]
mod tests {
    use super::Day;

    fn day(key: &str) -> Day {
        Day::parse_key(key).unwrap()
    }

    #[test]
    fn keys_round_trip_and_order() {
        for key in ["1970-01-01", "2026-10-30", "2028-02-29", "9999-12-31"] {
            assert_eq!(day(key).key(), key);
        }
        assert!(day("2026-10-29") < day("2026-10-30"));
        assert!(day("2026-12-31") < day("2027-01-01"));
    }

    #[test]
    fn counting_back_crosses_months_years_and_leap_days() {
        assert_eq!(day("2027-01-01").before(1).key(), "2026-12-31");
        assert_eq!(day("2026-03-01").before(1).key(), "2026-02-28");
        assert_eq!(day("2028-03-01").before(1).key(), "2028-02-29");
        assert_eq!(day("2100-03-01").before(1).key(), "2100-02-28");
        assert_eq!(day("2000-03-01").before(1).key(), "2000-02-29");
        assert_eq!(day("2026-10-30").before(365).key(), "2025-10-30");
        assert_eq!(day("2028-02-28").after(2).key(), "2028-03-01");
        assert_eq!(day("2026-12-31").after(1).key(), "2027-01-01");
    }

    #[test]
    fn impossible_dates_are_refused() {
        for key in [
            "2027-02-29",
            "2100-02-29",
            "2026-04-31",
            "2026-13-01",
            "2026-00-10",
            "2026-01-00",
            "2026-1-01",
            "20261001",
            "",
        ] {
            assert_eq!(Day::parse_key(key), None, "{key}");
        }
        assert!(Day::parse_key("2000-02-29").is_some());
    }

    #[test]
    fn timestamps_are_rfc3339_utc_only() {
        let ok = |ts: &str| Day::from_ts(ts).map(Day::key);
        assert_eq!(ok("2026-10-30T12:34:56Z").as_deref(), Some("2026-10-30"));
        assert_eq!(
            ok("2026-10-30T23:59:59.999999Z").as_deref(),
            Some("2026-10-30")
        );
        assert_eq!(
            ok("2026-10-30T00:00:00.123456789Z").as_deref(),
            Some("2026-10-30")
        );
        for ts in [
            "",
            "garbage",
            "2026-10-30",
            "2026-10-30T12:34:56",
            "2026-10-30T12:34:56+02:00",
            "2026-10-30T12:34:56.Z",
            "2026-10-30T24:00:00Z",
            "2026-10-30T12:60:00Z",
            "2026-10-30 12:34:56Z",
            "2026-10-30T12:34:56ZZ",
            "2026-02-30T12:34:56Z",
        ] {
            assert_eq!(ok(ts), None, "{ts}");
        }
    }

    #[test]
    fn months_are_key_prefixes() {
        assert_eq!(day("2026-10-30").month(), "2026-10");
        assert_eq!(day("2027-01-01").month(), "2027-01");
        assert!(
            day("2026-03-09")
                .key()
                .starts_with(&day("2026-03-09").month())
        );
    }

    #[test]
    fn unix_seconds_give_the_utc_day() {
        assert_eq!(Day::from_unix_secs(0).key(), "1970-01-01");
        assert_eq!(Day::from_unix_secs(86_399).key(), "1970-01-01");
        assert_eq!(Day::from_unix_secs(86_400).key(), "1970-01-02");
        assert_eq!(Day::from_unix_secs(1_793_318_400).key(), "2026-10-30");
    }
}
