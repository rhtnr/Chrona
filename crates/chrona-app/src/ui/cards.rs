//! Metric cards (M4a Task 7, mockup: "Metrics band"): the four RATE/BEAT
//! ERROR/AMPLITUDE/BEAT RATE cards that replace M3's `instrument::
//! numerals_strip`. The per-metric `(value, caption)` cell functions below
//! are moved from `ui::instrument` (there is no pre-existing pinned test
//! for any of them — see this task's report) and adapted to the new
//! layout's split value/unit styling: `value` is now the BARE numeral (no
//! embedded unit, no inline "⚠ uncal" marker — see [`rate_cell`]'s doc
//! comment), and `caption` is used by the render layer ONLY in the
//! below-tier case (`value == "—"`), since an at-tier caption would just
//! repeat the card's own persistent header label. The pure helpers
//! (`rate_cell`/`beat_error_cell`/`amplitude_cell`/`bph_cell`/
//! `uncal_pill_shown`/`bph_mode_tag`) are TDD'd first, below; the
//! egui-facing rendering that follows is exercised only by `cargo build` +
//! the workspace test suite (no window in CI) — same split as
//! `controls.rs`/`toolbar.rs`.

use chrona_dsp::{AmplitudeGateFail, MetricsSnapshot};
use eframe::egui;

use crate::presenter::format_bph_grouped;
use crate::theme::Palette;
use crate::ui::modals::HelpTopicId;
use crate::ui::toolbar::BphComboItem;

/// Below-tier / no-data sentinel every cell fn returns as `value` — single
/// source of truth so the render layer's "is this below tier" check
/// (`value_line`) can't drift from what the cell fns actually emit.
const EM_DASH: &str = "—";

/// `(value, caption)` for the RATE card. `value` is the bare signed numeral
/// (e.g. `"+12.0"`) with no embedded unit or calibration marker — unlike
/// `presenter::format_rate` (whose "⚠ uncal" suffix and unit are baked into
/// one string for the frozen headless/CLI report, and which stays
/// untouched by this task for exactly that reason), the card splits the
/// unit into its own 15pt-muted run (`value_line`) and conveys
/// uncalibrated-ness with a separate pill instead of inline text (see
/// [`uncal_pill_shown`]). `caption` is the M3 gate-reason text; the render
/// layer only shows it when `value` is the below-tier em dash.
pub fn rate_cell(metrics: Option<&MetricsSnapshot>) -> (String, String) {
    match metrics {
        Some(m) => match m.rate_s_per_day {
            Some(r) => (format!("{r:+.1}"), "RATE".to_string()),
            None => (EM_DASH.to_string(), "no BPH match".to_string()),
        },
        None => (EM_DASH.to_string(), "RATE".to_string()),
    }
}

/// `(value, caption)` for the BEAT ERROR card; only defined at tier ≥ T2 —
/// see [`rate_cell`]'s doc comment for the bare-numeral/below-tier-only-
/// caption shape.
pub fn beat_error_cell(metrics: Option<&MetricsSnapshot>) -> (String, String) {
    match metrics {
        Some(m) => match m.beat_error_ms {
            Some(v) => (format!("{v:.1}"), "BEAT ERROR".to_string()),
            None => (EM_DASH.to_string(), "need Tier 2".to_string()),
        },
        None => (EM_DASH.to_string(), "BEAT ERROR".to_string()),
    }
}

/// `(value, caption)` for the AMPLITUDE card. When gated, the specific gate
/// reason becomes the caption (mirrors M3's `instrument::amplitude_cell`,
/// moved here) instead of being embedded parenthetically in the value the
/// way `presenter::format_amplitude` does for the frozen headless report.
pub fn amplitude_cell(metrics: Option<&MetricsSnapshot>) -> (String, String) {
    match metrics {
        Some(m) => match m.amplitude_deg {
            Some(deg) => (format!("{deg:.0}"), "AMPLITUDE".to_string()),
            None => {
                let caption = match m.quality.amplitude_gate {
                    Some(AmplitudeGateFail::TicTocDisagree) => "tic/toc disagree",
                    Some(AmplitudeGateFail::OutOfRange) => "out of range",
                    _ => "need Tier 3",
                };
                (EM_DASH.to_string(), caption.to_string())
            }
        },
        None => (EM_DASH.to_string(), "AMPLITUDE".to_string()),
    }
}

