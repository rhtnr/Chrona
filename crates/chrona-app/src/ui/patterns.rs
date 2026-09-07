//! Classic tape-pattern legend (M4 Task 13, binding spec §6): a help modal
//! explaining the six classic timegrapher trace shapes and what each one
//! means, opened from the beat-trace header's "?" button (`ui::charts::
//! chart_header_row`). Two independent pieces, same split as `ui::modals`'
//! metric help modal (`ui::modals::HELP_TOPICS`/`render_help_modal`):
//!
//! - [`pattern_points`]: PURE polyline-group geometry for each of the six
//!   schematic shapes, TDD'd in `mod tests` below.
//! - [`PATTERNS`]: the six rows' pinned copy, transcribed verbatim from this
//!   task's binding brief (`.superpowers/sdd/2026-09-07-m4-trust-features/
//!   task-13-brief.md`) — every dash/space/punctuation mark is significant,
//!   same transcription-fidelity convention `ui::modals::HELP_TOPICS`' own
//!   doc comment describes.
//! - `render_pattern_legend_modal`: the egui-facing modal painting both,
//!   mirroring `ui::modals::render_help_modal`'s overlay/card/✕/Esc/
//!   click-outside close contract exactly (M4a modal convention) —
//!   exercised only by `cargo build` + the workspace test suite (no window
//!   in CI), same split as every other M4/M4a UI module.

use eframe::egui;

use crate::theme::Palette;

// ---------------------------------------------------------------------
// Pure geometry (TDD'd below in `mod tests`).
// ---------------------------------------------------------------------

/// Which of the six classic trace shapes a schematic depicts — one per
/// [`PATTERNS`] row, in the SAME order (index-aligned;
/// `patterns_cover_every_kind_in_declared_order` below pins this).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatternKind {
    Level,
    Slope,
    Bands,
    Wavy,
    Scatter,
    Jumps,
}

/// One [`PATTERNS`] row's pinned copy — schematic label + explanation.
pub struct PatternRow {
    pub kind: PatternKind,
    /// The schematic's short label — this task's binding brief numbers each
    /// row by exactly this phrase (e.g. "Level line").
    pub label: &'static str,
    /// The explanation text — binding copy, transcribed verbatim from this
    /// task's brief (write exactly, tune only for typos; none expected).
    pub text: &'static str,
}

/// The six classic tape-pattern rows (this task's binding brief — pinned
/// copy, write exactly). Order matches [`PatternKind`]'s declaration order
/// 1:1, and each row's schematic is drawn via `pattern_points(row.kind,
/// ...)`.
pub const PATTERNS: [PatternRow; 6] = [
    PatternRow {
        kind: PatternKind::Level,
        label: "Level line",
        text: "Healthy and on rate. The flatter, the closer to 0 s/d.",
    },
    PatternRow {
        kind: PatternKind::Slope,
        label: "Sloped line",
        text: "Running fast (rising) or slow (falling). Adjust the regulator.",
    },
    PatternRow {
        kind: PatternKind::Bands,
        label: "Two parallel bands",
        text: "Beat error: tick and tock are unevenly spaced. Adjust the stud/collet, not the regulator.",
    },
    PatternRow {
        kind: PatternKind::Wavy,
        label: "Wavy line",
        text: "Periodic fault — suggests a damaged tooth or bent pivot in the gear train; the wave period hints at which wheel.",
    },
    PatternRow {
        kind: PatternKind::Scatter,
        label: "Scattered dots, no line",
        text: "Weak or dirty signal — suggests a dirty escapement, low amplitude, or poor pickup. Check placement and run the Mic Doctor.",
    },
    PatternRow {
        kind: PatternKind::Jumps,
        label: "Sudden vertical jumps",
        text: "Sudden rate steps — suggests rebanking, a knock, or the hairspring touching. Re-test after winding fully.",
    },
];

/// Maps a (x_frac, y_frac) pair (each `0.0..=1.0`, y downward per screen
/// convention) onto `rect` — shared by every schematic helper below.
fn frac_to_pos(rect: egui::Rect, x_frac: f64, y_frac: f64) -> egui::Pos2 {
    egui::pos2(
        rect.left() + (rect.width() as f64 * x_frac) as f32,
        rect.top() + (rect.height() as f64 * y_frac) as f32,
    )
}

