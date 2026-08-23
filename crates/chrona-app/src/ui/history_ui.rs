//! Session-history + position-comparison cards (M4a Task 9, mockup:
//! "History + position comparison"): the two panel cards that fill the
//! bottom grid below the charts section. The pure row-model helpers below
//! (`session_time_label`, `comparison_rows`) are TDD'd first; the
//! egui-facing rendering that follows is exercised only by `cargo build` +
//! the workspace test suite (no window in CI) — same split as
//! `cards.rs`/`toolbar.rs`/`strip.rs`/`charts.rs`.

use eframe::egui;

use crate::engine::{ControlMsg, Engine, SourceSpec};
use crate::history::{HistoryIndex, POSITIONS, PositionRates, SessionEntry, bar_frac};
use crate::presenter::day_label;
use crate::theme::Palette;
use crate::ui::strip::SessionPanelState;

// ---------------------------------------------------------------------
// Pure helpers (TDD'd below in `mod tests`).
// ---------------------------------------------------------------------

/// "HH:MM"[, " · N min"] for one session-history row (mockup: `h.time`).
/// `HH:MM` is derived from `started_unix_s` via UTC calendar math — no
/// time-zone database, the same accepted GUI-only approximation of local
/// time as `presenter::format::day_label` (see its doc comment); not used
/// for anything honesty-sensitive. `duration_s`, when `Some` (the
/// session's finalized summary), is rounded to the nearest minute with a
/// "1 min" floor — a session that ran under 30s should still read as
/// "1 min", never a misleadingly-precise "0 min". `None` (no summary yet,
/// e.g. before the recording finalized) omits the duration suffix
/// entirely rather than fabricating one.
pub fn session_time_label(started_unix_s: u64, duration_s: Option<f64>) -> String {
    let sod = started_unix_s % 86_400; // seconds of day
    let hh = sod / 3_600;
    let mm = (sod % 3_600) / 60;
    let time = format!("{hh:02}:{mm:02}");
    match duration_s {
        Some(d) => {
            let mins = ((d / 60.0).round() as i64).max(1);
            format!("{time} · {mins} min")
        }
        None => time,
    }
}

/// One position-comparison row's already-resolved display fields (mockup:
/// `posRuns`), built fresh each frame from `PositionRates`. No egui/
/// `Color32` here — see the module doc comment.
#[derive(Debug, Clone, PartialEq)]
pub struct ComparisonRow {
    pub code: &'static str,
    /// `"+12.0"` / `"-4.7"`, or `"—"` when this position has no rated
    /// session yet.
    pub rate_label: String,
    /// Deviation-bar half-width, `0.0..=0.5` of the track's full width
    /// (`history::bar_frac`); `0.0` when there's no rate to show.
    pub bar_frac: f64,
    /// Whether the bar/rate falls left of center (a negative rate) — the
    /// mockup's `left: rate >= 0 ? '50%' : (50-w)+'%'` split.
    pub negative: bool,
    /// Whether this is the position currently selected in the strip
    /// (mockup: `code === position`) — the render layer's accent-vs-muted
    /// color decision.
    pub selected: bool,
}

/// Builds all 6 canonical-position comparison rows (mockup: `posRuns`)
/// from `rates`, index-aligned with `history::POSITIONS`;
/// `selected_position` is `ChronaApp::selected_position` (an index into the
/// same array), driving each row's `selected` flag (mockup: `code ===
/// position`) — the render layer's accent-vs-muted color decision. A
/// position with no rated session (`rates.by_code[i] == None`) gets the
/// `"—"` dash and a zero-width bar rather than a fabricated rate. PURE.
pub fn comparison_rows(rates: &PositionRates, selected_position: usize) -> Vec<ComparisonRow> {
    POSITIONS
        .iter()
        .enumerate()
        .map(|(i, (code, _label))| {
            let (rate_label, frac, negative) = match rates.by_code[i] {
                Some(r) => (format!("{r:+.1}"), bar_frac(r), r < 0.0),
                None => ("—".to_string(), 0.0, false),
            };
            ComparisonRow {
                code,
                rate_label,
                bar_frac: frac,
                negative,
                selected: i == selected_position,
            }
        })
        .collect()
}

