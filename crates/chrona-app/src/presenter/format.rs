//! Numeral and badge formatting: pure functions turning `MetricsSnapshot`
//! fields into display strings (rate, beat error, amplitude, tier badge,
//! rate-source label). No `egui` dependency (see the `presenter` module
//! docs).

use chrona_dsp::{AmplitudeGateFail, RateSource, Tier};
use jiff::{Timestamp, Unit, civil::Date, tz::TimeZone};

/// `"+12.3 s/d"`; an uncalibrated-clock warning is appended when
/// `calibrated` is false, and `None` (no rate to attribute) renders as an
/// em-dash.
pub fn format_rate(rate: Option<f64>, calibrated: bool) -> String {
    match rate {
        None => "—".to_string(),
        Some(r) => {
            let base = format!("{r:+.1} s/d");
            if calibrated {
                base
            } else {
                format!("{base} ⚠ uncal")
            }
        }
    }
}

/// `"0.8 ms"` (one decimal place); `None` (below Tier 2) renders as an
/// em-dash.
pub fn format_beat_error(be: Option<f64>) -> String {
    match be {
        None => "—".to_string(),
        Some(v) => format!("{v:.1} ms"),
    }
}

/// `"270°"` at Tier 3. Otherwise an em-dash annotated with why: the
/// specific gate that rejected the amplitude estimate when one fired, or
/// "need Tier 3" when no informative gate reason is available (not yet at
/// Tier 3, or too few unlock/drop intervals to judge).
pub fn format_amplitude(a: Option<f64>, gate: Option<AmplitudeGateFail>) -> String {
    match (a, gate) {
        (Some(deg), _) => format!("{deg:.0}°"),
        (None, Some(AmplitudeGateFail::TicTocDisagree)) => "— (tic/toc disagree)".to_string(),
        (None, Some(AmplitudeGateFail::OutOfRange)) => "— (out of range)".to_string(),
        (None, _) => "— (need Tier 3)".to_string(),
    }
}

/// `"system-clock cross-check: +3.2 ppm over 14 min"` (binding spec §3.4):
/// the toolbar cal popup's faint line, shown only when
/// `HealthView::clock_skew` is `Some`. `span_s` is reported coarsely, in
/// whole minutes (`(span_s / 60.0).round()`, away from zero on a tie,
/// `f64::round`'s documented behavior) — this is a rough cross-check, not a
/// precision figure. `ppm` always carries an explicit sign (matches
/// `format_rate`'s own `{:+.1}` convention above).
pub fn format_clock_skew(ppm: f64, span_s: f64) -> String {
    let min = (span_s / 60.0).round() as i64;
    format!("system-clock cross-check: {ppm:+.1} ppm over {min} min")
}

/// Header badge text for the current tier.
pub fn tier_label(t: Tier) -> &'static str {
    match t {
        Tier::T1 => "TIER 1 · rate",
        Tier::T2 => "TIER 2 · +beat error",
        Tier::T3 => "TIER 3 · full",
    }
}

/// Short label for which estimator produced `rate_s_per_day`. Mirrors
/// `MetricsSnapshot::rate_source`'s own `None` (no rate to attribute).
pub fn source_label(s: Option<RateSource>) -> Option<&'static str> {
    match s {
        Some(RateSource::UnlockingRegression) => Some("regression"),
        Some(RateSource::PeriodSlope) => Some("period"),
        None => None,
    }
}

/// GUI-only bph numeral formatting (design spec §3: "bph numerals grouped
/// with a thin space (`28 800`)") — visual only, unrelated to the frozen
/// headless `format_rate`/`format_beat_error`/`format_amplitude` numerals
/// above. Rounds to the nearest integer bph and groups thousands with a
/// THIN SPACE (U+2009).
pub fn format_bph_grouped(bph: f64) -> String {
    let n = bph.round() as i64;
    let digits = n.unsigned_abs().to_string();
    let len = digits.len();
    let mut grouped = String::with_capacity(len + len / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (len - i).is_multiple_of(3) {
            grouped.push('\u{2009}');
        }
        grouped.push(c);
    }
    if n < 0 {
        format!("-{grouped}")
    } else {
        grouped
    }
}

/// `"Today"` / `"Yesterday"` / `"YYYY-MM-DD"` for a session-history row
/// timestamp relative to now (design spec §7 — the history table's
/// newest-entry-day header). Both timestamps are bucketed into `tz`'s
/// LOCAL calendar days (`jiff::Zoned::date`, via `local_date` below) —
/// NOT raw 86_400-second UTC buckets — so "Today"/"Yesterday" flip at the
/// viewer's own local midnight, matching their wall clock (M4 Task 6; see
/// `ChronaApp::tz`'s doc comment for where `tz` comes from in production).
pub fn day_label(entry_unix: u64, now_unix: u64, tz: &TimeZone) -> String {
    let entry_date = local_date(entry_unix, tz);
    let now_date = local_date(now_unix, tz);
    let days = now_date
        .since((Unit::Day, entry_date))
        .expect("civil Date::since(Unit::Day) never fails for two in-range dates")
        .get_days();
    match days {
        0 => "Today".to_string(),
        1 => "Yesterday".to_string(),
        _ => format!(
            "{:04}-{:02}-{:02}",
            entry_date.year(),
            entry_date.month(),
            entry_date.day()
        ),
    }
}

