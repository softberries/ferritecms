//! The wall clock and UTC calendar dates, without a date library.
//!
//! `wasi:clocks/wall-clock` is linked for every plugin, so [`unix_now`] works without a
//! capability. The date conversions are Howard Hinnant's `days_from_civil` and
//! `civil_from_days` over the proleptic Gregorian calendar.

/// Seconds since 1970-01-01T00:00:00Z by the wall clock; 0 for a clock before 1970.
pub fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Days since 1970-01-01 of the date `year-month-day` (negative before it). `month` is
/// 1..=12 and `day` 1..=31; a day past the month's end rolls into the next month (31 April
/// is 1 May), so check a parsed date by converting it back with [`civil_from_days`].
pub fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let yoe = year - era * 400;
    let month = i64::from(month);
    let doy = (153 * ((month + 9) % 12) + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The date `(year, month, day)` that is `days` after 1970-01-01.
pub fn civil_from_days(days: i64) -> (i64, u32, u32) {
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

#[cfg(test)]
mod tests {
    use super::{civil_from_days, days_from_civil};

    #[test]
    fn known_dates() {
        for (days, date) in [
            (0, (1970, 1, 1)),
            (-1, (1969, 12, 31)),
            (11_016, (2000, 2, 29)),
            (19_782, (2024, 2, 29)),
            (47_482, (2100, 1, 1)),
            (2_932_896, (9999, 12, 31)),
        ] {
            assert_eq!(civil_from_days(days), date, "{days}");
            assert_eq!(days_from_civil(date.0, date.1, date.2), days, "{date:?}");
        }
    }

    #[test]
    fn every_day_of_four_centuries_round_trips() {
        let start = days_from_civil(1900, 1, 1);
        let mut previous = civil_from_days(start - 1);
        for days in start..start + 146_097 {
            let date = civil_from_days(days);
            assert_eq!(days_from_civil(date.0, date.1, date.2), days);
            assert!(date > previous, "{date:?} after {previous:?}");
            previous = date;
        }
    }

    #[test]
    fn a_day_past_the_end_of_a_month_rolls_over() {
        assert_eq!(civil_from_days(days_from_civil(2026, 4, 31)), (2026, 5, 1));
        assert_eq!(civil_from_days(days_from_civil(2027, 2, 29)), (2027, 3, 1));
    }
}
