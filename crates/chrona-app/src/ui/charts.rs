//! Charts view (M4a Task 8, mockup: "Chart"): the time-axis beat trace and
//! amplitude strip that replace M3's paper tape (`ui::instrument`, deleted
//! by this task). Ports the mockup's `drawBeat`/`drawAmp` canvas routines
//! (`docs/superpowers/specs/assets/2026-08-23-chrona-redesign.dc.html`)
//! onto `egui::Painter`, binding geometry — pad constants, gridline step
//! tables, dot radius, wrap-jump segmentation — verbatim. All the actual
//! trace MATH (accumulation, view transforms, trend, amplitude range) stays
//! in `presenter::trace` (Task 4); the pure functions below are thin
//! coordinate mappers over it, TDD'd on their own. The egui-facing
//! rendering that follows is exercised only by `cargo build` + the
//! workspace test suite (no window in CI) — same split as
//! `cards.rs`/`toolbar.rs`/`strip.rs`.

use std::collections::VecDeque;
use std::f32::consts::FRAC_PI_2;

use chrona_dsp::MetricsSnapshot;
use eframe::egui;

use crate::presenter::{
    AmpAccum, BeatAccum, GAP_BREAK_S, TraceView, Trend, WRAP_PRESETS_MS, amp_range, trend,
    wrap_signed, x_step_s, y_frac, y_step_ms,
};
use crate::theme::Palette;
use crate::ui::cards::amplitude_cell;

// ---------------------------------------------------------------------
// Pure geometry / segmentation helpers (TDD'd below in `mod tests`).
// ---------------------------------------------------------------------

/// One chart's plot padding, in screen px (mockup `drawBeat`/`drawAmp`:
/// `padL`/`padR`/`padT`/`padB`).
#[derive(Debug, Clone, Copy)]
struct Pad {
    l: f32,
    r: f32,
    t: f32,
    b: f32,
}

/// Beat-trace padding (mockup `drawBeat`: `padL 46, padR 12, padT 12,
/// padB 26`).
const BEAT_PAD: Pad = Pad {
    l: 46.0,
    r: 12.0,
    t: 12.0,
    b: 26.0,
};

/// Amplitude-strip padding (mockup `drawAmp`: `padL 46, padR 12, padT 8,
/// padB 20`).
const AMP_PAD: Pad = Pad {
    l: 46.0,
    r: 12.0,
    t: 8.0,
    b: 20.0,
};

impl Pad {
    /// The plot rect: `rect` shrunk by this padding on each side.
    fn plot_rect(&self, rect: egui::Rect) -> egui::Rect {
        egui::Rect::from_min_max(
            egui::pos2(rect.left() + self.l, rect.top() + self.t),
            egui::pos2(rect.right() - self.r, rect.bottom() - self.b),
        )
    }
}

/// Screen X for stream time `t` within the visible window `[t0, t0+span_s)`
/// (mockup: `X(t) = padL + pw*(t-t0)/span`, shared by `drawBeat`/`drawAmp`).
fn x_of(t: f64, t0: f64, span_s: f64, plot: egui::Rect) -> f32 {
    plot.left() + (plot.width() as f64 * (t - t0) / span_s) as f32
}

/// Screen Y for a beat offset WRAPPED into `wrap_ms` (mockup `drawBeat`'s
/// `Y(wrapV(v))`, used for dots/trend). A thin wrapper over
/// `presenter::trace::y_frac` — not a reimplementation.
fn y_of_beat(offset_ms: f64, wrap_ms: f64, plot: egui::Rect) -> f32 {
    plot.top() + (plot.height() as f64 * (0.5 - y_frac(offset_ms, wrap_ms))) as f32
}

/// Screen Y for an ALREADY-in-range grid coordinate `v_ms`
/// (`-wrap_ms/2..=wrap_ms/2`) — gridlines ONLY. Must NOT route through
/// `y_of_beat`/`wrap_signed`: the grid's own bottom line at exactly
/// `-wrap_ms/2` would otherwise wrap around to the top edge
/// (`wrap_signed`'s domain is the half-open `(-half, half]` — see its doc
/// comment in `presenter::trace`), which is correct for DATA but wrong for
/// an axis label. Mockup's `Y` function itself never wraps; wrapping
/// (`wrapV`) is applied separately, only to dot/trend values, before
/// calling it — this is that same unwrapped `Y`.
fn y_of_beat_grid(v_ms: f64, wrap_ms: f64, plot: egui::Rect) -> f32 {
    plot.top() + (plot.height() as f64 * (0.5 - v_ms / wrap_ms)) as f32
}