/// Flat line at mid-height — "healthy and on rate" (Step 1 pin: 1 group,
/// constant y).
fn level_points(rect: egui::Rect) -> Vec<egui::Pos2> {
    vec![frac_to_pos(rect, 0.0, 0.5), frac_to_pos(rect, 1.0, 0.5)]
}

/// Rises left-to-right in SCREEN space (y DECREASES as x increases) — the
/// "up is fast" convention `presenter::trace`/`ui::charts::y_of_beat` use
/// for the real beat trace (rising = running fast; see
/// `ui::charts::x_of_and_y_of_round_trip_at_the_pad_constants`'s own "+half
/// is the top edge" pin).
fn slope_points(rect: egui::Rect) -> Vec<egui::Pos2> {
    vec![frac_to_pos(rect, 0.0, 0.82), frac_to_pos(rect, 1.0, 0.18)]
}

/// y-fraction (0 = top, 1 = bottom) of each of the "two parallel bands"
/// schematic's two level lines — tick/tock evenly offset, never crossing.
const BAND_Y_FRACS: [f64; 2] = [0.32, 0.68];

/// Two parallel flat lines, offset by a fixed gap (Step 1 pin: 2 groups,
/// constant y-gap) — beat error: tick and tock unevenly spaced, but each
/// consistently so.
fn bands_points(rect: egui::Rect) -> Vec<Vec<egui::Pos2>> {
    BAND_Y_FRACS
        .iter()
        .map(|&y| vec![frac_to_pos(rect, 0.0, y), frac_to_pos(rect, 1.0, y)])
        .collect()
}

/// Sample count for the wavy sine schematic — dense enough to read as a
/// smooth curve at the ~120px mini-card width.
const WAVY_SAMPLES: usize = 48;
/// Full sine periods drawn across the wavy schematic's width. Each period
/// contributes 2 dy sign changes (one at its peak, one at its trough), so 2
/// periods gives 4 — comfortable margin over the Step 1 pin's "≥ 3" floor.
const WAVY_PERIODS: f64 = 2.0;

/// Sine wave — periodic fault: a damaged tooth or bent pivot recurring once
/// per revolution of the affected wheel (Step 1 pin: ≥ 3 dy sign changes).
fn wavy_points(rect: egui::Rect) -> Vec<egui::Pos2> {
    (0..WAVY_SAMPLES)
        .map(|i| {
            let t = i as f64 / (WAVY_SAMPLES - 1) as f64;
            let y = 0.5 + 0.36 * (t * WAVY_PERIODS * std::f64::consts::TAU).sin();
            frac_to_pos(rect, t, y)
        })
        .collect()
}

/// Fixed seed table for the "scattered dots" schematic: hand-picked
/// (x_frac, y_frac) pairs, NOT derived from any RNG call. Step 1's
/// determinism pin (`scatter_pattern_is_deterministic`) requires
/// `pattern_points` to return the exact same points on every call — a real
/// randomness source (even a seeded one, absent extra plumbing to pin that
/// seed) is more risk than this purely-decorative legend schematic is
/// worth, so the "randomness" is simply pre-computed once, by hand, into
/// this table. Values stay within `[0.03, 0.97]` so no dot sits flush on
/// the mini card's own border.
const SCATTER_SEED: [(f64, f64); 22] = [
    (0.04, 0.52),
    (0.09, 0.18),
    (0.13, 0.74),
    (0.18, 0.35),
    (0.22, 0.88),
    (0.27, 0.09),
    (0.31, 0.61),
    (0.36, 0.44),
    (0.40, 0.93),
    (0.45, 0.22),
    (0.49, 0.67),
    (0.54, 0.13),
    (0.58, 0.79),
    (0.63, 0.31),
    (0.67, 0.55),
    (0.72, 0.85),
    (0.76, 0.06),
    (0.81, 0.48),
    (0.85, 0.72),
    (0.90, 0.27),
    (0.94, 0.59),
    (0.97, 0.15),
];

