//! Position/session strip (M4a Task 7, mockup: "Position + session
//! strip"): the DU/DD/CU/CD/CL/CR segmented control, the signal meter, and
//! — mutually exclusive, same as the panel this replaces — either the
//! REPLAY mode line or the live recording elapsed clock. Absorbs the last
//! pieces of M3's `ui::session_panel` (deleted by this task): the
//! elapsed-time formatter and the REPLAY mode line move here verbatim
//! (behavior unchanged, just restyled onto `theme::Palette` tokens instead
//! of raw `Color32`s); the position INPUT itself is replaced outright by
//! the segmented control below (`history::POSITIONS`), which is why
//! `SessionPanelState` no longer carries a `position` field at all — that
//! selection now lives on `ChronaApp` (`selected_position`, an index into
//! `POSITIONS`) and is threaded into the toolbar's Record button as
//! `meta_position` (see `ui::toolbar::record_stop_button`).
//!
//! The pure helpers (`rec_elapsed_label`, `position_word`) are TDD'd
//! first, below; the egui-facing rendering that follows is exercised only
//! by `cargo build` + the workspace test suite (no window in CI) — same
//! split as `controls.rs`/`toolbar.rs`.

use std::path::PathBuf;
use std::time::Instant;

use eframe::egui;

use crate::engine::{ControlMsg, Engine, EngineSnapshot, SourceKind, SourceSpec};
use crate::history::{HistoryIndex, POSITIONS};
use crate::presenter::{SignalClass, signal_meter};
use crate::theme::Palette;

/// `mm:ss`, or `h:mm:ss` past the hour (behavior contract). PURE. Moved
/// verbatim from the old `ui::session_panel`.
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

/// The segmented control's small second line for one position's full name
/// (mockup: `full.split(' ')[1]` — "Dial up" -> "up"). PURE.
fn position_word(full: &str) -> &str {
    full.split(' ').nth(1).unwrap_or(full)
}

/// `<platform data dir>/chrona/recordings` — created on first record (see
/// `ui::toolbar::record_stop_button`'s `create_dir_all`), not eagerly at
/// app startup. `None` when the platform has no known data directory (same
/// "graceful, not fatal" shape as `chrona_session::ConfigStore::load_
/// default`'s config-dir lookup). Moved verbatim from the old
/// `ui::session_panel`.
pub fn default_recordings_dir() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", "chrona").map(|dirs| dirs.data_dir().join("recordings"))
}

/// Position/session strip's own state: the wall-clock instant the current
/// recording began, and the display name of the currently loaded replay
/// file. Moved from the old `ui::session_panel::SessionPanelState`, minus
/// its `position` field (replaced by `ChronaApp::selected_position` — see
/// the module doc comment).
///
/// These exist here rather than in `EngineSnapshot` because the engine
/// doesn't publish either: `recording` is only a `PathBuf` (no start
/// instant — the elapsed clock reads this struct's own `Instant`, held
/// from the moment the toolbar's Record button sent `StartRecording`), and
/// no snapshot field carries the active replay's source path at all
/// (`SourceRuntime::Replay` doesn't retain it). Both are set optimistically
/// by the control message's sender (the toolbar, not this module), and
/// neither is corrected against the snapshot afterwards — see
/// `ui::toolbar::record_stop_button`'s and `replay_mode_line`'s doc
/// comments for the exact residuals of that.
#[derive(Debug, Clone, Default)]
pub struct SessionPanelState {
    pub recording_started_at: Option<Instant>,
    pub replay_name: Option<String>,
    /// The WAV path last seen in `snap.recording`, held across frames only
    /// so `position_strip` can detect the `Some -> None` transition (a
    /// recording just finalized) and upsert the right path into the
    /// history index — see `position_strip`'s doc comment.
    last_recording: Option<PathBuf>,
}

// ---------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------

/// Borrowed, per-frame context `position_strip` needs beyond its own
/// state/`ui`/`palette`: bundled purely to keep the argument count sane
/// (`clippy::too_many_arguments`), same shape as `toolbar::ToolbarCtx`.
pub struct StripCtx<'a> {
    pub engine: &'a Engine,
    pub snap: &'a EngineSnapshot,
    pub last_device: &'a Option<String>,
    pub history: &'a mut HistoryIndex,
}

