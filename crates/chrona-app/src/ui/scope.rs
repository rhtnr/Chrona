//! Scope view (M4 Task 12, binding spec §6: "stacked per-beat waveform,
//! ~20 ms, pulse markers"): the collapsible "Scope" section between the
//! amplitude strip (`ui::charts::charts_section`) and the bottom grid
//! (`ui::history_ui::bottom_grid`), rendering the newest per-beat waveform
//! windows the engine publishes on `EngineSnapshot::scope` while the view is
//! open (`Analyzer::beat_scope`, Task 11).
//!
//! **Open state -> engine gating.** `ControlMsg::SetScope(bool)`'s own doc
//! comment: `true` makes the engine pay for `Analyzer::beat_scope` on every
//! publish tick, `false` reverts `EngineSnapshot::scope` to `None` and stops
//! that work. `scope_section` owns the UI half of that contract: it compares
//! `*open` before and after this frame's header click and sends `SetScope`
//! ONLY on an actual flip (see its own doc comment for exactly where) —
//! never unconditionally per frame, which would defeat the "pulled only
//! while watched" cost story `SetScope` exists for.
//!
//! **Manual chevron header, not `egui::CollapsingHeader`.** `ui::charts::
//! charts_section`'s own header (`chart_header_row`) is a plain always-open
//! row — there's no existing collapsible-section precedent in this codebase
//! to match instead. `egui::CollapsingHeader` brings its own visual language
//! (an indent triangle, a hover-highlighted background band) that doesn't
//! share the M4a "chart-panel" look this section's body uses (`palette.
//! chartbg` fill, `palette.border` stroke, 12px corner radius —
//! `ui::charts::beat_trace_panel`/`amplitude_panel`'s convention). A manual
//! chevron + title row, sized and colored off the same `section_title`/
//! legend-dot idiom `chart_header_row` already established, stays visually
//! consistent with the rest of the page instead of importing a second
//! "collapsible section" look with different chrome.
//!
//! Per-trace geometry (`scope_trace_points`, `scope_marker_x`,
//! `stack_layout`) is TDD'd in `mod tests`, below; the egui-facing rendering
//! that follows is exercised only by `cargo build` + the workspace test
//! suite (no window in CI) — same split as every other M4/M4a UI module
//! (`ui::charts`, `ui::doctor`, `ui::strip`, …).

use chrona_dsp::BeatScope;
use chrona_dsp::events::Parity;
use eframe::egui;

use crate::engine::{ControlMsg, Engine};
use crate::theme::{self, Palette};

// ---------------------------------------------------------------------
// Pure geometry helpers (TDD'd below in `mod tests`).
// ---------------------------------------------------------------------

/// Gap between consecutive stacked traces, px (task brief: "n rects with
/// 4 px gaps").
const SCOPE_ROW_GAP: f32 = 4.0;
/// Hard cap on rendered rows regardless of how many windows `scope` holds
/// (task brief: "up to 8 of the ≤16"). `Analyzer::beat_scope`'s own
/// `SCOPE_MAX_EVENTS` cap is 16 — this is a SEPARATE, tighter display cap so
/// a full panel always lays out as 8 fixed-height rows rather than shrinking
/// arbitrarily thin as more events accumulate.
const SCOPE_MAX_ROWS: usize = 8;
/// One row's height, px — this task's own layout choice (not spec-pinned
/// like `SCOPE_ROW_GAP`/`SCOPE_MAX_ROWS`), tall enough for a ~300-point
/// mini-waveform plus its drop/unlock marker labels.
const SCOPE_ROW_H: f32 = 32.0;
/// Panel padding between the chart-panel border and the stacked rows, px —
/// same role as `ui::charts`'s `Pad`, but uniform on all four sides (a
/// single per-trace SNR readout and drop/unlock labels don't need the
/// asymmetric axis-label gutters `ui::charts::BEAT_PAD`/`AMP_PAD` reserve).
const SCOPE_PANEL_PAD: f32 = 8.0;