// ---------------------------------------------------------------------
// Rendering: the bottom grid (history + position-comparison cards).
// Exercised by `cargo build` + the workspace test suite; no window in CI,
// so nothing below is unit-tested directly — see the module doc comment.
// ---------------------------------------------------------------------

/// Up to this many session-history rows are shown (brief: "up to 50
/// rows"). `HistoryIndex::entries()` is already newest-first, so this is
/// simply the 50 newest sessions across every watch — the mockup's history
/// table mixes watches (see its own demo data, two different watches in
/// one list), so this is a global log, NOT filtered to the toolbar's
/// selected watch. Only the comparison card below is per-watch.
const HISTORY_ROW_LIMIT: usize = 50;

/// Borrowed, per-frame context `bottom_grid` needs beyond `Palette`:
/// bundled purely to keep the argument count sane, same shape as
/// `toolbar::ToolbarCtx`/`strip::StripCtx`/`charts::ChartsCtx`.
pub struct BottomGridCtx<'a> {
    pub history: &'a HistoryIndex,
    /// Mutated on a history-row click exactly like `ui::toolbar::
    /// open_recording_button`'s optimistic `replay_name` set — row-click
    /// replay is the same flow, just a different trigger.
    pub session: &'a mut SessionPanelState,
    pub engine: &'a Engine,
    /// The selected watch's position-comparison math (`history::
    /// position_rates`), already resolved by the caller: `ChronaApp::ui`
    /// owns deciding what "selected watch" means (including the
    /// no-watch-selected case), this module only renders the result.
    pub rates: &'a PositionRates,
    pub selected_position: usize,
}

/// Renders the mockup's bottom grid: the Session-history card and the
/// Position-comparison card, side by side (mockup: `grid-template-columns:
/// repeat(auto-fit, minmax(380px, 1fr))`, approximated here as a fixed
/// 2-column split — same `ui.columns` precedent as `cards::
/// metrics_cards`'s 4-card row).
pub fn bottom_grid(ui: &mut egui::Ui, palette: &Palette, ctx: BottomGridCtx<'_>) {
    ui.columns(2, |cols| {
        history_card(&mut cols[0], palette, ctx.history, ctx.session, ctx.engine);
        comparison_card(&mut cols[1], palette, ctx.rates, ctx.selected_position);
    });
}

fn card_frame(ui: &mut egui::Ui, palette: &Palette, add_contents: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(palette.panel)
        .stroke(egui::Stroke::new(1.0, palette.border))
        .corner_radius(12.0)
        .inner_margin(egui::Margin::symmetric(16, 14))
        .show(ui, add_contents);
}

fn card_title(text: &str, color: egui::Color32) -> egui::RichText {
    egui::RichText::new(text)
        .font(egui::FontId::new(
            13.0,
            egui::FontFamily::Name(crate::theme::FAMILY_SEMIBOLD.into()),
        ))
        .color(color)
}

fn empty_state_label(ui: &mut egui::Ui, palette: &Palette, text: &str) {
    ui.add_space(4.0);
    ui.label(egui::RichText::new(text).size(12.5).color(palette.faint));
}

/// Wall-clock UNIX seconds, or `0` on any error (same graceful-fallback
/// shape as `history::sidecar_mtime_unix_s`) — feeds `day_label`'s "now"
/// side for the history card's header day label.
fn now_unix_s() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ---------------------------------------------------------------------
// Session-history card
// ---------------------------------------------------------------------

fn history_card(
    ui: &mut egui::Ui,
    palette: &Palette,
    history: &HistoryIndex,
    session: &mut SessionPanelState,
    engine: &Engine,
) {
    card_frame(ui, palette, |ui| {
        let now = now_unix_s();
        ui.horizontal(|ui| {
            ui.label(card_title("Session history", palette.text));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if let Some(newest) = history.entries().first() {
                    ui.label(
                        egui::RichText::new(day_label(newest.started_unix_s, now))
                            .size(11.0)
                            .color(palette.faint),
                    );
                }
            });
        });
        ui.add_space(10.0);

        if history.entries().is_empty() {
            empty_state_label(ui, palette, "no sessions yet — record one");
            return;
        }

        history_header_row(ui, palette);
        for entry in history.entries().iter().take(HISTORY_ROW_LIMIT) {
            if history_row(ui, palette, entry).clicked() {
                let name = entry
                    .wav_path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| entry.wav_path.display().to_string());
                session.replay_name = Some(name);
                engine.send(ControlMsg::SwitchSource(SourceSpec::ReplayFile {
                    path: entry.wav_path.clone(),
                }));
            }
        }
    });
}

