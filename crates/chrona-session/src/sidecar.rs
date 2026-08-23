//! The JSON sidecar written next to every recording: `SessionMeta`, plus
//! read/write helpers keyed off the WAV path by extension swap.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// Per-session metadata, serialized alongside the WAV as
/// `<stem>.json` (spec M3: recording).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SessionMeta {
    pub schema_version: u32,
    pub device_name: String,
    pub sample_rate_hz: f64,
    pub ppm_correction: f64,
    pub lift_angle_deg: f64,
    /// `"auto"`, `"free"`, or a numeric BPH string (mirrors the CLI's
    /// `--bph` flag) — kept as a string so the sidecar stays human-readable
    /// and forward-compatible with modes this crate doesn't know about yet.
    pub bph_mode: String,
    pub position: Option<String>,
    pub started_unix_s: u64,
    pub app_version: String,
}

/// The sidecar path for a given WAV path: same stem, `.json` extension.
pub(crate) fn sidecar_path(wav_path: &Path) -> PathBuf {
    wav_path.with_extension("json")
}

pub(crate) fn write_sidecar(wav_path: &Path, meta: &SessionMeta) -> Result<()> {
    let path = sidecar_path(wav_path);
    let json = serde_json::to_string_pretty(meta).context("serialize session sidecar")?;
    std::fs::write(&path, json).with_context(|| format!("write sidecar {}", path.display()))
}

/// Best-effort sidecar load: an absent file or one that fails to parse
/// returns `None` — never an error (spec M3: replay must work on bare WAVs).
pub(crate) fn read_sidecar(wav_path: &Path) -> Option<SessionMeta> {
    let text = std::fs::read_to_string(sidecar_path(wav_path)).ok()?;
    serde_json::from_str(&text).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidecar_path_swaps_extension() {
        let wav = Path::new("/tmp/chrona-20260101-000000.wav");
        assert_eq!(
            sidecar_path(wav),
            Path::new("/tmp/chrona-20260101-000000.json")
        );
    }

    #[test]
    fn unparseable_sidecar_is_none_not_error() {
        let dir = tempfile::tempdir().unwrap();
        let wav_path = dir.path().join("x.wav");
        let json_path = sidecar_path(&wav_path);
        std::fs::write(&json_path, b"not valid json {{{").unwrap();
        assert!(read_sidecar(&wav_path).is_none());
    }
}