/// Scattered dots, no line — weak/dirty signal (Step 1 pin: deterministic,
/// same output twice — see [`SCATTER_SEED`]'s own doc comment for why this
/// is a fixed table rather than any RNG call).
fn scatter_points(rect: egui::Rect) -> Vec<egui::Pos2> {
    SCATTER_SEED
        .iter()
        .map(|&(x, y)| frac_to_pos(rect, x, y))
        .collect()
}

/// The "sudden vertical jumps" schematic's three flat plateaus, as
/// (x_frac_start, x_frac_end, y_frac) triples — level thirds of the width
/// at three different heights, so the two transitions between them (50%,
/// then 35%, of the card's height) both clear the Step 1 pin's "> 30% of
/// height" floor.
const JUMP_PLATEAUS: [(f64, f64, f64); 3] =
    [(0.0, 0.32, 0.25), (0.34, 0.66, 0.75), (0.68, 1.0, 0.40)];

/// Three disconnected flat segments at different heights — a step function,
/// broken (not interpolated) between plateaus so the "sudden" jump reads as
/// an actual discontinuity rather than a steep diagonal (same "never draw a
/// line across what wasn't observed together" honesty shape
/// `ui::charts::amp_segments`/`ui::charts::trend_segments` use for gap/wrap
/// breaks, applied here to a didactic step rather than real data). Step 1
/// pin: exactly 2 discontinuities > 30% of the rect's height — see
/// [`JUMP_PLATEAUS`]'s own doc comment for why both of this triple's two
/// boundaries clear that floor.
fn jumps_points(rect: egui::Rect) -> Vec<Vec<egui::Pos2>> {
    JUMP_PLATEAUS
        .iter()
        .map(|&(x0, x1, y)| vec![frac_to_pos(rect, x0, y), frac_to_pos(rect, x1, y)])
        .collect()
}

/// Pure polyline-group geometry for `kind`'s schematic within `rect`
/// (screen px) — one `Vec<Pos2>` per drawn segment; the renderer paints each
/// group as tick-colored dots (`Scatter`) or a 1.5px line (every other
/// kind).
pub fn pattern_points(kind: PatternKind, rect: egui::Rect) -> Vec<Vec<egui::Pos2>> {
    match kind {
        PatternKind::Level => vec![level_points(rect)],
        PatternKind::Slope => vec![slope_points(rect)],
        PatternKind::Bands => bands_points(rect),
        PatternKind::Wavy => vec![wavy_points(rect)],
        PatternKind::Scatter => vec![scatter_points(rect)],
        PatternKind::Jumps => jumps_points(rect),
    }
}

// ---------------------------------------------------------------------
// Rendering: the pattern-legend modal. Exercised by `cargo build` + the
// workspace test suite; no window in CI, so nothing below is unit-tested
// directly — see the module doc comment.
// ---------------------------------------------------------------------

/// Each mini schematic card's fixed size, px (this task's brief: "~120×48").
const CARD_W: f32 = 120.0;
const CARD_H: f32 = 48.0;
/// Inset applied within each mini card before mapping `pattern_points` onto
/// it, so a line/dot never sits flush on the card's own border — same role
/// as `ui::scope`'s `SCOPE_ROW_INSET`.
const CARD_INSET: f32 = 6.0;
/// Schematic line stroke width, px (this task's brief: "1.5 px lines").
const LINE_WIDTH: f32 = 1.5;
/// Scatter dot radius, px — same scale as the real beat trace's own
/// close-zoom dot radius (`ui::charts::dot_radius`'s `span_s <= 200` case),
/// not imported from it (that fn is private and span-keyed, unrelated logic
/// here — same "each module owns its own tiny presentational constants"
/// precedent as `ui::scope`'s `SCOPE_ROW_INSET`).
const DOT_RADIUS: f32 = 1.8;
/// Vertical gap between consecutive [`PATTERNS`] rows in the modal.
const ROW_GAP: f32 = 14.0;
/// Horizontal gap between a row's schematic card and its text.
const ROW_TEXT_GAP: f32 = 14.0;