/// Screen Y for an amplitude degree value within `[lo, hi]` (mockup
/// `drawAmp`'s `Y(v) = padT + ph*(1-(v-lo)/(hi-lo))`).
fn y_of_amp(deg: f64, lo: f64, hi: f64, plot: egui::Rect) -> f32 {
    plot.top() + (plot.height() as f64 * (1.0 - (deg - lo) / (hi - lo))) as f32
}

/// Beat dot radius, px (mockup `drawBeat`: `r = span > 200 ? 1.3 : 1.8`).
fn dot_radius(span_s: f64) -> f32 {
    if span_s > 200.0 { 1.3 } else { 1.8 }
}

/// "±2 ms" / "±5 ms" / "±10 ms" / "±25 ms" — label for a
/// `presenter::trace::WRAP_PRESETS_MS` entry (full wrap width, ms).
/// Adapted from M3's `ui::instrument::wrap_label` for the new ±2/±5/±10/±25
/// preset set (M3's was ±1/±2/±5).
fn wrap_label(wrap_ms: f64) -> String {
    format!("±{} ms", (wrap_ms / 2.0).round() as i64)
}

/// Samples the rate-trend line across `[t0, t0+span_s]` at 301 points (300
/// sub-intervals, mockup `drawBeat`: `t += span/300`), wrapping each sample
/// into `wrap_ms` and splitting into a new polyline segment whenever a
/// consecutive pair jumps by more than `wrap_ms/4` (mockup's `moveTo` rule
/// — see the module doc comment's reconciliation note in the task report:
/// the mockup's local `wrap` var is the HALF-width, and its threshold is
/// `wrap/2` OF THAT, i.e. `(wrap_ms/2)/2 = wrap_ms/4` in our full-width
/// terms). Each segment is a `Vec<(t_s, wrapped_offset_ms)>`; painting maps
/// these through `y_of_beat_grid` (the values are ALREADY wrapped here, so
/// the non-wrapping mapper is correct — re-wrapping an already-wrapped
/// value is a no-op except exactly at the `-half` boundary, which
/// `y_of_beat_grid` (unlike `y_of_beat`) handles correctly).
fn trend_segments(tr: &Trend, t0: f64, span_s: f64, wrap_ms: f64) -> Vec<Vec<(f64, f64)>> {
    const N: i64 = 300;
    let threshold = wrap_ms / 4.0; // (wrap_ms/2)/2 — see this fn's doc comment
    let mut segments: Vec<Vec<(f64, f64)>> = Vec::new();
    let mut prev: Option<f64> = None;
    for i in 0..=N {
        let t = t0 + span_s * (i as f64 / N as f64);
        let raw = tr.anchor_off_ms + tr.slope_ms_per_s * (t - tr.anchor_t);
        let v = wrap_signed(raw, wrap_ms);
        match prev {
            Some(p) if (v - p).abs() <= threshold => {
                segments
                    .last_mut()
                    .expect("prev is Some only after a segment was pushed")
                    .push((t, v));
            }
            _ => segments.push(vec![(t, v)]),
        }
        prev = Some(v);
    }
    segments
}

/// Splits `points` (t, deg) into polyline segments for the amplitude strip:
/// only those within `[t0, t0+span_s]`, broken wherever consecutive KEPT
/// points are more than `gap_break_s` apart in time (design spec §11 /
/// HONESTY: never interpolate a line across an unobserved gap — the mockup
/// demo has no gaps in its synthetic data, so this segmentation has no
/// direct mockup counterpart; it's this task's own honesty requirement,
/// same shape as the trend/dot windowing above).
fn amp_segments(
    points: &VecDeque<(f64, f64)>,
    t0: f64,
    span_s: f64,
    gap_break_s: f64,
) -> Vec<Vec<(f64, f64)>> {
    let end = t0 + span_s;
    let mut segments: Vec<Vec<(f64, f64)>> = Vec::new();
    for &(t, deg) in points.iter().filter(|&&(t, _)| t >= t0 && t <= end) {
        match segments.last_mut() {
            Some(seg)
                if t - seg.last().expect("segments are never pushed empty").0 <= gap_break_s =>
            {
                seg.push((t, deg));
            }
            _ => segments.push(vec![(t, deg)]),
        }
    }
    segments
}

