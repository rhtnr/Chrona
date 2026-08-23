//! Streaming analyzer: precondition → envelope → period → fold → events → regression → amplitude → tier (spec §5, stages 1–7).

use crate::envelope::{DECIMATION, EnvelopeExtractor};
use crate::events::{BeatEvent, extract_events_from};
use crate::filter::{Butterworth, DcBlocker, FilterError};
use crate::fold::{FoldProfile, fold_with_octave_guard};
use crate::metrics::{AmplitudeGateFail, amplitude_from_events, regress_unlocking};
use crate::period::{PeriodEstimate, PeriodEstimator};
use crate::ring::SampleRing;
use crate::tier::{Tier, assign_tier};

#[derive(Debug, thiserror::Error)]
pub enum AnalyzerError {
    #[error(transparent)]
    Filter(#[from] FilterError),
    #[error("invalid analyzer config: {reason}")]
    InvalidConfig { reason: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BphMode {
    Auto,
    Fixed(u32),
    Free,
}

#[derive(Debug, Clone, Copy)]
pub struct AnalyzerConfig {
    pub sample_rate_hz: f64,
    pub bph_mode: BphMode,
    /// Audio-clock correction in ppm (spec §3.4). 0.0 = uncalibrated.
    pub ppm_correction: f64,
    /// Lift angle in degrees (spec §2.3; default 52, valid 10–90).
    pub lift_angle_deg: f64,
    /// Stage-6 averaging window in seconds (spec §2.3; default 30, valid 2–60).
    pub averaging_s: f64,
}

impl Default for AnalyzerConfig {
    fn default() -> Self {
        AnalyzerConfig {
            sample_rate_hz: 48_000.0,
            bph_mode: BphMode::Auto,
            ppm_correction: 0.0,
            lift_angle_deg: 52.0,
            averaging_s: 30.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateSource {
    PeriodSlope,
    UnlockingRegression,
}

#[derive(Debug, Clone, Copy)]
pub struct Quality {
    pub detection_ratio: f64,
    pub onset_jitter_ms: Option<f64>,
    pub mean_beat_snr_db: Option<f64>,
    pub unlocking_ratio: f64,
    pub clipped_samples: u64,
    pub amplitude_gate: Option<AmplitudeGateFail>,
}

#[derive(Debug, Clone, Copy)]
pub struct MetricsSnapshot {
    pub tier: Tier,
    pub bph_detected: f64,
    pub bph_nominal: Option<u32>,
    /// s/day, positive = fast; None when no defensible nominal (spec §3.1).
    pub rate_s_per_day: Option<f64>,
    /// Which estimator produced rate_s_per_day. `Some` iff rate_s_per_day
    /// is `Some` — `None` here means there is no rate to attribute.
    pub rate_source: Option<RateSource>,
    /// Only at tier ≥ T2.
    pub beat_error_ms: Option<f64>,
    /// Only at T3.
    pub amplitude_deg: Option<f64>,
    pub period: PeriodEstimate,
    pub quality: Quality,
    pub calibrated: bool,
}

/// One cached tape entry — the live view onto `Analyzer::tape_events()`.
/// A thin, `Copy` projection of `BeatEvent` (spec §5.5 / M3).
#[derive(Debug, Clone, Copy)]
pub struct TapeEvent {
    pub beat_index: i64,
    pub parity: crate::events::Parity,
    pub t_unlock_s: Option<f64>,
    pub t_drop_corr_s: f64,
}

/// Incremental analysis state (M3): the fold/extraction results from the
/// last refold, plus how far event extraction has progressed since. See
/// `Analyzer::current_metrics` for the refold/extend algorithm.
struct AnalysisCache {
    profile: FoldProfile,
    halved: bool,
    /// Nominal t_osc (seconds, pre-halving) the profile was folded at.
    t_osc_used: f64,
    /// env ring len when folded (fold_start recomputation anchor).
    env_len_at_fold: usize,
    /// Absolute env index of env_window[0] at fold time.
    env_start_at_fold: u64,
    /// How many cycles of the frozen grid have been extracted so far.
    extracted_through_cycle: usize,
    /// Absolute-seconds events, ascending, pruned to the 32 s ring span.
    events: Vec<BeatEvent>,
    /// raw_ring.total_pushed() at the last fold.
    last_fold_total: u64,
}

pub struct Analyzer {
    config: AnalyzerConfig,
    dc: DcBlocker,
    hp: Butterworth,
    envelope: EnvelopeExtractor,
    period: PeriodEstimator,
    raw_ring: SampleRing,
    env_ring: SampleRing,
    clipped: u64,
    scratch: Vec<f32>,
    cache: Option<AnalysisCache>,
}

impl Analyzer {
    pub fn new(config: AnalyzerConfig) -> Result<Self, AnalyzerError> {
        if !config.ppm_correction.is_finite() {
            return Err(AnalyzerError::InvalidConfig {
                reason: format!("ppm_correction = {} must be finite", config.ppm_correction),
            });
        }
        for (name, v, lo, hi) in [
            ("lift_angle_deg", config.lift_angle_deg, 10.0, 90.0),
            ("averaging_s", config.averaging_s, 2.0, 60.0),
        ] {
            if !v.is_finite() || !(lo..=hi).contains(&v) {
                return Err(AnalyzerError::InvalidConfig {
                    reason: format!("{name} = {v} outside [{lo}, {hi}]"),
                });
            }
        }
        let envelope = EnvelopeExtractor::new(config.sample_rate_hz)?;
        let period = PeriodEstimator::new(envelope.envelope_rate_hz());
        // Ring-capacity note: the aligned copy's start boundary in current_metrics() is
        // safe by construction (env_start·16 is always ≥ the raw ring's retained start);
        // its end boundary is handled there by trimming the trailing partial envelope
        // sample (the envelope leads the raw stream, so a full env window can imply a raw
        // range past what's been pushed). The +DECIMATION margin below is retained as
        // harmless slack, not load-bearing.
        let raw_cap = (32.0 * config.sample_rate_hz) as usize + DECIMATION;
        let env_cap = (32.0 * config.sample_rate_hz) as usize / DECIMATION;
        Ok(Analyzer {
            config,
            dc: DcBlocker::new(),
            hp: Butterworth::high_pass(config.sample_rate_hz, 3_000.0)?,
            envelope,
            period,
            raw_ring: SampleRing::new(raw_cap),
            env_ring: SampleRing::new(env_cap),
            clipped: 0,
            scratch: Vec::new(),
            cache: None,
        })
    }

    pub fn push_samples(&mut self, samples: &[f32]) {
        self.clipped += samples.iter().filter(|s| s.abs() >= 0.999).count() as u64;
        self.scratch.clear();
        self.scratch.reserve(samples.len());
        for &s in samples {
            self.scratch.push(self.hp.process(self.dc.process(s)));
        }
        let filtered = std::mem::take(&mut self.scratch);
        self.raw_ring.push_slice(&filtered);
        let mut env_out = Vec::with_capacity(filtered.len() / DECIMATION + 1);
        self.envelope.process(&filtered, &mut env_out);
        self.env_ring.push_slice(&env_out);
        self.period.push_envelope(&env_out);
        self.scratch = filtered;
    }

    /// Clipped-sample count, readable without a snapshot (spec §M3: the
    /// `no_beat` CLI report needs this even when no estimate is available).
    pub fn clipped_samples(&self) -> u64 {
        self.clipped
    }

    /// The cached events in the current 32 s ring, ascending time. Empty
    /// before the first successful fold.
    pub fn tape_events(&self) -> Vec<TapeEvent> {
        match &self.cache {
            None => Vec::new(),
            Some(c) => c
                .events
                .iter()
                .map(|e| TapeEvent {
                    beat_index: e.beat_index,
                    parity: e.parity,
                    t_unlock_s: e.t_unlock_s,
                    t_drop_corr_s: e.t_drop_corr_s,
                })
                .collect(),
        }
    }

    /// Incremental metrics (M3 breaking window). Refolds (recomputing the
    /// fold profile and octave-guard decision, and re-extracting every
    /// cycle) at most once per ~1 s of new data or when the period estimate
    /// has drifted >0.1% since the last fold; between refolds it extends the
    /// SAME frozen cycle grid forward and extracts only newly-completed
    /// cycles, appending to the cached event tape. Results are equal to the
    /// old one-shot recomputation on identical pushed data (see
    /// `incremental_equals_oneshot`).
    pub fn current_metrics(&mut self) -> Option<MetricsSnapshot> {
        let raw_est = self.period.estimate()?;
        let clock = 1.0 + self.config.ppm_correction / 1e6;
        let sr = self.config.sample_rate_hz;
        let env_rate = self.envelope.envelope_rate_hz();

        let need_refold = match &self.cache {
            None => true,
            Some(c) => {
                self.raw_ring
                    .total_pushed()
                    .saturating_sub(c.last_fold_total)
                    >= sr as u64
                    || (raw_est.t_osc_s - c.t_osc_used).abs() / c.t_osc_used > 0.001
            }
        };

        if need_refold {
            // Rebuild the env window exactly as M2's one-shot current() did,
            // including the trailing-partial-envelope-sample trim (the
            // envelope leads the raw stream at decimation phase 0, so a full
            // env window can imply a raw range past what's been pushed).
            let mut env_window = Vec::new();
            self.env_ring
                .copy_last(self.env_ring.len(), &mut env_window);
            let env_start_abs = self.env_ring.start_index();
            let usable = (self
                .raw_ring
                .total_pushed()
                .saturating_sub(env_start_abs * DECIMATION as u64)
                / DECIMATION as u64) as usize;
            env_window.truncate(usable.min(env_window.len()));

            let folded = fold_with_octave_guard(&env_window, raw_est.t_osc_s * env_rate);
            match folded {
                None => {
                    self.cache = None;
                    return Some(self.tier1_snapshot(raw_est, clock, None));
                }
                Some((profile, halved)) => {
                    self.cache = Some(AnalysisCache {
                        profile,
                        halved,
                        t_osc_used: raw_est.t_osc_s,
                        env_len_at_fold: env_window.len(),
                        env_start_at_fold: env_start_abs,
                        extracted_through_cycle: 0,
                        events: Vec::new(),
                        last_fold_total: self.raw_ring.total_pushed(),
                    });
                }
            }
        }

        let halving = if self.cache.as_ref().unwrap().halved {
            2.0
        } else {
            1.0
        };
        let t_osc_nominal = raw_est.t_osc_s / halving;
        let corrected = PeriodEstimate {
            t_osc_s: t_osc_nominal / clock,
            sigma_s: raw_est.sigma_s / halving / clock,
            window_s: raw_est.window_s,
        };

        // Extend the frozen cycle grid forward and extract newly-completed
        // cycles (design: see the module-level algorithm note above and the
        // task brief — the grid is anchored to a fold-time origin, computed
        // fresh each call from env_start_at_fold/env_len_at_fold so it never
        // depends on the CURRENT ring content, only on the profile's own
        // t_osc_env and where the last fold placed cycle 0).
        let (origin_abs, t_osc_env, cycles_now, from_cycle) = {
            let cache = self.cache.as_ref().unwrap();
            let t_osc_env = cache.profile.t_osc_env;
            // Identical two-line arithmetic to fold_envelope/extract_events's
            // own fold_start derivation, applied to the fold-time window, so
            // this exactly reproduces where that call placed cycle 0.
            let cycles_at_fold = (cache.env_len_at_fold as f64 / t_osc_env).floor() as usize;
            let usable_at_fold = (cycles_at_fold as f64 * t_osc_env) as usize;
            let fold_start_at_fold = cache.env_len_at_fold - usable_at_fold;
            let origin_abs = cache.env_start_at_fold + fold_start_at_fold as u64;

            // Global "fully raw-backed" env frontier (same trim rule as the
            // fold window above, just expressed from absolute index 0).
            let usable_abs_end = self.raw_ring.total_pushed() / DECIMATION as u64;
            let avail = usable_abs_end.saturating_sub(origin_abs) as f64;
            let cycles_now = (avail / t_osc_env).floor().max(0.0) as usize;
            (
                origin_abs,
                t_osc_env,
                cycles_now,
                cache.extracted_through_cycle,
            )
        };

        let newest_time = self.raw_ring.total_pushed() as f64 / sr;
        if cycles_now > from_cycle {
            // Truncate to a whole number of cycles from origin_abs so that
            // extract_events_from's own end-anchored fold_start computation
            // lands on 0 — i.e. cycle 0 (local) is cycle 0 (global, from
            // origin_abs) — keeping beat_index numbering stable across polls.
            let l_env = (cycles_now as f64 * t_osc_env) as usize;
            let mut env_window = Vec::new();
            let mut raw_window = Vec::new();
            let env_ok = self
                .env_ring
                .copy_range_abs(origin_abs, l_env, &mut env_window);
            let raw_ok = env_ok
                && self.raw_ring.copy_range_abs(
                    origin_abs * DECIMATION as u64,
                    l_env * DECIMATION,
                    &mut raw_window,
                );
            // Both copies are expected to succeed by construction (the ≥1 s
            // refold cadence keeps origin_abs well inside the 32 s ring); if
            // not, skip extraction this poll and retry once more data lands.
            if raw_ok {
                let new_events = extract_events_from(
                    &raw_window,
                    origin_abs * DECIMATION as u64,
                    &env_window,
                    sr,
                    &self.cache.as_ref().unwrap().profile,
                    from_cycle,
                );
                let cache = self.cache.as_mut().unwrap();
                cache.events.extend(new_events);
                cache
                    .events
                    .retain(|e| e.t_drop_corr_s >= newest_time - 32.0);
                cache.extracted_through_cycle = cycles_now;
            }
        }

        let recent: Vec<BeatEvent> = self
            .cache
            .as_ref()
            .unwrap()
            .events
            .iter()
            .filter(|e| e.t_drop_corr_s >= newest_time - self.config.averaging_s)
            .copied()
            .collect();

        let span = self
            .config
            .averaging_s
            .min(self.env_ring.len() as f64 / env_rate);
        let expected = (span / (t_osc_nominal / 2.0)).round().max(1.0);
        let detection_ratio = (recent.len() as f64 / expected).min(1.0);
        let unlocking_ratio = if recent.is_empty() {
            0.0
        } else {
            recent.iter().filter(|e| e.t_unlock_s.is_some()).count() as f64 / recent.len() as f64
        };
        let mean_beat_snr_db = if recent.is_empty() {
            None
        } else {
            Some(recent.iter().map(|e| e.snr_db as f64).sum::<f64>() / recent.len() as f64)
        };

        let regression = regress_unlocking(&recent);
        let amplitude = amplitude_from_events(&recent, t_osc_nominal, self.config.lift_angle_deg);
        let onset_jitter_ms = regression.map(|r| r.jitter_ms / clock);
        let tier = assign_tier(
            detection_ratio,
            onset_jitter_ms,
            unlocking_ratio,
            amplitude.is_ok(),
        );

        let bph_detected = crate::bph::bph_from_t_osc(corrected.t_osc_s);
        let (bph_nominal, period_rate) = self.resolve_mode(bph_detected, corrected.t_osc_s);
        // R2 (rate-source honesty gate): gate on the MODE-RESOLVED rate being
        // present, not just a nominal existing — a Fixed-mode >3% mismatch
        // resolves to (Some(nom), None) and must not be papered over with a
        // regression-derived rate (see fixed_mode_rejects_gross_mismatch).
        // rate_source is Some iff rate_s_per_day is Some (M3 honesty gate).
        let (rate_s_per_day, rate_source) = match (bph_nominal, period_rate, regression) {
            (Some(nom), Some(_), Some(r)) if tier >= Tier::T2 => {
                let t_beat_nom = crate::bph::t_beat_s(nom);
                let rate = 86_400.0 * (t_beat_nom - r.t_beat_s / clock) / t_beat_nom;
                (Some(rate), Some(RateSource::UnlockingRegression))
            }
            _ => (period_rate, period_rate.map(|_| RateSource::PeriodSlope)),
        };

        Some(MetricsSnapshot {
            tier,
            bph_detected,
            bph_nominal,
            rate_s_per_day,
            rate_source,
            beat_error_ms: (tier >= Tier::T2)
                .then(|| regression.map(|r| r.beat_error_ms / clock))
                .flatten(),
            amplitude_deg: (tier == Tier::T3)
                .then(|| amplitude.as_ref().ok().map(|a| a.degrees))
                .flatten(),
            period: corrected,
            quality: Quality {
                detection_ratio,
                onset_jitter_ms,
                mean_beat_snr_db,
                unlocking_ratio,
                clipped_samples: self.clipped,
                amplitude_gate: amplitude.err(),
            },
            calibrated: self.config.ppm_correction != 0.0,
        })
    }

    /// Fallback when fold/alignment can't run yet: period metrics only.
    fn tier1_snapshot(
        &self,
        raw_est: PeriodEstimate,
        clock: f64,
        halving: Option<f64>,
    ) -> MetricsSnapshot {
        let halving = halving.unwrap_or(1.0);
        let corrected = PeriodEstimate {
            t_osc_s: raw_est.t_osc_s / halving / clock,
            sigma_s: raw_est.sigma_s / halving / clock,
            window_s: raw_est.window_s,
        };
        let bph_detected = crate::bph::bph_from_t_osc(corrected.t_osc_s);
        let (bph_nominal, rate) = self.resolve_mode(bph_detected, corrected.t_osc_s);
        MetricsSnapshot {
            tier: Tier::T1,
            bph_detected,
            bph_nominal,
            rate_s_per_day: rate,
            rate_source: rate.map(|_| RateSource::PeriodSlope),
            beat_error_ms: None,
            amplitude_deg: None,
            period: corrected,
            quality: Quality {
                detection_ratio: 0.0,
                onset_jitter_ms: None,
                mean_beat_snr_db: None,
                unlocking_ratio: 0.0,
                clipped_samples: self.clipped,
                amplitude_gate: None,
            },
            calibrated: self.config.ppm_correction != 0.0,
        }
    }

    /// M1's BPH-mode honesty rules, unchanged (spec §3.1).
    fn resolve_mode(&self, bph_detected: f64, t_osc_s: f64) -> (Option<u32>, Option<f64>) {
        match self.config.bph_mode {
            BphMode::Free => (None, None),
            BphMode::Auto => match crate::bph::snap_to_table(bph_detected) {
                Some(nom) => (Some(nom), Some(crate::bph::rate_s_per_day(t_osc_s, nom))),
                None => (None, None),
            },
            BphMode::Fixed(nom) => {
                let dev = (bph_detected - nom as f64).abs() / nom as f64;
                let rate = (dev <= 0.03).then(|| crate::bph::rate_s_per_day(t_osc_s, nom));
                (Some(nom), rate)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synth::{SynthConfig, synthesize};

    fn analyze(cfg: &SynthConfig, mode: BphMode, ppm: f64) -> Option<MetricsSnapshot> {
        let x = synthesize(cfg).expect("valid synth config");
        let mut a = Analyzer::new(AnalyzerConfig {
            sample_rate_hz: cfg.sample_rate_hz,
            bph_mode: mode,
            ppm_correction: ppm,
            ..AnalyzerConfig::default()
        })
        .unwrap();
        a.push_samples(&x);
        a.current_metrics()
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
                let got = est.rate_s_per_day.expect("auto+snap must produce a rate");
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
            let got = est.rate_s_per_day.expect("rate");
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
        assert!((uncal.rate_s_per_day.unwrap() - apparent).abs() < 0.5);
        assert!(!uncal.calibrated);
        let cal = analyze(&cfg, BphMode::Auto, 50.0).unwrap();
        assert!(
            cal.rate_s_per_day.unwrap().abs() < 0.5,
            "corrected {:?}",
            cal.rate_s_per_day
        );
        assert!(cal.calibrated);
    }

    #[test]
    fn free_mode_reports_detection_but_no_rate() {
        let cfg = SynthConfig::default();
        let est = analyze(&cfg, BphMode::Free, 0.0).unwrap();
        assert!(est.rate_s_per_day.is_none());
        assert!(est.bph_nominal.is_none());
        assert!((est.bph_detected - 28_800.0).abs() < 30.0);
    }

    #[test]
    fn fixed_mode_rejects_gross_mismatch() {
        let cfg = SynthConfig::default(); // 28,800 bph signal
        let ok = analyze(&cfg, BphMode::Fixed(28_800), 0.0).unwrap();
        assert!(ok.rate_s_per_day.is_some());
        let wrong = analyze(&cfg, BphMode::Fixed(18_000), 0.0).unwrap();
        assert!(wrong.rate_s_per_day.is_none(), "3 % mismatch guard");
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
                ..AnalyzerConfig::default()
            })
            .unwrap()
        };
        let mut a = mk();
        a.push_samples(&x);
        let mut b = mk();
        for chunk in x.chunks(480) {
            b.push_samples(chunk);
        }
        let (ra, rb) = (a.current_metrics().unwrap(), b.current_metrics().unwrap());
        assert!((ra.period.t_osc_s - rb.period.t_osc_s).abs() < 1e-12);
    }

    #[test]
    fn silence_yields_none() {
        let mut a = Analyzer::new(AnalyzerConfig::default()).unwrap();
        a.push_samples(&vec![0.0f32; 48_000 * 10]);
        assert!(a.current_metrics().is_none());
    }

    #[test]
    fn full_metrics_at_tier3() {
        let cfg = SynthConfig {
            beat_error_ms: 0.8,
            amplitude_deg: 270.0,
            rate_s_per_day: 12.0,
            snr_db: 30.0,
            ..SynthConfig::default()
        };
        let est = analyze(&cfg, BphMode::Auto, 0.0).expect("snapshot");
        assert_eq!(est.tier, crate::tier::Tier::T3);
        assert_eq!(est.rate_source, Some(RateSource::UnlockingRegression));
        let be = est.beat_error_ms.expect("beat error at T3");
        assert!((be - 0.8).abs() <= 0.1, "be {be}");
        let amp = est.amplitude_deg.expect("amplitude at T3");
        assert!((amp - 270.0).abs() <= 5.0, "amp {amp}");
        let rate = est.rate_s_per_day.expect("rate");
        assert!((rate - 12.0).abs() <= 0.3, "rate {rate}");
        assert!(est.quality.detection_ratio > 0.8);
    }

    #[test]
    fn degradation_order_as_snr_falls() {
        let t3 = analyze(
            &SynthConfig {
                snr_db: 30.0,
                ..SynthConfig::default()
            },
            BphMode::Auto,
            0.0,
        )
        .expect("30 dB");
        assert_eq!(t3.tier, crate::tier::Tier::T3);
        let low = analyze(
            &SynthConfig {
                snr_db: 10.0,
                ..SynthConfig::default()
            },
            BphMode::Auto,
            0.0,
        )
        .expect("10 dB");
        assert!(low.tier <= crate::tier::Tier::T2, "tier {:?}", low.tier);
        assert!(low.amplitude_deg.is_none(), "no fabricated amplitude");
        assert!(low.rate_s_per_day.is_some(), "rate survives at low SNR");
    }

    #[test]
    fn octave_error_is_corrected_end_to_end() {
        // toc_gain 0.1 defeats the divisor walk; M1 would have reported 14400.
        let cfg = SynthConfig {
            toc_gain: 0.1,
            snr_db: 35.0,
            ..SynthConfig::default()
        };
        let est = analyze(&cfg, BphMode::Auto, 0.0).expect("snapshot");
        assert_eq!(
            est.bph_nominal,
            Some(28_800),
            "octave guard failed: {:?}",
            est.bph_detected
        );
    }

    #[test]
    fn full_metrics_survive_any_decimation_phase() {
        // Regression for C1: the envelope leads the raw stream (the
        // extractor emits at decimation phase 0), so env_total =
        // ceil(raw_total/16). Whenever raw total_pushed isn't a multiple of
        // 16, the un-truncated env window's aligned raw copy reaches past
        // raw_ring.total_pushed() and current() must not silently fall back
        // to a tier1-only snapshot.
        let cfg = SynthConfig {
            beat_error_ms: 0.8,
            amplitude_deg: 270.0,
            rate_s_per_day: 12.0,
            snr_db: 30.0,
            ..SynthConfig::default()
        };
        let x = synthesize(&cfg).expect("valid synth config");
        let mut a = Analyzer::new(AnalyzerConfig {
            sample_rate_hz: cfg.sample_rate_hz,
            ..AnalyzerConfig::default()
        })
        .unwrap();
        a.push_samples(&x);
        a.push_samples(&[0.0f32; 7]); // total_pushed % 16 == 7
        let est = a.current_metrics().expect("snapshot");
        assert_eq!(est.tier, crate::tier::Tier::T3, "tier {:?}", est.tier);
        let be = est.beat_error_ms.expect("beat error at T3");
        assert!((be - 0.8).abs() <= 0.1, "be {be}");
        let amp = est.amplitude_deg.expect("amplitude at T3");
        assert!((amp - 270.0).abs() <= 5.0, "amp {amp}");
    }

    #[test]
    fn clip_counter_accumulates() {
        let mut a = Analyzer::new(AnalyzerConfig::default()).unwrap();
        a.push_samples(&vec![1.5f32; 100]);
        a.push_samples(&vec![0.0f32; 48_000]);
        // No beat → None, but the counter lives on the analyzer; push a synth
        // signal and confirm it survives into the snapshot.
        let x = synthesize(&SynthConfig::default()).unwrap();
        a.push_samples(&x);
        let est = a.current_metrics().expect("snapshot");
        assert!(est.quality.clipped_samples >= 100);
        // This push sequence's total is % 16 == 4 — it was silently
        // exercising the C1 tier1 fallback until fixed.
        assert!(est.tier >= crate::tier::Tier::T2, "tier {:?}", est.tier);
    }

    #[test]
    fn octave_guard_no_false_halving_end_to_end() {
        // Real period estimate + moderate tic/toc asymmetry must not halve
        // (false-positive guard for the R4 backstop loosening).
        let cfg = SynthConfig {
            toc_gain: 0.5,
            snr_db: 30.0,
            ..SynthConfig::default()
        };
        let est = analyze(&cfg, BphMode::Auto, 0.0).expect("snapshot");
        assert_eq!(
            est.bph_nominal,
            Some(28_800),
            "false halving: {:?}",
            est.bph_detected
        );
    }

    #[test]
    fn config_validation_rejects_bad_lift_and_averaging() {
        for cfg in [
            AnalyzerConfig {
                lift_angle_deg: 5.0,
                ..AnalyzerConfig::default()
            },
            AnalyzerConfig {
                averaging_s: 100.0,
                ..AnalyzerConfig::default()
            },
            AnalyzerConfig {
                lift_angle_deg: f64::NAN,
                ..AnalyzerConfig::default()
            },
            AnalyzerConfig {
                ppm_correction: f64::NAN,
                ..AnalyzerConfig::default()
            },
        ] {
            assert!(matches!(
                Analyzer::new(cfg),
                Err(AnalyzerError::InvalidConfig { .. })
            ));
        }
    }

    #[test]
    fn incremental_equals_oneshot() {
        // Push identical data as (a) one shot and (b) 100 ms chunks with a
        // current_metrics() poll after every chunk (forcing many incremental
        // paths), then compare the FINAL snapshots field-by-field.
        let cfg = SynthConfig {
            beat_error_ms: 0.8,
            amplitude_deg: 270.0,
            rate_s_per_day: 12.0,
            snr_db: 30.0,
            ..SynthConfig::default()
        };
        let x = synthesize(&cfg).expect("valid synth config");
        let mk = || Analyzer::new(AnalyzerConfig::default()).expect("config");
        let mut a = mk();
        a.push_samples(&x);
        let one = a.current_metrics().expect("one-shot");
        let mut b = mk();
        let chunk = 4_800; // 100 ms
        let mut last = None;
        for c in x.chunks(chunk) {
            b.push_samples(c);
            last = b.current_metrics().or(last);
        }
        let inc = last.expect("incremental");
        assert_eq!(one.tier, inc.tier);
        assert_eq!(one.bph_nominal, inc.bph_nominal);
        let close = |x: Option<f64>, y: Option<f64>, tol: f64, what: &str| match (x, y) {
            (Some(x), Some(y)) => assert!((x - y).abs() < tol, "{what}: {x} vs {y}"),
            (a, b) => assert_eq!(a.is_some(), b.is_some(), "{what} presence"),
        };
        close(one.rate_s_per_day, inc.rate_s_per_day, 0.05, "rate");
        close(one.beat_error_ms, inc.beat_error_ms, 0.05, "beat error");
        close(one.amplitude_deg, inc.amplitude_deg, 1.0, "amplitude");
    }

    #[test]
    fn tape_events_are_ascending_and_bounded() {
        let x = synthesize(&SynthConfig::default()).expect("valid synth config");
        let mut a = Analyzer::new(AnalyzerConfig::default()).expect("config");
        a.push_samples(&x);
        let _ = a.current_metrics().expect("snapshot");
        let tape = a.tape_events();
        assert!(tape.len() > 200, "tape {} events", tape.len());
        for w in tape.windows(2) {
            assert!(w[1].t_drop_corr_s >= w[0].t_drop_corr_s);
        }
    }

    #[test]
    #[ignore = "perf harness; run in release with --ignored"]
    fn current_metrics_perf_bar() {
        let cfg = SynthConfig {
            duration_s: 40.0,
            ..SynthConfig::default()
        };
        let x = synthesize(&cfg).expect("valid synth config");
        let mut a = Analyzer::new(AnalyzerConfig::default()).expect("config");
        a.push_samples(&x);
        let _ = a.current_metrics(); // prime the cache
        let mut times = Vec::new();
        for _ in 0..50 {
            a.push_samples(&vec![0.0f32; 4_800]); // 100 ms of new data per poll
            let t0 = std::time::Instant::now();
            let _ = a.current_metrics();
            times.push(t0.elapsed());
        }
        times.sort();
        let p50 = times[times.len() / 2];
        assert!(
            p50 < std::time::Duration::from_millis(10),
            "p50 {p50:?} (bar: 10 ms release)"
        );
    }
}
