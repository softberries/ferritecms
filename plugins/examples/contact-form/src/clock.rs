//! The wall clock as an RFC 3339 UTC time, without a date library.

use ferrite_sdk::time;

/// Now, to the second: `2026-10-30T12:34:56Z`. `wasi:clocks/wall-clock` is linked for every
/// plugin; a clock before 1970 reads as 1970-01-01.
pub fn now() -> String {
    format_unix(time::unix_now())
}

fn format_unix(secs: u64) -> String {
    let (y, m, d) = time::civil_from_days(i64::try_from(secs / 86_400).unwrap_or(0));
    let rem = secs % 86_400;
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3_600,
        rem % 3_600 / 60,
        rem % 60
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