/// `(value, caption)` for the BEAT RATE card. Always renderable once
/// `metrics` exists (`bph_detected` has no `None` state): shows the
/// analyzer's snapped nominal when it has one, else the raw detected value
/// tilded — both run through `format_bph_grouped` (mockup: thin-space
/// thousands grouping) rather than M3's plain-digit formatting. Caption
/// says "BEAT RATE" (not M3's "BPH") to match this card's mockup header
/// label.
pub fn bph_cell(metrics: Option<&MetricsSnapshot>) -> (String, String) {
    match metrics {
        Some(m) => {
            let value = match m.bph_nominal {
                Some(n) => format_bph_grouped(n as f64),
                None => format!("~{}", format_bph_grouped(m.bph_detected)),
            };
            (value, "BEAT RATE".to_string())
        }
        None => (EM_DASH.to_string(), "BEAT RATE".to_string()),
    }
}

/// Whether the RATE card's "uncal" pill should show (mockup: warnbg/warnfg
/// pill) — iff a rate is currently shown at all (`rate_shown`) AND the
/// metrics are not calibrated. PURE.
pub fn uncal_pill_shown(rate_shown: bool, calibrated: bool) -> bool {
    rate_shown && !calibrated
}

/// The BEAT RATE card's right-side mode tag (mockup: "auto"/"fixed"/
/// "free", faint) for the toolbar's current `BphComboItem` selection.
/// `Other…` still resolves to "fixed" — it's the free-text `Fixed` entry
/// point (`ui::toolbar::apply_bph_selection`), not a distinct mode. PURE.
fn bph_mode_tag(selection: BphComboItem) -> &'static str {
    match selection {
        BphComboItem::Auto => "auto",
        BphComboItem::Free => "free",
        BphComboItem::Value(_) | BphComboItem::Other => "fixed",
    }
}

// ---------------------------------------------------------------------
// Rendering: the metrics band. Exercised by `cargo build` + the workspace
// test suite; no window in CI, so nothing here is unit-tested directly —
// see the module doc comment.
// ---------------------------------------------------------------------

/// Each card's mockup floor (`grid-template-columns:repeat(auto-fit,
/// minmax(200px, 1fr))`) — used only to pick between the two
/// `metrics_cards` layouts below, not as a hard per-card minimum.
const MIN_CARD_W: f32 = 200.0;

/// Metrics-band column count for `available_width` (T10 rider: the mockup
/// reflows via CSS `auto-fit`; this is a simple two-state approximation of
/// that, not a full responsive grid — 4-up once there's comfortably enough
/// room for four `MIN_CARD_W`-wide cards, else a 2x2 stack). PURE.
fn cards_columns(available_width: f32) -> usize {
    if available_width < 4.0 * MIN_CARD_W {
        2
    } else {
        4
    }
}