/// Renders one row's mini schematic: a `palette.chartbg` card (same fill/
/// border language as the real chart panels, at a smaller corner radius
/// appropriate to this miniature scale — `ui::charts::beat_trace_panel`/
/// `ui::scope::scope_panel`'s convention scaled down), with
/// `pattern_points` painted inside it as tick-colored dots
/// (`PatternKind::Scatter`) or a tick-colored 1.5px line (every other kind
/// — a single consistent color across all six mini schematics, since this
/// legend compares SHAPE, not color; the real beat trace's tick/tock color
/// split has no counterpart in a didactic illustration of one abstract
/// trace).
fn schematic_card(ui: &mut egui::Ui, palette: &Palette, kind: PatternKind) {
    let (response, painter) = ui.allocate_painter(egui::vec2(CARD_W, CARD_H), egui::Sense::hover());
    let rect = response.rect;
    painter.rect_filled(rect, 6.0, palette.chartbg);
    painter.rect_stroke(
        rect,
        6.0,
        egui::Stroke::new(1.0, palette.border),
        egui::StrokeKind::Inside,
    );

    let inner = rect.shrink(CARD_INSET);
    let groups = pattern_points(kind, inner);
    if kind == PatternKind::Scatter {
        for group in &groups {
            for &p in group {
                painter.circle_filled(p, DOT_RADIUS, palette.tick);
            }
        }
    } else {
        for group in &groups {
            if group.len() >= 2 {
                painter.line(group.clone(), egui::Stroke::new(LINE_WIDTH, palette.tick));
            }
        }
    }
}

/// Renders one [`PATTERNS`] row: the mini schematic card, then a semibold
/// label and the pinned explanation text beside it.
fn pattern_row(ui: &mut egui::Ui, palette: &Palette, row: &PatternRow) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = ROW_TEXT_GAP;
        schematic_card(ui, palette, row.kind);
        ui.vertical(|ui| {
            ui.label(
                egui::RichText::new(row.label)
                    .font(egui::FontId::new(
                        13.0,
                        egui::FontFamily::Name(crate::theme::FAMILY_SEMIBOLD.into()),
                    ))
                    .color(palette.text),
            );
            ui.add_space(2.0);
            ui.label(
                egui::RichText::new(row.text)
                    .size(12.5)
                    .color(palette.muted),
            );
        });
    });
}