/// Renders the position + session strip. `*selected_position` is an index
/// into `history::POSITIONS`; the segmented control is disabled while
/// `ctx.snap.recording.is_some()` (position can't change mid-recording —
/// M3 behavior, carried forward).
///
/// Also carries the recording→`None` upsert hook Task 5 put in the old
/// `session_panel` (moved here intact): the moment `snap.recording` goes
/// from `Some(path)` to `None`, the engine has already finalized that
/// recording — including rewriting its JSON sidecar with the stop-time
/// summary — so this is a safe, real point to `upsert` the just-finished
/// session into `ctx.history` (spec §7: "upsert after each finalized
/// recording"). `state.last_recording` is the only state this needs across
/// frames: the WAV path seen last time, so the transition itself (not just
/// the current `None`) can be detected.
pub fn position_strip(
    ui: &mut egui::Ui,
    palette: &Palette,
    selected_position: &mut usize,
    state: &mut SessionPanelState,
    ctx: StripCtx<'_>,
) {
    match (state.last_recording.take(), &ctx.snap.recording) {
        (Some(prev), None) => ctx.history.upsert(&prev),
        (_, Some(current)) => state.last_recording = Some(current.clone()),
        (None, None) => {}
    }

    let recording = ctx.snap.recording.is_some();

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 16.0;

        ui.label(strip_label(palette, "Position"));
        ui.add_enabled_ui(!recording, |ui| {
            position_segmented_control(ui, palette, selected_position);
        });

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ctx.snap.source_kind == SourceKind::Replay {
                replay_mode_line(ui, palette, state, ctx.snap, ctx.engine, ctx.last_device);
            } else if ctx.snap.recording.is_some() {
                elapsed_clock(ui, palette, state);
            }
            signal_meter_widget(ui, palette, ctx.snap.metrics.as_ref());
        });
    });
}

fn strip_label(palette: &Palette, text: &str) -> egui::RichText {
    egui::RichText::new(text.to_uppercase())
        .size(11.0)
        .color(palette.muted)
}

/// The DU/DD/CU/CD/CL/CR segmented control (mockup): a `panel`-filled,
/// `border`-stroked, 10-rounded pill holding 6 two-line buttons (mono code
/// plus a small full-name word); the selected button gets `accent` fill and
/// `accent_ink` text, others transparent fill and `muted` text. `*selected`
/// is an index into `history::POSITIONS`.
fn position_segmented_control(ui: &mut egui::Ui, palette: &Palette, selected: &mut usize) {
    egui::Frame::new()
        .fill(palette.panel)
        .stroke(egui::Stroke::new(1.0, palette.border))
        .corner_radius(10.0)
        .inner_margin(4.0)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                for (i, (code, full)) in POSITIONS.iter().enumerate() {
                    let is_selected = *selected == i;
                    if position_button(ui, palette, code, position_word(full), full, is_selected)
                        .clicked()
                    {
                        *selected = i;
                    }
                }
            });
        });
}

fn position_button(
    ui: &mut egui::Ui,
    palette: &Palette,
    code: &str,
    word: &str,
    full_title: &str,
    selected: bool,
) -> egui::Response {
    let (bg, fg) = if selected {
        (palette.accent, palette.accent_ink)
    } else {
        (egui::Color32::TRANSPARENT, palette.muted)
    };
    let code_font = egui::FontId::monospace(13.0);
    let word_font = egui::FontId::proportional(9.5);
    let code_galley = ui.painter().layout_no_wrap(code.to_string(), code_font, fg);
    let word_galley = ui.painter().layout_no_wrap(word.to_string(), word_font, fg);
    let pad_h = 12.0_f32;
    let pad_v = 5.0_f32;
    let gap = 2.0_f32;
    let content_w = code_galley.size().x.max(word_galley.size().x);
    let content_h = code_galley.size().y + gap + word_galley.size().y;
    let size = egui::vec2(content_w + pad_h * 2.0, content_h + pad_v * 2.0);

    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    if ui.is_rect_visible(rect) {
        ui.painter()
            .rect_filled(rect, egui::CornerRadius::same(7), bg);
        let code_pos = egui::pos2(
            rect.center().x - code_galley.size().x / 2.0,
            rect.top() + pad_v,
        );
        let word_pos = egui::pos2(
            rect.center().x - word_galley.size().x / 2.0,
            code_pos.y + code_galley.size().y + gap,
        );
        ui.painter().galley(code_pos, code_galley, fg);
        ui.painter().galley(word_pos, word_galley, fg);
    }
    response.on_hover_text(full_title.to_string())
}

