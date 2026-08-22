//! Stage 4 (spec §5.4): fold the envelope at the oscillation period with
//! trimmed-mean stacking; the two folded maxima are the tic and toc
//! drop-pulse anchors.

/// Number of phase bins in a fold profile.
pub const NBINS: usize = 512;

#[derive(Debug, Clone)]
pub struct FoldProfile {
    /// Trimmed-mean envelope value per phase bin.
    pub bins: Vec<f32>,
    /// The folding period, in envelope samples.
    pub t_osc_env: f64,
    /// Anchor phases in bin units, [0, NBINS). A is the global max; B the
    /// opposite-half max (the other beat's drop pulse).
    pub anchor_a_phase: f64,
    pub anchor_b_phase: f64,
    /// Anchor-A bin value over the profile median — a fold-quality signal.
    pub contrast: f32,
}

impl FoldProfile {
    /// Anchor phases as envelope-sample offsets within one cycle.
    pub fn anchor_env_offsets(&self) -> (f64, f64) {
        let scale = self.t_osc_env / NBINS as f64;
        (self.anchor_a_phase * scale, self.anchor_b_phase * scale)
    }
}

/// Parabolic refinement on a circular bin array around index `i`.
fn circular_parabolic(bins: &[f32], i: usize) -> f64 {
    let n = bins.len();
    let (a, b, c) = (
        bins[(i + n - 1) % n] as f64,
        bins[i] as f64,
        bins[(i + 1) % n] as f64,
    );
    let denom = a - 2.0 * b + c;
    let off = if denom.abs() < 1e-20 {
        0.0
    } else {
        (0.5 * (a - c) / denom).clamp(-0.5, 0.5)
    };
    (i as f64 + off).rem_euclid(n as f64)
}

/// Fold `env` at `t_osc_env` with per-bin trimmed-mean stacking (spec §5.4:
/// drop the top quintile of contributing cycles per bin).
pub fn fold_envelope(env: &[f32], t_osc_env: f64) -> Option<FoldProfile> {
    if !(t_osc_env.is_finite() && t_osc_env > 1.0) {
        return None;
    }
    let cycles = (env.len() as f64 / t_osc_env).floor() as usize;
    if cycles < 8 {
        return None;
    }
    let usable = (cycles as f64 * t_osc_env) as usize;
    // Bucket every sample of the freshest `usable` window by phase.
    let start = env.len() - usable;
    let mut buckets: Vec<Vec<f32>> = (0..NBINS).map(|_| Vec::with_capacity(cycles + 1)).collect();
    for (j, &v) in env[start..].iter().enumerate() {
        let phase = (j as f64).rem_euclid(t_osc_env) / t_osc_env;
        let bin = ((phase * NBINS as f64) as usize).min(NBINS - 1);
        buckets[bin].push(v);
    }
    let mut bins = vec![0.0f32; NBINS];
    for (bin, bucket) in bins.iter_mut().zip(buckets.iter_mut()) {
        if bucket.is_empty() {
            continue;
        }
        bucket.sort_by(f32::total_cmp);
        // Trimmed mean: drop the top quintile (impulse-noise robustness).
        let keep = (bucket.len() * 4).div_ceil(5).max(1);
        *bin = bucket[..keep].iter().sum::<f32>() / keep as f32;
    }

    // Anchor A: global max, parabolic-refined on the circle.
    let a_idx = bins
        .iter()
        .enumerate()
        .max_by(|x, y| x.1.total_cmp(y.1))
        .map(|(i, _)| i)?;
    let anchor_a_phase = circular_parabolic(&bins, a_idx);
    // Anchor B: max within the circular window centered half a cycle away,
    // width NBINS/4 (tolerates beat-error shifts up to ±T_osc/8).
    let center = (a_idx + NBINS / 2) % NBINS;
    let half_w = NBINS / 8;
    let b_idx = (0..=2 * half_w)
        .map(|k| (center + NBINS - half_w + k) % NBINS)
        .max_by(|&i, &j| bins[i].total_cmp(&bins[j]))?;
    let anchor_b_phase = circular_parabolic(&bins, b_idx);

    let mut sorted = bins.clone();
    sorted.sort_by(f32::total_cmp);
    let median = sorted[NBINS / 2].max(1e-12);
    let contrast = bins[a_idx] / median;

    Some(FoldProfile {
        bins,
        t_osc_env,
        anchor_a_phase,
        anchor_b_phase,
        contrast,
    })
}