/// `unix_s` converted to `tz`'s local calendar date — the shared
/// conversion `day_label`'s two timestamps both go through, so they
/// bucket by the exact same local-midnight boundary.
fn local_date(unix_s: u64, tz: &TimeZone) -> Date {
    Timestamp::from_second(unix_s as i64)
        .expect(
            "session/now timestamps are real wall-clock values, always within \
             jiff's representable ±9999-year range",
        )
        .to_zoned(tz.clone())
        .date()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formatting_strings() {
        assert_eq!(format_rate(Some(12.34), true), "+12.3 s/d");
        assert_eq!(format_rate(Some(-3.0), false), "-3.0 s/d ⚠ uncal");
        assert_eq!(format_rate(None, true), "—");
        assert_eq!(format_beat_error(Some(0.84)), "0.8 ms");
        assert_eq!(format_beat_error(None), "—");
        assert_eq!(format_amplitude(Some(270.4), None), "270°");
        assert_eq!(
            format_amplitude(None, Some(AmplitudeGateFail::TicTocDisagree)),
            "— (tic/toc disagree)"
        );
        assert_eq!(format_amplitude(None, None), "— (need Tier 3)");
        assert_eq!(tier_label(Tier::T3), "TIER 3 · full");
        assert_eq!(
            source_label(Some(RateSource::UnlockingRegression)),
            Some("regression")
        );
        assert_eq!(source_label(None), None);
    }

    #[test]
    fn clock_skew_line_formatting() {
        // M4 Task 7 (binding spec §3.4): the toolbar cal popup's faint
        // system-clock cross-check line. `min` is `span_s / 60`, rounded to
        // the nearest whole minute (a rough cross-check reports coarsely on
        // purpose). Positive ppm carries an explicit `+` (matches
        // `format_rate`'s own `{:+.1}` convention above).
        assert_eq!(
            format_clock_skew(3.2, 840.0), // 840s = 14min exactly
            "system-clock cross-check: +3.2 ppm over 14 min"
        );
        assert_eq!(
            format_clock_skew(-7.26, 900.0), // rounds to one decimal: -7.3
            "system-clock cross-check: -7.3 ppm over 15 min"
        );
        assert_eq!(
            format_clock_skew(0.0, 125.0), // 125/60 = 2.08... -> rounds to 2
            "system-clock cross-check: +0.0 ppm over 2 min"
        );
        assert_eq!(
            format_clock_skew(1.0, 150.0), // 150/60 = 2.5 -> rounds to 3 (half-up)
            "system-clock cross-check: +1.0 ppm over 3 min"
        );
    }

    #[test]
    fn bph_grouping_and_day_label() {
        assert_eq!(format_bph_grouped(28_800.0), "28\u{2009}800");
        assert_eq!(format_bph_grouped(9_000.0), "9\u{2009}000");
        assert_eq!(format_bph_grouped(108_000.0), "108\u{2009}000");
        let now = 1_700_000_000u64;
        assert_eq!(day_label(now, now, &TimeZone::UTC), "Today");
    }

    #[test]
    fn day_label_yesterday_and_older_dates() {
        // Re-pointed through an explicit UTC `TimeZone` (M4 Task 6): pins
        // the exact same literal continuity the old UTC-only
        // implementation had, just via the injectable-timezone API.
        assert_eq!(day_label(0, 0, &TimeZone::UTC), "Today");
        assert_eq!(day_label(0, 86_400, &TimeZone::UTC), "Yesterday");
        assert_eq!(day_label(0, 172_800, &TimeZone::UTC), "1970-01-01");
        // Known reference date (also pinned in chrona_session::civil's own
        // tests): 1_766_995_200 is 2025-12-29 UTC.
        assert_eq!(
            day_label(1_766_995_200, 1_766_995_200 + 5 * 86_400, &TimeZone::UTC),
            "2025-12-29"
        );
    }

    #[test]
    fn day_label_buckets_by_the_local_calendar_day_not_utc() {
        // entry = 2025-12-31T20:00:00Z = 1_767_211_200
        // now   = 2026-01-01T00:30:00Z = 1_767_227_400
        // (both verified with `date -j -u -f "%Y-%m-%dT%H:%M:%SZ" ... +%s`;
        // now - entry == 16_200s == 4h30m, i.e. 20:00 + 4:30 == 00:30 the
        // next UTC day, so the two literals are mutually consistent.)
        //
        // UTC-bucketed (the old implementation this replaces): entry's UTC
        // calendar day is Dec 31, now's is Jan 1 -> a 1-day gap ->
        // "Yesterday".
        //
        // Asia/Kolkata is a fixed UTC+05:30 offset (no DST since 1945):
        // entry's local time is 20:00 + 5:30 = 25:30 -> 01:30 IST on Jan 1;
        // now's local time is 00:30 + 5:30 = 06:00 IST, also Jan 1. Both
        // land on the SAME local calendar day, so the correct LOCAL answer
        // is "Today" — the exact UTC-vs-local flip this task fixes.
        let entry_unix = 1_767_211_200; // 2025-12-31T20:00:00Z
        let now_unix = 1_767_227_400; // 2026-01-01T00:30:00Z
        let tz = TimeZone::get("Asia/Kolkata").expect("Asia/Kolkata is a valid IANA zone");
        assert_eq!(day_label(entry_unix, now_unix, &tz), "Today");
    }
}
