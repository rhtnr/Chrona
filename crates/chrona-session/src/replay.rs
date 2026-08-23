//! Replay a recorded session through a fresh `chrona_dsp::Analyzer`,
//! reproducing what a live `chrona analyze` would have reported (spec M3).

use crate::reader::SessionReader;
use anyhow::{Context, Result};
use chrona_dsp::{Analyzer, AnalyzerConfig, BphMode, MetricsSnapshot};

/// Analyzer knobs to override for this replay. Any field left `None` falls
/// back to the session's sidecar value, and — if there is no sidecar, or
/// the sidecar doesn't cover that field — to `AnalyzerConfig::default()`.
#[derive(Debug, Clone, Copy, Default)]
pub struct ReplayOverrides {
    pub lift_angle_deg: Option<f64>,
    pub ppm_correction: Option<f64>,
    pub bph_mode: Option<BphMode>,
}

#[derive(Debug)]
pub struct ReplayResult {
    pub final_snapshot: Option<MetricsSnapshot>,
    pub tape: Vec<chrona_dsp::analyzer::TapeEvent>,
    pub duration_s: f64,
}

/// Mirrors the CLI's `parse_bph_mode` (`chrona-cli/src/commands.rs`):
/// duplicated rather than imported because `chrona-cli` is a bin crate, not
/// a linkable library.
fn parse_bph_mode(s: &str) -> Result<BphMode> {
    match s {
        "auto" => Ok(BphMode::Auto),
        "free" => Ok(BphMode::Free),
        n => {
            Ok(BphMode::Fixed(n.parse::<u32>().with_context(|| {
                format!("invalid sidecar bph_mode '{n}'")
            })?))
        }
    }
}

pub fn replay(reader: &SessionReader, overrides: ReplayOverrides) -> Result<ReplayResult> {
    let sidecar = reader.meta.as_ref();
    let defaults = AnalyzerConfig::default();

    let bph_mode = match overrides.bph_mode {
        Some(m) => m,
        None => match sidecar {
            Some(m) => parse_bph_mode(&m.bph_mode)?,
            None => defaults.bph_mode,
        },
    };
    let ppm_correction = overrides
        .ppm_correction
        .or_else(|| sidecar.map(|m| m.ppm_correction))
        .unwrap_or(defaults.ppm_correction);
    let lift_angle_deg = overrides
        .lift_angle_deg
        .or_else(|| sidecar.map(|m| m.lift_angle_deg))
        .unwrap_or(defaults.lift_angle_deg);

    let mut analyzer = Analyzer::new(AnalyzerConfig {
        sample_rate_hz: reader.sample_rate_hz,
        bph_mode,
        ppm_correction,
        lift_angle_deg,
        averaging_s: defaults.averaging_s,
    })?;
    analyzer.push_samples(&reader.samples);
    let final_snapshot = analyzer.current_metrics();
    let tape = analyzer.tape_events();
    let duration_s = reader.samples.len() as f64 / reader.sample_rate_hz;
    Ok(ReplayResult {
        final_snapshot,
        tape,
        duration_s,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replay_reproduces_analyze_results() {
        let cfg = chrona_dsp::synth::SynthConfig {
            beat_error_ms: 0.8,
            amplitude_deg: 270.0,
            rate_s_per_day: 12.0,
            snr_db: 30.0,
            ..Default::default()
        };
        let x = chrona_dsp::synth::synthesize(&cfg).unwrap();

        // Write a bare WAV (no sidecar) via hound directly, mirroring
        // SessionWriter's own WAV spec, so replay must fall through to
        // AnalyzerConfig::default() for every knob.
        let dir = tempfile::tempdir().unwrap();
        let wav_path = dir.path().join("no_sidecar.wav");
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: cfg.sample_rate_hz as u32,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let mut w = hound::WavWriter::create(&wav_path, spec).unwrap();
        for &s in &x {
            w.write_sample(s).unwrap();
        }
        w.finalize().unwrap();

        let reader = SessionReader::open(&wav_path).unwrap();
        assert!(reader.meta.is_none(), "no sidecar was written");

        let result = replay(&reader, ReplayOverrides::default()).unwrap();
        let s = result.final_snapshot.expect("snapshot");
        assert_eq!(s.bph_nominal, Some(28_800));
        assert!((s.beat_error_ms.unwrap() - 0.8).abs() <= 0.1);
        assert!((s.amplitude_deg.unwrap() - 270.0).abs() <= 5.0);
        assert!(result.tape.len() > 200);
        assert!((result.duration_s - 40.0).abs() < 0.1);
    }
}