/// Fixed column widths, px (mockup: `grid-template-columns:1fr 44px 90px
/// 64px 64px 54px`), and the 8px gap between them.
const HIST_POS_W: f32 = 44.0;
const HIST_TIME_W: f32 = 90.0;
const HIST_RATE_W: f32 = 64.0;
const HIST_ERR_W: f32 = 64.0;
const HIST_AMP_W: f32 = 54.0;
const HIST_GAP: f32 = 8.0;
/// Floor for the flexible Watch column so a very narrow card never
/// collapses it to nothing.
const HIST_WATCH_MIN_W: f32 = 40.0;
/// Session-history row height, px.
const HIST_ROW_H: f32 = 24.0;

/// Resolved left-edge X offsets (from the row rect's left edge) for each
/// column, given the row's total available width — the "1fr" Watch column
/// absorbs whatever's left after the 5 fixed columns and their gaps. Not
/// unit-tested (see the module doc comment): plain layout arithmetic over
/// the mockup's pinned pixel widths above, in the same untested render
/// layer as `charts.rs`'s `Pad`/`x_of` — the difference there is only that
/// those happened to get bonus geometry tests, not that this arithmetic
/// carries any different risk.
struct HistoryColumns {
    watch_w: f32,
    watch_x: f32,
    pos_x: f32,
    time_x: f32,
    rate_x: f32,
    err_x: f32,
    amp_x: f32,
}

fn history_columns(total_width: f32) -> HistoryColumns {
    let fixed_total = HIST_POS_W + HIST_TIME_W + HIST_RATE_W + HIST_ERR_W + HIST_AMP_W;
    let watch_w = (total_width - fixed_total - HIST_GAP * 5.0).max(HIST_WATCH_MIN_W);
    let watch_x = 0.0;
    let pos_x = watch_x + watch_w + HIST_GAP;
    let time_x = pos_x + HIST_POS_W + HIST_GAP;
    let rate_x = time_x + HIST_TIME_W + HIST_GAP;
    let err_x = rate_x + HIST_RATE_W + HIST_GAP;
    let amp_x = err_x + HIST_ERR_W + HIST_GAP;
    HistoryColumns {
        watch_w,
        watch_x,
        pos_x,
        time_x,
        rate_x,
        err_x,
        amp_x,
    }
}

/// The WATCH/POS/TIME/RATE/BEAT ERR/AMPL header row (brief: 11px faint
/// uppercase).
fn history_header_row(ui: &mut egui::Ui, palette: &Palette) {
    let width = ui.available_width();
    let (response, painter) = ui.allocate_painter(egui::vec2(width, 16.0), egui::Sense::hover());
    let rect = response.rect;
    if !ui.is_rect_visible(rect) {
        return;
    }
    let cols = history_columns(width);
    let font = egui::FontId::proportional(11.0);
    let y = rect.center().y;
    let left = rect.left();
    painter.text(
        egui::pos2(left + cols.watch_x, y),
        egui::Align2::LEFT_CENTER,
        "WATCH",
        font.clone(),
        palette.faint,
    );
    painter.text(
        egui::pos2(left + cols.pos_x, y),
        egui::Align2::LEFT_CENTER,
        "POS",
        font.clone(),
        palette.faint,
    );
    painter.text(
        egui::pos2(left + cols.time_x, y),
        egui::Align2::LEFT_CENTER,
        "TIME",
        font.clone(),
        palette.faint,
    );
    painter.text(
        egui::pos2(left + cols.rate_x + HIST_RATE_W, y),
        egui::Align2::RIGHT_CENTER,
        "RATE",
        font.clone(),
        palette.faint,
    );
    painter.text(
        egui::pos2(left + cols.err_x + HIST_ERR_W, y),
        egui::Align2::RIGHT_CENTER,
        "BEAT ERR",
        font.clone(),
        palette.faint,
    );
    painter.text(
        egui::pos2(left + cols.amp_x + HIST_AMP_W, y),
        egui::Align2::RIGHT_CENTER,
        "AMPL",
        font,
        palette.faint,
    );
}

