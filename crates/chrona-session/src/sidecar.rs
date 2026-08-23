//! The JSON sidecar written next to every recording: `SessionMeta`, plus
//! read/write helpers keyed off the WAV path by extension swap.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// The sidecar schema version this crate writes. `write_sidecar` stamps
/// every write with this value unconditionally, regardless of what the
/// in-memory `SessionMeta` carried — see
/// [`SessionMeta::schema_version`]'s doc comment for the read/write split.
pub const SIDECAR_SCHEMA_VERSION: u32 = 2;

/// Per-session metadata, serialized alongside the WAV as
/// `<stem>.json` (spec M3: recording; M4a sidecar v2 adds `watch` and
/// `summary` below).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SessionMeta {
    /// On **write**, always overwritten with [`SIDECAR_SCHEMA_VERSION`] by
    /// `write_sidecar`, regardless of what this field held — callers never
    /// need to (and can't reliably) set it themselves. On **read**, this
    /// faithfully reports whatever the file actually said; the reader adds
    /// no version validation (matches the sidecar's "best-effort, never an
    /// error" contract — older/newer versions still parse as far as
    /// `#[serde(default)]` on the fields added since each version allows).
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
    /// The watch under test (M4a: watch list), freeform. `#[serde(default)]`
    /// so v1 sidecars — written before this field existed — still parse.
    #[serde(default)]
    pub watch: Option<String>,
    /// Headline metrics at stop time, written by
    /// `SessionWriter::finalize_with` (M4a). `#[serde(default)]` for the
    /// same v1-compatibility reason as `watch`.
    #[serde(default)]
    pub summary: Option<SessionSummary>,
}

/// A snapshot of the session's headline metrics at stop time (M4a sidecar
/// v2), written into the sidecar by `SessionWriter::finalize_with`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SessionSummary {
    /// `"none"` | `"T1"` | `"T2"` | `"T3"`, mirroring the app's tier label.
    pub tier: String,
    pub rate_s_per_day: Option<f64>,
    pub beat_error_ms: Option<f64>,
    pub amplitude_deg: Option<f64>,
    pub bph_detected: Option<f64>,
    pub duration_s: f64,
}

/// The sidecar path for a given WAV path: same stem, `.json` extension.
pub(crate) fn sidecar_path(wav_path: &Path) -> PathBuf {
    wav_path.with_extension("json")
}

/// Writes `meta` to `wav_path`'s sidecar, first stamping a copy's
/// `schema_version` to [`SIDECAR_SCHEMA_VERSION`] unconditionally — the
/// caller's own `meta` (and whatever `schema_version` it happened to
/// carry) is left untouched; only the on-disk copy is corrected.
pub(crate) fn write_sidecar(wav_path: &Path, meta: &SessionMeta) -> Result<()> {
    let path = sidecar_path(wav_path);
    let mut meta = meta.clone();
    meta.schema_version = SIDECAR_SCHEMA_VERSION;
    let json = serde_json::to_string_pretty(&meta).context("serialize session sidecar")?;
    std::fs::write(&path, json).with_context(|| format!("write sidecar {}", path.display()))
}

/// Best-effort sidecar load: an absent file or one that fails to parse
/// returns `None` — never an error (spec M3: replay must work on bare WAVs).
pub(crate) fn read_sidecar(wav_path: &Path) -> Option<SessionMeta> {
    let text = std::fs::read_to_string(sidecar_path(wav_path)).ok()?;
    serde_json::from_str(&text).ok()
}