// ---------------------------------------------------------------------
// Rendering: the charts section. Exercised by `cargo build` + the
// workspace test suite; no window in CI, so nothing below is unit-tested
// directly — see the module doc comment.
// ---------------------------------------------------------------------

/// Amplitude strip's fixed height, px (mockup: `height:110px`).
const AMP_STRIP_H: f32 = 110.0;
/// Estimated height of `amp_header_row`'s single text row, px — used only
/// to budget the beat trace's height below; not pixel-exact (this whole
/// layer isn't pixel-tested, see the module doc comment).
const AMP_HEADER_H: f32 = 24.0;
/// Beat-trace panel's height floor (mockup: `min-height:260px`).
const BEAT_MIN_H: f32 = 260.0;
/// Vertical gap inserted between the charts section's own rows/panels.
const SECTION_GAP: f32 = 8.0;

/// Per-frame charts UI state (M4a Task 8) not already owned by
/// `presenter::TraceView`: currently just the wrap-band selection.
/// `TapeUiState` (M3) dies with `ui::instrument` — this is its Task 8
/// replacement, one field narrower (the old `max_beats` 800-floor was
/// tape-only bookkeeping; `BeatAccum`/`AmpAccum` have their own horizon
/// now, `presenter::trace::HORIZON_S`, so it isn't ported).
#[derive(Debug, Clone, Copy)]
pub struct ChartsUiState {
    /// Full wrap width, ms — one of `presenter::trace::WRAP_PRESETS_MS`.
    pub wrap_ms: f64,
}

impl Default for ChartsUiState {
    fn default() -> Self {
        // ±5 ms (mockup default `view.wrap = 5`; WRAP_PRESETS_MS[1] = 10.0
        // is the matching FULL width).
        ChartsUiState { wrap_ms: 10.0 }
    }
}

/// Borrowed, per-frame context `charts_section` needs beyond `TraceView`/
/// `ChartsUiState`: bundled purely to keep the argument count sane, same
/// shape as `toolbar::ToolbarCtx`/`strip::StripCtx`.
pub struct ChartsCtx<'a> {
    pub beat_accum: &'a BeatAccum,
    pub amp_accum: &'a AmpAccum,
    pub metrics: Option<&'a MetricsSnapshot>,
}

/// Renders the full charts section (mockup: "Chart" — the beat-trace
/// header row, the beat trace itself filling the available height down to
/// a `BEAT_MIN_H` floor, the amplitude header, and the fixed-height
/// amplitude strip). Replaces M3's `ui::instrument_view` in `ChronaApp`'s
/// `CentralPanel`. Any space left below (Task 9's session-history/
/// position-comparison cards) is simply not drawn into.
pub fn charts_section(
    ui: &mut egui::Ui,
    palette: &Palette,
    view: &mut TraceView,
    charts_ui: &mut ChartsUiState,
    ctx: ChartsCtx<'_>,
) {
    chart_header_row(ui, palette, view, &mut charts_ui.wrap_ms);
    ui.add_space(SECTION_GAP);

    let reserved_below = SECTION_GAP + AMP_HEADER_H + SECTION_GAP + AMP_STRIP_H;
    let beat_h = (ui.available_height() - reserved_below).max(BEAT_MIN_H);
    beat_trace_panel(
        ui,
        palette,
        view,
        charts_ui.wrap_ms,
        ctx.beat_accum,
        ctx.metrics,
        beat_h,
    );

    ui.add_space(SECTION_GAP);
    amp_header_row(ui, palette);
    ui.add_space(SECTION_GAP);
    amplitude_panel(
        ui,
        palette,
        view,
        ctx.amp_accum,
        ctx.metrics,
        ctx.beat_accum.newest_t(),
        AMP_STRIP_H,
    );
}

// ---------------------------------------------------------------------
// Beat-trace header row
// ---------------------------------------------------------------------

/// "Beat trace" / "Amplitude" section-header text (mockup:
/// `font-weight:600; font-size:13px`).
fn section_title(text: &str, color: egui::Color32) -> egui::RichText {
    egui::RichText::new(text)
        .font(egui::FontId::new(
            13.0,
            egui::FontFamily::Name(crate::theme::FAMILY_SEMIBOLD.into()),
        ))
        .color(color)
}

