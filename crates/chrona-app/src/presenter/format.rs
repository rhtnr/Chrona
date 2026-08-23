//! Numeral and badge formatting: pure functions turning `MetricsSnapshot`
//! fields into display strings (rate, beat error, amplitude, tier badge,
//! rate-source label). No `egui` dependency (see the `presenter` module
//! docs).

use chrona_dsp::{AmplitudeGateFail, RateSource, Tier};

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

/// Local re-implementation of `chrona_session::civil`'s day math (Howard
/// Hinnant's `civil_from_days`, <https://howardhinnant.github.io/date_algorithms.html>)
/// — kept dependency-free rather than reaching across the crate boundary
/// for a private helper (`day_label` only ever needs the read side).
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

/// `"Today"` / `"Yesterday"` / `"YYYY-MM-DD"` for a session-history row
/// timestamp relative to now (design spec §7 — the history table's
/// newest-entry-day header). Both timestamps are bucketed into UTC
/// calendar days (`unix_secs / 86_400`); a viewer west of UTC can see an
/// entry from a few hours ago labeled "Yesterday" a little early relative
/// to their own clock — an accepted, GUI-only approximation (spec
/// §10/§14), not used for anything honesty-sensitive.
pub fn day_label(entry_unix: u64, now_unix: u64) -> String {
    let entry_days = (entry_unix / 86_400) as i64;
    let now_days = (now_unix / 86_400) as i64;
    match now_days - entry_days {
        0 => "Today".to_string(),
        1 => "Yesterday".to_string(),
        _ => {
            let (y, m, d) = civil_from_days(entry_days);
            format!("{y:04}-{m:02}-{d:02}")
        }
    }
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
    fn bph_grouping_and_day_label() {
        assert_eq!(format_bph_grouped(28_800.0), "28\u{2009}800");
        assert_eq!(format_bph_grouped(9_000.0), "9\u{2009}000");
        assert_eq!(format_bph_grouped(108_000.0), "108\u{2009}000");
        let now = 1_700_000_000u64;
        assert_eq!(day_label(now, now), "Today");
    }

    #[test]
    fn day_label_yesterday_and_older_dates() {
        assert_eq!(day_label(0, 0), "Today");
        assert_eq!(day_label(0, 86_400), "Yesterday");
        assert_eq!(day_label(0, 172_800), "1970-01-01");
        // Known reference date (also pinned in chrona_session::civil's own
        // tests): 1_766_995_200 is 2025-12-29 UTC.
        assert_eq!(
            day_label(1_766_995_200, 1_766_995_200 + 5 * 86_400),
            "2025-12-29"
        );
    }
}
