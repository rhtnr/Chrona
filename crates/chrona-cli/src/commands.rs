use anyhow::{Context, bail};
use chrona_dsp::synth::{SynthConfig, synthesize};
use chrona_dsp::{Analyzer, AnalyzerConfig, BphMode};
use clap::Args;
use serde::Serialize;
use std::path::PathBuf;

#[derive(Args)]
pub struct SynthArgs {
    /// Output WAV path
    pub out: PathBuf,
    #[arg(long, default_value_t = 28_800)]
    pub bph: u32,
    /// Applied rate error in s/day (positive = fast)
    #[arg(long, default_value_t = 0.0, allow_negative_numbers = true)]
    pub rate: f64,
    #[arg(long, default_value_t = 0.0, allow_negative_numbers = true)]
    pub beat_error: f64,
    #[arg(long, default_value_t = 270.0)]
    pub amplitude: f64,
    #[arg(long, default_value_t = 52.0)]
    pub lift: f64,
    /// Drop-pulse peak vs noise RMS, dB
    #[arg(long, default_value_t = 30.0, allow_negative_numbers = true)]
    pub snr: f64,
    #[arg(long, default_value_t = 40.0)]
    pub duration: f64,
    #[arg(long, default_value_t = 1)]
    pub seed: u64,
    /// Write 16-bit PCM instead of 32-bit float
    #[arg(long)]
    pub pcm16: bool,
    /// Add mains hum at this frequency (e.g. 50 or 60)
    #[arg(long)]
    pub hum: Option<f64>,
}

pub fn run_synth(a: &SynthArgs) -> anyhow::Result<()> {
    let cfg = SynthConfig {
        bph: a.bph,
        rate_s_per_day: a.rate,
        beat_error_ms: a.beat_error,
        amplitude_deg: a.amplitude,
        lift_angle_deg: a.lift,
        snr_db: a.snr,
        duration_s: a.duration,
        seed: a.seed,
        hum_hz: a.hum,
        ..SynthConfig::default()
    };
    let samples = synthesize(&cfg).map_err(|e| anyhow::anyhow!(e))?;
    if a.pcm16 {
        crate::wav::write_mono_i16(&a.out, &samples, cfg.sample_rate_hz)
    } else {
        crate::wav::write_mono_f32(&a.out, &samples, cfg.sample_rate_hz)
    }
}

#[derive(Args)]
pub struct AnalyzeArgs {
    /// Input WAV file
    pub file: PathBuf,
    /// "auto", "free", or a numeric BPH like 28800
    #[arg(long, default_value = "auto")]
    pub bph: String,
    /// Audio-clock correction in ppm (spec §3.4)
    #[arg(long, default_value_t = 0.0)]
    pub ppm: f64,
    /// Lift angle in degrees (amplitude only; spec §2.3). Default 52.
    #[arg(long, default_value_t = 52.0)]
    pub lift: f64,
    /// Metrics averaging window, seconds (2-60).
    #[arg(long, default_value_t = 30.0)]
    pub averaging: f64,
    #[arg(long)]
    pub json: bool,
}

#[derive(Serialize)]
pub struct AnalyzeReport {
    pub status: &'static str, // "ok" | "no_beat"
    pub file: String,
    pub sample_rate_hz: f64,
    pub duration_s: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bph_detected: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bph_nominal: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rate_s_per_day: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub period_sigma_s: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window_s: Option<f64>,
    pub calibrated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tier: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rate_source: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub beat_error_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub amplitude_deg: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub amplitude_gate: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detection_ratio: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub onset_jitter_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unlocking_ratio: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mean_beat_snr_db: Option<f64>,
    pub clipped_samples: u64,
}

pub fn parse_bph_mode(s: &str) -> anyhow::Result<BphMode> {
    match s {
        "auto" => Ok(BphMode::Auto),
        "free" => Ok(BphMode::Free),
        n => Ok(BphMode::Fixed(
            n.parse::<u32>()
                .with_context(|| format!("invalid --bph '{n}'"))?,
        )),
    }
}

