//! Record & replay panel (T10): the position quick-picks/free text staged
//! for the *next* `StartRecording`, the live `● REC <name> <mm:ss>`
//! indicator, and the "REPLAY <name>" mode line + "Back to live". M4a
//! Task 6 moved the Record/Stop button and "Open recording…" (and the
//! `StartRecording`/`SwitchSource` sends that went with them) to
//! `ui::toolbar::toolbar_row`; what's left here is purely the position
//! input and the read-only elapsed/replay display, until Task 7 moves both
//! into the redesigned position strip and deletes this file. The pure
//! elapsed-time formatter (`rec_elapsed_label`) is TDD'd first, below; the
//! egui-facing rendering function comes after and is exercised only by
//! `cargo build` + the workspace test suite (no window in CI) — same split
//! as `controls.rs`/`ui::toolbar`.
//!
//! Controller ruling (M3 T10 dispatch): the blue "REPLAY <name>" indicator
//! rendered here is a **mode line** inside this panel, not a health banner —
//! `pick_banner` (T9) is untouched, and its own live banners (clipping,
//! overruns, engine notices) still show during replay; only silence is
//! kind-gated there, which is desirable when reviewing a suspect recording.

use std::path::PathBuf;
use std::time::Instant;

use eframe::egui;

use crate::engine::{ControlMsg, Engine, EngineSnapshot, SourceKind, SourceSpec};
use crate::history::HistoryIndex;

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
/// `ui::toolbar::record_stop_button`'s `create_dir_all`), not eagerly at
/// app startup. `None` when the platform has no known data directory (same
/// "graceful, not fatal" shape as `chrona_session::ConfigStore::load_
/// default`'s config-dir lookup).
pub fn default_recordings_dir() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", "chrona").map(|dirs| dirs.data_dir().join("recordings"))
}

/// Session panel's own state: the position text staged for the *next*
/// `StartRecording` (sent by `ui::toolbar::toolbar_row`'s Record button,
/// M4a Task 6), the wall-clock instant the current recording began, and the
/// display name of the currently loaded replay file.
///
/// The latter two exist here rather than in `EngineSnapshot` because the
/// engine doesn't publish either: `recording` is only a `PathBuf` (no start
/// instant — the elapsed clock reads this panel's own `Instant`, held from
/// the moment the toolbar's Record button sent `StartRecording`), and no
/// snapshot field carries the active replay's source path at all
/// (`SourceRuntime::Replay` doesn't retain it). Both are set optimistically
/// by the control message's sender (the toolbar, not this panel, since M4a
/// Task 6), and neither is corrected against the snapshot afterwards (a
/// snapshot-driven reset would race the engine's confirmation — see
/// `position_controls`' doc comment for `recording_started_at`
/// specifically). A failed switch/start just leaves the optimistic value
/// unused rather than lying, since each is only ever *displayed* behind the
/// snapshot condition that would make it visible (`recording_started_at`
/// behind `snap.recording.is_some()`, `replay_name` behind `source_kind ==
/// Replay`) — see `position_controls`' and `replay_mode_line`'s doc
/// comments for the exact residuals.
#[derive(Debug, Clone, Default)]
pub struct SessionPanelState {
    pub position: String,
    pub recording_started_at: Option<Instant>,
    pub replay_name: Option<String>,
    /// The WAV path last seen in `snap.recording`, held across frames only
    /// so `session_panel` can detect the `Some -> None` transition (a
    /// recording just finalized) and upsert the right path into the
    /// history index — see `session_panel`'s doc comment.
    last_recording: Option<PathBuf>,
}

// ---------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------