/// Bar heights, in px, left to right (design spec §3 / M4a Task 7 brief):
/// bottom-aligned, lit per [`signal_meter`]'s bar count.
const SIGNAL_BAR_HEIGHTS: [f32; 4] = [8.0, 12.0, 16.0, 20.0];

fn signal_class_color(palette: &Palette, class: SignalClass) -> egui::Color32 {
    match class {
        SignalClass::Good => palette.good,
        SignalClass::Warn => palette.warnfg,
        SignalClass::Faint => palette.faint,
    }
}

fn signal_meter_widget(
    ui: &mut egui::Ui,
    palette: &Palette,
    metrics: Option<&chrona_dsp::MetricsSnapshot>,
) {
    let (bars, label, class) = signal_meter(metrics);
    let color = signal_class_color(palette, class);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 9.0;
        ui.label(strip_label(palette, "Signal"));
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 3.0;
            for (i, h) in SIGNAL_BAR_HEIGHTS.iter().enumerate() {
                let lit = (i as u8) < bars;
                let bar_color = if lit { color } else { palette.border2 };
                let max_h = SIGNAL_BAR_HEIGHTS[SIGNAL_BAR_HEIGHTS.len() - 1];
                let (rect, _resp) =
                    ui.allocate_exact_size(egui::vec2(5.0, max_h), egui::Sense::hover());
                if ui.is_rect_visible(rect) {
                    let bar_rect = egui::Rect::from_min_max(
                        egui::pos2(rect.left(), rect.bottom() - h),
                        egui::pos2(rect.right(), rect.bottom()),
                    );
                    ui.painter()
                        .rect_filled(bar_rect, egui::CornerRadius::same(2), bar_color);
                }
            }
        });
        ui.label(egui::RichText::new(label).size(12.0).color(color));
    });
}

fn elapsed_clock(ui: &mut egui::Ui, palette: &Palette, state: &SessionPanelState) {
    let elapsed_s = state
        .recording_started_at
        .map(|t| t.elapsed().as_secs())
        .unwrap_or(0);
    ui.label(
        egui::RichText::new(format!("● {}", rec_elapsed_label(elapsed_s)))
            .monospace()
            .size(12.0)
            .color(palette.rec),
    );
}

/// The "REPLAY <name>" mode line (M3 semantics/copy, controller ruling:
/// lives here, not in the health-banner slot) plus "· done" once
/// `replay_done`, and the "Back to live" button. Restyled onto
/// `palette.accent` (a soft, low-alpha fill via `theme::with_alpha` — Task 6
/// precedent allows deriving alpha variants of palette colors this way)
/// instead of M3's raw dark-blue RGB(30, 60, 130) fill.
///
/// `state.replay_name` is set optimistically by the "Open recording…"
/// handler before the engine confirms the switch. If `SwitchSource` then
/// fails (bad/corrupt WAV), `source_kind` never becomes `Replay`, so this
/// function is simply never called — the stale name sits unused until the
/// next successful pick overwrites it.
fn replay_mode_line(
    ui: &mut egui::Ui,
    palette: &Palette,
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
    // A plain nested `ui.horizontal` (left-to-right, regardless of the
    // parent strip's own right-to-left layout — see `position_strip`'s
    // trailing `with_layout` block) so the pill visually precedes the
    // button in normal reading order without depending on the parent's
    // add-order-is-reversed convention.
    ui.horizontal(|ui| {
        egui::Frame::new()
            .fill(crate::theme::with_alpha(palette.accent, 40))
            .corner_radius(6.0)
            .inner_margin(egui::Margin::symmetric(8, 4))
            .show(ui, |ui| {
                ui.label(egui::RichText::new(text).color(palette.accent));
            });
        if ui.button("Back to live").clicked() {
            state.replay_name = None;
            engine.send(ControlMsg::SwitchSource(SourceSpec::Mic {
                device_id: last_device.clone(),
            }));
        }
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

    #[test]
    fn position_codes_render_two_line_data() {
        let expected = [
            ("DU", "Dial up", "up"),
            ("DD", "Dial down", "down"),
            ("CU", "Crown up", "up"),
            ("CD", "Crown down", "down"),
            ("CL", "Crown left", "left"),
            ("CR", "Crown right", "right"),
        ];
        assert_eq!(POSITIONS.len(), expected.len());
        for (i, (code, full, word)) in expected.iter().enumerate() {
            assert_eq!(POSITIONS[i], (*code, *full), "POSITIONS[{i}]");
            assert_eq!(position_word(full), *word, "position_word({full:?})");
        }
    }
}