/// The beat-trace panel's header row (mockup: "Beat trace" semibold, the
/// Tick/Tock/Rate-trend legend, a spacer, the pan/zoom hint, a `Live ⏵`
/// chip when panned away from live, the `+`/`−`/`Fit` buttons, and the
/// wrap-band selector).
fn chart_header_row(ui: &mut egui::Ui, palette: &Palette, view: &mut TraceView, wrap_ms: &mut f64) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 14.0;
        ui.label(section_title("Beat trace", palette.text));
        legend_dot_item(ui, palette, palette.tick, "Tick");
        legend_dot_item(ui, palette, palette.accent, "Tock");
        legend_line_item(ui, palette, palette.good, "Rate trend");

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            wrap_combo(ui, wrap_ms);
            ui.add_space(6.0);
            if zoom_button(ui, palette, "Fit", egui::vec2(40.0, 28.0), 12.0).clicked() {
                view.fit();
            }
            if zoom_button(ui, palette, "−", egui::vec2(28.0, 28.0), 15.0).clicked() {
                view.zoom(1.5);
            }
            if zoom_button(ui, palette, "+", egui::vec2(28.0, 28.0), 15.0).clicked() {
                view.zoom(1.0 / 1.5);
            }
            ui.add_space(6.0);
            if !view.follow && live_chip(ui, palette).clicked() {
                view.fit();
            }
            hint_label(ui, palette);
        });
    });
}

fn legend_dot_item(ui: &mut egui::Ui, palette: &Palette, color: egui::Color32, text: &str) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let (rect, _resp) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
        if ui.is_rect_visible(rect) {
            ui.painter().circle_filled(rect.center(), 4.0, color);
        }
        ui.label(egui::RichText::new(text).size(12.0).color(palette.muted));
    });
}

fn legend_line_item(ui: &mut egui::Ui, palette: &Palette, color: egui::Color32, text: &str) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let (rect, _resp) = ui.allocate_exact_size(egui::vec2(14.0, 2.0), egui::Sense::hover());
        if ui.is_rect_visible(rect) {
            ui.painter().rect_filled(rect, 0.0, color);
        }
        ui.label(egui::RichText::new(text).size(12.0).color(palette.muted));
    });
}

fn hint_label(ui: &mut egui::Ui, palette: &Palette) {
    ui.label(
        egui::RichText::new("scroll to zoom · drag to pan")
            .size(11.0)
            .color(palette.faint),
    );
}

/// `Live ⏵` chip (mockup has no counterpart — the mockup's demo data never
/// pans away from live; this is this task's own affordance for getting
/// back). Only rendered by the caller when `!view.follow`; clicking it
/// calls `TraceView::fit`.
fn live_chip(ui: &mut egui::Ui, palette: &Palette) -> egui::Response {
    ui.add(
        egui::Button::new(
            egui::RichText::new("Live ⏵")
                .size(12.0)
                .color(palette.accent_ink),
        )
        .fill(palette.accent)
        .corner_radius(7.0),
    )
}

fn zoom_button(
    ui: &mut egui::Ui,
    palette: &Palette,
    label: &str,
    size: egui::Vec2,
    font_size: f32,
) -> egui::Response {
    ui.add_sized(
        size,
        egui::Button::new(
            egui::RichText::new(label)
                .size(font_size)
                .color(palette.text),
        )
        .fill(palette.panel2)
        .stroke(egui::Stroke::new(1.0, palette.border2))
        .corner_radius(7.0),
    )
}

fn wrap_combo(ui: &mut egui::Ui, wrap_ms: &mut f64) {
    egui::ComboBox::from_label("Wrap")
        .selected_text(wrap_label(*wrap_ms))
        .width(64.0)
        .show_ui(ui, |ui| {
            for preset in WRAP_PRESETS_MS {
                ui.selectable_value(wrap_ms, preset, wrap_label(preset));
            }
        });
}

// ---------------------------------------------------------------------
// Beat-trace panel
// ---------------------------------------------------------------------

fn empty_state(painter: &egui::Painter, rect: egui::Rect, palette: &Palette, text: &str) {
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        text,
        egui::FontId::proportional(13.0),
        palette.faint,
    );
}

