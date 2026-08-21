//! Streaming filters for the precondition stage (spec §5.1).

use biquad::{Biquad, Coefficients, DirectForm2Transposed, Q_BUTTERWORTH_F32, ToHertz, Type};

#[derive(Debug, thiserror::Error)]
pub enum FilterError {
    #[error("invalid filter parameters: cutoff {cutoff_hz} Hz at sample rate {sample_rate_hz} Hz")]
    InvalidParams { sample_rate_hz: f64, cutoff_hz: f64 },
}

/// One-pole DC blocker: y[n] = x[n] − x[n−1] + R·y[n−1], R = 0.995.
pub struct DcBlocker {
    x1: f32,
    y1: f32,
}

impl DcBlocker {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        DcBlocker { x1: 0.0, y1: 0.0 }
    }
    pub fn process(&mut self, s: f32) -> f32 {
        let y = s - self.x1 + 0.995 * self.y1;
        self.x1 = s;
        self.y1 = y;
        y
    }
}

/// 2nd-order Butterworth section (spec §5.1: HP @ 3 kHz; §5.2: LP @ 1.5 kHz).
pub struct Butterworth {
    inner: DirectForm2Transposed<f32>,
}

impl Butterworth {
    pub fn high_pass(sample_rate_hz: f64, cutoff_hz: f64) -> Result<Self, FilterError> {
        Self::build(Type::HighPass, sample_rate_hz, cutoff_hz)
    }
    pub fn low_pass(sample_rate_hz: f64, cutoff_hz: f64) -> Result<Self, FilterError> {
        Self::build(Type::LowPass, sample_rate_hz, cutoff_hz)
    }
    fn build(ty: Type<f32>, sample_rate_hz: f64, cutoff_hz: f64) -> Result<Self, FilterError> {
        if !(cutoff_hz > 0.0 && cutoff_hz < sample_rate_hz / 2.0) {
            return Err(FilterError::InvalidParams {
                sample_rate_hz,
                cutoff_hz,
            });
        }
        let coeffs = Coefficients::<f32>::from_params(
            ty,
            (sample_rate_hz as f32).hz(),
            (cutoff_hz as f32).hz(),
            Q_BUTTERWORTH_F32,
        )
        .map_err(|_| FilterError::InvalidParams {
            sample_rate_hz,
            cutoff_hz,
        })?;
        Ok(Butterworth {
            inner: DirectForm2Transposed::<f32>::new(coeffs),
        })
    }
    pub fn process(&mut self, s: f32) -> f32 {
        self.inner.run(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(sr: f64, f: f64, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| (2.0 * std::f64::consts::PI * f * i as f64 / sr).sin() as f32)
            .collect()
    }

    /// Steady-state RMS gain of a filter for a pure tone (skip the transient).
    fn tone_gain(mut filt: impl FnMut(f32) -> f32, sr: f64, f: f64) -> f64 {
        let x = sine(sr, f, (sr * 0.5) as usize);
        let y: Vec<f32> = x.iter().map(|&s| filt(s)).collect();
        let tail = &y[y.len() / 2..];
        let rms = |v: &[f32]| {
            (v.iter().map(|s| (*s as f64).powi(2)).sum::<f64>() / v.len() as f64).sqrt()
        };
        rms(tail) / rms(&x[x.len() / 2..])
    }

    #[test]
    fn dc_blocker_removes_offset() {
        let mut dc = DcBlocker::new();
        let y: Vec<f32> = (0..48_000).map(|_| dc.process(0.5)).collect();
        assert!(y[y.len() - 1].abs() < 1e-3);
    }

    #[test]
    fn high_pass_3k_attenuates_hum_passes_ticks() {
        let sr = 48_000.0;
        let mut hp = Butterworth::high_pass(sr, 3_000.0).unwrap();
        let g_low = tone_gain(|s| hp.process(s), sr, 100.0);
        assert!(g_low < 0.01, "100 Hz gain {g_low}"); // ≥ −40 dB (2nd order, ~59 dB expected)
        let mut hp2 = Butterworth::high_pass(sr, 3_000.0).unwrap();
        let g_band = tone_gain(|s| hp2.process(s), sr, 7_800.0);
        assert!(g_band > 0.9, "7.8 kHz gain {g_band}");
    }

    #[test]
    fn low_pass_1k5_passes_low_blocks_high() {
        let sr = 48_000.0;
        let mut lp = Butterworth::low_pass(sr, 1_500.0).unwrap();
        let g_pass = tone_gain(|s| lp.process(s), sr, 100.0);
        assert!(g_pass > 0.95, "100 Hz gain {g_pass}");
        let mut lp2 = Butterworth::low_pass(sr, 1_500.0).unwrap();
        let g_stop = tone_gain(|s| lp2.process(s), sr, 12_000.0);
        assert!(g_stop < 0.01, "12 kHz gain {g_stop}");
    }

    #[test]
    fn invalid_cutoff_is_an_error() {
        assert!(Butterworth::low_pass(48_000.0, 0.0).is_err());
        assert!(Butterworth::low_pass(48_000.0, 30_000.0).is_err()); // above Nyquist
    }
}
