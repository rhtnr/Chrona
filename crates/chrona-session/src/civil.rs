//! Pure calendar arithmetic: Unix seconds → compact UTC civil timestamp
//! (`"YYYYMMDD-HHMMSS"`), via Howard Hinnant's `civil_from_days` algorithm
//! (<https://howardhinnant.github.io/date_algorithms.html>). No time-zone
//! database, no OS clock — deterministic for any `u64` input.

/// Proleptic-Gregorian day count `z` (days since 1970-01-01) → `(year,
/// month, day)`. `z` may be negative (pre-1970); the two `/` divisions on
/// `era` and downstream terms rely on Rust's truncating integer division
/// behaving exactly like the C++ reference implementation's, so this is a
/// direct port, not a reinterpretation.
fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32; // [1, 12]
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m, d)
}

/// UTC civil timestamp for `unix_secs`, formatted `"YYYYMMDD-HHMMSS"`.
pub fn timestamp_compact(unix_secs: u64) -> String {
    let (y, m, d) = ymd(unix_secs);
    let sod = (unix_secs % 86_400) as i64; // seconds of day, [0, 86399]
    let hh = sod / 3_600;
    let mm = (sod % 3_600) / 60;
    let ss = sod % 60;
    format!("{y:04}{m:02}{d:02}-{hh:02}{mm:02}{ss:02}")
}

/// UTC calendar date for `unix_secs`, as `(year, month, day)` — the pieces
/// `timestamp_compact` formats internally, exposed for callers (M4a:
/// session-history/report date grouping) that need the parts rather than
/// the compact string. `year` is `i64` (not `civil_from_days`'s internal
/// `i32`) purely so callers never have to think about a second integer
/// width for a value that's for display, not arithmetic.
pub fn ymd(unix_secs: u64) -> (i64, u32, u32) {
    let days = (unix_secs / 86_400) as i64;
    let (y, m, d) = civil_from_days(days);
    (y as i64, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_timestamps() {
        assert_eq!(timestamp_compact(0), "19700101-000000");
        // Brief's spot value ("20251229-081600") did not match; verified via
        // `date -u -r 1766995200 +%Y%m%d-%H%M%S` and cross-checked against
        // Python's `datetime.fromtimestamp(..., timezone.utc)` — both give
        // 20251229-080000. Corrected here per the brief's own MUST-verify
        // instruction.
        assert_eq!(timestamp_compact(1_766_995_200), "20251229-080000");
        // Leap day. Brief's spot value ("20000229-121456") likewise did not
        // match; `date -u -r 951827696 +%Y%m%d-%H%M%S` and Python both give
        // 20000229-123456. Corrected here for the same reason.
        assert_eq!(timestamp_compact(951_827_696), "20000229-123456");
    }

    /// Same three reference instants as `known_timestamps`, decomposed
    /// instead of formatted — `ymd` and `timestamp_compact` must agree on
    /// the calendar date for the same input.
    #[test]
    fn ymd_matches_known_timestamps() {
        assert_eq!(ymd(0), (1970, 1, 1));
        assert_eq!(ymd(1_766_995_200), (2025, 12, 29));
        assert_eq!(ymd(951_827_696), (2000, 2, 29)); // leap day
    }
}