/// Renders the metrics band (mockup: "Metrics band") — 4-up, or a 2x2
/// stack on a narrow window (`cards_columns`) — and returns `Some(topic)`
/// iff one card's "?" button was clicked this frame — the caller
/// (`ChronaApp::ui`) owns `open_help` and decides what "just opened" means
/// for `ui::modals::render_help_modal`'s click-outside guard, so this
/// function only ever reports the request, never mutates help-modal state
/// itself.
pub fn metrics_cards(
    ui: &mut egui::Ui,
    palette: &Palette,
    metrics: Option<&MetricsSnapshot>,
    bph_selection: BphComboItem,
) -> Option<HelpTopicId> {
    let mut clicked = None;
    if cards_columns(ui.available_width()) == 4 {
        ui.columns(4, |cols| {
            if rate_card(&mut cols[0], palette, metrics) {
                clicked = Some(HelpTopicId::Rate);
            }
            if beat_error_card(&mut cols[1], palette, metrics) {
                clicked = Some(HelpTopicId::BeatError);
            }
            if amplitude_card(&mut cols[2], palette, metrics) {
                clicked = Some(HelpTopicId::Amplitude);
            }
            if bph_card(&mut cols[3], palette, metrics, bph_selection) {
                clicked = Some(HelpTopicId::BeatRate);
            }
        });
    } else {
        ui.columns(2, |cols| {
            if rate_card(&mut cols[0], palette, metrics) {
                clicked = Some(HelpTopicId::Rate);
            }
            if beat_error_card(&mut cols[1], palette, metrics) {
                clicked = Some(HelpTopicId::BeatError);
            }
        });
        ui.add_space(12.0); // mockup: metrics-band grid `gap:12px`
        ui.columns(2, |cols| {
            if amplitude_card(&mut cols[0], palette, metrics) {
                clicked = Some(HelpTopicId::Amplitude);
            }
            if bph_card(&mut cols[1], palette, metrics, bph_selection) {
                clicked = Some(HelpTopicId::BeatRate);
            }
        });
    }
    clicked
}

fn card_frame(ui: &mut egui::Ui, palette: &Palette, add_contents: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(palette.panel)
        .stroke(egui::Stroke::new(1.0, palette.border))
        .corner_radius(12.0)
        .inner_margin(egui::Margin::symmetric(16, 14))
        .show(ui, |ui| {
            ui.set_min_height(88.0);
            add_contents(ui);
        });
}

fn help_button(ui: &mut egui::Ui, palette: &Palette) -> egui::Response {
    ui.add_sized(
        [16.0, 16.0],
        egui::Button::new(egui::RichText::new("?").size(10.0).color(palette.faint))
            .fill(egui::Color32::TRANSPARENT)
            .stroke(egui::Stroke::new(1.0, palette.border2))
            .corner_radius(8.0),
    )
}

/// Label + "?" button, left-aligned (mockup's card header row). Returns
/// `true` iff the "?" was clicked this frame.
fn card_label_with_help(ui: &mut egui::Ui, palette: &Palette, label: &str) -> bool {
    let mut clicked = false;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        ui.label(egui::RichText::new(label).size(11.0).color(palette.muted));
        if help_button(ui, palette).clicked() {
            clicked = true;
        }
    });
    clicked
}

const UNCAL_TOOLTIP: &str = "Not calibrated — measured against uncalibrated system clock";

fn uncal_pill(ui: &mut egui::Ui, palette: &Palette) {
    egui::Frame::new()
        .fill(palette.warnbg)
        .corner_radius(99.0)
        .inner_margin(egui::Margin::symmetric(8, 2))
        .show(ui, |ui| {
            ui.label(
                egui::RichText::new("uncal")
                    .size(11.0)
                    .color(palette.warnfg),
            );
        })
        .response
        .on_hover_text(UNCAL_TOOLTIP);
}

/// The 30pt monospace value + 15pt muted unit (mockup: value line), or —
/// below tier (`value == EM_DASH`) — the value plus the 11pt muted `caption`
/// gate-reason instead of a unit.
fn value_line(ui: &mut egui::Ui, palette: &Palette, value: &str, unit: &str, caption: &str) {
    ui.add_space(4.0);
    if value == EM_DASH {
        ui.label(
            egui::RichText::new(value)
                .monospace()
                .size(30.0)
                .color(palette.text),
        );
        ui.label(egui::RichText::new(caption).size(11.0).color(palette.muted));
    } else {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            ui.label(
                egui::RichText::new(value)
                    .monospace()
                    .size(30.0)
                    .color(palette.text),
            );
            ui.label(egui::RichText::new(unit).size(15.0).color(palette.muted));
        });
    }
}

