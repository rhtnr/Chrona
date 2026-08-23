//! Quartz-reference timebase calibration (spec §3.4): recover the audio
//! clock's ppm error from a recording of any quartz watch's 1 Hz tick.

use crate::envelope::EnvelopeExtractor;
use crate::filter::{Butterworth, DcBlocker};

const MIN_SECONDS: f64 = 300.0;
const MAX_RESIDUAL_PPM: f64 = 2.0;

#[derive(Debug, thiserror::Error)]
pub enum CalError {
    #[error("recording too short: {seconds:.0} s (need at least {required:.0} s of quartz tick)")]
    TooShort { seconds: f64, required: f64 },
    #[error("no 1 Hz quartz tick found — clamp a quartz watch to the pickup and re-record")]
    NoTicks,
    #[error(
        "calibration fit unstable ({residual_ppm:.2} ppm residual) — re-record in a quieter setup"
    )]
    Unstable { residual_ppm: f64 },
    #[error("bad input: {reason}")]
    BadInput { reason: String },
}

#[derive(Debug, Clone, Copy)]
pub struct QuartzCalResult {
    /// Audio-clock error in ppm; pass directly as the analyzer's ppm correction.
    pub ppm: f64,
    /// Standard error of the fit, ppm.
    pub residual_ppm: f64,
    pub events: usize,
    pub duration_s: f64,
}

fn parabolic3(a: f64, b: f64, c: f64) -> f64 {
    let denom = a - 2.0 * b + c;
    if denom.abs() < 1e-20 {
        0.0
    } else {
        (0.5 * (a - c) / denom).clamp(-0.5, 0.5)
    }
}

