//! Envelope stage (spec §5.2): full-wave rectify → Butterworth LP @ min(1.5 kHz, 0.36·sr/16) → ÷16 decimate.

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
        // Alias-safe envelope LP (spec §5.2 as amended): must sit below the
        // post-decimation Nyquist sr/(2·DECIMATION). 0.36 leaves transition-band margin.
        let cutoff_hz = (0.36 * sample_rate_hz / DECIMATION as f64).min(1_500.0);
        Ok(EnvelopeExtractor {
            lp: Butterworth::low_pass(sample_rate_hz, cutoff_hz)?,
            sample_rate_hz,
            phase: 0,
        })
    }

    pub fn envelope_rate_hz(&self) -> f64 {
        self.sample_rate_hz / DECIMATION as f64
    }

    /// Rectify → low-pass → keep every 16th sample. Appends to `out`.
    pub fn process(&mut self, samples: &[f32], out: &mut Vec<f32>) {
        out.reserve(samples.len() / DECIMATION + 1);
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

    #[test]
    fn cutoff_stays_below_post_decimation_nyquist_at_44100() {
        // At 44.1 kHz the post-decimation Nyquist is 44100/32 = 1378 Hz; the fixed
        // 1.5 kHz cutoff sat ABOVE it (M1 known issue). The cutoff must now be
        // min(1500, 0.45·sr/16) = 1240 Hz there, and a 1360 Hz tone (below old
        // cutoff, above new) must be strongly attenuated before decimation.
        // Probe 1: 1360 Hz (post-rectification fundamental at 2720 Hz — we probe the LP
        // directly with a slow+fast mix).
        let sr = 44_100.0;
        let mut env = EnvelopeExtractor::new(sr).unwrap();
        assert!((env.envelope_rate_hz() - sr / 16.0).abs() < 1e-9);
        // Rectified DC passes; a tone near the old cutoff must not alias through.
        // Feed |sin| at 1360 Hz.
        let n = (sr * 2.0) as usize;
        let x: Vec<f32> = (0..n)
            .map(|i| {
                let t = i as f64 / sr;
                (0.5 + 0.5 * (2.0 * std::f64::consts::PI * 1_360.0 * t).sin()) as f32
            })
            .collect();
        let mut out = Vec::new();
        env.process(&x, &mut out);
        // Remove the DC part, measure residual ripple at the tone frequency.
        let tail = &out[out.len() / 2..];
        let mean = tail.iter().map(|v| *v as f64).sum::<f64>() / tail.len() as f64;
        let rms = (tail.iter().map(|v| (*v as f64 - mean).powi(2)).sum::<f64>()
            / tail.len() as f64)
            .sqrt();
        // Input ripple RMS is 0.5/√2 ≈ 0.354; require attenuation to < 0.18.
        assert!(rms < 0.18, "1360 Hz probe: ripple rms {}", rms);

        // Probe 2: 2720 Hz (the post-rectification harmonic).
        let mut env2 = EnvelopeExtractor::new(sr).unwrap();
        let x2: Vec<f32> = (0..n)
            .map(|i| {
                let t = i as f64 / sr;
                (0.5 + 0.5 * (2.0 * std::f64::consts::PI * 2_720.0 * t).sin()) as f32
            })
            .collect();
        let mut out2 = Vec::new();
        env2.process(&x2, &mut out2);
        let tail2 = &out2[out2.len() / 2..];
        let mean2 = tail2.iter().map(|v| *v as f64).sum::<f64>() / tail2.len() as f64;
        let rms2 = (tail2
            .iter()
            .map(|v| (*v as f64 - mean2).powi(2))
            .sum::<f64>()
            / tail2.len() as f64)
            .sqrt();
        // Higher frequency should be more attenuated; require rms < 0.05.
        assert!(rms2 < 0.05, "2720 Hz probe: ripple rms {}", rms2);
    }
}
