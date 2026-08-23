//! `SessionWriter`: streams f32 mono samples to a 32-bit float WAV, writing
//! the JSON sidecar up front (spec M3: recording).

use crate::sidecar::{SessionMeta, write_sidecar};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

pub struct SessionWriter {
    writer: hound::WavWriter<std::io::BufWriter<std::fs::File>>,
    wav_path: PathBuf,
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
        Ok(SessionWriter { writer, wav_path })
    }

    pub fn push(&mut self, samples: &[f32]) -> Result<()> {
        for &s in samples {
            self.writer.write_sample(s)?;
        }
        Ok(())
    }

    /// Finalizes the WAV and returns its path.
    pub fn finalize(self) -> Result<PathBuf> {
        self.writer.finalize()?;
        Ok(self.wav_path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reader::SessionReader;

    #[test]
    fn write_read_roundtrip_preserves_meta_and_samples() {
        let dir = tempfile::tempdir().unwrap();
        let meta = SessionMeta {
            schema_version: 1,
            device_name: "TestMic".into(),
            sample_rate_hz: 48_000.0,
            ppm_correction: 12.5,
            lift_angle_deg: 52.0,
            bph_mode: "auto".into(),
            position: Some("DU".into()),
            started_unix_s: 1_700_000_000,
            app_version: "test".into(),
        };
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
}
