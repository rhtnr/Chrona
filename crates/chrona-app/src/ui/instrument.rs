//! Instrument view: the paper-tape `Painter`, replicating a mechanical
//! timegrapher's readout. The only place `chrona_dsp`/`EngineSnapshot` data
//! meets `egui` — everything numeric or geometric it needs comes
//! pre-computed from the `presenter` layer, which stays `egui`-free by
//! design. The old big-numerals strip (spec §6) moved to `ui::cards`/
//! `ui::strip` in M4a Task 7; only the paper tape remains here (Task 8
//! redesigns it further).

use eframe::egui;

use crate::engine::EngineSnapshot;
use crate::presenter::{TapeDot, TapeParams, tape_dots};

/// Tape display knobs. `instrument_view` owns `wrap_ms` outright — its own
/// wrap selector (in `tape_panel`, below) is the only thing that mutates
/// it; the `&mut` in `instrument_view`'s signature is for that selector,
/// not for some other panel elsewhere in the frame. `max_beats` has no UI
/// control at all in M3 and stays fixed at the floor documented below.
#[derive(Debug, Clone, Copy)]
pub struct TapeUiState {
    /// Full display height, in milliseconds of deviation (e.g. `10.0` ⇒ the
    /// frame spans ±5 ms of wrap). See `presenter::TapeParams::wrap_ms`.
    pub wrap_ms: f64,
    /// Rolling window width, in beats, kept on screen. **800 is a floor —
    /// do not lower it.** The tape's constant-velocity index span can reach
    /// ~640 beats at the analyzer's ~32 s ring depth; a smaller
    /// `max_beats` can drive a `TapeDot::beat_x` negative (see
    /// `presenter::tape::tape_dots`'s windowing doc comment), which is why
    /// the paint loop below also clamps defensively rather than trusting
    /// the presenter's `0..=1` contract blindly.
    pub max_beats: usize,
}

impl Default for TapeUiState {
    fn default() -> Self {
        TapeUiState {
            wrap_ms: 10.0,
            max_beats: 800,
        }
    }
}

/// Tic dot color (Weishi-style two-color tape).
const TIC_COLOR: egui::Color32 = egui::Color32::from_rgb(255, 200, 60);
/// Toc dot color.
const TOC_COLOR: egui::Color32 = egui::Color32::from_rgb(90, 180, 255);

/// Wrap (y-scale) presets, full display height in ms. ±1 / ±2 / ±5 ms.
pub const WRAP_PRESETS: [f64; 3] = [2.0, 4.0, 10.0];

/// "±1 ms" / "±2 ms" / "±5 ms" — label for a wrap preset.
pub fn wrap_label(wrap_ms: f64) -> String {
    format!("±{} ms", (wrap_ms / 2.0).round() as i64)
}

/// Maps a normalized [`TapeDot`] into screen space inside `rect`.
/// `beat_x` runs left→right; `dev_y` runs **up** for a positive (fast)
/// deviation — the Watch-O-Scope convention (spec §6) — which is why the
/// height term is *subtracted* rather than added (screen y grows downward).
pub fn dot_to_screen(d: &TapeDot, rect: egui::Rect) -> egui::Pos2 {
    egui::Pos2::new(
        rect.left() + d.beat_x as f32 * rect.width(),
        rect.center().y - d.dev_y as f32 * rect.height(),
    )
}

/// Renders the instrument view: the wrap selector plus the paper tape
/// filling whatever vertical space remains in `ui`. The numerals strip that
/// used to render above these (spec §6) moved to `ui::cards`/`ui::strip`
/// (M4a Task 7) — `ChronaApp::ui` renders those separately, above this.
pub fn instrument_view(ui: &mut egui::Ui, snap: &EngineSnapshot, tape_ui: &mut TapeUiState) {
    wrap_selector(ui, tape_ui);
    tape_panel(ui, snap, tape_ui);
}

/// Slim right-aligned row picking `tape_ui.wrap_ms` among `WRAP_PRESETS`
/// (spec §6's adjustable y-scale; protocol A.2 exercises it). Sits directly
/// above the tape itself, so it's visible without resizing the window.
fn wrap_selector(ui: &mut egui::Ui, tape_ui: &mut TapeUiState) {
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            egui::ComboBox::from_label("wrap")
                .selected_text(wrap_label(tape_ui.wrap_ms))
                .show_ui(ui, |ui| {
                    for preset in WRAP_PRESETS {
                        ui.selectable_value(&mut tape_ui.wrap_ms, preset, wrap_label(preset));
                    }
                });
        });
    });
}

// ---------------------------------------------------------------------
// Paper tape
// ---------------------------------------------------------------------