/// One session-history row: hover fill + a top border rule (mockup:
/// `border-top:1px solid var(--border)` on every row), then the 6 cells.
/// Returns the row's `Response` (`Sense::click()`); `history_card` above
/// performs the replay switch inline when it's clicked — the action is
/// simple/uniform enough not to need bubbling further up to `ChronaApp`,
/// unlike e.g. `cards::metrics_cards`'s help-topic return (which DOES
/// need to reach `ChronaApp` to coordinate with the help-modal state).
fn history_row(ui: &mut egui::Ui, palette: &Palette, entry: &SessionEntry) -> egui::Response {
    let width = ui.available_width();
    let (response, painter) =
        ui.allocate_painter(egui::vec2(width, HIST_ROW_H), egui::Sense::click());
    let rect = response.rect;
    if !ui.is_rect_visible(rect) {
        return response;
    }

    if response.hovered() {
        painter.rect_filled(rect, 4.0, palette.panel2);
    }
    painter.hline(
        rect.x_range(),
        rect.top(),
        egui::Stroke::new(1.0, palette.border),
    );

    let cols = history_columns(width);
    let left = rect.left();
    let y = rect.center().y;
    let mono = egui::FontId::monospace(13.0);
    let prop = egui::FontId::proportional(13.0);
    let muted_prop = egui::FontId::proportional(12.0);

    let watch_text = entry.watch.as_deref().unwrap_or("—");
    let watch_shown = ellipsize(
        &painter,
        watch_text,
        prop.clone(),
        cols.watch_w,
        palette.text,
    );
    painter.text(
        egui::pos2(left + cols.watch_x, y),
        egui::Align2::LEFT_CENTER,
        watch_shown,
        prop,
        palette.text,
    );

    painter.text(
        egui::pos2(left + cols.pos_x, y),
        egui::Align2::LEFT_CENTER,
        entry.position.as_deref().unwrap_or("—"),
        mono.clone(),
        palette.text,
    );

    let duration_s = entry.summary.as_ref().map(|s| s.duration_s);
    painter.text(
        egui::pos2(left + cols.time_x, y),
        egui::Align2::LEFT_CENTER,
        session_time_label(entry.started_unix_s, duration_s),
        muted_prop,
        palette.muted,
    );

    let rate_text = entry
        .summary
        .as_ref()
        .and_then(|s| s.rate_s_per_day)
        .map(|r| format!("{r:+.1}"))
        .unwrap_or_else(|| "—".to_string());
    painter.text(
        egui::pos2(left + cols.rate_x + HIST_RATE_W, y),
        egui::Align2::RIGHT_CENTER,
        rate_text,
        mono.clone(),
        palette.text,
    );

    let err_text = entry
        .summary
        .as_ref()
        .and_then(|s| s.beat_error_ms)
        .map(|v| format!("{v:.1}"))
        .unwrap_or_else(|| "—".to_string());
    painter.text(
        egui::pos2(left + cols.err_x + HIST_ERR_W, y),
        egui::Align2::RIGHT_CENTER,
        err_text,
        mono.clone(),
        palette.muted,
    );

    let amp_text = entry
        .summary
        .as_ref()
        .and_then(|s| s.amplitude_deg)
        .map(|v| format!("{v:.0}°"))
        .unwrap_or_else(|| "—".to_string());
    painter.text(
        egui::pos2(left + cols.amp_x + HIST_AMP_W, y),
        egui::Align2::RIGHT_CENTER,
        amp_text,
        mono,
        palette.muted,
    );

    response
}