/// The window's own relative-time extent (seconds from the drop anchor):
/// the first sample's time (`window_start_rel_drop_s`) to the last sample's.
/// Shared by `scope_trace_points` and `scope_marker_x` so a marker drawn
/// against the same `scope`/rect always lines up with its trace. A window of
/// 0 or 1 samples collapses to a zero-width extent — both callers below
/// treat that as "map everything to center" rather than dividing by zero.
fn scope_x_extent(scope: &BeatScope) -> (f64, f64) {
    let start = scope.window_start_rel_drop_s;
    let n = scope.samples.len();
    let end = if n > 1 {
        start + (n - 1) as f64 * scope.sample_step_s
    } else {
        start
    };
    (start, end)
}

/// Screen X for a window-relative time `t_rel_s` (seconds from the drop
/// anchor) within `extent` (`scope_x_extent`'s result), spanning `rect`'s
/// full width. A zero-or-negative-width extent (≤ 1 sample) maps everything
/// to the rect's horizontal center instead of dividing by zero.
fn scope_x_of(t_rel_s: f64, extent: (f64, f64), rect: egui::Rect) -> f32 {
    let (start, end) = extent;
    let span = end - start;
    if span <= 0.0 {
        return rect.center().x;
    }
    rect.left() + (rect.width() as f64 * (t_rel_s - start) / span) as f32
}

/// Maps one [`BeatScope`]'s `samples` onto `rect`: x from each
/// sample's index (`scope_x_of`/`scope_x_extent`, i.e. `window_start_rel_
/// drop_s + i * sample_step_s`), y from the sample's amplitude normalized to
/// THIS WINDOW'S OWN max-abs sample — display-only (a coarse eyeball
/// waveform, never a calibrated voltage axis; see `BeatScope::samples`'s own
/// doc comment for why the source signal itself is already a display-only
/// compromise). A silent (all-zero) window, or one with 0 or 1 samples,
/// never divides by zero: every point maps to `rect`'s vertical (and, for
/// ≤ 1 sample, horizontal) center. Positive samples plot ABOVE center
/// (screen-up-for-positive — chosen for consistency with `presenter::trace`'s
/// own "up" convention elsewhere in this app, not because a raw microphone
/// waveform's sign carries physical meaning).
fn scope_trace_points(scope: &BeatScope, rect: egui::Rect) -> Vec<egui::Pos2> {
    let extent = scope_x_extent(scope);
    let max_abs = scope.samples.iter().fold(0.0f32, |m, &s| m.max(s.abs()));
    scope
        .samples
        .iter()
        .enumerate()
        .map(|(i, &s)| {
            let t_rel = scope.window_start_rel_drop_s + i as f64 * scope.sample_step_s;
            let x = scope_x_of(t_rel, extent, rect);
            let norm = if max_abs > 0.0 { s / max_abs } else { 0.0 };
            let y = rect.center().y - norm * (rect.height() / 2.0);
            egui::pos2(x, y)
        })
        .collect()
}

/// Screen X for a marker at window-relative time `t_rel_s` (drop = `0.0`,
/// unlock = `scope.t_unlock_rel_s`) against the SAME `scope`/`rect` a trace
/// was drawn with (`scope_trace_points`) — shares `scope_x_extent` so the
/// two never disagree about where the window starts or ends.
fn scope_marker_x(scope: &BeatScope, t_rel_s: f64, rect: egui::Rect) -> f32 {
    scope_x_of(t_rel_s, scope_x_extent(scope), rect)
}

/// Splits `rect` into up to `SCOPE_MAX_ROWS` equal-height sub-rects stacked
/// top-to-bottom with `SCOPE_ROW_GAP` px of clear space between consecutive
/// rows. `n` is clamped to `SCOPE_MAX_ROWS` before laying out, so a caller
/// passing a larger count never produces more than 8 rows; `n == 0` returns
/// an empty `Vec`. Row 0 is `rect`'s TOP row — `scope`'s own newest-first
/// order (`Analyzer::beat_scope`'s doc comment / Task 11's report), so a
/// caller pairs `stack_layout(...)` 1:1 against `scope[..n]` with no
/// reversal.
fn stack_layout(n: usize, rect: egui::Rect) -> Vec<egui::Rect> {
    let n = n.min(SCOPE_MAX_ROWS);
    if n == 0 {
        return Vec::new();
    }
    let total_gap = SCOPE_ROW_GAP * (n - 1) as f32;
    let row_h = ((rect.height() - total_gap) / n as f32).max(0.0);
    (0..n)
        .map(|i| {
            let top = rect.top() + i as f32 * (row_h + SCOPE_ROW_GAP);
            egui::Rect::from_min_size(
                egui::pos2(rect.left(), top),
                egui::vec2(rect.width(), row_h),
            )
        })
        .collect()
}

