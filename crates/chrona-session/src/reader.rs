//! `SessionReader`: loads a WAV (any PCM/float, mono or averaged down from
//! multi-channel) plus its optional JSON sidecar.

use crate::sidecar::{SessionMeta, read_sidecar};
use anyhow::{Context, Result};
use std::path::Path;

pub struct SessionReader {
    pub samples: Vec<f32>,
    pub sample_rate_hz: f64,
    /// `None` when the sidecar is absent or fails to parse — never an error.
    pub meta: Option<SessionMeta>,
}

impl SessionReader {
    pub fn open(wav: &Path) -> Result<SessionReader> {
        let mut reader =
            hound::WavReader::open(wav).with_context(|| format!("open {}", wav.display()))?;
        let spec = reader.spec();
        let ch = spec.channels as usize;
        let interleaved: Vec<f32> = match spec.sample_format {
            hound::SampleFormat::Float => reader.samples::<f32>().collect::<Result<_, _>>()?,
            hound::SampleFormat::Int => {
                let scale = (1i64 << (spec.bits_per_sample - 1)) as f32;
                reader
                    .samples::<i32>()
                    .map(|s| s.map(|v| v as f32 / scale))
                    .collect::<Result<_, _>>()?
            }
        };
        let samples: Vec<f32> = if ch <= 1 {
            interleaved
        } else {
            interleaved
                .chunks_exact(ch)
                .map(|frame| frame.iter().sum::<f32>() / ch as f32)
                .collect()
        };
        Ok(SessionReader {
            samples,
            sample_rate_hz: spec.sample_rate as f64,
            meta: read_sidecar(wav),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_sidecar_is_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let wav_path = dir.path().join("bare.wav");
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 48_000,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let mut w = hound::WavWriter::create(&wav_path, spec).unwrap();
        for i in 0..100 {
            w.write_sample(i as f32 / 100.0).unwrap();
        }
        w.finalize().unwrap();

        let r = SessionReader::open(&wav_path).unwrap();
        assert_eq!(r.samples.len(), 100);
        assert!(r.meta.is_none());
    }
}
