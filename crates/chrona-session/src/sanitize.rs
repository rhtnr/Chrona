//! Shared lift/ppm sanitization (M4 Task 5 debt: previously duplicated
//! between `chrona-app`'s `ui::controls` — for `ConfigStore`-loaded
//! values — and hand-rolled again in `chrona-app`'s `engine.rs` for a
//! replay sidecar's values). One copy here, consumed by both `chrona-app`
//! (re-exported from `ui::controls` so every existing call site keeps
//! compiling unchanged) and this crate's own [`crate::replay::replay`],
//! whose sidecar application previously hard-failed the whole replay on an
//! out-of-range value instead of clamping like the rest of the app.

/// Config/sidecar files are hand-editable (TOML has literal `inf`/`nan`; a
/// JSON sidecar can carry a huge-exponent numeral that overflows to
/// infinity on parse even though normal serialization never writes one).
/// Gate on finiteness FIRST — `f64::clamp` propagates a `NaN` input through
/// unchanged rather than saturating it, so checking after would let one
/// slip past — then clamp into `chrona_dsp::Analyzer::new`'s `[10, 90]`
/// lift-angle domain.
pub fn sanitize_lift(lift: f64) -> f64 {
    if lift.is_finite() {
        lift.clamp(10.0, 90.0)
    } else {
        52.0
    }
}

/// Same finiteness-first discipline as [`sanitize_lift`], clamped to the
/// ±500 ppm UI/sanity range. Unlike lift, `Analyzer::new` itself only
/// requires `ppm_correction` finite (no range check) — this range clamp is
/// sanity/UI consistency, not a build-safety requirement.
pub fn sanitize_ppm(ppm: f64) -> f64 {
    if ppm.is_finite() {
        ppm.clamp(-500.0, 500.0)
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_lift_clamps_finite_out_of_range_and_defaults_non_finite() {
        assert_eq!(sanitize_lift(500.0), 90.0);
        assert_eq!(sanitize_lift(5.0), 10.0);
        assert_eq!(sanitize_lift(f64::NAN), 52.0);
        assert_eq!(sanitize_lift(f64::INFINITY), 52.0);
        assert_eq!(sanitize_lift(52.0), 52.0);
    }

    #[test]
    fn sanitize_ppm_clamps_finite_out_of_range_and_defaults_non_finite() {
        assert_eq!(sanitize_ppm(f64::NAN), 0.0);
        assert_eq!(sanitize_ppm(f64::INFINITY), 0.0);
        assert_eq!(sanitize_ppm(9999.0), 500.0);
        assert_eq!(sanitize_ppm(-9999.0), -500.0);
        assert_eq!(sanitize_ppm(25.0), 25.0);
    }
}
