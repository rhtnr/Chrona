//! Deterministic synthetic watch-signal generator (spec §10).
//! Ticks are 3 damped sine bursts (unlocking / impulse / drop, spec §2.1);
//! unlocking→drop spacing comes from inverting the amplitude formula (spec §2.3).

#[derive(Debug, thiserror::Error)]
pub enum SynthError {
    #[error(
        "amplitude {amplitude_deg}° is below lift/2 ({lift_angle_deg}°/2); the amplitude formula is not invertible there"
    )]
    AmplitudeBelowLiftDomain {
        amplitude_deg: f64,
        lift_angle_deg: f64,
    },
    #[error("invalid synth config: {reason}")]
    InvalidConfig { reason: String },
}

/// xorshift64* — tiny deterministic PRNG; no external dependency (spec §10 determinism).
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed.max(1)) // xorshift state must be nonzero
    }
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    /// Uniform in [-1, 1).
    pub fn next_f32(&mut self) -> f32 {
        ((self.next_u64() >> 40) as f32 / (1u64 << 24) as f32) * 2.0 - 1.0
    }
}

pub struct SynthConfig {
    pub sample_rate_hz: f64,
    pub duration_s: f64,
    pub bph: u32,
    pub rate_s_per_day: f64,
    pub beat_error_ms: f64,
    pub amplitude_deg: f64,
    pub lift_angle_deg: f64,
    /// Drop-pulse peak amplitude vs noise RMS, in dB.
    pub snr_db: f64,
    pub hum_hz: Option<f64>,
    pub seed: u64,
    /// Gain applied to every burst of odd-index (toc) beats; 1.0 = symmetric. Values ≪ 1 model extreme tic/toc asymmetry (octave-guard testing).
    pub toc_gain: f64,
}

impl Default for SynthConfig {
    fn default() -> Self {
        SynthConfig {
            sample_rate_hz: 48_000.0,
            duration_s: 40.0,
            bph: 28_800,
            rate_s_per_day: 0.0,
            beat_error_ms: 0.0,
            amplitude_deg: 270.0,
            lift_angle_deg: 52.0,
            snr_db: 30.0,
            hum_hz: None,
            seed: 1,
            toc_gain: 1.0,
        }
    }
}

/// Spec §2.3 amplitude formula solved for Δt: `Δt = (T_osc/π)·asin(L/(2A))`.
pub fn unlock_to_drop_dt_s(amplitude_deg: f64, lift_angle_deg: f64, t_osc_s: f64) -> f64 {
    (t_osc_s / std::f64::consts::PI) * (lift_angle_deg / (2.0 * amplitude_deg)).asin()
}

/// Add one damped sine burst at time `t0` (seconds).
fn add_burst(x: &mut [f32], sr: f64, t0: f64, amp: f64, f_hz: f64, tau_s: f64) {
    if t0 < 0.0 {
        return;
    }
    let start = (t0 * sr) as usize;
    let len = (tau_s * 6.0 * sr) as usize; // ~6τ ≈ −52 dB tail
    for i in 0..len {
        let Some(s) = x.get_mut(start + i) else { break };
        let t = i as f64 / sr;
        *s += (amp * (-t / tau_s).exp() * (2.0 * std::f64::consts::PI * f_hz * t).sin()) as f32;
    }
}

