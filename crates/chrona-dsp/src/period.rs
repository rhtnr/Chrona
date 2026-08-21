//! Stepped-window beat-period estimator with σ gate (spec §5.3).

use crate::autocorr::{autocorrelate, fundamental_beat_lag, refine_peak_near};
use std::collections::VecDeque;

/// Stepped analysis windows in seconds, tried longest-first (spec §5.3).
pub const WINDOWS_S: [f64; 4] = [4.0, 8.0, 16.0, 32.0];
/// σ gate: accept when σ_T < T_osc · SIGMA_GATE (spec §5.3: σ < period/10⁴).
pub const SIGMA_GATE: f64 = 1e-4;
/// Full-oscillation search band in seconds (12000–72000 bph with margin).
const T_OSC_BAND_S: (f64, f64) = (0.08, 0.72);

#[derive(Debug, Clone, Copy)]
pub struct PeriodEstimate {
    /// Full oscillation period (tic + toc) in seconds, at the nominal sample clock.
    pub t_osc_s: f64,
    /// Standard error of the period fit, seconds.
    pub sigma_s: f64,
    /// Analysis window that produced the estimate, seconds.
    pub window_s: f64,
}

pub struct PeriodEstimator {
    env_rate_hz: f64,
    ring: VecDeque<f32>,
    capacity: usize,
}

