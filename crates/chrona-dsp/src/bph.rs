//! Beat arithmetic and standard beat-rate tables (spec §2.2–§2.3).

/// Auto-detect BPH set (spec §2.2 — Weishi + tg union).
pub const AUTO_BPH: [u32; 11] = [
    12_000, 14_400, 17_280, 18_000, 19_800, 21_600, 25_200, 28_800, 36_000, 43_200, 72_000,
];

/// Beat period in seconds (spec §2.2): one tic or one toc.
pub fn t_beat_s(bph: u32) -> f64 {
    3600.0 / bph as f64
}

/// Full oscillation period in seconds (spec §2.2): tic + toc.
pub fn t_osc_s(bph: u32) -> f64 {
    7200.0 / bph as f64
}

/// Detected beats-per-hour from a measured full-oscillation period.
pub fn bph_from_t_osc(t_osc_s: f64) -> f64 {
    7200.0 / t_osc_s
}

/// Snap a detected BPH to the auto-detect table iff within 1.5 % relative deviation.
pub fn snap_to_table(bph_detected: f64) -> Option<u32> {
    AUTO_BPH
        .iter()
        .copied()
        .map(|b| (b, ((bph_detected - b as f64) / b as f64).abs()))
        .filter(|(_, dev)| *dev < 0.015)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(b, _)| b)
}

/// Rate in s/day from measured full-oscillation period vs nominal (spec §2.3).
/// Positive = fast.
pub fn rate_s_per_day(t_osc_measured_s: f64, bph_nominal: u32) -> f64 {
    let t_nom = t_osc_s(bph_nominal);
    86_400.0 * (t_nom - t_osc_measured_s) / t_nom
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn beat_arithmetic_28800() {
        assert!((t_beat_s(28_800) - 0.125).abs() < 1e-12);
        assert!((t_osc_s(28_800) - 0.250).abs() < 1e-12);
        assert!((bph_from_t_osc(0.250) - 28_800.0).abs() < 1e-9);
    }

    #[test]
    fn snap_within_tolerance() {
        assert_eq!(snap_to_table(28_795.0), Some(28_800)); // 0.017 % off
        assert_eq!(snap_to_table(28_800.0 * 1.014), Some(28_800)); // 1.4 % off — inside
        assert_eq!(snap_to_table(28_800.0 * 1.016), None); // 1.6 % off — outside
        assert_eq!(snap_to_table(21_650.0), Some(21_600));
    }

    #[test]
    fn rate_sign_convention_positive_is_fast() {
        // Watch beating exactly nominal: 0 s/d.
        assert!((rate_s_per_day(0.250, 28_800)).abs() < 1e-9);
        // Faster watch → shorter measured period → positive rate.
        // T̂ = T_nom·(1 − 10/86400) is a watch gaining 10 s/d.
        let t_fast = 0.250 * (1.0 - 10.0 / 86_400.0);
        assert!((rate_s_per_day(t_fast, 28_800) - 10.0).abs() < 1e-6);
        let t_slow = 0.250 * (1.0 + 30.0 / 86_400.0);
        assert!((rate_s_per_day(t_slow, 28_800) + 30.0).abs() < 1e-6);
    }
}