/// Truncates `text` with a trailing "…" so it fits within `max_width` px
/// at `font` (mockup: CSS `text-overflow:ellipsis`, which raw
/// `Painter::text` has no built-in equivalent of); returns `text`
/// unchanged when it already fits. `color` doesn't affect the measured
/// width at all (only the eventual paint does) — callers pass the same
/// `palette` color the text will actually be painted with, purely so this
/// measure-only pass never introduces a color literal of its own.
fn ellipsize(
    painter: &egui::Painter,
    text: &str,
    font: egui::FontId,
    max_width: f32,
    color: egui::Color32,
) -> String {
    let full_w = painter
        .layout_no_wrap(text.to_string(), font.clone(), color)
        .size()
        .x;
    if full_w <= max_width {
        return text.to_string();
    }
    let mut out = String::new();
    for c in text.chars() {
        let candidate = format!("{out}{c}…");
        let w = painter
            .layout_no_wrap(candidate, font.clone(), color)
            .size()
            .x;
        if w > max_width {
            break;
        }
        out.push(c);
    }
    format!("{out}…")
}

// ---------------------------------------------------------------------
// Position-comparison card
// ---------------------------------------------------------------------

fn comparison_card(
    ui: &mut egui::Ui,
    palette: &Palette,
    rates: &PositionRates,
    selected_position: usize,
) {
    card_frame(ui, palette, |ui| {
        ui.horizontal(|ui| {
            ui.label(card_title("Position comparison", palette.text));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                spread_label(ui, palette, rates.spread);
            });
        });
        ui.add_space(10.0);

        if rates.by_code.iter().all(Option::is_none) {
            empty_state_label(
                ui,
                palette,
                "record sessions in different positions to compare",
            );
            return;
        }

        for row in comparison_rows(rates, selected_position) {
            comparison_row(ui, palette, &row);
            ui.add_space(7.0); // mockup: flex column `gap:7px`
        }
        ui.add_space(3.0);
        ui.label(
            egui::RichText::new("Bars show rate deviation from 0 s/d · center line = on time")
                .size(11.0)
                .color(palette.faint),
        );
    });
}

/// "Δ spread N.N s/d" (mockup header, right side), or "Δ spread —" when
/// `spread` is `None` (fewer than 2 rated positions — `history::
/// PositionRates::spread`'s own doc comment). A plain nested
/// `ui.horizontal` — left-to-right regardless of the parent's own
/// right-to-left layout, same reasoning as `strip::replay_mode_line`'s
/// pill-then-button ordering.
fn spread_label(ui: &mut egui::Ui, palette: &Palette, spread: Option<f64>) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        ui.label(
            egui::RichText::new("Δ spread")
                .size(12.0)
                .color(palette.muted),
        );
        let value = match spread {
            Some(s) => format!("{s:.1} s/d"),
            None => "—".to_string(),
        };
        ui.label(
            egui::RichText::new(value)
                .monospace()
                .size(12.0)
                .color(palette.text),
        );
    });
}

const CMP_CODE_W: f32 = 40.0;
const CMP_RATE_W: f32 = 64.0;
const CMP_GAP: f32 = 10.0;
const CMP_TRACK_H: f32 = 8.0;
const CMP_ROW_H: f32 = 16.0;

