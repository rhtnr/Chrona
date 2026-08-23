//! Record & replay panel (T10): starts/stops session recording to a WAV +
//! JSON sidecar via the engine's `StartRecording`/`StopRecording`, and
//! switches the engine into replaying a previously recorded file. The pure
//! elapsed-time formatter (`rec_elapsed_label`) is TDD'd first, below; the
//! egui-facing rendering function comes after and is exercised only by
//! `cargo build` + the workspace test suite (no window in CI) — same split
//! as T9's controls row.
//!
//! Controller ruling (M3 T10 dispatch): the blue "REPLAY <name>" indicator
//! rendered here is a **mode line** inside this panel, not a health banner —
//! `pick_banner` (T9) is untouched, and its own live banners (clipping,
//! overruns, engine notices) still show during replay; only silence is
//! kind-gated there, which is desirable when reviewing a suspect recording.

use std::path::{Path, PathBuf};
use std::time::Instant;

use eframe::egui;

use crate::engine::{ControlMsg, Engine, EngineSnapshot, SourceKind, SourceSpec};

/// Position quick-buttons (behavior contract): dial-up/down, crown-up/down/
/// left/right — the standard timegrapher position vocabulary.
const POSITION_PRESETS: [&str; 6] = ["DU", "DD", "CU", "CD", "CL", "CR"];

/// `mm:ss`, or `h:mm:ss` past the hour (behavior contract). PURE.
pub fn rec_elapsed_label(elapsed_s: u64) -> String {
    let h = elapsed_s / 3_600;
    let m = (elapsed_s % 3_600) / 60;
    let s = elapsed_s % 60;
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}

/// `<platform data dir>/chrona/recordings` — created on first record (see
/// `record_controls`' `create_dir_all`), not eagerly at app startup. `None`
/// when the platform has no known data directory (same "graceful, not
/// fatal" shape as `chrona_session::ConfigStore::load_default`'s config-dir
/// lookup).
pub fn default_recordings_dir() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", "chrona").map(|dirs| dirs.data_dir().join("recordings"))
}

/// Session panel's own state: the position text staged for the *next*
/// `StartRecording`, the wall-clock instant the current recording began,
/// and the display name of the currently loaded replay file.
///
/// The latter two exist here rather than in `EngineSnapshot` because the
/// engine doesn't publish either: `recording` is only a `PathBuf` (no start
/// instant — the elapsed clock is this panel's own `Instant`, held from the
/// moment it sent `StartRecording`), and no snapshot field carries the
/// active replay's source path at all (`SourceRuntime::Replay` doesn't
/// retain it). Both are set optimistically when this panel sends the
/// corresponding control message and self-correct from the next snapshot
/// otherwise (see `session_panel`'s first lines) — a failed switch/start
/// just leaves them unused rather than lying, since the snapshot that would
/// make them visible (`recording_path.is_some()` / `source_kind == Replay`)
/// never arrives.
#[derive(Debug, Clone, Default)]
pub struct SessionPanelState {
    pub position: String,
    pub recording_started_at: Option<Instant>,
    pub replay_name: Option<String>,
}

// ---------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------

/// Renders the record & replay panel. `recordings_dir` is created (if
/// missing) the first time this session records. `last_device` is the
/// controls row's currently selected mic device, wired into
/// `SourceSpec::Mic` on "Back to live".
pub fn session_panel(
    ui: &mut egui::Ui,
    state: &mut SessionPanelState,
    engine: &Engine,
    snap: &EngineSnapshot,
    recordings_dir: &Path,
    last_device: &Option<String>,
) {
    // Self-correcting: whatever the reason recording isn't active per the
    // engine's own snapshot — an explicit stop, a mid-recording source
    // switch the engine auto-stopped (T7's recording-safe switching), or a
    // start that failed — the elapsed clock resets rather than trusting our
    // own optimistic toggle state.
    if snap.recording.is_none() {
        state.recording_started_at = None;
    }

    ui.horizontal(|ui| {
        if snap.source_kind != SourceKind::Replay {
            record_controls(ui, state, engine, snap, recordings_dir);
            ui.separator();
        }
        if ui.button("Open recording…").clicked()
            && let Some(path) = rfd::FileDialog::new()
                .add_filter("WAV", &["wav"])
                .pick_file()
        {
            state.replay_name = Some(
                path.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.display().to_string()),
            );
            engine.send(ControlMsg::SwitchSource(SourceSpec::ReplayFile { path }));
        }
    });

    if snap.source_kind == SourceKind::Replay {
        replay_mode_line(ui, state, snap, engine, last_device);
    }
}

