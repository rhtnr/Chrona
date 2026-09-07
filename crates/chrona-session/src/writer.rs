//! `SessionWriter`: streams f32 mono samples to a 32-bit float WAV, writing
//! the JSON sidecar up front (spec M3: recording).

use crate::sidecar::{SessionMeta, SessionSummary, write_sidecar};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

pub struct SessionWriter {
    writer: hound::WavWriter<std::io::BufWriter<std::fs::File>>,
    wav_path: PathBuf,
    meta: SessionMeta,
    samples_written: u64,
    /// M4 Task 4 fault-injection seam (see the `test-fault-injection`
    /// feature doc comment in Cargo.toml). `push` fails once
    /// `samples_written` has already reached this many samples, simulating
    /// e.g. a full disk mid-recording. `pub` under this feature only, so a
    /// same-crate test can also set it directly (no env var, no risk of
    /// racing another test's `create` call — see `push`'s doc comment).
    /// `None` (the default: unset env var, untouched by a test) never
    /// fires — every non-test build, and most tests even under the
    /// feature.
    #[cfg(feature = "test-fault-injection")]
    pub fail_push_after: Option<u64>,
}

impl SessionWriter {
    /// Opens `<dir>/chrona-<timestamp>.wav` (32-bit float mono at
    /// `meta.sample_rate_hz`) and writes `<dir>/chrona-<timestamp>.json`
    /// immediately, where `<timestamp>` is `meta.started_unix_s` formatted
    /// by [`crate::civil::timestamp_compact`].
    pub fn create(dir: &Path, meta: SessionMeta) -> Result<SessionWriter> {
        let stamp = crate::civil::timestamp_compact(meta.started_unix_s);
        let wav_path = dir.join(format!("chrona-{stamp}.wav"));
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: meta.sample_rate_hz as u32,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let writer = hound::WavWriter::create(&wav_path, spec)
            .with_context(|| format!("create {}", wav_path.display()))?;
        write_sidecar(&wav_path, &meta)?;
        Ok(SessionWriter {
            writer,
            wav_path,
            meta,
            samples_written: 0,
            #[cfg(feature = "test-fault-injection")]
            fail_push_after: std::env::var("CHRONA_TEST_FAIL_PUSH_AFTER")
                .ok()
                .and_then(|s| s.parse().ok()),
        })
    }

    /// Writes `samples` to the WAV.
    ///
    /// Under the `test-fault-injection` feature (M4 Task 4), fails with an
    /// error instead once `samples_written` has already reached
    /// `fail_push_after` — simulating a mid-recording write failure (e.g. a
    /// full disk) for the engine's write-failure banner to exercise end to
    /// end, without a real one. An empty `samples` never trips this (there
    /// is nothing to fail to write); `fail_push_after: None` (the default —
    /// see its doc comment) never does either, so this is a no-op change
    /// for every non-test build and most tests even under the feature.
    pub fn push(&mut self, samples: &[f32]) -> Result<()> {
        #[cfg(feature = "test-fault-injection")]
        if !samples.is_empty()
            && self
                .fail_push_after
                .is_some_and(|limit| self.samples_written >= limit)
        {
            anyhow::bail!(
                "fault injection: simulated write failure after {} samples",
                self.samples_written
            );
        }
        for &s in samples {
            self.writer.write_sample(s)?;
        }
        self.samples_written += samples.len() as u64;
        Ok(())
    }

    /// Finalizes the WAV and returns its path. Delegates to
    /// [`SessionWriter::finalize_with`] with no summary — the sidecar is
    /// rewritten with `summary: None` (duration bookkeeping only).
    pub fn finalize(self) -> Result<PathBuf> {
        self.finalize_with(None)
    }

