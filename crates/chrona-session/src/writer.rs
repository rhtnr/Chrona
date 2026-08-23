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
        })
    }

    pub fn push(&mut self, samples: &[f32]) -> Result<()> {
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
}
