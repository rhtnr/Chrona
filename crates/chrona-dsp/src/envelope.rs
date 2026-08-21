//! Envelope stage (spec §5.2): full-wave rectify → Butterworth LP @ 1.5 kHz → ÷16 decimate.

use crate::filter::{Butterworth, FilterError};

/// Decimation factor from audio rate to envelope rate (48 kHz → 3 kHz).
pub const DECIMATION: usize = 16;

pub struct EnvelopeExtractor {
    lp: Butterworth,
    sample_rate_hz: f64,
    /// Phase within the decimation cycle, carried across `process` calls.
    phase: usize,
}

impl EnvelopeExtractor {
    pub fn new(sample_rate_hz: f64) -> Result<Self, FilterError> {
        Ok(EnvelopeExtractor {
            // 1.5 kHz keeps the envelope below the 3 kHz post-decimation Nyquist (spec §5.2).
            lp: Butterworth::low_pass(sample_rate_hz, 1_500.0)?,
            sample_rate_hz,
            phase: 0,
        })
    }

    pub fn envelope_rate_hz(&self) -> f64 {
        self.sample_rate_hz / DECIMATION as f64
    }

    /// Rectify → low-pass → keep every 16th sample. Appends to `out`.
    pub fn process(&mut self, samples: &[f32], out: &mut Vec<f32>) {
        for &s in samples {
            let e = self.lp.process(s.abs());
            if self.phase == 0 {
                out.push(e);
            }
            self.phase = (self.phase + 1) % DECIMATION;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synth::{SynthConfig, synthesize};

    #[test]
    fn envelope_is_nonnegative_and_decimated() {
        let cfg = SynthConfig {
            duration_s: 2.0,
            ..SynthConfig::default()
        };
        let x = synthesize(&cfg).expect("valid synth config");
        let mut env = EnvelopeExtractor::new(cfg.sample_rate_hz).unwrap();
        let mut out = Vec::new();
        env.process(&x, &mut out);
        assert_eq!(out.len(), x.len() / DECIMATION);
        // LP ringing can undershoot slightly; envelope must be essentially nonnegative.
        assert!(out.iter().all(|&e| e > -0.05));
        assert!((env.envelope_rate_hz() - 3_000.0).abs() < 1e-9);
    }

    #[test]
    fn envelope_peaks_once_per_beat() {
        // At high SNR the decimated envelope must peak near each drop pulse.
        let cfg = SynthConfig {
            duration_s: 4.0,
            snr_db: 40.0,
            ..SynthConfig::default()
        };
        let x = synthesize(&cfg).expect("valid synth config");
        let mut env = EnvelopeExtractor::new(cfg.sample_rate_hz).unwrap();
        let mut out = Vec::new();
        env.process(&x, &mut out);
        let env_rate = env.envelope_rate_hz();
        let t_beat = crate::bph::t_beat_s(cfg.bph);
        // For beats 2..=20: max envelope sample within ±10 ms of the expected drop time
        // must exceed 3× the window's median (the drop stands out of the floor).
        for k in 2..=20 {
            let t_drop = k as f64 * t_beat;
            let lo = ((t_drop - 0.010) * env_rate) as usize;
            let hi = ((t_drop + 0.010) * env_rate) as usize;
            let peak = out[lo..hi].iter().cloned().fold(f32::MIN, f32::max);
            let mut w: Vec<f32> = out[lo.saturating_sub(100)..hi + 100].to_vec();
            w.sort_by(f32::total_cmp);
            let median = w[w.len() / 2];
            assert!(
                peak > 3.0 * median.max(1e-6),
                "beat {k}: peak {peak}, median {median}"
            );
        }
    }

    #[test]
    fn streaming_chunks_equal_one_shot() {
        let cfg = SynthConfig {
            duration_s: 1.0,
            ..SynthConfig::default()
        };
        let x = synthesize(&cfg).expect("valid synth config");
        let mut a = EnvelopeExtractor::new(cfg.sample_rate_hz).unwrap();
        let mut one = Vec::new();
        a.process(&x, &mut one);
        let mut b = EnvelopeExtractor::new(cfg.sample_rate_hz).unwrap();
        let mut chunked = Vec::new();
        for chunk in x.chunks(479) {
            // deliberately not a multiple of DECIMATION
            b.process(chunk, &mut chunked);
        }
        assert_eq!(one, chunked);
    }
}