fn rate_card(ui: &mut egui::Ui, palette: &Palette, metrics: Option<&MetricsSnapshot>) -> bool {
    let (value, caption) = rate_cell(metrics);
    let rate_shown = metrics.and_then(|m| m.rate_s_per_day).is_some();
    let calibrated = metrics.is_some_and(|m| m.calibrated);
    let show_pill = uncal_pill_shown(rate_shown, calibrated);
    let mut help_clicked = false;
    card_frame(ui, palette, |ui| {
        ui.horizontal(|ui| {
            if card_label_with_help(ui, palette, "RATE") {
                help_clicked = true;
            }
            if show_pill {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    uncal_pill(ui, palette);
                });
            }
        });
        value_line(ui, palette, &value, " s/d", &caption);
    });
    help_clicked
}

fn beat_error_card(
    ui: &mut egui::Ui,
    palette: &Palette,
    metrics: Option<&MetricsSnapshot>,
) -> bool {
    let (value, caption) = beat_error_cell(metrics);
    let mut help_clicked = false;
    card_frame(ui, palette, |ui| {
        if card_label_with_help(ui, palette, "BEAT ERROR") {
            help_clicked = true;
        }
        value_line(ui, palette, &value, " ms", &caption);
    });
    help_clicked
}

fn amplitude_card(ui: &mut egui::Ui, palette: &Palette, metrics: Option<&MetricsSnapshot>) -> bool {
    let (value, caption) = amplitude_cell(metrics);
    let mut help_clicked = false;
    card_frame(ui, palette, |ui| {
        if card_label_with_help(ui, palette, "AMPLITUDE") {
            help_clicked = true;
        }
        value_line(ui, palette, &value, "°", &caption);
    });
    help_clicked
}