/// One position-comparison row (mockup `posRuns` row): the position code
/// (mono, accent when selected), the deviation track (panel2 background,
/// a border2 center tick, and the deviation bar itself — accent when
/// selected, else border2, spanning from center rightward for a
/// non-negative rate or leftward for a negative one), and the right-
/// aligned rate.
fn comparison_row(ui: &mut egui::Ui, palette: &Palette, row: &ComparisonRow) {
    let width = ui.available_width();
    let (response, painter) =
        ui.allocate_painter(egui::vec2(width, CMP_ROW_H), egui::Sense::hover());
    let rect = response.rect;
    if !ui.is_rect_visible(rect) {
        return;
    }

    let code_color = if row.selected {
        palette.accent
    } else {
        palette.muted
    };
    let bar_color = if row.selected {
        palette.accent
    } else {
        palette.border2
    };
    let y = rect.center().y;

    painter.text(
        egui::pos2(rect.left(), y),
        egui::Align2::LEFT_CENTER,
        row.code,
        egui::FontId::monospace(13.0),
        code_color,
    );

    let track_left = rect.left() + CMP_CODE_W + CMP_GAP;
    let track_right = (rect.right() - CMP_RATE_W - CMP_GAP).max(track_left);
    let track_rect = egui::Rect::from_min_max(
        egui::pos2(track_left, y - CMP_TRACK_H / 2.0),
        egui::pos2(track_right, y + CMP_TRACK_H / 2.0),
    );
    painter.rect_filled(track_rect, 4.0, palette.panel2);

    let cx = track_rect.center().x;
    painter.line_segment(
        [
            egui::pos2(cx, track_rect.top() - 2.0),
            egui::pos2(cx, track_rect.bottom() + 2.0),
        ],
        egui::Stroke::new(1.0, palette.border2),
    );

    if row.bar_frac > 0.0 {
        let bar_w = row.bar_frac as f32 * track_rect.width();
        let bar_left = if row.negative { cx - bar_w } else { cx };
        let bar_rect = egui::Rect::from_min_max(
            egui::pos2(bar_left, track_rect.top()),
            egui::pos2(bar_left + bar_w, track_rect.bottom()),
        );
        painter.rect_filled(bar_rect, 4.0, bar_color);
    }

    painter.text(
        egui::pos2(rect.right(), y),
        egui::Align2::RIGHT_CENTER,
        &row.rate_label,
        egui::FontId::monospace(13.0),
        palette.text,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_time_label_with_and_without_duration() {
        // Same pinned instant chrona_session::civil/export.rs's own tests
        // use: 1_766_995_200 == 2025-12-29 08:00:00 UTC.
        assert_eq!(
            session_time_label(1_766_995_200, Some(126.0)),
            "08:00 · 2 min"
        );
        assert_eq!(
            session_time_label(1_766_995_200, None),
            "08:00",
            "no summary yet: duration suffix omitted entirely"
        );
    }

    #[test]
    fn session_time_label_duration_rounds_with_a_one_minute_floor() {
        assert_eq!(
            session_time_label(0, Some(10.0)),
            "00:00 · 1 min",
            "under 30s floors to 1 min, never 0"
        );
        assert_eq!(
            session_time_label(0, Some(29.0)),
            "00:00 · 1 min",
            "rounds down under 30s"
        );
        assert_eq!(
            session_time_label(0, Some(31.0)),
            "00:00 · 1 min",
            "rounds to nearest, 31s -> 1 min"
        );
        assert_eq!(
            session_time_label(0, Some(150.0)),
            "00:00 · 3 min",
            "150s / 60 = 2.5 rounds up to 3"
        );
    }

    #[test]
    fn comparison_rows_covers_all_positions_with_accent_selection_and_dash_handling() {
        let rates = PositionRates {
            by_code: [Some(12.0), None, Some(-7.5), None, None, None],
            spread: Some(19.5),
        };
        let rows = comparison_rows(&rates, 2); // CU selected
        assert_eq!(rows.len(), 6);

        assert_eq!(rows[0].code, "DU");
        assert_eq!(rows[0].rate_label, "+12.0");
        assert_eq!(rows[0].bar_frac, bar_frac(12.0));
        assert!(!rows[0].negative);
        assert!(
            !rows[0].selected,
            "DU is not the selected position (index 2)"
        );

        assert_eq!(rows[1].code, "DD");
        assert_eq!(rows[1].rate_label, "—");
        assert_eq!(
            rows[1].bar_frac, 0.0,
            "no fabricated bar width for an absent rate"
        );
        assert!(!rows[1].negative);
        assert!(!rows[1].selected);

        assert_eq!(rows[2].code, "CU");
        assert_eq!(rows[2].rate_label, "-7.5");
        assert_eq!(rows[2].bar_frac, bar_frac(7.5));
        assert!(
            rows[2].negative,
            "negative rate flips the bar to the left half"
        );
        assert!(rows[2].selected, "index 2 is the selected position");

        assert_eq!(rows[5].code, "CR");
        assert_eq!(rows[5].rate_label, "—");
        assert!(!rows[5].selected);
    }
}