/// Spec §3.4 quartz calibration: envelope → gated 1 Hz tick tracking →
/// least-squares slope of tick times vs the integer-second grid.
pub fn calibrate_quartz(samples: &[f32], sample_rate_hz: f64) -> Result<QuartzCalResult, CalError> {
    if !(sample_rate_hz.is_finite() && sample_rate_hz > 0.0) {
        return Err(CalError::BadInput {
            reason: format!("sample rate {sample_rate_hz}"),
        });
    }
    let duration_s = samples.len() as f64 / sample_rate_hz;
    if duration_s < MIN_SECONDS {
        return Err(CalError::TooShort {
            seconds: duration_s,
            required: MIN_SECONDS,
        });
    }

    // Precondition + envelope (same chain as the analyzer).
    let mut dc = DcBlocker::new();
    let mut hp =
        Butterworth::high_pass(sample_rate_hz, 3_000.0).map_err(|e| CalError::BadInput {
            reason: e.to_string(),
        })?;
    let mut envx = EnvelopeExtractor::new(sample_rate_hz).map_err(|e| CalError::BadInput {
        reason: e.to_string(),
    })?;
    let filtered: Vec<f32> = samples.iter().map(|&s| hp.process(dc.process(s))).collect();
    let mut env = Vec::new();
    envx.process(&filtered, &mut env);
    let env_rate = envx.envelope_rate_hz();

    // Noise floor and detectability.
    let mut sorted = env.clone();
    sorted.sort_by(f32::total_cmp);
    let floor = sorted[sorted.len() / 2].max(1e-9);
    let thr = 5.0 * floor;

    // Seed: strongest sample in the first 2 s must clear the threshold.
    let head = &env[..((2.0 * env_rate) as usize).min(env.len())];
    let (t0_idx, &t0_v) = head
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .ok_or(CalError::NoTicks)?;
    if t0_v < thr {
        return Err(CalError::NoTicks);
    }

    // Gated tracking: predict each next tick at +T̂, search ±50 ms.
    let gate = (0.05 * env_rate) as usize;
    let mut t_hat = env_rate; // one nominal second, in envelope samples
    let mut ticks: Vec<(f64, f64)> = vec![(0.0, t0_idx as f64)];
    let mut k = 1.0f64;
    loop {
        let pred = t0_idx as f64 + k * t_hat;
        let center = pred.round() as i64;
        let lo = (center - gate as i64).max(1) as usize;
        let hi = ((center + gate as i64) as usize).min(env.len().saturating_sub(2));
        if lo + 2 >= hi {
            break;
        }
        let pk = (lo..hi)
            .max_by(|&i, &j| env[i].total_cmp(&env[j]))
            .unwrap_or(lo);
        if env[pk] >= thr {
            let frac =
                pk as f64 + parabolic3(env[pk - 1] as f64, env[pk] as f64, env[pk + 1] as f64);
            ticks.push((k, frac));
            t_hat = (frac - t0_idx as f64) / k; // running period refinement
        }
        k += 1.0;
        if k > duration_s + 2.0 {
            break;
        }
    }

    let expected = (duration_s - 2.0).max(1.0);
    if (ticks.len() as f64) < 0.8 * expected {
        return Err(CalError::NoTicks);
    }

    // LS: idx = a + T·k  (T in envelope samples per true second of the quartz).
    let n = ticks.len() as f64;
    let (mut sk, mut st, mut sk2, mut skt) = (0.0f64, 0.0, 0.0, 0.0);
    for &(k, t) in &ticks {
        sk += k;
        st += t;
        sk2 += k * k;
        skt += k * t;
    }
    let denom = n * sk2 - sk * sk;
    if denom.abs() < 1e-9 {
        return Err(CalError::NoTicks);
    }
    let t_slope = (n * skt - sk * st) / denom;
    let a = (st - t_slope * sk) / n;
    let ssr: f64 = ticks
        .iter()
        .map(|&(k, t)| (t - (a + t_slope * k)).powi(2))
        .sum();
    let sigma_slope = (ssr / (n - 2.0)).sqrt() / (sk2 - sk * sk / n).sqrt();

    let ppm = (t_slope / env_rate - 1.0) * 1e6;
    let residual_ppm = sigma_slope / env_rate * 1e6;
    if residual_ppm > MAX_RESIDUAL_PPM {
        return Err(CalError::Unstable { residual_ppm });
    }
    Ok(QuartzCalResult {
        ppm,
        residual_ppm,
        events: ticks.len(),
        duration_s,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synth::synthesize_quartz;

    #[test]
    fn recovers_injected_ppm_within_half_ppm() {
        for ppm_true in [-80.0f64, 0.0, 50.0] {
            let x = synthesize_quartz(320.0, 48_000.0, ppm_true, 30.0, 5).unwrap();
            let r = calibrate_quartz(&x, 48_000.0).unwrap_or_else(|e| panic!("{ppm_true}: {e}"));
            assert!(
                (r.ppm - ppm_true).abs() <= 0.5,
                "ppm {ppm_true}: got {}",
                r.ppm
            );
            assert!(r.residual_ppm < 1.0, "residual {}", r.residual_ppm);
            assert!(r.events >= 250, "events {}", r.events);
        }
    }

    #[test]
    fn short_recording_is_rejected() {
        let x = synthesize_quartz(60.0, 48_000.0, 0.0, 30.0, 1).unwrap();
        assert!(matches!(
            calibrate_quartz(&x, 48_000.0),
            Err(CalError::TooShort { .. })
        ));
    }

    #[test]
    fn noise_only_yields_no_ticks() {
        let mut rng = crate::synth::Rng::new(4);
        let x: Vec<f32> = (0..(48_000.0 * 320.0) as usize)
            .map(|_| 0.02 * rng.next_f32())
            .collect();
        assert!(matches!(
            calibrate_quartz(&x, 48_000.0),
            Err(CalError::NoTicks)
        ));
    }

    #[test]
    fn bad_input_all_zeros_and_nan() {
        // calibrate_quartz(&[0.0; 48_000], f64::NAN) is Err(CalError::BadInput{..})
        let result = calibrate_quartz(&[0.0f32; 48_000], f64::NAN);
        assert!(
            matches!(result, Err(CalError::BadInput { .. })),
            "all zeros + NAN sample_rate should error BadInput"
        );
    }

    #[test]
    fn unstable_corruption_yields_high_residual_or_no_ticks() {
        // Interleave two quartz signals with ppm −400 and +400 (concatenate
        // 160 s of each into one 320 s buffer) → split slope's residual should
        // exceed 2 ppm → Err(CalError::Unstable{..}). If tracker loses ticks
        // on the seam, also accept NoTicks.
        let sr = 48_000.0;
        let mut corrupted = Vec::new();

        // First 160 s: quartz at −400 ppm
        let x1 = synthesize_quartz(160.0, sr, -400.0, 30.0, 1).unwrap();
        corrupted.extend(&x1);

        // Next 160 s: quartz at +400 ppm
        let x2 = synthesize_quartz(160.0, sr, 400.0, 30.0, 1).unwrap();
        corrupted.extend(&x2);

        let result = calibrate_quartz(&corrupted, sr);
        match result {
            Err(CalError::Unstable { residual_ppm }) => {
                assert!(
                    residual_ppm > 2.0,
                    "unstable residual {residual_ppm} should exceed 2 ppm"
                );
            }
            Err(CalError::NoTicks) => {
                // Also acceptable: tracker lost ticks on the seam.
            }
            other => {
                panic!("expected Err(Unstable) or Err(NoTicks), got {:?}", other);
            }
        }
    }
}
