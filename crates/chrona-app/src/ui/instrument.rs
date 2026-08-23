//! Instrument view (spec §6): the big-numerals strip and the paper-tape
//! `Painter`, replicating a mechanical timegrapher's readout. The only
//! place `chrona_dsp`/`EngineSnapshot` data meets `egui` — everything
//! numeric or geometric it needs comes pre-computed from the `presenter`
//! layer (T6), which stays `egui`-free by design.

use chrona_dsp::{AmplitudeGateFail, MetricsSnapshot, Tier};
use eframe::egui;

use crate::engine::EngineSnapshot;
use crate::presenter::{
    TapeDot, TapeParams, format_amplitude, format_beat_error, format_rate, source_label, tape_dots,
    tier_label,
};

/// Tape display knobs T9's controls row owns; `instrument_view` only reads
/// them each frame (the `&mut` in its signature is so T9 can mutate this
/// same value from its own widgets earlier in the frame).
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

/// Renders the instrument view: the numerals strip on top, the paper tape
/// filling whatever vertical space remains in `ui`.
pub fn instrument_view(ui: &mut egui::Ui, snap: &EngineSnapshot, tape_ui: &mut TapeUiState) {
    numerals_strip(ui, snap.metrics.as_ref());
    ui.separator();
    tape_panel(ui, snap, tape_ui);
}

// ---------------------------------------------------------------------
// Numerals strip
// ---------------------------------------------------------------------

fn numerals_strip(ui: &mut egui::Ui, metrics: Option<&MetricsSnapshot>) {
    ui.horizontal(|ui| match metrics {
        Some(m) => {
            ui.label(tier_badge(m.tier));
            if let Some(label) = source_label(m.rate_source) {
                ui.label(
                    egui::RichText::new(label)
                        .size(11.0)
                        .color(egui::Color32::from_gray(160)),
                );
            }
        }
        None => {
            ui.label(
                egui::RichText::new("NO SIGNAL")
                    .size(11.0)
                    .color(egui::Color32::from_gray(120)),
            );
        }
    });

    ui.columns(4, |cols| {
        let (rate_val, rate_cap) = rate_cell(metrics);
        let (be_val, be_cap) = beat_error_cell(metrics);
        let (amp_val, amp_cap) = amplitude_cell(metrics);
        let (bph_val, bph_cap) = bph_cell(metrics);
        numeral_cell(&mut cols[0], &rate_val, &rate_cap);
        numeral_cell(&mut cols[1], &be_val, &be_cap);
        numeral_cell(&mut cols[2], &amp_val, &amp_cap);
        numeral_cell(&mut cols[3], &bph_val, &bph_cap);
    });
}

/// One big numeral (spec: `RichText::size(34.0).monospace()`) plus its
/// small caption line.
fn numeral_cell(ui: &mut egui::Ui, value: &str, caption: &str) {
    ui.vertical(|ui| {
        ui.label(egui::RichText::new(value).size(34.0).monospace());
        ui.label(egui::RichText::new(caption).size(11.0));
    });
}

fn tier_badge(tier: Tier) -> egui::RichText {
    let color = match tier {
        Tier::T1 => egui::Color32::from_gray(180),
        Tier::T2 => egui::Color32::from_rgb(235, 175, 40),
        Tier::T3 => egui::Color32::from_rgb(95, 200, 120),
    };
    egui::RichText::new(tier_label(tier))
        .size(13.0)
        .strong()
        .color(color)
}

/// `(value, caption)` for the rate cell. `format_rate` already embeds the
/// `⚠ uncal` marker when uncalibrated; when there's no rate at all the
/// caption explains why (no defensible nominal BPH to detrend against —
/// spec §3.1). The presenter layer doesn't expose a more specific reason
/// than that (unlike amplitude's gate enum) so this is as precise as T8
/// can honestly be here.
fn rate_cell(metrics: Option<&MetricsSnapshot>) -> (String, String) {
    match metrics {
        Some(m) => {
            let caption = if m.rate_s_per_day.is_some() {
                "RATE"
            } else {
                "no BPH match"
            };
            (
                format_rate(m.rate_s_per_day, m.calibrated),
                caption.to_string(),
            )
        }
        None => ("—".to_string(), "RATE".to_string()),
    }
}

/// `(value, caption)` for the beat-error cell; only defined at tier ≥ T2.
fn beat_error_cell(metrics: Option<&MetricsSnapshot>) -> (String, String) {
    match metrics {
        Some(m) => {
            let caption = if m.beat_error_ms.is_some() {
                "BEAT ERROR"
            } else {
                "need Tier 2"
            };
            (format_beat_error(m.beat_error_ms), caption.to_string())
        }
        None => ("—".to_string(), "BEAT ERROR".to_string()),
    }
}

/// `(value, caption)` for the amplitude cell. When gated, `format_amplitude`
/// already embeds the gate reason in its string (e.g. `"— (out of
/// range)"`); that reason is pulled into the caption instead so the 34 pt
/// numeral itself stays a bare `"—"` rather than a long parenthetical.
fn amplitude_cell(metrics: Option<&MetricsSnapshot>) -> (String, String) {
    match metrics {
        Some(m) => match m.amplitude_deg {
            Some(_) => (
                format_amplitude(m.amplitude_deg, m.quality.amplitude_gate),
                "AMPLITUDE".to_string(),
            ),
            None => {
                let caption = match m.quality.amplitude_gate {
                    Some(AmplitudeGateFail::TicTocDisagree) => "tic/toc disagree",
                    Some(AmplitudeGateFail::OutOfRange) => "out of range",
                    _ => "need Tier 3",
                };
                ("—".to_string(), caption.to_string())
            }
        },
        None => ("—".to_string(), "AMPLITUDE".to_string()),
    }
}

/// `(value, caption)` for the BPH cell. Always renderable once `metrics`
/// exists (`bph_detected` has no `None` state); shows the snapped nominal
/// when the analyzer has one, else the raw detected value, tilded.
fn bph_cell(metrics: Option<&MetricsSnapshot>) -> (String, String) {
    match metrics {
        Some(m) => {
            let value = match m.bph_nominal {
                Some(n) => n.to_string(),
                None => format!("~{:.0}", m.bph_detected),
            };
            (value, "BPH".to_string())
        }
        None => ("—".to_string(), "BPH".to_string()),
    }
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
}
