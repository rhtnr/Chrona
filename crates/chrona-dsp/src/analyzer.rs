//! Streaming analyzer: precondition → envelope → period → rate (spec §5, stages 1–3 + 6).

use crate::envelope::EnvelopeExtractor;
use crate::filter::{Butterworth, DcBlocker, FilterError};
use crate::period::{PeriodEstimate, PeriodEstimator};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BphMode {
    /// Detect and snap to the standard table (spec §2.2).
    Auto,
    /// User-pinned nominal BPH.
    Fixed(u32),
    /// Report detection only; no rate (no nominal reference — spec §3.1).
    Free,
}

#[derive(Debug, Clone, Copy)]
pub struct AnalyzerConfig {
    pub sample_rate_hz: f64,
    pub bph_mode: BphMode,
    /// Audio-clock correction in ppm (spec §3.4). 0.0 = uncalibrated.
    pub ppm_correction: f64,
}

impl Default for AnalyzerConfig {
    fn default() -> Self {
        AnalyzerConfig {
            sample_rate_hz: 48_000.0,
            bph_mode: BphMode::Auto,
            ppm_correction: 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct RateEstimate {
    pub period: PeriodEstimate,
    /// Clock-corrected detected beats per hour.
    pub bph_detected: f64,
    /// The nominal reference in force (snapped, or the Fixed pin), if any.
    pub bph_nominal: Option<u32>,
    /// Rate vs nominal, s/day, positive = fast. None when no defensible nominal (spec §3.1).
    pub seconds_per_day: Option<f64>,
    pub calibrated: bool,
}

pub struct Analyzer {
    config: AnalyzerConfig,
    dc: DcBlocker,
    hp: Butterworth,
    envelope: EnvelopeExtractor,
    period: PeriodEstimator,
    scratch: Vec<f32>,
}

impl Analyzer {
    pub fn new(config: AnalyzerConfig) -> Result<Self, FilterError> {
        let envelope = EnvelopeExtractor::new(config.sample_rate_hz)?;
        let period = PeriodEstimator::new(envelope.envelope_rate_hz());
        Ok(Analyzer {
            config,
            dc: DcBlocker::new(),
            hp: Butterworth::high_pass(config.sample_rate_hz, 3_000.0)?, // spec §5.1
            envelope,
            period,
            scratch: Vec::new(),
        })
    }

    pub fn push_samples(&mut self, samples: &[f32]) {
        self.scratch.clear();
        self.scratch.reserve(samples.len());
        for &s in samples {
            self.scratch.push(self.hp.process(self.dc.process(s)));
        }
        let filtered = std::mem::take(&mut self.scratch);
        let mut env_out = Vec::with_capacity(filtered.len() / crate::envelope::DECIMATION + 1);
        self.envelope.process(&filtered, &mut env_out);
        self.period.push_envelope(&env_out);
        self.scratch = filtered; // reuse the allocation next call
    }

    pub fn current(&self) -> Option<RateEstimate> {
        let raw = self.period.estimate()?;
        // Spec §3.4: sr_eff = sr_nom·(1 + ppm/1e6) ⇒ true seconds = nominal seconds / (1 + ppm/1e6).
        let clock = 1.0 + self.config.ppm_correction / 1e6;
        let period = PeriodEstimate {
            t_osc_s: raw.t_osc_s / clock,
            sigma_s: raw.sigma_s / clock,
            window_s: raw.window_s,
        };
        let bph_detected = crate::bph::bph_from_t_osc(period.t_osc_s);

        let (bph_nominal, seconds_per_day) = match self.config.bph_mode {
            BphMode::Free => (None, None),
            BphMode::Auto => match crate::bph::snap_to_table(bph_detected) {
                Some(nom) => (
                    Some(nom),
                    Some(crate::bph::rate_s_per_day(period.t_osc_s, nom)),
                ),
                None => (None, None),
            },
            BphMode::Fixed(nom) => {
                let dev = (bph_detected - nom as f64).abs() / nom as f64;
                let rate = (dev <= 0.03).then(|| crate::bph::rate_s_per_day(period.t_osc_s, nom));
                (Some(nom), rate)
            }
        };

        Some(RateEstimate {
            period,
            bph_detected,
            bph_nominal,
            seconds_per_day,
            calibrated: self.config.ppm_correction != 0.0,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synth::{SynthConfig, synthesize};

    fn analyze(cfg: &SynthConfig, mode: BphMode, ppm: f64) -> Option<RateEstimate> {
        let x = synthesize(cfg).expect("valid synth config");
        let mut a = Analyzer::new(AnalyzerConfig {
            sample_rate_hz: cfg.sample_rate_hz,
            bph_mode: mode,
            ppm_correction: ppm,
        })
        .unwrap();
        a.push_samples(&x);
        a.current()
    }

    #[test]
    fn property_grid_rate_recovery_at_30db() {
        // Global Constraints bar: ±0.5 s/d at 30 dB SNR.
        for bph in [18_000u32, 21_600, 28_800, 36_000] {
            for rate in [-30.0f64, -5.0, 0.0, 12.0, 45.0] {
                let cfg = SynthConfig {
                    bph,
                    rate_s_per_day: rate,
                    beat_error_ms: 0.6,
                    snr_db: 30.0,
                    ..SynthConfig::default()
                };
                let est = analyze(&cfg, BphMode::Auto, 0.0)
                    .unwrap_or_else(|| panic!("no estimate: bph {bph} rate {rate}"));
                assert_eq!(est.bph_nominal, Some(bph), "bph {bph} rate {rate}");
                let got = est.seconds_per_day.expect("auto+snap must produce a rate");
                assert!(
                    (got - rate).abs() < 0.5,
                    "bph {bph}: want {rate}, got {got}"
                );
            }
        }
    }

    #[test]
    fn rate_recovery_at_10db() {
        // Global Constraints bar: ±1.0 s/d at 10 dB SNR.
        for rate in [-30.0f64, 45.0] {
            let cfg = SynthConfig {
                rate_s_per_day: rate,
                snr_db: 10.0,
                ..SynthConfig::default()
            };
            let est = analyze(&cfg, BphMode::Auto, 0.0).expect("10 dB must estimate");
            let got = est.seconds_per_day.expect("rate");
            assert!((got - rate).abs() < 1.0, "want {rate}, got {got}");
        }
    }

    #[test]
    fn ppm_correction_removes_clock_error() {
        // An ADC running 50 ppm fast makes a perfect watch look −4.32 s/d slow.
        let apparent = -86_400.0 * 50e-6;
        let cfg = SynthConfig {
            rate_s_per_day: apparent,
            ..SynthConfig::default()
        };
        let uncal = analyze(&cfg, BphMode::Auto, 0.0).unwrap();
        assert!((uncal.seconds_per_day.unwrap() - apparent).abs() < 0.5);
        assert!(!uncal.calibrated);
        let cal = analyze(&cfg, BphMode::Auto, 50.0).unwrap();
        assert!(
            cal.seconds_per_day.unwrap().abs() < 0.5,
            "corrected {:?}",
            cal.seconds_per_day
        );
        assert!(cal.calibrated);
    }

    #[test]
    fn free_mode_reports_detection_but_no_rate() {
        let cfg = SynthConfig::default();
        let est = analyze(&cfg, BphMode::Free, 0.0).unwrap();
        assert!(est.seconds_per_day.is_none());
        assert!(est.bph_nominal.is_none());
        assert!((est.bph_detected - 28_800.0).abs() < 30.0);
    }

    #[test]
    fn fixed_mode_rejects_gross_mismatch() {
        let cfg = SynthConfig::default(); // 28,800 bph signal
        let ok = analyze(&cfg, BphMode::Fixed(28_800), 0.0).unwrap();
        assert!(ok.seconds_per_day.is_some());
        let wrong = analyze(&cfg, BphMode::Fixed(18_000), 0.0).unwrap();
        assert!(wrong.seconds_per_day.is_none(), "3 % mismatch guard");
        assert_eq!(wrong.bph_nominal, Some(18_000)); // the pin is still reported
    }

    #[test]
    fn chunked_push_equals_one_shot() {
        let cfg = SynthConfig {
            duration_s: 20.0,
            ..SynthConfig::default()
        };
        let x = synthesize(&cfg).expect("valid synth config");
        let mk = || {
            Analyzer::new(AnalyzerConfig {
                sample_rate_hz: cfg.sample_rate_hz,
                bph_mode: BphMode::Auto,
                ppm_correction: 0.0,
            })
            .unwrap()
        };
        let mut a = mk();
        a.push_samples(&x);
        let mut b = mk();
        for chunk in x.chunks(480) {
            b.push_samples(chunk);
        }
        let (ra, rb) = (a.current().unwrap(), b.current().unwrap());
        assert!((ra.period.t_osc_s - rb.period.t_osc_s).abs() < 1e-12);
    }

    #[test]
    fn silence_yields_none() {
        let mut a = Analyzer::new(AnalyzerConfig::default()).unwrap();
        a.push_samples(&vec![0.0f32; 48_000 * 10]);
        assert!(a.current().is_none());
    }
}
