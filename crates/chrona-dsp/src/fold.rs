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
    let mut buckets: Vec<Vec<f32>> =
        (0..NBINS).map(|_| Vec::with_capacity(cycles + 1)).collect();
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
}
