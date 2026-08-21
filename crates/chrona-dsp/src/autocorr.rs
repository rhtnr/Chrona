//! FFT-based autocorrelation and peak location (spec §5.3).

use realfft::RealFftPlanner;

pub struct Peak {
    /// Fractional lag in samples (parabolically refined).
    pub lag: f64,
    pub value: f32,
}

/// Linear autocorrelation via FFT: zero-pad to ≥ 2n (next power of two) to avoid
/// circular wrap-around, forward FFT, |X|², inverse FFT, normalize by lag 0.
pub fn autocorrelate(x: &[f32]) -> Vec<f32> {
    let n = x.len();
    if n == 0 {
        return Vec::new();
    }
    let padded = (2 * n).next_power_of_two();
    let mut planner = RealFftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(padded);
    let ifft = planner.plan_fft_inverse(padded);

    let mut buf = vec![0.0f32; padded];
    buf[..n].copy_from_slice(x);
    let mut spectrum = fft.make_output_vec();
    // Invariant: buffer lengths come from the planner itself, so process cannot fail.
    fft.process(&mut buf, &mut spectrum)
        .expect("planner-sized buffers");
    for c in spectrum.iter_mut() {
        *c *= c.conj(); // |X|² (imaginary parts become 0)
    }
    let mut r = vec![0.0f32; padded];
    ifft.process(&mut spectrum, &mut r)
        .expect("planner-sized buffers");

    let r0 = r[0];
    if r0 <= f32::MIN_POSITIVE {
        return vec![0.0; n]; // silence in → flat autocorrelation, not NaN
    }
    r.truncate(n);
    r.iter().map(|v| v / r0).collect()
}

/// Parabolic interpolation around integer peak `i` → fractional lag offset in (−0.5, 0.5).
fn parabolic_offset(r: &[f32], i: usize) -> f64 {
    let (a, b, c) = (r[i - 1] as f64, r[i] as f64, r[i + 1] as f64);
    let denom = a - 2.0 * b + c;
    if denom.abs() < 1e-20 {
        0.0
    } else {
        (0.5 * (a - c) / denom).clamp(-0.5, 0.5)
    }
}

/// Below this normalized autocorrelation value, a "local max" is FFT round-off
/// noise, not signal: lag-0 is normalized to 1.0, and round-trip forward+inverse
/// FFT via realfft (which does not normalize by length) leaves float32 residue
/// on the order of 1e-8 at lags whose true autocorrelation is exactly zero —
/// five-plus orders of magnitude below any genuine peak.
const NOISE_FLOOR: f32 = 1e-6;

/// Highest interior local maximum in `[min_lag, max_lag]`, parabolic-refined.
pub fn find_peak_in_band(r: &[f32], min_lag: usize, max_lag: usize) -> Option<Peak> {
    let lo = min_lag.max(1);
    let hi = max_lag.min(r.len().saturating_sub(2));
    let mut best: Option<usize> = None;
    for i in lo..=hi {
        if r[i] > r[i - 1]
            && r[i] >= r[i + 1]
            && r[i] > NOISE_FLOOR
            && best.is_none_or(|b| r[i] > r[b])
        {
            best = Some(i);
        }
    }
    best.map(|i| Peak {
        lag: i as f64 + parabolic_offset(r, i),
        value: r[i],
    })
}

/// Peak restricted to `expected_lag·(1 ± tolerance)` (per-cycle refinement, spec §5.3).
pub fn refine_peak_near(r: &[f32], expected_lag: f64, tolerance: f64) -> Option<Peak> {
    let lo = (expected_lag * (1.0 - tolerance)).floor() as usize;
    let hi = (expected_lag * (1.0 + tolerance)).ceil() as usize;
    find_peak_in_band(r, lo, hi)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// O(n²) reference: linear autocorrelation r[k] = Σ x[n]·x[n+k].
    fn naive_autocorr(x: &[f32]) -> Vec<f32> {
        let n = x.len();
        let mut r = vec![0.0f32; n];
        for k in 0..n {
            let mut acc = 0.0f64;
            for i in 0..n - k {
                acc += x[i] as f64 * x[i + k] as f64;
            }
            r[k] = acc as f32;
        }
        let r0 = r[0].max(f32::MIN_POSITIVE);
        r.iter().map(|v| v / r0).collect()
    }

    #[test]
    fn matches_naive_reference() {
        let mut rng = crate::synth::Rng::new(7);
        let x: Vec<f32> = (0..1024).map(|_| rng.next_f32()).collect();
        let fast = autocorrelate(&x);
        let slow = naive_autocorr(&x);
        assert_eq!(fast.len(), 1024);
        for (i, (a, b)) in fast.iter().zip(slow.iter()).enumerate() {
            assert!((a - b).abs() < 1e-4, "lag {i}: {a} vs {b}");
        }
    }

    #[test]
    fn impulse_train_peaks_at_its_period() {
        // Period-97 impulse train → autocorr local max at lag 97.
        let mut x = vec![0.0f32; 4096];
        for i in (0..4096).step_by(97) {
            x[i] = 1.0;
        }
        let r = autocorrelate(&x);
        let p = find_peak_in_band(&r, 50, 150).unwrap();
        assert!((p.lag - 97.0).abs() < 0.5, "lag {}", p.lag);
    }

    #[test]
    fn parabolic_refinement_finds_fractional_lag() {
        // A slightly-detuned sinusoid has a fractional-lag autocorr peak.
        let period = 100.25f64;
        let x: Vec<f32> = (0..8192)
            .map(|i| (2.0 * std::f64::consts::PI * i as f64 / period).cos() as f32)
            .collect();
        let r = autocorrelate(&x);
        let p = find_peak_in_band(&r, 80, 120).unwrap();
        assert!((p.lag - period).abs() < 0.1, "lag {}", p.lag);
    }

    #[test]
    fn refine_near_expected() {
        let mut x = vec![0.0f32; 4096];
        for i in (0..4096).step_by(97) {
            x[i] = 1.0;
        }
        let r = autocorrelate(&x);
        let p = refine_peak_near(&r, 95.0, 0.05).unwrap(); // 95·(1±5 %) covers 97
        assert!((p.lag - 97.0).abs() < 0.5);
        assert!(refine_peak_near(&r, 60.0, 0.02).is_none()); // no peak near 60
    }

    #[test]
    fn zero_input_is_flat_not_nan() {
        let r = autocorrelate(&[0.0; 512]);
        assert!(r.iter().all(|v| v.is_finite()));
        assert!(find_peak_in_band(&r, 10, 100).is_none());
    }
}