pub fn synthesize(cfg: &SynthConfig) -> Result<Vec<f32>, SynthError> {
    // Every f64 knob must be finite: NaN compares false everywhere, which would
    // silently defeat the range checks below and the beat-grid loop's exit test,
    // hanging `synthesize` forever instead of erroring out.
    let finite_fields = [
        ("sample_rate_hz", cfg.sample_rate_hz),
        ("duration_s", cfg.duration_s),
        ("rate_s_per_day", cfg.rate_s_per_day),
        ("beat_error_ms", cfg.beat_error_ms),
        ("amplitude_deg", cfg.amplitude_deg),
        ("lift_angle_deg", cfg.lift_angle_deg),
        ("snr_db", cfg.snr_db),
        ("hum_hz", cfg.hum_hz.unwrap_or(0.0)),
        ("toc_gain", cfg.toc_gain),
    ];
    for (name, v) in finite_fields {
        if !v.is_finite() {
            return Err(SynthError::InvalidConfig {
                reason: format!("{name} must be finite, got {v}"),
            });
        }
    }
    // Both are already confirmed finite above, so a direct comparison is safe
    // (and, unlike a negated one, doesn't trip clippy::neg_cmp_op_on_partial_ord).
    if cfg.sample_rate_hz <= 0.0 {
        return Err(SynthError::InvalidConfig {
            reason: format!("sample_rate_hz must be > 0, got {}", cfg.sample_rate_hz),
        });
    }
    if cfg.duration_s < 0.0 {
        return Err(SynthError::InvalidConfig {
            reason: format!("duration_s must be >= 0, got {}", cfg.duration_s),
        });
    }
    if cfg.bph < 1 {
        return Err(SynthError::InvalidConfig {
            reason: format!("bph must be >= 1, got {}", cfg.bph),
        });
    }

    if cfg.toc_gain <= 0.0 {
        return Err(SynthError::InvalidConfig {
            reason: format!("toc_gain {} must be > 0", cfg.toc_gain),
        });
    }

    if cfg.amplitude_deg < cfg.lift_angle_deg / 2.0 {
        return Err(SynthError::AmplitudeBelowLiftDomain {
            amplitude_deg: cfg.amplitude_deg,
            lift_angle_deg: cfg.lift_angle_deg,
        });
    }

    let sr = cfg.sample_rate_hz;
    let n = (cfg.duration_s * sr) as usize;
    let mut x = vec![0.0f32; n];
    let mut rng = Rng::new(cfg.seed);

    // Rate-adjusted beat period (spec §2.3: T̂ = T_nom·(1 − rate/86400)).
    let t_beat = crate::bph::t_beat_s(cfg.bph) * (1.0 - cfg.rate_s_per_day / 86_400.0);
    // Finite by construction (bph >= 1 and rate_s_per_day is finite, both
    // confirmed above), so this direct comparison is safe.
    if t_beat <= 0.0 {
        return Err(SynthError::InvalidConfig {
            reason: format!(
                "rate {} s/d makes the beat period non-positive",
                cfg.rate_s_per_day
            ),
        });
    }
    let t_osc = 2.0 * t_beat;
    let dt = unlock_to_drop_dt_s(cfg.amplitude_deg, cfg.lift_angle_deg, t_osc);
    let be = cfg.beat_error_ms / 1000.0;

    let noise_rms = 0.02f64;
    let tick_amp = noise_rms * 10f64.powf(cfg.snr_db / 20.0);

    // Tick clusters (spec §2.1): the *drop* lands on the beat grid (it is the
    // detection anchor); unlocking precedes it by dt. Beat error shifts
    // alternate beats by ±be/2 so tic→toc and toc→tick intervals differ by 2·(be/2).
    let mut k = 0usize;
    loop {
        #[allow(clippy::manual_is_multiple_of)]
        let shift = if k % 2 == 0 { be / 2.0 } else { -be / 2.0 };
        let t_drop = (k as f64 + 1.0) * t_beat + shift;
        if t_drop + 0.02 >= cfg.duration_s {
            break;
        }
        let t_unlock = t_drop - dt;
        let impulse_jitter = 0.15 * dt * rng.next_f32() as f64;
        let t_impulse = t_unlock + 0.4 * dt + impulse_jitter;
        #[allow(clippy::manual_is_multiple_of)]
        let g = if k % 2 == 0 { 1.0 } else { cfg.toc_gain };
        add_burst(&mut x, sr, t_unlock, tick_amp * 0.35 * g, 5_200.0, 0.0008);
        add_burst(&mut x, sr, t_impulse, tick_amp * 0.25 * g, 6_500.0, 0.0006);
        add_burst(&mut x, sr, t_drop, tick_amp * g, 7_800.0, 0.0012);
        k += 1;
    }

    // White noise at the configured floor. Uniform [−1,1) has RMS 1/√3, so scale
    // by √3 to make `noise_rms` the actual RMS.
    let noise_gain = noise_rms * 3f64.sqrt();
    for s in x.iter_mut() {
        *s += (noise_gain * rng.next_f32() as f64) as f32;
    }

    // Optional mains hum + 2 harmonics (spec §3.2 hum detection needs this in fixtures).
    if let Some(f) = cfg.hum_hz {
        for (i, s) in x.iter_mut().enumerate() {
            let t = i as f64 / sr;
            let hum = 0.5
                * noise_rms
                * ((2.0 * std::f64::consts::PI * f * t).sin()
                    + 0.6 * (2.0 * std::f64::consts::PI * 2.0 * f * t).sin()
                    + 0.4 * (2.0 * std::f64::consts::PI * 3.0 * f * t).sin());
            *s += hum as f32;
        }
    }

    // Normalize to ≤ 0.9 peak, preserving ratios (never clip the synthetic ADC).
    let peak = x.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    if peak > 0.9 {
        let g = 0.9 / peak;
        for s in x.iter_mut() {
            *s *= g;
        }
    }
    Ok(x)
}

