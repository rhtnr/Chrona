//! Shared parabolic-vertex interpolation core.
//!
//! Four sites in this crate each independently computed the same
//! three-point parabolic vertex offset before this module existed:
//! `cal.rs`'s quartz-tick refinement, `autocorr.rs`'s autocorrelation-peak
//! refinement, `fold.rs`'s circular fold-bin refinement, and
//! `events.rs`'s matched-filter correlation-peak refinement. Read side by
//! side, their formulas — denominator, degenerate-guard threshold, and
//! clamp bounds — were byte-for-byte identical:
//!
//! ```text
//! denom = a - 2·b + c
//! offset = if |denom| < 1e-20 { 0.0 } else { (0.5·(a-c)/denom).clamp(-0.5, 0.5) }
//! ```
//!
//! so [`parabolic3`] below unifies that formula, guard, and clamp into one
//! core (the interface's clamp-unification precondition — "only if the
//! copies agree" — holds). What differed, and stays differing in each call
//! site's own thin wrapper, is how the three input samples are located and
//! how the offset is combined back with the vertex index:
//!
//! - **`cal.rs::calibrate_quartz`** extracts its triple by direct linear
//!   indexing at the call site itself (`env[pk-1], env[pk], env[pk+1]`) and
//!   calls the core straight from there — no separate wrapper function is
//!   needed, since indexing already lived outside the pre-unification
//!   `parabolic3`. The caller's own search bounds (`lo = (center-gate).max(1)`,
//!   `hi = (center+gate).min(env.len()-2)`) keep `pk` interior. The offset
//!   is added to `pk as f64` by the caller; the core never sees the index.
//! - **`events.rs::correlate_in_gate`** follows the exact same shape as
//!   `cal.rs`: it extracts its triple at the call site
//!   (`corr[pi-1].abs(), corr[pi].abs(), corr[pi+1].abs()`, as `f64`) and
//!   calls the core directly, with no wrapper function. Its interior-index
//!   guard is an explicit `if pi == 0 || pi + 1 == corr.len() { 0.0 } else
//!   { .. }` at the call site rather than a search-bound invariant, but the
//!   effect — the core only ever sees an interior triple — is the same. The
//!   offset is added to `lo as f64 + pi as f64` by the caller.
//! - **`autocorr.rs::parabolic_offset(r, i)`** indexes its slice *linearly*
//!   inside the wrapper (`r[i-1], r[i], r[i+1]`) — this relies on the
//!   caller keeping `i` interior, which `find_peak_in_band` guarantees via
//!   its own search-band clamp (`lo = min_lag.max(1)`,
//!   `hi = max_lag.min(r.len()-2)`). Like `cal.rs`, it returns the bare
//!   offset; callers add `i as f64` themselves.
//! - **`fold.rs::circular_parabolic(bins, i)`** indexes its slice
//!   *circularly* (`bins[(i+n-1)%n], bins[i], bins[(i+1)%n]`), so it is safe
//!   for every `i` in `0..n` including the array ends — the fold's bin
//!   array is a phase circle, not a line. Its return contract also differs
//!   from the other three: instead of a bare offset, it adds the vertex
//!   index itself and wraps the sum into `[0, n)` via `.rem_euclid`,
//!   returning an absolute (wrapped) bin position rather than an offset for
//!   the caller to add.

/// Vertex offset, in `[-0.5, 0.5]`, of the parabola through `(-1,a), (0,b),
/// (1,c)`. Degenerate (near-flat or non-concave) triples — `|denom| < 1e-20`
/// — return `0.0` rather than dividing by a near-zero denominator.
pub(crate) fn parabolic3(a: f64, b: f64, c: f64) -> f64 {
    let denom = a - 2.0 * b + c;
    if denom.abs() < 1e-20 {
        0.0
    } else {
        (0.5 * (a - c) / denom).clamp(-0.5, 0.5)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn symmetric_triple_has_zero_offset() {
        assert_eq!(parabolic3(0.0, 1.0, 0.0), 0.0);
    }

    #[test]
    fn matches_hand_computed_fractional_offset() {
        // 0.5·(0.5−0.9)/(0.5−2·1.0+0.9) = 0.5·(−0.4)/(−0.6) = (−0.2)/(−0.6) = 1/3 = 0.3(3).
        // Computed independently of the implementation (not by re-running its
        // formula): 1.0/3.0 is f64's nearest representable value to the exact
        // repeating decimal 0.3(3).
        let expected = 1.0 / 3.0;
        let got = parabolic3(0.5, 1.0, 0.9);
        assert!((got - expected).abs() < 1e-12, "got {got}, want {expected}");
    }

    #[test]
    fn antisymmetric_under_a_c_swap() {
        // a, b, c chosen as exact dyadic fractions (0.25 = 2^-2, 0.625 = 5·2^-3,
        // 0.5 = 2^-1) so every intermediate (subtraction, ×0.5, the one division
        // -0.125/-0.5 = 0.25) is exact in f64 — verified by hand, not estimated:
        // denom = 0.25 − 1.25 + 0.5 = −0.5 for both orderings (a+c is preserved
        // under the swap), numerator flips sign (0.5·(a−c) vs 0.5·(c−a)), so the
        // offset must flip sign exactly, with no rounding to blur the equality.
        let (a, b, c) = (0.25, 0.625, 0.5);
        assert_eq!(parabolic3(a, b, c), -parabolic3(c, b, a));
    }
}