/// Renders the position input + elapsed/replay display. `last_device` is
/// the controls' currently selected mic device, wired into `SourceSpec::
/// Mic` on "Back to live".
///
/// `history` is kept in sync here rather than by the engine itself: the
/// moment `snap.recording` goes from `Some(path)` to `None`, the engine has
/// already finalized that recording — including rewriting its JSON sidecar
/// with the stop-time summary (see `Engine`'s `StopRecording` handling,
/// which calls `finalize_with` synchronously before the snapshot publishing
/// that clears `recording_path`) — so this is a safe, real point to
/// `upsert` the just-finished session into the index (spec §7: "upsert
/// after each finalized recording"). `state.last_recording` is the only
/// state this needs across frames: the WAV path seen last time, so the
/// transition itself (not just the current `None`) can be detected.
pub fn session_panel(
    ui: &mut egui::Ui,
    state: &mut SessionPanelState,
    engine: &Engine,
    snap: &EngineSnapshot,
    last_device: &Option<String>,
    history: &mut HistoryIndex,
) {
    match (state.last_recording.take(), &snap.recording) {
        (Some(prev), None) => history.upsert(&prev),
        (_, Some(current)) => state.last_recording = Some(current.clone()),
        (None, None) => {}
    }

    // `recording_started_at` is owned entirely by the toolbar's Record/Stop
    // click handler (`ui::toolbar::record_stop_button`, M4a Task 6: Record
    // sets it, Stop clears it) — there is deliberately no "reset it
    // whenever snap.recording is None" check here. `snap` lags the click
    // that sends StartRecording/StopRecording by up to a full engine tick
    // (~50 ms) before the confirming snapshot publishes; a blanket reset
    // gated on `snap.recording` would fire on any repaint during that
    // window (egui repaints often, and the engine itself requests one on
    // every ~10 Hz publish) and wipe the `Some` the click just set,
    // permanently — nothing would ever set it again until the next Stop,
    // so the elapsed label would wedge at "00:00" for the rest of the
    // recording. See `position_controls`' doc comment for the (harmless)
    // residuals of *not* having this reset.
    if snap.source_kind != SourceKind::Replay {
        position_controls(ui, state, snap);
    }

    if snap.source_kind == SourceKind::Replay {
        replay_mode_line(ui, state, snap, engine, last_device);
    }
}

/// Position quick-picks + free text, disabled while recording, and the live
/// `● REC <filename> <mm:ss>` indicator. Only called for Mic/Simulate
/// sources (behavior contract: recording isn't offered while replaying).
/// The Record/Stop button itself, and the `StartRecording`/`StopRecording`
/// sends, moved to `ui::toolbar::record_stop_button` (M4a Task 6) — this is
/// just the input that button reads (`state.position`) plus the read-only
/// elapsed display below it.
///
/// `state.recording_started_at` is written only by the toolbar's Record/
/// Stop click handler — the Record click sets it, the Stop click clears
/// it — never reset elsewhere (see `session_panel`'s doc comment on why a
/// snapshot-driven reset would race). Two residuals of that:
/// - A `StartRecording` the engine rejects (e.g. an uncreatable
///   `recordings_dir`) leaves `recording_started_at` set even though
///   `snap.recording` stays `None` — inert, since the elapsed label below
///   only renders `if let Some(path) = &snap.recording`; the next Record
///   click overwrites it with a fresh `Instant` regardless.
/// - Clicking Stop clears it immediately (not gated on the engine's
///   confirmation), so for the handful of frames until `snap.recording`
///   itself catches up to `None`, the REC label can still render with
///   elapsed pinned at `00:00` — a brief, harmless "stopping…" flicker.
fn position_controls(ui: &mut egui::Ui, state: &mut SessionPanelState, snap: &EngineSnapshot) {
    let recording = snap.recording.is_some();
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
///
/// `state.replay_name` is set optimistically by the "Open recording…"
/// handler before the engine confirms the switch. If `SwitchSource` then
/// fails (bad/corrupt WAV), `source_kind` never becomes `Replay`, so this
/// function is simply never called — the stale name sits unused until the
/// next successful pick overwrites it (same "only visible behind the
/// condition that proves it's live" shape as `position_controls`' elapsed
/// clock).
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
