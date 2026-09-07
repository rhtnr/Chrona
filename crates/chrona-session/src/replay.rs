//! Replay a recorded session through a fresh `chrona_dsp::Analyzer`,
//! reproducing what a live `chrona analyze` would have reported (spec M3).

use crate::reader::SessionReader;
use crate::sanitize::{sanitize_lift, sanitize_ppm};
use crate::sidecar::SessionMeta;
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

/// Resolves the `ppm_correction`/`lift_angle_deg` knobs `replay` builds its
/// `Analyzer` from: an explicit override is trusted as-is (the caller's own
/// deliberate intent — not sidecar data, so not sanitized); absent that, a
/// sidecar value is run through [`sanitize_ppm`]/[`sanitize_lift`] first —
/// a hand-edited (or otherwise corrupted) sidecar can carry a value outside
/// `Analyzer::new`'s valid domain, and this is `replay`'s M4 Task 5
/// alignment with the identical clamp `chrona-app`'s engine and
/// `ControlsState` already apply to the same two fields (see
/// `crate::sanitize`) — replacing the old behavior of forwarding the raw
/// value and letting `Analyzer::new` hard-fail the *entire* replay over one
/// bad field. With neither an override nor a sidecar, falls back to
/// `defaults` (always analyzer-valid by construction, so never sanitized).
///
/// Split out from `replay` (rather than inlined) so this — the actual
/// "replay-path alignment" this task is about — is directly unit-testable
/// without a synthesized WAV or a full `Analyzer` run.
fn resolve_ppm_lift(
    overrides: &ReplayOverrides,
    sidecar: Option<&SessionMeta>,
    defaults: &AnalyzerConfig,
) -> (f64, f64) {
    let ppm_correction = overrides.ppm_correction.unwrap_or_else(|| {
        sidecar
            .map(|m| sanitize_ppm(m.ppm_correction))
            .unwrap_or(defaults.ppm_correction)
    });
    let lift_angle_deg = overrides.lift_angle_deg.unwrap_or_else(|| {
        sidecar
            .map(|m| sanitize_lift(m.lift_angle_deg))
            .unwrap_or(defaults.lift_angle_deg)
    });
    (ppm_correction, lift_angle_deg)
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
    let (ppm_correction, lift_angle_deg) = resolve_ppm_lift(&overrides, sidecar, &defaults);

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
            watch: None,
            summary: None,
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
            watch: None,
            summary: None,
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

    /// M4 Task 5 (replay-path alignment): a hand-edited (or otherwise
    /// corrupted) sidecar can carry a `lift_angle_deg` outside
    /// `Analyzer::new`'s `[10, 90]` domain. Before this task, `replay`
    /// passed the raw sidecar value straight into `AnalyzerConfig` and let
    /// `Analyzer::new`'s `?` hard-fail the *entire* replay -- no plot, no
    /// tape, nothing -- over one bad field, unlike `chrona-app`'s engine
    /// (`SourceRuntime::build`), which has always clamped the same field.
    /// This test pins the aligned behavior: `replay` now clamps via
    /// `chrona_session::sanitize::sanitize_lift` instead, exactly like the
    /// engine, so the replay still succeeds (lift 5.0 -> clamped to 10.0;
    /// verified indirectly through the amplitude it produces, since
    /// `ReplayResult` doesn't expose the applied config directly).
    ///
    /// FLIPPED from a hard-Err expectation: prior to the Step 1 fix in this
    /// task, `replay(&reader, ReplayOverrides::default())` here returned
    /// `Err(..)` (confirmed RED via a real `cargo test` run against the
    /// pre-fix code -- see task-5-report.md); this now asserts `Ok(..)` with
    /// the clamped lift's effect instead.
    #[test]
    fn replay_sidecar_out_of_range_lift_clamps_instead_of_erroring() {
        let cfg = chrona_dsp::synth::SynthConfig {
            beat_error_ms: 0.8,
            amplitude_deg: 270.0,
            rate_s_per_day: 12.0,
            snr_db: 30.0,
            ..Default::default()
        };
        let x = chrona_dsp::synth::synthesize(&cfg).unwrap();

        let dir = tempfile::tempdir().unwrap();
        let wav_path = dir.path().join("out_of_range_lift.wav");
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

        use crate::sidecar::{SessionMeta, write_sidecar};
        let meta = SessionMeta {
            schema_version: 1,
            device_name: "test".to_string(),
            sample_rate_hz: cfg.sample_rate_hz,
            ppm_correction: 0.0,
            lift_angle_deg: 5.0, // out of Analyzer::new's [10, 90] domain
            bph_mode: "auto".to_string(),
            position: None,
            started_unix_s: 0,
            app_version: "test".to_string(),
            watch: None,
            summary: None,
        };
        write_sidecar(&wav_path, &meta).unwrap();

        let reader = SessionReader::open(&wav_path).unwrap();

        let result = replay(&reader, ReplayOverrides::default());
        let result = result
            .expect("replay must succeed on an out-of-range sidecar lift (clamped, not hard-Err)");
        let s = result.final_snapshot.expect("snapshot");

        // Lift clamped 5.0 -> 10.0: amplitude ~= 270*10/52 ~= 51.9 +/- 6.
        if let Some(amp) = s.amplitude_deg {
            let expected = 270.0 * 10.0 / 52.0;
            assert!(
                (amp - expected).abs() <= 6.0,
                "amplitude {amp} should be ~{expected} (within +/-6) with lift clamped to 10; got {amp}"
            );
        }
    }

    /// A minimal `SessionMeta` for `resolve_ppm_lift` tests below — only
    /// `ppm_correction`/`lift_angle_deg` vary per test; the rest are
    /// irrelevant filler.
    fn meta_with(ppm_correction: f64, lift_angle_deg: f64) -> SessionMeta {
        SessionMeta {
            schema_version: 1,
            device_name: "test".to_string(),
            sample_rate_hz: 48_000.0,
            ppm_correction,
            lift_angle_deg,
            bph_mode: "auto".to_string(),
            position: None,
            started_unix_s: 0,
            app_version: "test".to_string(),
            watch: None,
            summary: None,
        }
    }

    /// M4 Task 5 (replay-path alignment), the precise unit-level pin for
    /// `resolve_ppm_lift`'s sidecar branch: an out-of-range-but-*finite*
    /// sidecar `ppm_correction` (9999.0 — a value `serde_json` can and does
    /// deserialize just fine, unlike a genuinely non-finite one; see this
    /// test's own doc note below) clamps to the ±500 sanity range, and an
    /// out-of-range `lift_angle_deg` clamps into `Analyzer::new`'s
    /// `[10, 90]` domain — both via `sanitize_ppm`/`sanitize_lift`
    /// (`crate::sanitize`), exactly like `chrona-app`'s engine and
    /// `ControlsState`.
    ///
    /// A true *non-finite* sidecar value (NaN/±Infinity) was the other
    /// candidate for this pin, but is unreachable through any real,
    /// on-disk sidecar: JSON has no NaN/Infinity literal, and — verified
    /// directly against this workspace's actual `serde_json` version via a
    /// throwaway probe during development (not kept, per report — this
    /// comment records the finding instead) — a numeral whose value
    /// overflows `f64::MAX` (e.g. `1e400`) does not silently parse to
    /// `f64::INFINITY` the way `str::parse::<f64>` does; `serde_json`
    /// rejects it outright with a "number out of range" parse error, so
    /// the *entire* sidecar fails to parse and `SessionReader::open` sees
    /// no sidecar at all (`None`), never a `Some(meta)` with a non-finite
    /// field. `sanitize_lift`/`sanitize_ppm`'s own non-finite branch is
    /// still real and independently pinned (`crate::sanitize`'s own unit
    /// tests) — it is load-bearing for `ControlsState::from_config`, whose
    /// TOML config source (unlike JSON) does have literal `inf`/`nan`
    /// tokens — it's specifically the JSON-sidecar path where it's dead
    /// code, which is what this comment documents.
    #[test]
    fn resolve_ppm_lift_clamps_out_of_range_finite_sidecar_values() {
        let sidecar = meta_with(9999.0, 5.0);
        let defaults = AnalyzerConfig::default();
        let (ppm, lift) = resolve_ppm_lift(&ReplayOverrides::default(), Some(&sidecar), &defaults);
        assert_eq!(ppm, 500.0, "9999.0 clamps to the +500 sanity bound");
        assert_eq!(
            lift, 10.0,
            "5.0 clamps into Analyzer::new's [10, 90] domain"
        );

        let sidecar_negative = meta_with(-9999.0, 95.0);
        let (ppm, lift) = resolve_ppm_lift(
            &ReplayOverrides::default(),
            Some(&sidecar_negative),
            &defaults,
        );
        assert_eq!(ppm, -500.0, "-9999.0 clamps to the -500 sanity bound");
        assert_eq!(
            lift, 90.0,
            "95.0 clamps into Analyzer::new's [10, 90] domain"
        );
    }

    /// An explicit override is the caller's own deliberate intent, not
    /// sidecar data — it is used as-is, never run through
    /// `sanitize_ppm`/`sanitize_lift`, even when a (clamp-worthy) sidecar
    /// value is also present.
    #[test]
    fn resolve_ppm_lift_override_is_not_sanitized_and_beats_sidecar() {
        let sidecar = meta_with(25.0, 38.0);
        let defaults = AnalyzerConfig::default();
        let overrides = ReplayOverrides {
            ppm_correction: Some(9999.0),
            lift_angle_deg: Some(95.0),
            bph_mode: None,
        };
        let (ppm, lift) = resolve_ppm_lift(&overrides, Some(&sidecar), &defaults);
        assert_eq!(
            ppm, 9999.0,
            "an explicit override is trusted as-is, unclamped"
        );
        assert_eq!(
            lift, 95.0,
            "an explicit override is trusted as-is, unclamped"
        );
    }

    /// With neither an override nor a sidecar, falls back to
    /// `AnalyzerConfig::default()` — already analyzer-valid by
    /// construction, so untouched by sanitization either way.
    #[test]
    fn resolve_ppm_lift_defaults_without_override_or_sidecar() {
        let defaults = AnalyzerConfig::default();
        let (ppm, lift) = resolve_ppm_lift(&ReplayOverrides::default(), None, &defaults);
        assert_eq!(ppm, defaults.ppm_correction);
        assert_eq!(lift, defaults.lift_angle_deg);
    }
}
