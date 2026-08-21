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
    crate::wav::write_mono_f32(&a.out, &samples, cfg.sample_rate_hz)
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
    })?;
    analyzer.push_samples(&samples);
    let duration_s = samples.len() as f64 / sr;
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
    };
    Ok(match analyzer.current() {
        None => base("no_beat"),
        Some(est) => AnalyzeReport {
            bph_detected: Some(est.bph_detected),
            bph_nominal: est.bph_nominal,
            rate_s_per_day: est.seconds_per_day,
            period_sigma_s: Some(est.period.sigma_s),
            window_s: Some(est.period.window_s),
            calibrated: est.calibrated,
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
                None => println!("Rate: — (no nominal beat rate to compare against)"),
            }
            if let (Some(sig), Some(w)) = (r.period_sigma_s, r.window_s) {
                println!("Period σ: {:.1} µs over {w:.0} s window", sig * 1e6);
            }
        }
        _ => println!(
            "No beat found — is a watch near the microphone? (try `chrona` Mic Doctor in a later milestone)"
        ),
    }
}