/// The scope panel's total height for `visible_n` rows (already clamped to
/// `SCOPE_MAX_ROWS` by the caller — `scope_panel` — but floored at 1 row's
/// worth here regardless, so an empty scope still reserves enough height for
/// the "scope needs detected beats" empty-state text rather than collapsing
/// to a sliver).
fn scope_panel_height(visible_n: usize) -> f32 {
    let rows = visible_n.max(1) as f32;
    rows * SCOPE_ROW_H + (rows - 1.0).max(0.0) * SCOPE_ROW_GAP + 2.0 * SCOPE_PANEL_PAD
}

// ---------------------------------------------------------------------
// Rendering: the Scope section. Exercised by `cargo build` + the workspace
// test suite; no window in CI, so nothing below is unit-tested directly —
// see the module doc comment.
// ---------------------------------------------------------------------

/// Vertical gap between the header row and the panel below it — same value
/// as `ui::charts`'s own `SECTION_GAP`, not imported from it (that constant
/// isn't `pub`; this module owns its own copy, same as every other small
/// per-module layout constant in `ui::*`).
const SCOPE_SECTION_GAP: f32 = 8.0;
/// Inset applied within each stacked row before mapping its trace/markers —
/// separate from `SCOPE_ROW_GAP` (the space BETWEEN rows): this is
/// breathing room WITHIN one row, so a peak sample or a marker label never
/// sits flush against that row's own edge.
const SCOPE_ROW_INSET: f32 = 3.0;

/// Borrowed, per-frame context `scope_section` needs beyond `open`: the
/// engine handle `SetScope` is sent through, and this frame's scope data
/// (`EngineSnapshot::scope.as_deref()`) — bundled purely to keep the
/// argument count sane, same shape as `ChartsCtx`/`DoctorCtx`/`StripCtx`.
pub struct ScopeCtx<'a> {
    pub engine: &'a Engine,
    pub scope: Option<&'a [BeatScope]>,
}

/// Renders the Scope section: the "Scope" header (chevron + title + the
/// tick/tock legend), and — only while open — the stacked-waveform panel
/// below it. Toggling the header sends `ControlMsg::SetScope` ONLY on the
/// actual open/close flip (never unconditionally every frame): `was_open` is
/// captured before `header_row` can mutate `*open`, so the comparison right
/// after it sees exactly this frame's click, if any. This is the entire
/// "trace the state change to the engine" contract the task brief asks for —
/// opening starts the engine's per-tick `Analyzer::beat_scope` extraction,
/// closing reverts `EngineSnapshot::scope` back to `None` and stops paying
/// for it (see `ControlMsg::SetScope`'s own doc comment for the full engine-
/// side contract this drives).
pub fn scope_section(ui: &mut egui::Ui, palette: &Palette, open: &mut bool, ctx: ScopeCtx<'_>) {
    let was_open = *open;
    header_row(ui, palette, open);
    if *open != was_open {
        ctx.engine.send(ControlMsg::SetScope(*open));
    }
    if !*open {
        return;
    }
    ui.add_space(SCOPE_SECTION_GAP);
    scope_panel(ui, palette, ctx.scope);
}

/// "Scope" section title (mirrors `ui::charts`'s own `section_title`: 13px
/// semibold — not imported since that helper isn't `pub`, same "each module
/// owns its own tiny presentational helpers" precedent as `ui::doctor`'s
/// `heading`/`ui::cards`'s cell fns).
fn section_title(text: &str, color: egui::Color32) -> egui::RichText {
    egui::RichText::new(text)
        .font(egui::FontId::new(
            13.0,
            egui::FontFamily::Name(theme::FAMILY_SEMIBOLD.into()),
        ))
        .color(color)
}

