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

    #[test]
    fn replay_sidecar_values_take_effect_without_overrides() {
        // (a) no overrides → sidecar values take effect.
        // Synthesize a 28800 bph watch at rate 12.0 s/day.
        let cfg = chrona_dsp::synth::SynthConfig {
            beat_error_ms: 0.8,
            amplitude_deg: 270.0,
            rate_s_per_day: 12.0,
            snr_db: 30.0,
            ..Default::default()
        };
        let x = chrona_dsp::synth::synthesize(&cfg).unwrap();

        // Write a WAV with a sidecar: lift 38, ppm 25, bph "28800".
        let dir = tempfile::tempdir().unwrap();
        let wav_path = dir.path().join("with_sidecar.wav");
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

        // Write the sidecar
        use crate::sidecar::{SessionMeta, write_sidecar};
        let meta = SessionMeta {
            schema_version: 1,
            device_name: "test".to_string(),
            sample_rate_hz: cfg.sample_rate_hz,
            ppm_correction: 25.0,
            lift_angle_deg: 38.0,
            bph_mode: "28800".to_string(),
            position: None,
            started_unix_s: 0,
            app_version: "test".to_string(),
        };
        write_sidecar(&wav_path, &meta).unwrap();

        let reader = SessionReader::open(&wav_path).unwrap();
        assert!(reader.meta.is_some(), "sidecar should be present");

        // Replay without overrides — sidecar should be used.
        let result = replay(&reader, ReplayOverrides::default()).unwrap();
        let s = result.final_snapshot.expect("snapshot");
        assert!(
            s.calibrated,
            "ppm_correction should make snapshot calibrated"
        );

        // Amplitude should reflect lift 38: ≈ 270·38/52 ≈ 197 ± 6.
        if let Some(amp) = s.amplitude_deg {
            let expected = 270.0 * 38.0 / 52.0; // ≈ 197.3
            assert!(
                (amp - expected).abs() <= 6.0,
                "amplitude {amp} should be ~{expected} (within ±6) with lift 38; got {amp}"
            );
        }
    }

    #[test]
    fn replay_override_beats_sidecar() {
        // (b) an explicit override beats the sidecar.
        // Use the same 40 s synth signal for both tests (one 40 s reused).
        let cfg = chrona_dsp::synth::SynthConfig {
            beat_error_ms: 0.8,
            amplitude_deg: 270.0,
            rate_s_per_day: 12.0,
            snr_db: 30.0,
            ..Default::default()
        };
        let x = chrona_dsp::synth::synthesize(&cfg).unwrap();

        let dir = tempfile::tempdir().unwrap();
        let wav_path = dir.path().join("override_test.wav");
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

        // Write sidecar with lift 38, ppm 25, bph "28800".
        use crate::sidecar::{SessionMeta, write_sidecar};
        let meta = SessionMeta {
            schema_version: 1,
            device_name: "test".to_string(),
            sample_rate_hz: cfg.sample_rate_hz,
            ppm_correction: 25.0,
            lift_angle_deg: 38.0,
            bph_mode: "28800".to_string(),
            position: None,
            started_unix_s: 0,
            app_version: "test".to_string(),
        };
        write_sidecar(&wav_path, &meta).unwrap();

        let reader = SessionReader::open(&wav_path).unwrap();

        // Replay with lift override (52.0) — should override the sidecar's 38.0.
        let overrides = ReplayOverrides {
            lift_angle_deg: Some(52.0),
            ..Default::default()
        };
        let result = replay(&reader, overrides).unwrap();
        let s = result.final_snapshot.expect("snapshot");

        // Amplitude should be back to nominal ≈ 270 ± 5 (using default lift 52).
        if let Some(amp) = s.amplitude_deg {
            assert!(
                (amp - 270.0).abs() <= 5.0,
                "amplitude {amp} should be ~270 ± 5 with override lift 52; got {amp}"
            );
        }
    }
}