/// Count clusters of bins above `median + 0.5·(max − median)`, treating the
/// bin array as circular; returns cluster count and the circular gaps (in bins)
/// between consecutive cluster centroids.
fn significant_clusters(bins: &[f32]) -> (usize, Vec<f64>) {
    let n = bins.len();
    let mut sorted = bins.to_vec();
    sorted.sort_by(f32::total_cmp);
    let median = sorted[n / 2];
    let max = sorted[n - 1];
    let thr = median + 0.5 * (max - median);
    let above: Vec<bool> = bins.iter().map(|&v| v > thr).collect();
    if above.iter().all(|&b| b) || above.iter().all(|&b| !b) {
        return (0, Vec::new());
    }
    // Walk the circle once, starting just after a below-threshold bin.
    let start = (0..n).find(|&i| !above[i]).unwrap_or(0);
    let mut centroids = Vec::new();
    let mut run: Vec<usize> = Vec::new();
    for k in 1..=n {
        let i = (start + k) % n;
        if above[i] {
            run.push(k); // unwrapped position to keep centroids monotonic
        } else if !run.is_empty() {
            let c = run.iter().sum::<usize>() as f64 / run.len() as f64;
            centroids.push(c);
            run.clear();
        }
    }
    let count = centroids.len();
    let mut gaps = Vec::new();
    if count >= 2 {
        for w in centroids.windows(2) {
            gaps.push(w[1] - w[0]);
        }
        gaps.push(n as f64 - (centroids[count - 1] - centroids[0])); // wrap gap
    }
    (count, gaps)
}

/// Absolute-bin centroids of clusters above `median + factor·(max −
/// median)`, treating the bin array as circular (same threshold/run logic
/// as `significant_clusters`, but exposing positions instead of gaps —
/// needed to locate the quarter-cycle midpoints between two dominant
/// clusters, see `fold_with_octave_guard`'s secondary-peak probe).
fn cluster_centroids(bins: &[f32], factor: f64) -> Vec<f64> {
    let n = bins.len();
    let mut sorted = bins.to_vec();
    sorted.sort_by(f32::total_cmp);
    let median = sorted[n / 2];
    let max = sorted[n - 1];
    let thr = median + (factor as f32) * (max - median);
    let above: Vec<bool> = bins.iter().map(|&v| v > thr).collect();
    if above.iter().all(|&b| b) || above.iter().all(|&b| !b) {
        return Vec::new();
    }
    let start = (0..n).find(|&i| !above[i]).unwrap_or(0);
    let mut centroids = Vec::new();
    let mut run: Vec<usize> = Vec::new();
    for k in 1..=n {
        let i = (start + k) % n;
        if above[i] {
            run.push(k);
        } else if !run.is_empty() {
            let c = run.iter().sum::<usize>() as f64 / run.len() as f64;
            centroids.push((start as f64 + c).rem_euclid(n as f64));
            run.clear();
        }
    }
    centroids
}