/// Renders the classic tape-pattern legend modal when `*open` (no-op
/// otherwise): overlay + centered card, same shape as `ui::modals::
/// render_help_modal` (M4a modal convention) — title + ✕ header, then the
/// six [`PATTERNS`] rows (schematic + copy). ✕, a click on the scrim
/// outside the card, and Esc all close — same three-way close contract as
/// `render_help_modal`/`render_add_watch_modal`.
///
/// `just_opened_this_frame`: `true` on the exact frame `*open` transitions
/// to `true` (the beat-trace header's "?" button click) — same reason
/// `render_help_modal` takes this as a caller-computed flag rather than
/// tracking it internally: without it, the SAME click that opened the modal
/// would register as an "outside" click against the not-yet-shown card and
/// close it again in the same frame (identical bug/fix shape to
/// `render_add_watch_modal`'s own `just_opened`).
pub fn render_pattern_legend_modal(
    ctx: &egui::Context,
    palette: &Palette,
    open: &mut bool,
    just_opened_this_frame: bool,
) {
    if !*open {
        return;
    }

    let screen = ctx.input(|i| i.content_rect());
    let scrim_layer = egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("chrona_patterns_scrim"),
    );
    ctx.layer_painter(scrim_layer).rect_filled(
        screen,
        0.0,
        crate::theme::with_alpha(palette.overlay, 140),
    );

    let mut close = false;

    let card_response = egui::Area::new(egui::Id::new("chrona_patterns_card"))
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(palette.panel)
                .stroke(egui::Stroke::new(1.0, palette.border2))
                .corner_radius(14.0)
                .inner_margin(22.0)
                .show(ui, |ui| {
                    ui.set_width(560.0_f32.min(screen.width() - 48.0 - 44.0));

                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new("Classic tape patterns")
                                .font(egui::FontId::new(
                                    16.0,
                                    egui::FontFamily::Name(crate::theme::FAMILY_SEMIBOLD.into()),
                                ))
                                .color(palette.text),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let close_resp = ui.add(
                                egui::Button::new(
                                    egui::RichText::new("✕").size(14.0).color(palette.muted),
                                )
                                .fill(palette.panel2)
                                .stroke(egui::Stroke::new(1.0, palette.border2)),
                            );
                            if close_resp.clicked() {
                                close = true;
                            }
                        });
                    });
                    ui.add_space(10.0);

                    for (i, row) in PATTERNS.iter().enumerate() {
                        pattern_row(ui, palette, row);
                        if i + 1 < PATTERNS.len() {
                            ui.add_space(ROW_GAP);
                        }
                    }
                });
        })
        .response;

    if !just_opened_this_frame
        && ctx.input(|i| i.pointer.any_click())
        && ctx
            .input(|i| i.pointer.interact_pos())
            .is_some_and(|pos| !card_response.rect.contains(pos))
    {
        close = true;
    }
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        close = true;
    }

    if close {
        *open = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_rect() -> egui::Rect {
        egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(120.0, 48.0))
    }

    #[test]
    fn level_pattern_is_one_group_with_constant_y() {
        let rect = test_rect();
        let groups = pattern_points(PatternKind::Level, rect);
        assert_eq!(groups.len(), 1);
        let g = &groups[0];
        assert!(g.len() >= 2);
        let y0 = g[0].y;
        for p in g {
            assert!((p.y - y0).abs() < 1e-4);
        }
    }

    #[test]
    fn slope_pattern_rises_left_to_right_matching_up_is_fast_convention() {
        let rect = test_rect();
        let groups = pattern_points(PatternKind::Slope, rect);
        assert_eq!(groups.len(), 1);
        let g = &groups[0];
        let first = *g.first().expect("slope has at least 2 points");
        let last = *g.last().expect("slope has at least 2 points");
        assert!(last.x > first.x, "slope must run left-to-right");
        assert!(
            last.y < first.y,
            "rising (fast) must decrease screen y left-to-right — up-is-fast convention"
        );
    }

    #[test]
    fn bands_pattern_is_two_groups_with_constant_gap() {
        let rect = test_rect();
        let groups = pattern_points(PatternKind::Bands, rect);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].len(), groups[1].len());
        assert!(!groups[0].is_empty());
        let gap0 = groups[1][0].y - groups[0][0].y;
        for (i, (p0, p1)) in groups[0].iter().zip(groups[1].iter()).enumerate() {
            let gap = p1.y - p0.y;
            assert!(
                (gap - gap0).abs() < 1e-4,
                "band gap must stay constant across x: gap0={gap0} gap[{i}]={gap}"
            );
        }
        assert!(
            gap0.abs() > 1e-3,
            "the two bands must actually be offset from each other, got gap {gap0}"
        );
    }

    #[test]
    fn wavy_pattern_oscillates_with_at_least_three_dy_sign_changes() {
        let rect = test_rect();
        let groups = pattern_points(PatternKind::Wavy, rect);
        assert_eq!(groups.len(), 1);
        let g = &groups[0];
        let mut sign_changes = 0;
        let mut last_sign = 0i32;
        for w in g.windows(2) {
            let dy = w[1].y - w[0].y;
            let sign = if dy > 1e-6 {
                1
            } else if dy < -1e-6 {
                -1
            } else {
                0
            };
            if sign != 0 {
                if last_sign != 0 && sign != last_sign {
                    sign_changes += 1;
                }
                last_sign = sign;
            }
        }
        assert!(
            sign_changes >= 3,
            "expected >= 3 dy sign changes, got {sign_changes}"
        );
    }

    #[test]
    fn scatter_pattern_is_deterministic() {
        let rect = test_rect();
        let a = pattern_points(PatternKind::Scatter, rect);
        let b = pattern_points(PatternKind::Scatter, rect);
        assert_eq!(a.len(), b.len());
        for (ga, gb) in a.iter().zip(b.iter()) {
            assert_eq!(ga.len(), gb.len());
            for (pa, pb) in ga.iter().zip(gb.iter()) {
                assert_eq!(pa.x, pb.x);
                assert_eq!(pa.y, pb.y);
            }
        }
    }

    #[test]
    fn jumps_pattern_has_exactly_two_large_discontinuities() {
        let rect = test_rect();
        let groups = pattern_points(PatternKind::Jumps, rect);
        assert!(
            groups.len() >= 2,
            "jumps needs at least 2 groups to have any discontinuity"
        );
        let threshold = 0.3 * rect.height();
        let mut discontinuities = 0;
        for w in groups.windows(2) {
            let prev_end = w[0].last().expect("non-empty group").y;
            let next_start = w[1].first().expect("non-empty group").y;
            if (next_start - prev_end).abs() > threshold {
                discontinuities += 1;
            }
        }
        assert_eq!(discontinuities, 2);
    }

    #[test]
    fn all_pattern_points_are_finite_and_within_rect_bounds() {
        let rect = test_rect();
        let kinds = [
            PatternKind::Level,
            PatternKind::Slope,
            PatternKind::Bands,
            PatternKind::Wavy,
            PatternKind::Scatter,
            PatternKind::Jumps,
        ];
        for kind in kinds {
            for group in pattern_points(kind, rect) {
                for p in group {
                    assert!(
                        p.x.is_finite() && p.y.is_finite(),
                        "{kind:?} produced a non-finite point"
                    );
                    assert!(
                        p.x >= rect.left() - 1e-3 && p.x <= rect.right() + 1e-3,
                        "{kind:?} point x={} outside rect [{}, {}]",
                        p.x,
                        rect.left(),
                        rect.right()
                    );
                    assert!(
                        p.y >= rect.top() - 1e-3 && p.y <= rect.bottom() + 1e-3,
                        "{kind:?} point y={} outside rect [{}, {}]",
                        p.y,
                        rect.top(),
                        rect.bottom()
                    );
                }
            }
        }
    }

    #[test]
    fn patterns_complete_and_non_empty() {
        assert_eq!(PATTERNS.len(), 6);
        for row in &PATTERNS {
            assert!(!row.label.is_empty(), "{:?} label empty", row.kind);
            assert!(!row.text.is_empty(), "{:?} text empty", row.kind);
        }
    }

    #[test]
    fn patterns_cover_every_kind_in_declared_order() {
        let expected = [
            PatternKind::Level,
            PatternKind::Slope,
            PatternKind::Bands,
            PatternKind::Wavy,
            PatternKind::Scatter,
            PatternKind::Jumps,
        ];
        for (row, kind) in PATTERNS.iter().zip(expected.iter()) {
            assert_eq!(row.kind, *kind);
        }
    }

    /// Overclaim guard (this task's brief): rows 4-6 (wavy/scatter/jumps)
    /// each point at a possible cause rather than asserting one outright —
    /// every one hedges with "suggests", and the scatter row's actionable
    /// next step ("Check placement...") uses "Check" rather than a bare
    /// imperative diagnosis. Asserted against the fixed row INDEX (3..6),
    /// not searched by kind, so this test fails loudly if `PATTERNS`' order
    /// ever drifted from `PatternKind`'s declaration order — a second,
    /// independent check on top of
    /// `patterns_cover_every_kind_in_declared_order`.
    #[test]
    fn patterns_rows_4_to_6_carry_the_overclaim_guard_hedging() {
        for row in &PATTERNS[3..6] {
            assert!(
                row.text.contains("suggests"),
                "{:?} must hedge its diagnosis with 'suggests': {}",
                row.kind,
                row.text
            );
        }
        assert!(
            PATTERNS[4].text.contains("Check"),
            "scatter row must include a 'Check ...' actionable hedge: {}",
            PATTERNS[4].text
        );
    }

    /// Defensive transcription-fidelity guard (this repo's convention per
    /// `ui::modals::HELP_TOPICS`' doc comment: "every dash/space/
    /// punctuation mark is significant") — the pinned copy's hedge clause is
    /// introduced with an em dash (—, U+2014), not a hyphen or en dash.
    #[test]
    fn patterns_rows_4_to_6_use_an_em_dash_before_the_hedge() {
        for row in &PATTERNS[3..6] {
            assert!(
                row.text.contains('\u{2014}'),
                "{:?} must use an em dash before its hedge, per the pinned copy: {}",
                row.kind,
                row.text
            );
        }
    }
}