fn bph_card(
    ui: &mut egui::Ui,
    palette: &Palette,
    metrics: Option<&MetricsSnapshot>,
    bph_selection: BphComboItem,
) -> bool {
    let (value, caption) = bph_cell(metrics);
    let mut help_clicked = false;
    card_frame(ui, palette, |ui| {
        ui.horizontal(|ui| {
            if card_label_with_help(ui, palette, "BEAT RATE") {
                help_clicked = true;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    egui::RichText::new(bph_mode_tag(bph_selection))
                        .size(11.0)
                        .color(palette.faint),
                );
            });
        });
        value_line(ui, palette, &value, " bph", &caption);
    });
    help_clicked
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrona_dsp::{PeriodEstimate, Quality, Tier};

    fn mk_metrics(
        rate: Option<f64>,
        beat_error: Option<f64>,
        amplitude: Option<f64>,
        gate: Option<AmplitudeGateFail>,
        bph_nominal: Option<u32>,
        bph_detected: f64,
        calibrated: bool,
    ) -> MetricsSnapshot {
        MetricsSnapshot {
            tier: Tier::T3,
            bph_detected,
            bph_nominal,
            rate_s_per_day: rate,
            rate_source: None,
            beat_error_ms: beat_error,
            amplitude_deg: amplitude,
            period: PeriodEstimate {
                t_osc_s: 0.25,
                sigma_s: 0.0,
                window_s: 4.0,
            },
            quality: Quality {
                detection_ratio: 1.0,
                onset_jitter_ms: None,
                mean_beat_snr_db: None,
                unlocking_ratio: 0.0,
                clipped_samples: 0,
                amplitude_gate: gate,
            },
            calibrated,
        }
    }

    #[test]
    fn rate_cell_value_and_caption() {
        let m = mk_metrics(Some(12.34), None, None, None, None, 0.0, true);
        assert_eq!(
            rate_cell(Some(&m)),
            ("+12.3".to_string(), "RATE".to_string())
        );
        let m_neg = mk_metrics(Some(-3.0), None, None, None, None, 0.0, false);
        assert_eq!(
            rate_cell(Some(&m_neg)),
            ("-3.0".to_string(), "RATE".to_string()),
            "no embedded unit or uncal marker in the bare value, regardless of calibration"
        );
        let m_none = mk_metrics(None, None, None, None, None, 0.0, true);
        assert_eq!(
            rate_cell(Some(&m_none)),
            ("—".to_string(), "no BPH match".to_string())
        );
        assert_eq!(rate_cell(None), ("—".to_string(), "RATE".to_string()));
    }

    #[test]
    fn beat_error_cell_value_and_caption() {
        let m = mk_metrics(None, Some(0.84), None, None, None, 0.0, true);
        assert_eq!(
            beat_error_cell(Some(&m)),
            ("0.8".to_string(), "BEAT ERROR".to_string())
        );
        let m_none = mk_metrics(None, None, None, None, None, 0.0, true);
        assert_eq!(
            beat_error_cell(Some(&m_none)),
            ("—".to_string(), "need Tier 2".to_string())
        );
        assert_eq!(
            beat_error_cell(None),
            ("—".to_string(), "BEAT ERROR".to_string())
        );
    }

    #[test]
    fn amplitude_cell_value_and_gate_reasons() {
        let m = mk_metrics(None, None, Some(270.4), None, None, 0.0, true);
        assert_eq!(
            amplitude_cell(Some(&m)),
            ("270".to_string(), "AMPLITUDE".to_string())
        );
        let disagree = mk_metrics(
            None,
            None,
            None,
            Some(AmplitudeGateFail::TicTocDisagree),
            None,
            0.0,
            true,
        );
        assert_eq!(
            amplitude_cell(Some(&disagree)),
            ("—".to_string(), "tic/toc disagree".to_string())
        );
        let out_of_range = mk_metrics(
            None,
            None,
            None,
            Some(AmplitudeGateFail::OutOfRange),
            None,
            0.0,
            true,
        );
        assert_eq!(
            amplitude_cell(Some(&out_of_range)),
            ("—".to_string(), "out of range".to_string())
        );
        let need_t3 = mk_metrics(None, None, None, None, None, 0.0, true);
        assert_eq!(
            amplitude_cell(Some(&need_t3)),
            ("—".to_string(), "need Tier 3".to_string())
        );
        assert_eq!(
            amplitude_cell(None),
            ("—".to_string(), "AMPLITUDE".to_string())
        );
    }

    #[test]
    fn bph_cell_grouped_nominal_and_tilded_detected() {
        let nominal = mk_metrics(None, None, None, None, Some(28_800), 28_760.0, true);
        assert_eq!(
            bph_cell(Some(&nominal)),
            ("28\u{2009}800".to_string(), "BEAT RATE".to_string())
        );
        let detected_only = mk_metrics(None, None, None, None, None, 9_123.0, true);
        assert_eq!(
            bph_cell(Some(&detected_only)),
            ("~9\u{2009}123".to_string(), "BEAT RATE".to_string())
        );
        assert_eq!(bph_cell(None), ("—".to_string(), "BEAT RATE".to_string()));
    }

    #[test]
    fn uncal_pill_shown_iff_rate_and_uncalibrated() {
        assert!(uncal_pill_shown(true, false));
        assert!(!uncal_pill_shown(true, true));
        assert!(!uncal_pill_shown(false, false));
        assert!(!uncal_pill_shown(false, true));
    }

    #[test]
    fn cards_columns_switches_at_four_card_widths() {
        assert_eq!(cards_columns(799.0), 2, "just under 4*200px stacks 2x2");
        assert_eq!(cards_columns(800.0), 4, "exactly 4*200px is 4-up");
        assert_eq!(cards_columns(1200.0), 4, "comfortably wide stays 4-up");
        assert_eq!(cards_columns(400.0), 2, "narrow window stacks 2x2");
    }

    #[test]
    fn bph_mode_tag_mapping() {
        assert_eq!(bph_mode_tag(BphComboItem::Auto), "auto");
        assert_eq!(bph_mode_tag(BphComboItem::Free), "free");
        assert_eq!(bph_mode_tag(BphComboItem::Value(28_800)), "fixed");
        assert_eq!(bph_mode_tag(BphComboItem::Other), "fixed");
    }
}
