//! Stage 7 (spec §3.1, §5.7): tier assignment from quality signals.

/// Signal-quality tier. `Analyzer::current()` returning `None` is Tier 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tier {
    /// Rate + BPH only (long-window periodicity).
    T1,
    /// + beat error (per-beat onsets separable).
    T2,
    /// + amplitude (intra-tick pulse structure resolvable).
    T3,
}

/// Spec §3.1 thresholds (initial values, tunable against the fixture corpus).
pub fn assign_tier(
    detection_ratio: f64,
    onset_jitter_ms: Option<f64>,
    unlocking_ratio: f64,
    amplitude_ok: bool,
) -> Tier {
    let t2 = detection_ratio >= 0.6 && onset_jitter_ms.is_some_and(|j| j <= 0.5);
    if t2 && unlocking_ratio >= 0.5 && amplitude_ok {
        Tier::T3
    } else if t2 {
        Tier::T2
    } else {
        Tier::T1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thresholds_match_spec() {
        assert_eq!(assign_tier(0.9, Some(0.1), 0.9, true), Tier::T3);
        assert_eq!(assign_tier(0.9, Some(0.1), 0.9, false), Tier::T2); // amp gated
        assert_eq!(assign_tier(0.9, Some(0.1), 0.4, true), Tier::T2); // unlocking thin
        assert_eq!(assign_tier(0.59, Some(0.1), 0.9, true), Tier::T1); // detection < 60 %
        assert_eq!(assign_tier(0.9, Some(0.6), 0.9, true), Tier::T1); // jitter > 0.5 ms
        assert_eq!(assign_tier(0.9, None, 0.9, true), Tier::T1); // no regression
    }
}