pub fn run_analyze(a: &AnalyzeArgs) -> anyhow::Result<AnalyzeReport> {
    let (samples, sr) = crate::wav::read_mono(&a.file)?;
    if samples.is_empty() {
        bail!("empty WAV: {}", a.file.display());
    }
    let mut analyzer = Analyzer::new(AnalyzerConfig {
        sample_rate_hz: sr,
        bph_mode: parse_bph_mode(&a.bph)?,
        ppm_correction: a.ppm,
        lift_angle_deg: a.lift,
        averaging_s: a.averaging,
    })?;
    analyzer.push_samples(&samples);
    let duration_s = samples.len() as f64 / sr;
    let clipped_samples = analyzer.clipped_samples();
    let base = |status| AnalyzeReport {
        status,
        file: a.file.display().to_string(),
        sample_rate_hz: sr,
        duration_s,
        bph_detected: None,
        bph_nominal: None,
        rate_s_per_day: None,
        period_sigma_s: None,
        window_s: None,
        calibrated: a.ppm != 0.0,
        tier: None,
        rate_source: None,
        beat_error_ms: None,
        amplitude_deg: None,
        amplitude_gate: None,
        detection_ratio: None,
        onset_jitter_ms: None,
        unlocking_ratio: None,
        mean_beat_snr_db: None,
        clipped_samples,
    };
    Ok(match analyzer.current_metrics() {
        None => base("no_beat"),
        Some(est) => AnalyzeReport {
            bph_detected: Some(est.bph_detected),
            bph_nominal: est.bph_nominal,
            rate_s_per_day: est.rate_s_per_day,
            period_sigma_s: Some(est.period.sigma_s),
            window_s: Some(est.period.window_s),
            calibrated: est.calibrated,
            tier: Some(match est.tier {
                chrona_dsp::Tier::T1 => "T1",
                chrona_dsp::Tier::T2 => "T2",
                chrona_dsp::Tier::T3 => "T3",
            }),
            rate_source: est.rate_source.map(|s| match s {
                chrona_dsp::RateSource::PeriodSlope => "period_slope",
                chrona_dsp::RateSource::UnlockingRegression => "unlocking_regression",
            }),
            beat_error_ms: est.beat_error_ms,
            amplitude_deg: est.amplitude_deg,
            amplitude_gate: est.quality.amplitude_gate.map(|g| format!("{g:?}")),
            detection_ratio: Some(est.quality.detection_ratio),
            onset_jitter_ms: est.quality.onset_jitter_ms,
            unlocking_ratio: Some(est.quality.unlocking_ratio),
            mean_beat_snr_db: est.quality.mean_beat_snr_db,
            clipped_samples: est.quality.clipped_samples,
            ..base("ok")
        },
    })
}

pub fn print_human(r: &AnalyzeReport) {
    println!(
        "File: {} ({} Hz, {:.1} s)",
        r.file, r.sample_rate_hz, r.duration_s
    );
    match r.status {
        "ok" => {
            match (r.bph_nominal, r.bph_detected) {
                (Some(nom), Some(det)) => println!("Beat rate: {nom} bph (detected {det:.1})"),
                (None, Some(det)) => {
                    println!("Beat rate: {det:.1} bph detected (no nominal reference)")
                }
                _ => {}
            }
            match r.rate_s_per_day {
                Some(rate) => {
                    let badge = if r.calibrated {
                        ""
                    } else {
                        "  [uncalibrated timebase]"
                    };
                    println!("Rate: {rate:+.1} s/d{badge}");
                }
                // Spec §3.1: no defensible nominal → no rate, with the reason.
                // In Fixed mode `bph_nominal` is pinned even when detection
                // deviated too far to trust, so the reason differs from Auto/Free
                // mode's true "nothing to compare against".
                None if r.bph_nominal.is_some() => println!(
                    "Rate: — (detected beat rate differs more than 3% from the pinned nominal)"
                ),
                None => println!("Rate: — (no nominal beat rate to compare against)"),
            }
            if let (Some(sig), Some(w)) = (r.period_sigma_s, r.window_s) {
                println!("Period σ: {:.1} µs over {w:.0} s window", sig * 1e6);
            }
            if let Some(t) = r.tier {
                println!("Signal tier: {t}");
            }
            match r.beat_error_ms {
                Some(be) => println!("Beat error: {be:.1} ms"),
                None => println!(
                    "Beat error: — (needs Tier 2: ≥60% of beats detected with ≤0.5 ms jitter)"
                ),
            }
            match r.amplitude_deg {
                Some(a) => println!("Amplitude: {a:.0}°"),
                None => match &r.amplitude_gate {
                    Some(g) => println!("Amplitude: — (gate: {g})"),
                    None => println!(
                        "Amplitude: — (needs Tier 3: unlocking pulse resolved — try a contact mic)"
                    ),
                },
            }
            if r.clipped_samples > 0 {
                println!(
                    "WARNING: {} clipped samples — reduce input gain",
                    r.clipped_samples
                );
            }
        }
        _ => println!(
            "No beat found — is a watch near the microphone? (try `chrona` Mic Doctor in a later milestone)"
        ),
    }
}

#[derive(Args)]
pub struct CalibrateArgs {
    /// WAV recording of a quartz watch (≥ 5 minutes)
    pub file: PathBuf,
    #[arg(long)]
    pub json: bool,
}

#[derive(Serialize)]
pub struct CalReport {
    pub ppm: f64,
    pub residual_ppm: f64,
    pub events: usize,
    pub duration_s: f64,
}

pub fn run_calibrate(a: &CalibrateArgs) -> anyhow::Result<CalReport> {
    let (samples, sr) = crate::wav::read_mono(&a.file)?;
    let r = chrona_dsp::cal::calibrate_quartz(&samples, sr)?;
    Ok(CalReport {
        ppm: r.ppm,
        residual_ppm: r.residual_ppm,
        events: r.events,
        duration_s: r.duration_s,
    })
}