impl PeriodEstimator {
    pub fn new(envelope_rate_hz: f64) -> Self {
        let capacity = (WINDOWS_S[3] * envelope_rate_hz) as usize;
        PeriodEstimator {
            env_rate_hz: envelope_rate_hz,
            ring: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    pub fn push_envelope(&mut self, env: &[f32]) {
        for &e in env {
            if self.ring.len() == self.capacity {
                self.ring.pop_front();
            }
            self.ring.push_back(e);
        }
    }

    pub fn estimate(&self) -> Option<PeriodEstimate> {
        for &window_s in WINDOWS_S.iter().rev() {
            let n = (window_s * self.env_rate_hz) as usize;
            if self.ring.len() < n {
                continue;
            }
            // Freshest `n` envelope samples, mean-removed (spec §5.2).
            let start = self.ring.len() - n;
            let mut x: Vec<f32> = self.ring.iter().skip(start).copied().collect();
            let mean = x.iter().map(|v| *v as f64).sum::<f64>() / n as f64;
            for v in x.iter_mut() {
                *v -= mean as f32;
            }
            if let Some(est) = self.estimate_window(&x, window_s) {
                return Some(est);
            }
        }
        None
    }

    fn estimate_window(&self, x: &[f32], window_s: f64) -> Option<PeriodEstimate> {
        let r = autocorrelate(x);
        let beat_lo = (T_OSC_BAND_S.0 / 2.0 * self.env_rate_hz) as usize;
        let beat_hi = (T_OSC_BAND_S.1 / 2.0 * self.env_rate_hz) as usize;
        let beat = fundamental_beat_lag(&r, beat_lo, beat_hi)?;
        let mut t_hat = 2.0 * beat.lag; // full oscillation, in envelope samples

        // The coarse beat-level seed can be biased when alternate beats are shifted
        // (beat error splits the single-beat autocorrelation peak) — but the
        // full-oscillation peak near k=1 is not (spec §5.3: full-oscillation
        // multiples are immune to alternate-beat shifts). One wide-tolerance lookup
        // re-anchors the seed on that unbiased peak before the tight per-cycle walk
        // below, which must stay tight to avoid the neighboring beat sub-harmonic
        // peak (spaced t_hat/2 away) once k grows.
        if let Some(p) = refine_peak_near(&r, t_hat, 0.03) {
            t_hat = p.lag;
        }

        // Per-cycle refinement over full-oscillation multiples (beat-error immune).
        let max_k = ((x.len() as f64 * 0.9) / t_hat).floor() as usize;
        let max_k = max_k.min(64);
        let mut ks: Vec<f64> = Vec::new();
        let mut lags: Vec<f64> = Vec::new();
        for k in 1..=max_k {
            if let Some(p) = refine_peak_near(&r, k as f64 * t_hat, 0.005) {
                ks.push(k as f64);
                lags.push(p.lag);
            }
        }
        if ks.len() < 8 {
            return None;
        }
        // Least squares through the origin: T = Σ(k·lag) / Σk².
        let sum_k2: f64 = ks.iter().map(|k| k * k).sum();
        let sum_klag: f64 = ks.iter().zip(&lags).map(|(k, l)| k * l).sum();
        let t_fit = sum_klag / sum_k2;
        let ss_res: f64 = ks
            .iter()
            .zip(&lags)
            .map(|(k, l)| (l - t_fit * k).powi(2))
            .sum();
        let sigma_t = (ss_res / (ks.len() as f64 - 1.0)).sqrt() / sum_k2.sqrt();

        let t_osc_s = t_fit / self.env_rate_hz;
        let sigma_s = sigma_t / self.env_rate_hz;
        let bph = crate::bph::bph_from_t_osc(t_osc_s);
        let in_band = (12_000.0 * 0.98..=72_000.0 * 1.02).contains(&bph);
        (sigma_s < t_osc_s * SIGMA_GATE && in_band).then_some(PeriodEstimate {
            t_osc_s,
            sigma_s,
            window_s,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::EnvelopeExtractor;
    use crate::synth::{SynthConfig, synthesize};

    fn estimate_for(cfg: &SynthConfig) -> Option<PeriodEstimate> {
        let x = synthesize(cfg).expect("valid synth config");
        let mut env = EnvelopeExtractor::new(cfg.sample_rate_hz).unwrap();
        let mut out = Vec::new();
        env.process(&x, &mut out);
        let mut pe = PeriodEstimator::new(env.envelope_rate_hz());
        pe.push_envelope(&out);
        pe.estimate()
    }

    #[test]
    fn clean_28800_within_a_ppm_scale_error() {
        let cfg = SynthConfig {
            snr_db: 30.0,
            ..SynthConfig::default()
        };
        let est = estimate_for(&cfg).expect("clean signal must estimate");
        let rel = (est.t_osc_s - 0.250).abs() / 0.250;
        assert!(rel < 6e-6, "rel err {rel}"); // ±0.5 s/d bar = 5.8 ppm (Global Constraints)
        assert!(est.sigma_s < est.t_osc_s * SIGMA_GATE);
        assert!(est.window_s >= 16.0, "window {}", est.window_s);
    }

    #[test]
    fn beat_error_does_not_bias_the_period() {
        let cfg = SynthConfig {
            beat_error_ms: 0.8,
            snr_db: 30.0,
            ..SynthConfig::default()
        };
        let est = estimate_for(&cfg).expect("beat error must not break estimation");
        let rel = (est.t_osc_s - 0.250).abs() / 0.250;
        assert!(rel < 6e-6, "rel err {rel}");
    }

    #[test]
    fn other_beat_rates_estimate_correctly() {
        for bph in [18_000u32, 21_600, 36_000] {
            let cfg = SynthConfig {
                bph,
                snr_db: 30.0,
                ..SynthConfig::default()
            };
            let est = estimate_for(&cfg).unwrap_or_else(|| panic!("bph {bph}"));
            let expected = crate::bph::t_osc_s(bph);
            let rel = (est.t_osc_s - expected).abs() / expected;
            assert!(rel < 6e-6, "bph {bph}: rel err {rel}");
        }
    }

    #[test]
    fn low_snr_still_estimates_within_relaxed_bar() {
        let cfg = SynthConfig {
            snr_db: 10.0,
            ..SynthConfig::default()
        };
        let est = estimate_for(&cfg).expect("10 dB SNR must still estimate");
        let rel = (est.t_osc_s - 0.250).abs() / 0.250;
        assert!(rel < 1.2e-5, "rel err {rel}"); // ±1.0 s/d bar (Global Constraints)
    }

    #[test]
    fn pure_noise_returns_none() {
        let mut rng = crate::synth::Rng::new(42);
        let noise: Vec<f32> = (0..(3_000.0 * 34.0) as usize)
            .map(|_| 0.02 * rng.next_f32())
            .collect();
        let mut pe = PeriodEstimator::new(3_000.0);
        pe.push_envelope(&noise);
        assert!(pe.estimate().is_none(), "σ gate must reject noise");
    }

    #[test]
    fn insufficient_data_returns_none() {
        let cfg = SynthConfig {
            duration_s: 2.0,
            ..SynthConfig::default()
        };
        assert!(estimate_for(&cfg).is_none()); // < smallest 4 s window
    }
}