/// Renders the beat-trace panel: background/border, the empty states (no
/// nominal to grid against, or a nominal but no events placed yet), or the
/// gridlines + axis labels + rotated axis title + tic/toc dots + rate-trend
/// line — plus the wheel-zoom/drag-pan interactions (mockup: `drawBeat` +
/// `bindPanZoom`).
fn beat_trace_panel(
    ui: &mut egui::Ui,
    palette: &Palette,
    view: &mut TraceView,
    wrap_ms: f64,
    beat_accum: &BeatAccum,
    metrics: Option<&MetricsSnapshot>,
    height: f32,
) {
    let width = ui.available_width();
    let (response, painter) =
        ui.allocate_painter(egui::vec2(width, height), egui::Sense::click_and_drag());
    let rect = response.rect;
    painter.rect_filled(rect, 12.0, palette.chartbg);
    painter.rect_stroke(
        rect,
        12.0,
        egui::Stroke::new(1.0, palette.border),
        egui::StrokeKind::Inside,
    );

    if metrics.and_then(|m| m.bph_nominal).is_none() {
        empty_state(
            &painter,
            rect,
            palette,
            "no beat grid — select or detect a beat rate",
        );
        return;
    }
    let Some(newest) = beat_accum.newest_t() else {
        empty_state(&painter, rect, palette, "listening…");
        return;
    };

    let end = view.end_t(newest);
    let t0 = end - view.span_s;
    let plot = BEAT_PAD.plot_rect(rect);

    draw_beat_gridlines(&painter, plot, wrap_ms, t0, view.span_s, end, palette);
    draw_rotated_axis_title(&painter, rect, plot, palette);

    let clipped = painter.with_clip_rect(plot);
    let radius = dot_radius(view.span_s);
    for p in beat_accum.points() {
        if p.t_s < t0 || p.t_s > end {
            continue;
        }
        let pos = egui::pos2(
            x_of(p.t_s, t0, view.span_s, plot),
            y_of_beat(p.offset_ms, wrap_ms, plot),
        );
        let color = if p.is_tic {
            palette.tick
        } else {
            palette.accent
        };
        clipped.circle_filled(pos, radius, color);
    }

    if let Some(tr) = trend(beat_accum.points(), metrics.and_then(|m| m.rate_s_per_day)) {
        for seg in trend_segments(&tr, t0, view.span_s, wrap_ms) {
            if seg.len() < 2 {
                continue;
            }
            let pts: Vec<egui::Pos2> = seg
                .iter()
                .map(|&(t, v)| {
                    egui::pos2(
                        x_of(t, t0, view.span_s, plot),
                        y_of_beat_grid(v, wrap_ms, plot),
                    )
                })
                .collect();
            clipped.line(pts, egui::Stroke::new(1.5, palette.good));
        }
    }

    // Interactions (mockup `bindPanZoom`): wheel-zoom while hovering
    // anywhere over the panel (the mockup's listener is on the whole
    // canvas, padding included, not just the inner plot), drag-pan
    // anywhere the drag started on this panel. Both need a resolved
    // window, guaranteed here (`newest` is `Some`).
    if response.hovered() {
        // egui 0.36 exposes `smooth_scroll_delta`, not the brief's
        // tentative `raw_scroll_delta` (no such field exists on this
        // version's `InputState`).
        //
        // Sign: egui's `smooth_scroll_delta.y` is positive when the
        // CONTENT moves down — i.e. the user scrolled UP — the opposite
        // sign of DOM `deltaY` for the same physical gesture. This is
        // confirmed by egui's own `WheelState::smooth_wheel_delta` doc
        // comment (`wheel_state.rs`) AND, unambiguously, by
        // `ScrollArea`'s own application of it (`scroll_area.rs`):
        // `scrolling_up = ... && scroll_delta > 0.0`, then
        // `state.offset -= scroll_delta` — positive delta DECREASES the
        // scroll offset, i.e. scrolls UP/toward the top. DOM's `deltaY`
        // is the opposite: positive increases `scrollTop`, i.e. scrolls
        // DOWN/toward the bottom. The mockup's `deltaY > 0` (scroll down)
        // zooms OUT, so the equivalent egui gesture is `smooth_scroll_
        // delta.y < 0` (also "scroll down" in DOM terms) zooming OUT.
        let scroll_y = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll_y < 0.0 {
            view.zoom(1.15); // DOM-equivalent "scroll down" → zoom OUT
        } else if scroll_y > 0.0 {
            view.zoom(1.0 / 1.15); // DOM-equivalent "scroll up" → zoom IN
        }
    }
    if response.dragged() && plot.width() > 0.0 {
        // Positive dx (dragging right) must PAN BACKWARD (reveal older
        // data) — see the task report's pan-sign reconciliation note.
        // `TraceView::pan`'s own convention is "positive dt_s pans
        // backward"; egui's `drag_delta().x` is positive-when-dragging-
        // right, the same sign the mockup's `dx = clientX - dragStartX`
        // uses — so no sign flip is needed here.
        let dt_s = response.drag_delta().x as f64 / plot.width() as f64 * view.span_s;
        view.pan(dt_s, newest);
    }
    if response.hovered() || response.dragged() {
        ui.ctx().set_cursor_icon(if response.dragged() {
            egui::CursorIcon::Grabbing
        } else {
            egui::CursorIcon::Grab
        });
    }
}