fn tape_panel(ui: &mut egui::Ui, snap: &EngineSnapshot, tape_ui: &TapeUiState) {
    let remaining_size = ui.available_size();
    let (response, painter) = ui.allocate_painter(remaining_size, egui::Sense::hover());
    let rect = response.rect;
    painter.rect_filled(rect, 4.0, egui::Color32::from_gray(16));

    let Some(metrics) = snap.metrics.as_ref() else {
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "listening… place the watch near the microphone",
            egui::FontId::proportional(16.0),
            egui::Color32::from_gray(150),
        );
        return;
    };

    draw_gridlines(&painter, rect, tape_ui.wrap_ms);

    // No defensible nominal (Free mode, or Auto/Fixed that never snapped —
    // spec §3.1): nothing to detrend against, so the tape stays blank
    // (gridlines only) rather than plotting garbage against a made-up
    // nominal.
    let Some(bph_nominal) = metrics.bph_nominal else {
        return;
    };
    let params = TapeParams {
        bph_nominal,
        wrap_ms: tape_ui.wrap_ms,
        max_beats: tape_ui.max_beats,
    };
    for d in tape_dots(&snap.tape, &params) {
        // Presenter contract promises `beat_x` in `0..=1`; skip defensively
        // rather than trust it blindly (controller ruling — `max_beats`
        // below the 800 floor can otherwise compute a negative `beat_x`).
        if d.beat_x < 0.0 || d.beat_x > 1.0 {
            continue;
        }
        let color = if d.is_tic { TIC_COLOR } else { TOC_COLOR };
        painter.circle_filled(dot_to_screen(&d, rect), 1.6, color);
    }
}

/// Horizontal gridlines every 1 ms of deviation; the zero line is brighter.
fn draw_gridlines(painter: &egui::Painter, rect: egui::Rect, wrap_ms: f64) {
    let half_ms = (wrap_ms / 2.0).floor() as i64;
    for k in -half_ms..=half_ms {
        let dev_y = k as f64 / wrap_ms;
        let y = rect.center().y - dev_y as f32 * rect.height();
        let color = if k == 0 {
            egui::Color32::from_gray(70)
        } else {
            egui::Color32::from_gray(40)
        };
        painter.line_segment(
            [egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)],
            egui::Stroke::new(1.0, color),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect() -> egui::Rect {
        egui::Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(300.0, 120.0))
    }

    fn dot(beat_x: f64, dev_y: f64) -> TapeDot {
        TapeDot {
            beat_x,
            dev_y,
            is_tic: true,
        }
    }

    #[test]
    fn center_dot_lands_at_rect_center() {
        let r = rect();
        let pos = dot_to_screen(&dot(0.5, 0.0), r);
        assert!((pos.x - r.center().x).abs() < 1e-3, "x {}", pos.x);
        assert!((pos.y - r.center().y).abs() < 1e-3, "y {}", pos.y);
    }

    #[test]
    fn positive_half_deviation_reaches_the_top_edge() {
        let r = rect();
        let pos = dot_to_screen(&dot(0.5, 0.5), r);
        assert!(
            (pos.y - r.top()).abs() < 1e-3,
            "y {} vs top {}",
            pos.y,
            r.top()
        );
    }

    #[test]
    fn beat_x_one_reaches_the_right_edge() {
        let r = rect();
        let pos = dot_to_screen(&dot(1.0, 0.0), r);
        assert!(
            (pos.x - r.right()).abs() < 1e-3,
            "x {} vs right {}",
            pos.x,
            r.right()
        );
    }

    #[test]
    fn negative_deviation_lands_below_center() {
        let r = rect();
        let pos = dot_to_screen(&dot(0.5, -0.25), r);
        assert!(
            pos.y > r.center().y,
            "y {} should be below center {}",
            pos.y,
            r.center().y
        );
    }

    #[test]
    fn wrap_presets_are_exactly_one_two_five_ms() {
        assert_eq!(WRAP_PRESETS, [2.0, 4.0, 10.0]);
        assert!(
            WRAP_PRESETS.contains(&4.0),
            "protocol A.2's 'change wrap to ±2 ms' step must be selectable"
        );
    }

    #[test]
    fn wrap_label_formats_each_preset() {
        assert_eq!(wrap_label(2.0), "±1 ms");
        assert_eq!(wrap_label(4.0), "±2 ms");
        assert_eq!(wrap_label(10.0), "±5 ms");
    }

    #[test]
    fn tape_ui_state_default_wrap_is_five_ms() {
        // Pin: protocol A.2 starts at ±5 ms before switching to ±2 ms.
        assert_eq!(TapeUiState::default().wrap_ms, 10.0);
    }
}