pub fn print_cal_human(r: &CalReport) {
    println!(
        "Timebase: {:+.1} ppm (±{:.2}) from {} ticks over {:.0} s",
        r.ppm, r.residual_ppm, r.events, r.duration_s
    );
    println!("Use it: chrona analyze <watch.wav> --ppm {:.1}", r.ppm);
}

#[derive(Args)]
pub struct VerifyArgs {
    /// Directory containing *.json expectations next to their WAV files
    pub dir: PathBuf,
}

#[derive(serde::Deserialize)]
struct Expectation {
    file: String,
    #[serde(default = "default_bph_mode")]
    bph: String,
    #[serde(default)]
    ppm: f64,
    #[serde(default = "default_lift")]
    lift: f64,
    expect_rate_s_per_day: f64,
    tol_rate: f64,
    #[serde(default)]
    expect_beat_error_ms: Option<f64>,
    #[serde(default)]
    tol_beat_error: Option<f64>,
    #[serde(default)]
    expect_amplitude_deg: Option<f64>,
    #[serde(default)]
    tol_amplitude: Option<f64>,
}

fn default_bph_mode() -> String {
    "auto".into()
}

fn default_lift() -> f64 {
    52.0
}

/// Returns the number of failures.
pub fn run_verify(a: &VerifyArgs) -> anyhow::Result<usize> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(&a.dir)
        .with_context(|| format!("read dir {}", a.dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    entries.sort();
    if entries.is_empty() {
        println!(
            "verify: no expectations in {} — nothing to do",
            a.dir.display()
        );
        return Ok(0);
    }
    let mut failures = 0usize;
    for path in &entries {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
        let exp: Expectation =
            serde_json::from_str(&text).with_context(|| format!("parse {}", path.display()))?;
        let report = run_analyze(&AnalyzeArgs {
            file: a.dir.join(&exp.file),
            bph: exp.bph.clone(),
            ppm: exp.ppm,
            lift: exp.lift,
            averaging: 30.0,
            json: false,
        })?;
        let mut verdict = String::new();
        let mut passed = true;

        // Rate check
        match report.rate_s_per_day {
            Some(rate) if (rate - exp.expect_rate_s_per_day).abs() <= exp.tol_rate => {
                verdict.push_str(&format!(
                    "PASS  rate {rate:+.2} s/d (want {:+.2} ± {})",
                    exp.expect_rate_s_per_day, exp.tol_rate
                ));
            }
            Some(rate) => {
                passed = false;
                verdict.push_str(&format!(
                    "FAIL  rate {rate:+.2} s/d (want {:+.2} ± {})",
                    exp.expect_rate_s_per_day, exp.tol_rate
                ));
            }
            None => {
                passed = false;
                verdict.push_str(&format!("FAIL  no rate (status {})", report.status));
            }
        };

        // Beat error check. Presence of `expect_beat_error_ms` alone triggers
        // the check; a missing `tol_beat_error` defaults to 0.15 ms (see
        // fixtures/README's schema).
        if let Some(expect_be) = exp.expect_beat_error_ms {
            let tol_be = exp.tol_beat_error.unwrap_or(0.15);
            match report.beat_error_ms {
                Some(be) if (be - expect_be).abs() <= tol_be => {
                    verdict.push_str(&format!(" be={be:.2}"));
                }
                Some(be) => {
                    passed = false;
                    verdict.push_str(&format!(
                        " FAIL be {be:.2} (want {expect_be:.2} ± {tol_be})"
                    ));
                }
                None => {
                    passed = false;
                    let tier = report.tier.unwrap_or("unknown");
                    verdict.push_str(&format!(" FAIL no beat_error (tier {tier})"));
                }
            }
        }

        // Amplitude check. Presence of `expect_amplitude_deg` alone triggers
        // the check; a missing `tol_amplitude` defaults to 10.0° (see
        // fixtures/README's schema).
        if let Some(expect_amp) = exp.expect_amplitude_deg {
            let tol_amp = exp.tol_amplitude.unwrap_or(10.0);
            match report.amplitude_deg {
                Some(amp) if (amp - expect_amp).abs() <= tol_amp => {
                    verdict.push_str(&format!(" amp={amp:.0}"));
                }
                Some(amp) => {
                    passed = false;
                    verdict.push_str(&format!(
                        " FAIL amp {amp:.0} (want {expect_amp:.0} ± {tol_amp})"
                    ));
                }
                None => {
                    passed = false;
                    let tier = report.tier.unwrap_or("unknown");
                    verdict.push_str(&format!(" FAIL no amplitude (tier {tier})"));
                }
            }
        }

        if !passed {
            failures += 1;
        }
        println!("{}: {verdict}", exp.file);
    }
    Ok(failures)
}