fn draw_beat_gridlines(
    painter: &egui::Painter,
    plot: egui::Rect,
    wrap_ms: f64,
    t0: f64,
    span_s: f64,
    end: f64,
    palette: &Palette,
) {
    let font = egui::FontId::monospace(11.0);
    let half = wrap_ms / 2.0;
    let step = y_step_ms(wrap_ms);
    let mut v = -half;
    while v <= half + 1e-9 {
        let y = y_of_beat_grid(v, wrap_ms, plot);
        let color = if v.abs() < 1e-9 {
            palette.grid0
        } else {
            palette.grid
        };
        painter.line_segment(
            [egui::pos2(plot.left(), y), egui::pos2(plot.right(), y)],
            egui::Stroke::new(1.0, color),
        );
        let sign = if v > 0.0 { "+" } else { "" };
        let label = if step < 1.0 {
            format!("{sign}{v:.1}")
        } else {
            format!("{sign}{:.0}", v.round())
        };
        painter.text(
            egui::pos2(plot.left() - 8.0, y),
            egui::Align2::RIGHT_CENTER,
            label,
            font.clone(),
            palette.faint,
        );
        v += step;
    }

    let x_step = x_step_s(span_s);
    let mut t = (t0 / x_step).ceil() * x_step;
    while t <= end + 1e-9 {
        let x = x_of(t, t0, span_s, plot);
        painter.line_segment(
            [egui::pos2(x, plot.top()), egui::pos2(x, plot.bottom())],
            egui::Stroke::new(1.0, palette.grid),
        );
        let ago = (end - t).round() as i64;
        let label = if ago == 0 {
            "now".to_string()
        } else {
            format!("-{ago}s")
        };
        painter.text(
            egui::pos2(x, plot.bottom() + BEAT_PAD.b / 2.0),
            egui::Align2::CENTER_CENTER,
            label,
            font.clone(),
            palette.faint,
        );
        t += x_step;
    }
}

/// The rotated "offset (ms)" y-axis title (mockup: translate to the plot's
/// left-pad gutter at its vertical center, then `ctx.rotate(-PI/2)`). Uses
/// `TextShape::with_angle_and_anchor` rather than the plain `with_angle`
/// the brief sketches, so the text's own CENTER lands on the pivot:
/// `with_angle` alone rotates around the galley's top-left corner, which
/// would need extra manual offset math to center, while
/// `with_angle_and_anchor` is epaint's built-in solution for exactly that.
fn draw_rotated_axis_title(
    painter: &egui::Painter,
    rect: egui::Rect,
    plot: egui::Rect,
    palette: &Palette,
) {
    let font = egui::FontId::proportional(11.0);
    let galley = painter.layout_no_wrap("offset (ms)".to_string(), font, palette.faint);
    let pivot = egui::pos2(rect.left() + 12.0, plot.top() + plot.height() / 2.0);
    let text_shape = egui::epaint::TextShape::new(pivot, galley, palette.faint)
        .with_angle_and_anchor(-FRAC_PI_2, egui::Align2::CENTER_CENTER);
    painter.add(text_shape);
}

// ---------------------------------------------------------------------
// Amplitude strip
// ---------------------------------------------------------------------

fn amp_header_row(ui: &mut egui::Ui, palette: &Palette) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        ui.label(section_title("Amplitude", palette.text));
        ui.label(
            egui::RichText::new("same time scale")
                .size(11.0)
                .color(palette.faint),
        );
    });
}