    /// Finalizes the WAV, then rewrites the sidecar with `summary` (its
    /// `duration_s` overwritten by the actual recorded duration —
    /// `samples_written / sample_rate_hz` — computed here rather than
    /// trusted from the caller). Returns the WAV path.
    pub fn finalize_with(self, summary: Option<SessionSummary>) -> Result<PathBuf> {
        let SessionWriter {
            writer,
            wav_path,
            mut meta,
            samples_written,
            ..
        } = self;
        writer.finalize()?;
        let duration_s = samples_written as f64 / meta.sample_rate_hz;
        meta.summary = summary.map(|mut s| {
            s.duration_s = duration_s;
            s
        });
        write_sidecar(&wav_path, &meta)?;
        Ok(wav_path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reader::SessionReader;

    fn test_meta(sample_rate_hz: f64) -> SessionMeta {
        SessionMeta {
            schema_version: 1,
            device_name: "TestMic".into(),
            sample_rate_hz,
            ppm_correction: 12.5,
            lift_angle_deg: 52.0,
            bph_mode: "auto".into(),
            position: Some("DU".into()),
            started_unix_s: 1_700_000_000,
            app_version: "test".into(),
            watch: None,
            summary: None,
        }
    }

    #[test]
    fn write_read_roundtrip_preserves_meta_and_samples() {
        let dir = tempfile::tempdir().unwrap();
        let meta = test_meta(48_000.0);
        let mut w = SessionWriter::create(dir.path(), meta.clone()).unwrap();
        let samples: Vec<f32> = (0..4_800).map(|i| (i as f32 / 4_800.0) - 0.5).collect();
        w.push(&samples).unwrap();
        let wav = w.finalize().unwrap();
        let r = SessionReader::open(&wav).unwrap();
        assert_eq!(r.samples.len(), 4_800);
        assert!((r.samples[2_400] - samples[2_400]).abs() < 1e-6);
        let m = r.meta.expect("sidecar");
        assert_eq!(m.device_name, "TestMic");
        assert_eq!(m.position.as_deref(), Some("DU"));
    }

    /// `finalize_with(Some(summary))` rewrites the sidecar with the summary
    /// attached, its `duration_s` overwritten from the actual sample count
    /// (1.0 s of samples at 48 kHz here) rather than whatever the caller
    /// put in the field.
    #[test]
    fn finalize_with_rewrites_sidecar_with_duration() {
        use crate::sidecar::{SessionSummary, read_meta};

        let dir = tempfile::tempdir().unwrap();
        let meta = test_meta(48_000.0);
        let mut w = SessionWriter::create(dir.path(), meta).unwrap();
        w.push(&vec![0.0f32; 48_000]).unwrap();
        let summary = SessionSummary {
            tier: "T3".to_string(),
            rate_s_per_day: Some(1.0),
            beat_error_ms: Some(0.5),
            amplitude_deg: Some(250.0),
            bph_detected: Some(28_800.0),
            duration_s: 999.0, // must be overwritten by finalize_with
        };
        let wav_path = w.finalize_with(Some(summary)).unwrap();

        let loaded = read_meta(&wav_path).expect("sidecar present");
        let loaded_summary = loaded.summary.expect("summary present");
        assert_eq!(loaded_summary.tier, "T3");
        assert!(
            (loaded_summary.duration_s - 1.0).abs() < 1e-9,
            "duration_s should be ~1.0 (48_000 samples @ 48 kHz), got {}",
            loaded_summary.duration_s
        );
    }

    /// `finalize_with(None)` still rewrites the sidecar (duration
    /// bookkeeping happens either way) but leaves `summary` absent.
    #[test]
    fn finalize_none_keeps_summary_absent() {
        use crate::sidecar::read_meta;

        let dir = tempfile::tempdir().unwrap();
        let meta = test_meta(48_000.0);
        let mut w = SessionWriter::create(dir.path(), meta).unwrap();
        w.push(&vec![0.0f32; 4_800]).unwrap();
        let wav_path = w.finalize_with(None).unwrap();

        let loaded = read_meta(&wav_path).expect("sidecar present");
        assert!(loaded.summary.is_none());
    }

    /// Pins `push`'s fault-injection threshold semantics directly (no env
    /// var, so this can't race another test's `create` call over the
    /// process-global environment — see `fail_push_after`'s and `push`'s
    /// doc comments; the engine-level env-var path is covered separately
    /// by chrona-app's `tests/engine.rs`).
    #[cfg(feature = "test-fault-injection")]
    #[test]
    fn push_fails_once_the_configured_sample_count_is_reached() {
        let dir = tempfile::tempdir().unwrap();
        let meta = test_meta(48_000.0);
        let mut w = SessionWriter::create(dir.path(), meta).unwrap();
        assert_eq!(
            w.fail_push_after, None,
            "no env var set for this test process: must default to None"
        );
        w.fail_push_after = Some(10);

        w.push(&[0.0f32; 10]).unwrap(); // exactly at the limit: still fine
        let err = w.push(&[0.0f32]).unwrap_err(); // one more: fails
        assert!(
            err.to_string().contains("10 samples"),
            "unexpected error message: {err}"
        );

        // An empty push never trips it, even past the limit — nothing was
        // actually being written.
        w.push(&[]).unwrap();
    }
}