/// Public wrapper over [`read_sidecar`] (M4a: callers outside this crate,
/// e.g. a watch-history UI, need read access without opening the WAV via a
/// full `SessionReader`). Same best-effort `None` semantics.
pub fn read_meta(wav_path: &Path) -> Option<SessionMeta> {
    read_sidecar(wav_path)
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

    /// A v1 sidecar (written before `watch`/`summary` existed) must still
    /// parse, with both new fields defaulting to `None` — the whole point
    /// of `#[serde(default)]` on them.
    #[test]
    fn v1_sidecar_loads_with_none_new_fields() {
        let dir = tempfile::tempdir().unwrap();
        let wav_path = dir.path().join("legacy.wav");
        let json_path = sidecar_path(&wav_path);
        let v1_json = r#"{
            "schema_version": 1,
            "device_name": "OldMic",
            "sample_rate_hz": 48000.0,
            "ppm_correction": 0.0,
            "lift_angle_deg": 52.0,
            "bph_mode": "auto",
            "position": null,
            "started_unix_s": 1700000000,
            "app_version": "0.1.0"
        }"#;
        std::fs::write(&json_path, v1_json).unwrap();
        let meta = read_meta(&wav_path).expect("v1 sidecar should still parse");
        assert_eq!(meta.watch, None);
        assert_eq!(meta.summary, None);
    }

    /// A full v2 sidecar (with `watch` and `summary` populated) round-trips
    /// through write/read with equality preserved, and the raw JSON text
    /// carries `schema_version: 2` (not just the parsed struct).
    #[test]
    fn v2_roundtrip_preserves_summary() {
        let dir = tempfile::tempdir().unwrap();
        let wav_path = dir.path().join("v2.wav");
        let summary = SessionSummary {
            tier: "T3".to_string(),
            rate_s_per_day: Some(12.0),
            beat_error_ms: Some(0.8),
            amplitude_deg: Some(270.0),
            bph_detected: Some(28_800.0),
            duration_s: 45.0,
        };
        let meta = SessionMeta {
            schema_version: 2,
            device_name: "TestMic".to_string(),
            sample_rate_hz: 48_000.0,
            ppm_correction: 12.5,
            lift_angle_deg: 52.0,
            bph_mode: "auto".to_string(),
            position: Some("DU".into()),
            started_unix_s: 1_700_000_000,
            app_version: "test".into(),
            watch: Some("Seiko 5".to_string()),
            summary: Some(summary),
        };
        write_sidecar(&wav_path, &meta).unwrap();
        let loaded = read_meta(&wav_path).expect("v2 sidecar should parse");
        assert_eq!(loaded, meta);
        assert_eq!(loaded.schema_version, 2);

        let text = std::fs::read_to_string(sidecar_path(&wav_path)).unwrap();
        assert!(
            text.contains("\"schema_version\": 2"),
            "expected schema_version: 2 in raw JSON, got: {text}"
        );
    }

    /// `write_sidecar` must stamp the current schema version
    /// unconditionally, never trusting whatever the caller happened to set
    /// — the whole point of enforcing `SIDECAR_SCHEMA_VERSION` at this one
    /// write choke point rather than leaving every call site to remember
    /// it. Also confirms the caller's own `meta` is never mutated in
    /// place — only the on-disk copy is corrected.
    #[test]
    fn write_sidecar_always_stamps_current_schema_version() {
        let dir = tempfile::tempdir().unwrap();
        let wav_path = dir.path().join("stamped.wav");
        let meta = SessionMeta {
            schema_version: 1, // deliberately stale
            device_name: "TestMic".to_string(),
            sample_rate_hz: 48_000.0,
            ppm_correction: 0.0,
            lift_angle_deg: 52.0,
            bph_mode: "auto".to_string(),
            position: None,
            started_unix_s: 1_700_000_000,
            app_version: "test".into(),
            watch: None,
            summary: None,
        };
        write_sidecar(&wav_path, &meta).unwrap();

        let text = std::fs::read_to_string(sidecar_path(&wav_path)).unwrap();
        assert!(
            text.contains("\"schema_version\": 2"),
            "write_sidecar must stamp schema_version to SIDECAR_SCHEMA_VERSION (2), got: {text}"
        );
        assert_eq!(
            meta.schema_version, 1,
            "write_sidecar takes &SessionMeta — the caller's own copy must be untouched"
        );
    }
}