/// Renders the amplitude strip: background/border, the below-tier empty
/// state (reusing `ui::cards::amplitude_cell`'s gate-reason caption), or
/// the lo/mid/hi gridlines + the gap-broken amplitude polyline, on the
/// SAME time window as the beat trace above (mockup: "same time scale").
fn amplitude_panel(
    ui: &mut egui::Ui,
    palette: &Palette,
    view: &TraceView,
    amp_accum: &AmpAccum,
    metrics: Option<&MetricsSnapshot>,
    newest_t: Option<f64>,
    height: f32,
) {
    let width = ui.available_width();
    let (response, painter) = ui.allocate_painter(egui::vec2(width, height), egui::Sense::hover());
    let rect = response.rect;
    painter.rect_filled(rect, 12.0, palette.chartbg);
    painter.rect_stroke(
        rect,
        12.0,
        egui::Stroke::new(1.0, palette.border),
        egui::StrokeKind::Inside,
    );

    let has_amplitude = metrics.and_then(|m| m.amplitude_deg).is_some();
    if !has_amplitude {
        let (_value, reason) = amplitude_cell(metrics);
        empty_state(
            &painter,
            rect,
            palette,
            &format!("amplitude requires Tier 3 · {reason}"),
        );
        return;
    }
    // Defensive: `amplitude_deg` is Some but our own `BeatAccum` hasn't
    // placed a point yet. Both are fed from the same per-frame snapshot in
    // `ChronaApp::ui` (see `app.rs`), so this shouldn't happen in
    // practice — but a chart honesty rule (spec §3/§10/§11: never draw
    // what wasn't observed) means falling back to a caption here rather
    // than computing a window from a `None`.
    let Some(newest) = newest_t else {
        empty_state(&painter, rect, palette, "listening…");
        return;
    };

    let end = view.end_t(newest);
    let t0 = end - view.span_s;
    let plot = AMP_PAD.plot_rect(rect);
    let (lo, hi) = amp_range(amp_accum.points());

    draw_amp_gridlines(&painter, plot, lo, hi, palette);

    let clipped = painter.with_clip_rect(plot);
    for seg in amp_segments(amp_accum.points(), t0, view.span_s, GAP_BREAK_S) {
        if seg.len() < 2 {
            continue;
        }
        let pts: Vec<egui::Pos2> = seg
            .iter()
            .map(|&(t, deg)| {
                egui::pos2(x_of(t, t0, view.span_s, plot), y_of_amp(deg, lo, hi, plot))
            })
            .collect();
        clipped.line(pts, egui::Stroke::new(1.5, palette.accent));
    }
}

