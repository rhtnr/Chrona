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
}