/// The header row: a clickable "▸/▾ Scope" chevron+title (toggles `*open`,
/// styled as a frameless button so it reads as plain text rather than a
/// button chrome — see the module doc comment for why this is a manual row
/// rather than `egui::CollapsingHeader`), then the tick/tock legend (task
/// brief: "The section header carries a small legend... like the beat-trace
/// header" — `ui::charts::chart_header_row`'s own Tick/Tock dots).
fn header_row(ui: &mut egui::Ui, palette: &Palette, open: &mut bool) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 14.0;
        let chevron = if *open { "\u{25be}" } else { "\u{25b8}" }; // ▾ open / ▸ closed
        let resp = ui.add(
            egui::Button::new(section_title(&format!("{chevron} Scope"), palette.text))
                .frame(false),
        );
        if resp.clicked() {
            *open = !*open;
        }
        legend_dot(ui, palette, palette.tick, "Tick");
        legend_dot(ui, palette, palette.accent, "Tock");
    });
}

fn legend_dot(ui: &mut egui::Ui, palette: &Palette, color: egui::Color32, text: &str) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let (rect, _resp) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
        if ui.is_rect_visible(rect) {
            ui.painter().circle_filled(rect.center(), 4.0, color);
        }
        ui.label(egui::RichText::new(text).size(12.0).color(palette.muted));
    });
}

fn empty_state(painter: &egui::Painter, rect: egui::Rect, palette: &Palette, text: &str) {
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        text,
        egui::FontId::proportional(13.0),
        palette.faint,
    );
}

/// Renders the panel: chart-panel background/border (`palette.chartbg`
/// fill, `palette.border` stroke, 12px corner radius — the M4a convention
/// `ui::charts::beat_trace_panel`/`amplitude_panel` both use), the empty
/// state, or up to `SCOPE_MAX_ROWS` stacked per-beat traces.
///
/// **Honesty:** `scope` is `None` on the very frame the header just opened
/// (this frame's snapshot was captured before the `SetScope(true)` this
/// click sends even reaches the engine — the next publish tick is when
/// `EngineSnapshot::scope` actually turns `Some`), and `Some(&[])` once it's
/// open but the analyzer hasn't cached any beats yet. Both read as the SAME
/// "nothing to show yet" state to a user — there's no honest distinction
/// between "not wired up yet" and "wired up, nothing detected" that's worth
/// making from this panel alone — so both collapse to one `events.unwrap_or
/// (&[])` and the same empty-state text (task brief: "scope needs detected
/// beats").
fn scope_panel(ui: &mut egui::Ui, palette: &Palette, scope: Option<&[BeatScope]>) {
    let events = scope.unwrap_or(&[]);
    let visible_n = events.len().min(SCOPE_MAX_ROWS);
    let height = scope_panel_height(visible_n);
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

    if visible_n == 0 {
        empty_state(&painter, rect, palette, "scope needs detected beats");
        return;
    }

    let inner = rect.shrink(SCOPE_PANEL_PAD);
    let rows = stack_layout(visible_n, inner);
    for (i, (row_rect, ev)) in rows.iter().zip(events.iter()).enumerate() {
        // Newest (row 0, top) only — see `draw_trace_row`'s own doc comment
        // for why drop/unlock get a text label on just this one row.
        draw_trace_row(&painter, palette, ev, *row_rect, i == 0);
    }
}