fn draw_amp_gridlines(
    painter: &egui::Painter,
    plot: egui::Rect,
    lo: f64,
    hi: f64,
    palette: &Palette,
) {
    let font = egui::FontId::monospace(11.0);
    for v in [lo, (lo + hi) / 2.0, hi] {
        let y = y_of_amp(v, lo, hi, plot);
        painter.line_segment(
            [egui::pos2(plot.left(), y), egui::pos2(plot.right(), y)],
            egui::Stroke::new(1.0, palette.grid),
        );
        painter.text(
            egui::pos2(plot.left() - 8.0, y),
            egui::Align2::RIGHT_CENTER,
            format!("{v:.0}°"),
            font.clone(),
            palette.faint,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_rect() -> egui::Rect {
        egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(400.0, 300.0))
    }

    #[test]
    fn pad_constants_match_the_mockup() {
        assert_eq!(
            (BEAT_PAD.l, BEAT_PAD.r, BEAT_PAD.t, BEAT_PAD.b),
            (46.0, 12.0, 12.0, 26.0)
        );
        assert_eq!(
            (AMP_PAD.l, AMP_PAD.r, AMP_PAD.t, AMP_PAD.b),
            (46.0, 12.0, 8.0, 20.0)
        );
    }

    #[test]
    fn x_of_and_y_of_round_trip_at_the_pad_constants() {
        let rect = test_rect();
        let plot = BEAT_PAD.plot_rect(rect);
        assert_eq!(plot.left(), rect.left() + 46.0);
        assert_eq!(plot.right(), rect.right() - 12.0);
        assert_eq!(plot.top(), rect.top() + 12.0);
        assert_eq!(plot.bottom(), rect.bottom() - 26.0);

        // x_of: window start/end land exactly on the plot's left/right
        // edges; the midpoint lands at the plot's horizontal center.
        assert!((x_of(100.0, 100.0, 60.0, plot) - plot.left()).abs() < 1e-4);
        assert!((x_of(160.0, 100.0, 60.0, plot) - plot.right()).abs() < 1e-4);
        assert!((x_of(130.0, 100.0, 60.0, plot) - plot.center().x).abs() < 1e-3);

        // y_of_beat (wrapping): zero offset is vertical center; +half is
        // the top edge (up is fast, presenter::trace's binding convention).
        assert!((y_of_beat(0.0, 10.0, plot) - plot.center().y).abs() < 1e-3);
        assert!((y_of_beat(5.0, 10.0, plot) - plot.top()).abs() < 1e-3);

        // y_of_beat_grid (non-wrapping): -half is the BOTTOM edge — this is
        // exactly the case y_of_beat gets wrong (wrap_signed(-5,10) wraps
        // to +5, the half-open domain's boundary), which is why gridlines
        // use this separate mapper.
        assert!((y_of_beat_grid(-5.0, 10.0, plot) - plot.bottom()).abs() < 1e-3);
        assert!((y_of_beat_grid(5.0, 10.0, plot) - plot.top()).abs() < 1e-3);

        // y_of_amp: lo lands at the bottom edge, hi at the top.
        assert!((y_of_amp(240.0, 240.0, 300.0, plot) - plot.bottom()).abs() < 1e-3);
        assert!((y_of_amp(300.0, 240.0, 300.0, plot) - plot.top()).abs() < 1e-3);
    }

    #[test]
    fn dot_radius_table() {
        assert_eq!(dot_radius(50.0), 1.8);
        assert_eq!(dot_radius(200.0), 1.8, "span == 200 is NOT > 200");
        assert_eq!(dot_radius(200.001), 1.3);
        assert_eq!(dot_radius(300.0), 1.3);
    }

    #[test]
    fn wrap_label_pins() {
        assert_eq!(wrap_label(4.0), "±2 ms");
        assert_eq!(wrap_label(10.0), "±5 ms");
        assert_eq!(wrap_label(20.0), "±10 ms");
        assert_eq!(wrap_label(50.0), "±25 ms");
    }

    #[test]
    fn wrap_presets_ms_contains_two_ms_full_width_for_protocol_a2() {
        assert!(
            WRAP_PRESETS_MS.contains(&4.0),
            "protocol A.2's 'change wrap to ±2 ms' step must be selectable"
        );
    }

    #[test]
    fn trend_segment_breaks_at_wrap_jump() {
        let tr = Trend {
            anchor_t: 0.0,
            anchor_off_ms: 0.0,
            slope_ms_per_s: 1.0,
        };
        let wrap_ms = 10.0; // half = 5.0, break threshold = half/2 = 2.5
        let segments = trend_segments(&tr, 0.0, 30.0, wrap_ms);
        assert!(
            segments.len() >= 2,
            "a 30 ms sweep across a 10 ms wrap band must produce multiple segments, got {}",
            segments.len()
        );
        let threshold = wrap_ms / 4.0;
        for seg in &segments {
            for w in seg.windows(2) {
                assert!(
                    (w[1].1 - w[0].1).abs() <= threshold + 1e-9,
                    "no in-segment jump may exceed the wrap threshold: {} -> {}",
                    w[0].1,
                    w[1].1
                );
            }
        }
        let total_points: usize = segments.iter().map(Vec::len).sum();
        assert_eq!(total_points, 301, "301 samples across 300 sub-intervals");
    }

    #[test]
    fn trend_segments_stay_together_without_a_wrap_jump() {
        let tr = Trend {
            anchor_t: 0.0,
            anchor_off_ms: 0.0,
            slope_ms_per_s: 0.0,
        };
        let segments = trend_segments(&tr, 0.0, 30.0, 10.0);
        assert_eq!(
            segments.len(),
            1,
            "a flat trend never crosses the wrap edge"
        );
        assert_eq!(segments[0].len(), 301);
    }

    #[test]
    fn amp_segments_break_at_gaps_and_filter_the_window() {
        let mut pts = VecDeque::new();
        pts.push_back((0.0, 270.0));
        pts.push_back((1.0, 271.0));
        pts.push_back((2.0, 270.0));
        // gap of 3.5 s > GAP_BREAK_S (2.0 s):
        pts.push_back((5.5, 268.0));
        pts.push_back((6.5, 269.0));
        // outside the [0,10] window, must be excluded entirely:
        pts.push_back((100.0, 300.0));

        let segs = amp_segments(&pts, 0.0, 10.0, GAP_BREAK_S);
        assert_eq!(
            segs.len(),
            2,
            "one gap > GAP_BREAK_S splits into two segments"
        );
        assert_eq!(segs[0].len(), 3);
        assert_eq!(segs[1].len(), 2);
    }

    #[test]
    fn amp_segments_single_run_when_no_gap_exceeds_the_break() {
        let mut pts = VecDeque::new();
        for i in 0..5 {
            pts.push_back((i as f64, 270.0));
        }
        let segs = amp_segments(&pts, 0.0, 10.0, GAP_BREAK_S);
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].len(), 5);
    }
}