/// Record toggle button, position quick-picks + free text, and the live
/// `● REC <filename> <mm:ss>` indicator. Only called for Mic/Simulate
/// sources (behavior contract: recording isn't offered while replaying).
fn record_controls(
    ui: &mut egui::Ui,
    state: &mut SessionPanelState,
    engine: &Engine,
    snap: &EngineSnapshot,
    recordings_dir: &Path,
) {
    let recording = snap.recording.is_some();
    let label = if recording { "■ Stop" } else { "● Record" };
    if ui.button(label).clicked() {
        if recording {
            engine.send(ControlMsg::StopRecording);
            state.recording_started_at = None;
        } else {
            // "created on first record" (behavior contract) — idempotent,
            // best-effort: a failure here just means the subsequent
            // StartRecording fails too and surfaces its own error banner.
            let _ = std::fs::create_dir_all(recordings_dir);
            let trimmed = state.position.trim();
            let meta_position = if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            };
            engine.send(ControlMsg::StartRecording {
                dir: recordings_dir.to_path_buf(),
                meta_position,
            });
            state.recording_started_at = Some(Instant::now());
        }
    }

    ui.add_enabled_ui(!recording, |ui| {
        for preset in POSITION_PRESETS {
            if ui.small_button(preset).clicked() {
                state.position = preset.to_string();
            }
        }
        ui.add(
            egui::TextEdit::singleline(&mut state.position)
                .hint_text("position")
                .desired_width(70.0),
        );
    });

    if let Some(path) = &snap.recording {
        let elapsed_s = state
            .recording_started_at
            .map(|t| t.elapsed().as_secs())
            .unwrap_or(0);
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        ui.label(
            egui::RichText::new(format!("● REC {name} {}", rec_elapsed_label(elapsed_s)))
                .color(egui::Color32::from_rgb(220, 60, 60)),
        );
    }
}

/// The blue "REPLAY <name>" mode line (controller ruling: lives here, not
/// in the health-banner slot) plus "· done" once `replay_done`, and the
/// "Back to live" button.
fn replay_mode_line(
    ui: &mut egui::Ui,
    state: &mut SessionPanelState,
    snap: &EngineSnapshot,
    engine: &Engine,
    last_device: &Option<String>,
) {
    let name = state.replay_name.as_deref().unwrap_or("recording");
    let mut text = format!("REPLAY {name}");
    if snap.replay_done {
        text.push_str(" · done");
    }
    egui::Frame::new()
        .fill(egui::Color32::from_rgb(30, 60, 130))
        .inner_margin(6.0)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(text).color(egui::Color32::WHITE));
                if ui.button("Back to live").clicked() {
                    state.replay_name = None;
                    engine.send(ControlMsg::SwitchSource(SourceSpec::Mic {
                        device_id: last_device.clone(),
                    }));
                }
            });
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rec_elapsed_label_table() {
        let cases: [(u64, &str); 7] = [
            (0, "00:00"),
            (7, "00:07"),
            (59, "00:59"),
            (60, "01:00"),
            (754, "12:34"),
            (3599, "59:59"),
            (5025, "1:23:45"),
        ];
        for (secs, want) in cases {
            assert_eq!(rec_elapsed_label(secs), want, "elapsed_s={secs}");
        }
    }
}