/// Renders one stacked row: a faint zero-baseline (so an all-silent trace
/// still shows SOMETHING rather than blank space), the drop marker (every
/// trace has one — `grid0`, labeled "drop" on the first/newest row only),
/// the unlock marker when `t_unlock_rel_s` is `Some` (`good` color, labeled
/// "unlock" on the first row only — an honestly absent marker for beats
/// whose unlocking pulse wasn't found, never guessed; see `BeatScope::
/// t_unlock_rel_s`'s own doc comment), the parity-tinted waveform itself,
/// and a right-aligned faint mono SNR readout.
fn draw_trace_row(
    painter: &egui::Painter,
    palette: &Palette,
    scope: &BeatScope,
    row: egui::Rect,
    is_first: bool,
) {
    let inner = row.shrink(SCOPE_ROW_INSET);
    let label_font = egui::FontId::monospace(9.0);

    painter.line_segment(
        [
            egui::pos2(inner.left(), inner.center().y),
            egui::pos2(inner.right(), inner.center().y),
        ],
        egui::Stroke::new(1.0, palette.grid),
    );

    let drop_x = scope_marker_x(scope, 0.0, inner);
    painter.line_segment(
        [
            egui::pos2(drop_x, inner.top()),
            egui::pos2(drop_x, inner.bottom()),
        ],
        egui::Stroke::new(1.0, palette.grid0),
    );
    if is_first {
        painter.text(
            egui::pos2(drop_x, inner.top()),
            egui::Align2::CENTER_TOP,
            "drop",
            label_font.clone(),
            palette.faint,
        );
    }

    if let Some(t_unlock) = scope.t_unlock_rel_s {
        let unlock_x = scope_marker_x(scope, t_unlock, inner);
        painter.line_segment(
            [
                egui::pos2(unlock_x, inner.top()),
                egui::pos2(unlock_x, inner.bottom()),
            ],
            egui::Stroke::new(1.0, palette.good),
        );
        if is_first {
            painter.text(
                egui::pos2(unlock_x, inner.top()),
                egui::Align2::CENTER_TOP,
                "unlock",
                label_font,
                palette.good,
            );
        }
    }

    let color = match scope.parity {
        Parity::Tic => palette.tick,
        Parity::Toc => palette.accent,
    };
    let clipped = painter.with_clip_rect(inner);
    let pts = scope_trace_points(scope, inner);
    match pts.len() {
        0 => {}
        1 => {
            clipped.circle_filled(pts[0], 1.5, color);
        }
        _ => {
            clipped.line(pts, egui::Stroke::new(1.2, color));
        }
    }

    // Honesty: `snr_db` is `Option` (mirrors `BeatEvent`'s own binding
    // interface, per `BeatScope::snr_db`'s doc comment) — an em-dash
    // rather than a fabricated number on the `None` side, same convention
    // as `ui::cards`'s `EM_DASH`.
    let snr_text = match scope.snr_db {
        Some(db) => format!("snr {db:.0} dB"),
        None => "snr \u{2014} dB".to_string(),
    };
    painter.text(
        egui::pos2(inner.right(), inner.top()),
        egui::Align2::RIGHT_TOP,
        snr_text,
        egui::FontId::monospace(10.0),
        palette.faint,
    );
}

#[cfg(test)]
mod tests {
    use eframe::egui;

    use super::*;

    /// Builds a `BeatScope` from just the fields each test cares about —
    /// `t_drop_corr_s` is display-only labeling `scope_trace_points`/
    /// `scope_marker_x` never read, so it's pinned to `0.0` throughout.
    fn test_scope(
        samples: Vec<f32>,
        window_start_rel_drop_s: f64,
        sample_step_s: f64,
        t_unlock_rel_s: Option<f64>,
        parity: Parity,
    ) -> BeatScope {
        BeatScope {
            t_drop_corr_s: 0.0,
            parity,
            samples,
            sample_step_s,
            window_start_rel_drop_s,
            t_unlock_rel_s,
            snr_db: None,
        }
    }

