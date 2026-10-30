//! The wall clock as an RFC 3339 UTC time, without a date library.

/// Now, to the second: `2026-10-30T12:34:56Z`. `wasi:clocks/wall-clock` is linked for every
/// plugin; a clock before 1970 reads as 1970-01-01.
pub fn now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    format_unix(secs)
}

fn format_unix(secs: u64) -> String {
    let (y, m, d) = civil_from_days(i64::try_from(secs / 86_400).unwrap_or(0));
    let rem = secs % 86_400;
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3_600,
        rem % 3_600 / 60,
        rem % 60
    )
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

#[cfg(test)]
mod tests {
    use super::format_unix;

    #[test]
    fn known_instants() {
        assert_eq!(format_unix(0), "1970-01-01T00:00:00Z");
        assert_eq!(format_unix(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(format_unix(1_709_251_199), "2024-02-29T23:59:59Z");
        assert_eq!(format_unix(4_102_444_800), "2100-01-01T00:00:00Z");
    }
}