/// 1 Hz quartz stepper reference for timebase calibration (spec §3.4).
/// The tick grid's true period is `1.0 · (1 + ppm_offset/1e6)` in nominal-clock
/// seconds — i.e. what a perfect quartz looks like through an ADC that is
/// `ppm_offset` ppm fast.
pub fn synthesize_quartz(
    duration_s: f64,
    sample_rate_hz: f64,
    ppm_offset: f64,
    snr_db: f64,
    seed: u64,
) -> Result<Vec<f32>, SynthError> {
    for (name, v) in [
        ("duration_s", duration_s),
        ("sample_rate_hz", sample_rate_hz),
        ("ppm_offset", ppm_offset),
        ("snr_db", snr_db),
    ] {
        if !v.is_finite() {
            return Err(SynthError::InvalidConfig {
                reason: format!("{name} is not finite"),
            });
        }
    }
    if sample_rate_hz <= 0.0 || duration_s < 0.0 {
        return Err(SynthError::InvalidConfig {
            reason: "sample_rate_hz must be > 0 and duration_s >= 0".into(),
        });
    }
    let sr = sample_rate_hz;
    let n = (duration_s * sr) as usize;
    let mut x = vec![0.0f32; n];
    let mut rng = Rng::new(seed);
    let noise_rms = 0.02f64;
    let tick_amp = noise_rms * 10f64.powf(snr_db / 20.0);
    let period = 1.0 * (1.0 + ppm_offset / 1e6);
    if period <= 0.0 {
        return Err(SynthError::InvalidConfig {
            reason: format!("ppm_offset {ppm_offset} makes the tick period non-positive"),
        });
    }
    let mut k = 1u64;
    loop {
        let t = k as f64 * period;
        if t + 0.02 >= duration_s {
            break;
        }
        add_burst(&mut x, sr, t, tick_amp, 4_000.0, 0.0015);
        k += 1;
    }
    // White noise at the configured floor. Uniform [−1,1) has RMS 1/√3, so scale
    // by √3 to make `noise_rms` the actual RMS.
    let noise_gain = noise_rms * 3f64.sqrt();
    for s in x.iter_mut() {
        *s += (noise_gain * rng.next_f32() as f64) as f32;
    }
    let peak = x.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    if peak > 0.9 {
        let g = 0.9 / peak;
        for s in x.iter_mut() {
            *s *= g;
        }
    }
    Ok(x)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_for_seed() {
        let cfg = SynthConfig {
            duration_s: 2.0,
            ..SynthConfig::default()
        };
        assert_eq!(
            synthesize(&cfg).expect("valid synth config"),
            synthesize(&cfg).expect("valid synth config")
        );
        let other = SynthConfig { seed: 2, ..cfg };
        assert_ne!(
            synthesize(&cfg).expect("valid synth config"),
            synthesize(&other).expect("valid synth config")
        );
    }

    #[test]
    fn dt_formula_matches_spec_worked_example() {
        // Spec §2.3: 28,800 bph, L = 52°, A ≈ 296° → Δt ≈ 7 ms.
        let dt = unlock_to_drop_dt_s(296.0, 52.0, 0.250);
        assert!((dt - 0.007).abs() < 0.0001, "dt = {dt}");
    }

    #[test]
    fn drop_pulses_land_on_the_configured_beat_grid() {
        // Loud drop pulses must appear once per beat, at the rate-adjusted period.
        let cfg = SynthConfig {
            duration_s: 6.0,
            rate_s_per_day: 30.0, // fast watch
            snr_db: 60.0,         // near-clean so peaks dominate
            ..SynthConfig::default()
        };
        let x = synthesize(&cfg).expect("valid synth config");
        let sr = cfg.sample_rate_hz;
        let t_beat = crate::bph::t_beat_s(cfg.bph) * (1.0 - cfg.rate_s_per_day / 86_400.0);
        // Find the absolute peak within each expected beat window; successive peak
        // spacing must equal t_beat within 1 ms (drop bursts are the loudest, spec §2.1).
        let n_beats = 20;
        let mut peaks = Vec::new();
        for k in 0..n_beats {
            let lo = ((k as f64 + 0.2) * t_beat * sr) as usize;
            let hi = ((k as f64 + 1.2) * t_beat * sr) as usize;
            let (idx, _) = x[lo..hi]
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
                .unwrap();
            peaks.push((lo + idx) as f64 / sr);
        }
        for w in peaks.windows(2) {
            assert!(
                ((w[1] - w[0]) - t_beat).abs() < 1e-3,
                "spacing {}",
                w[1] - w[0]
            );
        }
    }

    #[test]
    fn output_is_normalized_and_finite() {
        let cfg = SynthConfig {
            duration_s: 2.0,
            snr_db: 0.0,
            ..SynthConfig::default()
        };
        let x = synthesize(&cfg).expect("valid synth config");
        assert!(x.iter().all(|s| s.is_finite()));
        let peak = x.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(peak <= 0.9 + 1e-6, "peak {peak}");
    }

    #[test]
    fn amplitude_below_lift_domain_is_an_error() {
        let cfg = SynthConfig {
            amplitude_deg: 20.0,
            lift_angle_deg: 52.0,
            ..SynthConfig::default()
        };
        assert!(synthesize(&cfg).is_err());
    }

    #[test]
    fn amplitude_at_exact_domain_boundary_is_ok() {
        let cfg = SynthConfig {
            amplitude_deg: 26.0,
            lift_angle_deg: 52.0,
            ..SynthConfig::default()
        };
        let x = synthesize(&cfg).expect("valid synth config");
        assert!(x.iter().all(|s| s.is_finite()));
    }

    #[test]
    fn nan_rate_is_an_error() {
        let cfg = SynthConfig {
            rate_s_per_day: f64::NAN,
            ..SynthConfig::default()
        };
        assert!(synthesize(&cfg).is_err());
    }

    #[test]
    fn rate_that_stalls_the_beat_grid_is_an_error() {
        // rate_s_per_day = 86_400 drives t_beat to exactly zero (never mind
        // faster-than-that, which would go negative) — the beat loop's exit
        // condition would never be reached.
        let cfg = SynthConfig {
            rate_s_per_day: 86_400.0,
            ..SynthConfig::default()
        };
        assert!(synthesize(&cfg).is_err());
    }

    #[test]
    fn infinite_duration_is_an_error() {
        let cfg = SynthConfig {
            duration_s: f64::INFINITY,
            ..SynthConfig::default()
        };
        assert!(synthesize(&cfg).is_err());
    }

    #[test]
    fn toc_gain_scales_alternate_beats() {
        let strong = SynthConfig {
            duration_s: 4.0,
            snr_db: 50.0,
            ..SynthConfig::default()
        };
        let weak = SynthConfig {
            toc_gain: 0.1,
            ..strong
        };
        let (xs, xw) = (synthesize(&strong).unwrap(), synthesize(&weak).unwrap());
        let sr = strong.sample_rate_hz;
        let t_beat = crate::bph::t_beat_s(strong.bph);
        // Peak amplitude around beat 4 (k=3, toc: k%2==1 with drops at (k+1)·t_beat
        // ⇒ beat index k=3 → drop at 4·t_beat) must shrink ~10x; beat 5 (tic) must not.
        let peak_near = |x: &[f32], t: f64| {
            let (lo, hi) = (((t - 0.01) * sr) as usize, ((t + 0.01) * sr) as usize);
            x[lo..hi].iter().fold(0.0f32, |m, s| m.max(s.abs()))
        };
        let toc_t = 4.0 * t_beat;
        let tic_t = 5.0 * t_beat;
        assert!(
            peak_near(&xw, toc_t) < 0.25 * peak_near(&xs, toc_t),
            "toc not attenuated"
        );
        assert!(
            peak_near(&xw, tic_t) > 0.8 * peak_near(&xs, tic_t),
            "tic wrongly attenuated"
        );
    }

    #[test]
    fn toc_gain_must_be_finite_and_positive() {
        let bad = SynthConfig {
            toc_gain: f64::NAN,
            ..SynthConfig::default()
        };
        assert!(synthesize(&bad).is_err());
        let bad2 = SynthConfig {
            toc_gain: 0.0,
            ..SynthConfig::default()
        };
        assert!(synthesize(&bad2).is_err());
    }

    #[test]
    fn quartz_ticks_land_on_the_ppm_stretched_grid() {
        let x = synthesize_quartz(20.0, 48_000.0, 100.0, 40.0, 3).unwrap();
        let sr = 48_000.0;
        let period = 1.0 * (1.0 + 100.0 / 1e6);
        for k in 1..=18u32 {
            let t = k as f64 * period;
            let (lo, hi) = (((t - 0.02) * sr) as usize, ((t + 0.02) * sr) as usize);
            let (idx, v) = x[lo..hi]
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
                .unwrap();
            assert!(v.abs() > 0.05, "tick {k} missing");
            let t_found = (lo + idx) as f64 / sr;
            assert!((t_found - t).abs() < 5e-3, "tick {k} at {t_found} want {t}");
        }
        assert!(synthesize_quartz(f64::NAN, 48_000.0, 0.0, 30.0, 1).is_err());
    }

    #[test]
    fn extreme_negative_ppm_is_an_error() {
        assert!(synthesize_quartz(20.0, 48_000.0, -1_000_000.0, 30.0, 1).is_err());
        assert!(synthesize_quartz(20.0, 48_000.0, -2_000_000.0, 30.0, 1).is_err());
    }
}