/// Fold with period-doubling detection (retires M1's octave-error limitation).
pub fn fold_with_octave_guard(env: &[f32], t_osc_env: f64) -> Option<(FoldProfile, bool)> {
    let full = fold_envelope(env, t_osc_env)?;
    let (count, gaps) = significant_clusters(&full.bins);
    let quarter = NBINS as f64 / 4.0;
    let four_even = count == 4 && gaps.iter().all(|g| (g - quarter).abs() < quarter / 4.0);

    // A profile folded at 2·T_osc_true collapses, at the 0.5 threshold, to
    // exactly two dominant antipodal clusters (the two strong tic peaks):
    // measured evidence (task-5-report.md) shows the two attenuated toc
    // peaks a quarter-cycle off each one can be *shorter* than the strong
    // peaks' own unlock/impulse precursor bursts (spec §2.1), so no single
    // global threshold recovers all four clusters evenly — lowering the
    // factor either leaves the toc peaks undetected or also splits off the
    // precursor bursts as spurious extra clusters. When the profile has
    // that two-dominant-antipodal shape, probe narrow windows centered on
    // the two quarter-cycle midpoints instead — far enough from each
    // dominant peak's own short precursor skirt not to re-detect it — for
    // a secondary bump clearly above the profile's noise floor.
    let four_even = four_even || {
        count == 2
            && gaps.len() == 2
            && gaps
                .iter()
                .all(|g| (g - 2.0 * quarter).abs() < quarter / 2.0)
            && {
                let centroids = cluster_centroids(&full.bins, 0.5);
                let mut sorted = full.bins.clone();
                sorted.sort_by(f32::total_cmp);
                let floor = sorted[NBINS / 2] as f64;
                let probe = (NBINS / 16) as i64;
                centroids.len() == 2
                    && centroids.iter().all(|&c| {
                        let mid = (c + quarter).rem_euclid(NBINS as f64).round() as i64;
                        let window_max = (-probe..=probe)
                            .map(|d| full.bins[(mid + d).rem_euclid(NBINS as i64) as usize] as f64)
                            .fold(0.0, f64::max);
                        window_max > 1.8 * floor
                    })
            }
    };

    if four_even
        && let Some(half) = fold_envelope(env, t_osc_env / 2.0)
        && half.contrast >= full.contrast
    {
        return Some((half, true));
    }
    Some((full, false))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::EnvelopeExtractor;
    use crate::synth::{SynthConfig, synthesize};

    fn envelope_of(cfg: &SynthConfig) -> (Vec<f32>, f64) {
        let x = synthesize(cfg).expect("valid synth config");
        let mut env = EnvelopeExtractor::new(cfg.sample_rate_hz).unwrap();
        let mut out = Vec::new();
        env.process(&x, &mut out);
        (out, env.envelope_rate_hz())
    }

    fn circular_dist(a: f64, b: f64, n: f64) -> f64 {
        let d = (a - b).rem_euclid(n);
        d.min(n - d)
    }

    #[test]
    fn anchors_sit_half_a_cycle_apart() {
        let cfg = SynthConfig {
            snr_db: 30.0,
            ..SynthConfig::default()
        };
        let (env, env_rate) = envelope_of(&cfg);
        let t_osc_env = crate::bph::t_osc_s(cfg.bph) * env_rate;
        let p = fold_envelope(&env, t_osc_env).expect("enough cycles");
        let sep = circular_dist(p.anchor_a_phase, p.anchor_b_phase, NBINS as f64);
        assert!((sep - NBINS as f64 / 2.0).abs() < 8.0, "separation {sep}");
        assert!(p.contrast > 3.0, "contrast {}", p.contrast);
    }

    #[test]
    fn beat_error_shifts_but_does_not_lose_anchors() {
        let cfg = SynthConfig {
            beat_error_ms: 2.0,
            snr_db: 30.0,
            ..SynthConfig::default()
        };
        let (env, env_rate) = envelope_of(&cfg);
        let t_osc_env = crate::bph::t_osc_s(cfg.bph) * env_rate;
        let p = fold_envelope(&env, t_osc_env).expect("enough cycles");
        // ±BE/2 shifts move the anchors ±(be/2)/T_osc·NBINS ≈ ±2.0 bins for 2 ms.
        let sep = circular_dist(p.anchor_a_phase, p.anchor_b_phase, NBINS as f64);
        let expected_shift = 2.0e-3 / crate::bph::t_osc_s(cfg.bph) * NBINS as f64;
        assert!(
            (sep - NBINS as f64 / 2.0).abs() < expected_shift + 8.0,
            "separation {sep} (allowed shift {expected_shift})"
        );
    }

    #[test]
    fn noise_yields_low_contrast() {
        let mut rng = crate::synth::Rng::new(9);
        let env: Vec<f32> = (0..(3_000.0 * 12.0) as usize)
            .map(|_| 0.02 * rng.next_f32().abs())
            .collect();
        let p = fold_envelope(&env, 750.0).expect("cycles fit");
        assert!(p.contrast < 2.0, "contrast {}", p.contrast);
    }

    #[test]
    fn too_few_cycles_or_bad_period_is_none() {
        let env = vec![0.0f32; 1000];
        assert!(fold_envelope(&env, 750.0).is_none()); // 1.3 cycles < 8
        assert!(fold_envelope(&env, f64::NAN).is_none());
        assert!(fold_envelope(&env, 0.0).is_none());
    }

    #[test]
    fn octave_guard_halves_a_doubled_period() {
        // toc_gain 0.1 defeats the 40% divisor walk upstream (beat-lag autocorr
        // value ≈ 2g/(1+g²) ≈ 0.198 < 0.40), so the period stage would deliver
        // 2·T_osc. The guard must recognize the 4-cluster fold and halve it.
        let cfg = SynthConfig {
            toc_gain: 0.1,
            snr_db: 35.0,
            ..SynthConfig::default()
        };
        let (env, env_rate) = envelope_of(&cfg);
        let t_osc_true = crate::bph::t_osc_s(cfg.bph) * env_rate;
        let (p, halved) = fold_with_octave_guard(&env, 2.0 * t_osc_true).expect("fold");
        assert!(halved, "guard must detect the doubled period");
        assert!(
            (p.t_osc_env - t_osc_true).abs() / t_osc_true < 0.01,
            "t_osc {}",
            p.t_osc_env
        );
    }

    #[test]
    fn octave_guard_leaves_a_correct_period_alone() {
        for cfg in [
            SynthConfig {
                snr_db: 30.0,
                ..SynthConfig::default()
            },
            SynthConfig {
                beat_error_ms: 2.0,
                snr_db: 30.0,
                ..SynthConfig::default()
            },
            SynthConfig {
                toc_gain: 0.5,
                snr_db: 30.0,
                ..SynthConfig::default()
            },
        ] {
            let (env, env_rate) = envelope_of(&cfg);
            let t_osc_env = crate::bph::t_osc_s(cfg.bph) * env_rate;
            let (p, halved) = fold_with_octave_guard(&env, t_osc_env).expect("fold");
            assert!(
                !halved,
                "false halving (be={} toc={})",
                cfg.beat_error_ms, cfg.toc_gain
            );
            assert!((p.t_osc_env - t_osc_env).abs() < 1e-9);
        }
    }
}