    fn test_rect() -> egui::Rect {
        egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(300.0, 40.0))
    }

    #[test]
    fn scope_trace_points_pins_edges_and_peak() {
        let scope = test_scope(vec![0.0, 1.0, -0.5, 0.25], -0.010, 0.001, None, Parity::Tic);
        let rect = test_rect();
        let pts = scope_trace_points(&scope, rect);
        assert_eq!(pts.len(), 4);

        // First/last sample land exactly on the rect's left/right edges —
        // the window's own extent (`window_start_rel_drop_s` .. last
        // sample's time) spans the FULL rect width.
        assert!((pts[0].x - rect.left()).abs() < 1e-3);
        assert!((pts[3].x - rect.right()).abs() < 1e-3);

        // The max-abs sample (index 1, value 1.0 — the positive peak) sits
        // at the TOP edge (display-only normalization to this window's own
        // max-abs sample, screen-up-for-positive convention).
        assert!((pts[1].y - rect.top()).abs() < 1e-3);
        // Index 2 (value -0.5, normalized -0.5) sits halfway BELOW center.
        assert!((pts[2].y - (rect.center().y + 0.25 * rect.height())).abs() < 1e-3);
    }

    #[test]
    fn scope_trace_points_round_trips_index_and_normalized_amplitude() {
        // Algebraic round-trip (per this repo's floating-point round-trip
        // caution: verify by inverting the exact formula, not by eyeballing
        // plausibility): invert each returned point's (x, y) back to
        // (window-relative time, normalized amplitude) via the mapping's
        // own inverse, and check the recovered values match what was fed
        // in — not merely "looks reasonable".
        let samples = vec![0.2_f32, -0.9, 0.4, 0.9, -0.1, 0.0];
        let scope = test_scope(samples.clone(), -0.010, 0.0025, None, Parity::Toc);
        let rect = test_rect();
        let max_abs = samples.iter().fold(0.0f32, |m, &s| m.max(s.abs()));
        let t0 = scope.window_start_rel_drop_s;
        let t1 = t0 + (samples.len() - 1) as f64 * scope.sample_step_s;

        let pts = scope_trace_points(&scope, rect);
        assert_eq!(pts.len(), samples.len());
        for (i, p) in pts.iter().enumerate() {
            let expected_t_rel = t0 + i as f64 * scope.sample_step_s;
            let recovered_t =
                t0 + (t1 - t0) * (p.x as f64 - rect.left() as f64) / rect.width() as f64;
            assert!(
                (recovered_t - expected_t_rel).abs() < 1e-9,
                "index {i}: x round-trip, recovered {recovered_t} expected {expected_t_rel}"
            );

            let recovered_norm = (rect.center().y - p.y) / (rect.height() / 2.0);
            let expected_norm = samples[i] / max_abs;
            assert!(
                (recovered_norm - expected_norm).abs() < 1e-4,
                "index {i}: y round-trip, recovered {recovered_norm} expected {expected_norm}"
            );
        }
    }

    #[test]
    fn scope_trace_points_handles_empty_and_degenerate_windows() {
        let rect = test_rect();

        let empty = test_scope(vec![], -0.010, 0.001, None, Parity::Tic);
        assert!(scope_trace_points(&empty, rect).is_empty());

        // A single sample has a zero-width extent (start == end) -> maps to
        // the rect's horizontal center rather than dividing by zero.
        let one = test_scope(vec![0.5], -0.010, 0.001, None, Parity::Tic);
        let pts = scope_trace_points(&one, rect);
        assert_eq!(pts.len(), 1);
        assert!((pts[0].x - rect.center().x).abs() < 1e-3);
        assert!(pts[0].x.is_finite() && pts[0].y.is_finite());

        // An all-silent window (max-abs == 0) never divides by zero either
        // -> every point sits on the vertical center.
        let silent = test_scope(vec![0.0, 0.0, 0.0], -0.010, 0.001, None, Parity::Tic);
        let pts = scope_trace_points(&silent, rect);
        for p in pts {
            assert!((p.y - rect.center().y).abs() < 1e-3);
            assert!(p.y.is_finite());
        }
    }

    #[test]
    fn scope_marker_x_matches_trace_extent_at_drop_and_window_bounds() {
        // extent = [-0.010, 0.010] (start -0.010, 4 steps of 0.005 -> end
        // 0.010) -> the drop (t_rel = 0.0) sits at the EXACT midpoint.
        let scope = test_scope(
            vec![0.0, 0.5, 1.0, 0.5, 0.0],
            -0.010,
            0.005,
            Some(-0.004),
            Parity::Tic,
        );
        let rect = test_rect();

        let drop_x = scope_marker_x(&scope, 0.0, rect);
        assert!((drop_x - rect.center().x).abs() < 1e-3);

        // The window's own start/end bounds map to the rect's left/right
        // edges — the SAME extent `scope_trace_points` uses, so a marker
        // never disagrees with its trace about where the window begins or
        // ends.
        assert!((scope_marker_x(&scope, -0.010, rect) - rect.left()).abs() < 1e-3);
        assert!((scope_marker_x(&scope, 0.010, rect) - rect.right()).abs() < 1e-3);

        // Unlock at -4 ms is (−0.004 − (−0.010)) / (0.010 − (−0.010)) =
        // 0.006 / 0.020 = 30% of the way from the window start to its end.
        let unlock_x = scope_marker_x(&scope, -0.004, rect);
        let expected = rect.left() + 0.3 * rect.width();
        assert!((unlock_x - expected).abs() < 1e-2);
    }

    #[test]
    fn scope_marker_x_degenerate_window_maps_to_center() {
        let rect = test_rect();
        let one = test_scope(vec![0.3], -0.010, 0.001, None, Parity::Toc);
        assert!((scope_marker_x(&one, 0.0, rect) - rect.center().x).abs() < 1e-3);
    }

    #[test]
    fn scope_layout_constants_match_the_brief() {
        assert_eq!(SCOPE_ROW_GAP, 4.0, "brief: \"n rects with 4 px gaps\"");
        assert_eq!(SCOPE_MAX_ROWS, 8, "brief: \"up to 8 rendered\"");
    }

    #[test]
    fn stack_layout_produces_n_rects_with_4px_gaps() {
        let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(200.0, 100.0));
        let rows = stack_layout(3, rect);
        assert_eq!(rows.len(), 3);

        // Rows keep the full rect's width and stack top-to-bottom, in
        // order — row 0 is `scope`'s own newest event (`Analyzer::
        // beat_scope`'s newest-first order, Task 11's report), so the
        // caller pairs `stack_layout(...)` 1:1 against `scope[..n]` with no
        // reversal.
        for r in &rows {
            assert_eq!(r.left(), rect.left());
            assert_eq!(r.right(), rect.right());
        }
        assert_eq!(rows[0].top(), rect.top());
        // Not exact equality: `top + i*(row_h+gap)` accumulates ordinary
        // float rounding across 3 rows (this repo's floating-point
        // round-trip caution — an epsilon check here, not a claim that
        // `stack_layout` is exact to the last bit).
        assert!((rows[2].bottom() - rect.bottom()).abs() < 1e-3);

        // Exactly `SCOPE_ROW_GAP` px of clear space between consecutive
        // rows.
        for w in rows.windows(2) {
            assert!((w[1].top() - w[0].bottom() - SCOPE_ROW_GAP).abs() < 1e-4);
        }

        // Equal-height rows.
        let h = rows[0].height();
        for r in &rows {
            assert!((r.height() - h).abs() < 1e-4);
        }
    }

    #[test]
    fn stack_layout_caps_at_scope_max_rows_regardless_of_n() {
        let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(200.0, 400.0));
        assert_eq!(stack_layout(16, rect).len(), SCOPE_MAX_ROWS);
        assert_eq!(stack_layout(100, rect).len(), SCOPE_MAX_ROWS);
        assert_eq!(stack_layout(SCOPE_MAX_ROWS, rect).len(), SCOPE_MAX_ROWS);
        assert_eq!(
            stack_layout(SCOPE_MAX_ROWS - 1, rect).len(),
            SCOPE_MAX_ROWS - 1
        );
    }

    #[test]
    fn stack_layout_zero_is_empty() {
        let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(200.0, 100.0));
        assert!(stack_layout(0, rect).is_empty());
    }

    #[test]
    fn scope_panel_height_grows_with_row_count_and_floors_at_one_row() {
        let h0 = scope_panel_height(0);
        let h1 = scope_panel_height(1);
        assert_eq!(
            h0, h1,
            "an empty scope still reserves one row's worth of height for the empty-state text"
        );
        let h2 = scope_panel_height(2);
        assert!((h2 - h1 - SCOPE_ROW_H - SCOPE_ROW_GAP).abs() < 1e-4);
        let h8 = scope_panel_height(8);
        let expected = 8.0 * SCOPE_ROW_H + 7.0 * SCOPE_ROW_GAP + 2.0 * SCOPE_PANEL_PAD;
        assert!((h8 - expected).abs() < 1e-4);
    }
}
