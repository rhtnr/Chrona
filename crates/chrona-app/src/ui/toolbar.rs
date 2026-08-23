//! Redesigned toolbar (M4a Task 6, replaces the M3 controls row): logo,
//! watch library combo + add-watch modal trigger, the device/lift/BPH/
//! averaging controls (M3 semantics, restyled per the mockup), the
//! timebase-calibration readout, and the right-aligned theme toggle /
//! open-recording / export / record controls. The pure decision helpers
//! below (`averaging_options`, `bph_combo_items`, `export_enabled`) are
//! TDD'd first; the egui-facing rendering that follows is exercised only by
//! `cargo build` + the workspace test suite (no window in CI) — same split
//! as `controls.rs`/`strip.rs`.

use std::path::Path;
use std::time::{Duration, Instant};

use chrona_session::ConfigStore;
use eframe::egui;

use crate::engine::{ControlMsg, Engine, EngineSnapshot, SourceSpec};
use crate::history::HistoryIndex;
use crate::theme::{self, Palette, Theme};
use crate::ui::controls::{
    BphModeUi, ControlsState, current_device_name, device_picker, to_bph_mode,
};
use crate::ui::modals::{AddWatchModalState, render_add_watch_modal};
use crate::ui::strip::SessionPanelState;

/// Averaging combo's fixed option set (behavior contract; spec §14 deviation
/// #1: the mockup's 120 s is dropped — the analyzer's valid range is
/// 2-60 s), seconds.
pub fn averaging_options() -> [f64; 3] {
    [10.0, 30.0, 60.0]
}

/// One BPH combo row (behavior contract, spec §12): a directly-sendable
/// fixed mode, or a sentinel — `Auto`/`Free` pass through unchanged, and
/// `Other` reveals the M3 validated free-text `Fixed` field instead of
/// carrying a value itself.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BphComboItem {
    Auto,
    Free,
    Value(u32),
    Other,
}

/// The BPH combo's fixed row order (behavior contract): Auto, Free, the 5
/// common beat rates, then `Other…` last.
pub fn bph_combo_items() -> [BphComboItem; 8] {
    [
        BphComboItem::Auto,
        BphComboItem::Free,
        BphComboItem::Value(18_000),
        BphComboItem::Value(21_600),
        BphComboItem::Value(25_200),
        BphComboItem::Value(28_800),
        BphComboItem::Value(36_000),
        BphComboItem::Other,
    ]
}

/// Export is enabled iff a watch is selected AND it has at least one
/// recorded session (behavior contract).
pub fn export_enabled(selected_watch: Option<&str>, has_sessions: bool) -> bool {
    selected_watch.is_some() && has_sessions
}

// ---------------------------------------------------------------------
// Rendering: the toolbar. Exercised by `cargo build` + the workspace test
// suite; no window in CI, so nothing here is unit-tested directly — see the
// module doc comment.
// ---------------------------------------------------------------------

/// Toolbar-owned state (M4a Task 6) that doesn't belong on `ControlsState`
/// (M3 device/lift/bph/averaging knobs) or `ConfigStore` (persisted
/// values): the selected watch (seeded from, and kept mirrored into,
/// `config.last_watch`), which BPH combo row is active (distinct from
/// `ControlsState::bph_mode_ui`, which only ever holds the free-text
/// buffer for the `Other…` case — see `apply_bph_selection`), the add-watch
/// modal, and the export-report request flag the Export button sets
/// (consumed each frame by `ChronaApp::run_export`, M4a Task 9).
pub struct ToolbarState {
    pub selected_watch: Option<String>,
    pub bph_selection: BphComboItem,
    pub add_watch_modal: AddWatchModalState,
    pub export_requested: bool,
}

/// Borrowed, per-frame context `toolbar_row` needs beyond `ToolbarState`:
/// bundled into one struct purely to keep `toolbar_row`'s own argument
/// count sane (`clippy::too_many_arguments`) — these fields aren't
/// otherwise related to each other.
pub struct ToolbarCtx<'a> {
    pub session: &'a mut SessionPanelState,
    pub snap: &'a EngineSnapshot,
    pub recordings_dir: &'a Path,
    pub history: &'a HistoryIndex,
    /// The canonical position code (`history::POSITIONS`) currently
    /// selected in the redesigned position strip (M4a Task 7; the strip
    /// itself renders below the toolbar, but `ChronaApp::ui` resolves the
    /// code from `selected_position` before either renders — see
    /// `record_stop_button`, the only consumer). Replaces the M3 session-
    /// panel's free-text position field this ctx used to carry via
    /// `session.position`.
    pub selected_position: &'a str,
}

/// Renders the full toolbar row (logo/title, watch combo, device/lift/bph/
/// averaging controls, cal readout, and the right-aligned theme/open/
/// export/record controls) plus the add-watch modal when open. Returns
/// `true` iff a *debounced* `ConfigStore` field changed this frame (the
/// device/lift knobs — same contract the old `controls_row` had), so the
/// caller can mark it dirty for `ChronaApp`'s debounced save. Watch
/// selection and the theme toggle are NOT part of that: both save
/// immediately, inline, per the M4a Task 6 behavior contract.
pub fn toolbar_row(
    ui: &mut egui::Ui,
    controls: &mut ControlsState,
    engine: &Engine,
    config: &mut ConfigStore,
    theme: &mut Theme,
    state: &mut ToolbarState,
    ctx: ToolbarCtx<'_>,
) -> bool {
    let palette = Palette::of(*theme);
    let recording = ctx.snap.recording.is_some();
    let mut dirty = false;

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;

        logo_and_title(ui, palette);
        ui.separator();
        watch_combo(
            ui,
            palette,
            config,
            &mut state.selected_watch,
            &mut state.add_watch_modal,
        );
        ui.separator();
        dirty |= device_picker(ui, controls, engine, config);
        dirty |= lift_control(ui, palette, controls, engine, config);
        bph_combo(ui, palette, controls, engine, &mut state.bph_selection);
        averaging_combo(ui, palette, controls, engine);
        dirty |= cal_ppm_control(ui, palette, controls, engine, config);

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            // Added in reverse of visual left-to-right order (mockup:
            // theme toggle, Open recording…, Export report, Record) since
            // `right_to_left` stacks from the row's right edge inward.
            record_stop_button(
                ui,
                palette,
                engine,
                recording,
                RecordButtonCtx {
                    session: ctx.session,
                    recordings_dir: ctx.recordings_dir,
                    selected_watch: &state.selected_watch,
                    selected_position: ctx.selected_position,
                },
            );
            export_button(
                ui,
                palette,
                state.selected_watch.as_deref(),
                ctx.history,
                &mut state.export_requested,
            );
            open_recording_button(ui, palette, engine, ctx.session);
            theme_toggle_button(ui, palette, theme, config);
        });
    });

    render_add_watch_modal(
        ui.ctx(),
        palette,
        &mut state.add_watch_modal,
        config,
        &mut state.selected_watch,
    );

    dirty
}

/// Saves `config` immediately (not the debounced save the continuous-drag
/// controls use) — the same ignore-with-`eprintln!` error style as
/// `ChronaApp::maybe_save_config`. Used by the watch combo/add-watch modal
/// and the theme toggle, both of which are the M4a Task 6 "save
/// immediately" behavior contract rather than a continuous drag.
fn persist_config_now(config: &ConfigStore) {
    if let Err(e) = config.save() {
        eprintln!("chrona: failed to save config: {e}");
    }
}

fn muted_label(palette: &Palette, text: &str) -> egui::RichText {
    egui::RichText::new(text).color(palette.muted).size(13.0)
}

fn logo_and_title(ui: &mut egui::Ui, palette: &Palette) {
    let (rect, _response) = ui.allocate_exact_size(egui::vec2(26.0, 26.0), egui::Sense::hover());
    if ui.is_rect_visible(rect) {
        ui.painter()
            .rect_filled(rect, egui::CornerRadius::same(7), palette.accent);
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "C",
            egui::FontId::new(13.0, egui::FontFamily::Name(theme::FAMILY_BOLD.into())),
            palette.accent_ink,
        );
    }
    ui.label(
        egui::RichText::new("Chrona")
            .font(egui::FontId::new(
                15.0,
                egui::FontFamily::Name(theme::FAMILY_SEMIBOLD.into()),
            ))
            .color(palette.text),
    );
}

/// Watch library combo (max width ~180, behavior contract): lists
/// `config.watches` plus a trailing "+ Add new watch…" row that opens the
/// add-watch modal. Selecting an existing watch persists `config.last_watch`
/// immediately (behavior contract) — not the debounced save `dirty`
/// triggers for the continuous-drag controls.
fn watch_combo(
    ui: &mut egui::Ui,
    palette: &Palette,
    config: &mut ConfigStore,
    selected_watch: &mut Option<String>,
    modal: &mut AddWatchModalState,
) {
    ui.label(muted_label(palette, "Watch"));
    let selected_text = selected_watch
        .clone()
        .unwrap_or_else(|| "Select watch".to_string());
    egui::ComboBox::from_id_salt("chrona_toolbar_watch")
        .selected_text(selected_text)
        .width(180.0)
        .show_ui(ui, |ui| {
            for w in config.watches.clone() {
                let is_selected = selected_watch.as_deref() == Some(w.as_str());
                if ui.selectable_label(is_selected, &w).clicked() && !is_selected {
                    *selected_watch = Some(w.clone());
                    config.last_watch = Some(w);
                    persist_config_now(config);
                }
            }
            ui.separator();
            if ui.selectable_label(false, "+ Add new watch…").clicked() {
                modal.open_modal();
            }
        });
}

/// Lift `DragValue` (monospace, clamp 10-90°, behavior contract): the M3
/// preset submenu is retired (spec §14 deviation #1's sibling — the mockup
/// shows a single numeric field, no presets).
fn lift_control(
    ui: &mut egui::Ui,
    palette: &Palette,
    controls: &mut ControlsState,
    engine: &Engine,
    config: &mut ConfigStore,
) -> bool {
    ui.label(muted_label(palette, "Lift"));
    let mut dirty = false;
    ui.scope(|ui| {
        ui.style_mut().override_font_id = Some(egui::FontId::monospace(13.0));
        let resp = ui.add(
            egui::DragValue::new(&mut controls.lift)
                .range(10.0..=90.0)
                .suffix("°"),
        );
        dirty = resp.changed();
    });
    if dirty {
        engine.send(ControlMsg::SetLift(controls.lift));
        config.default_lift_deg = controls.lift;
    }
    dirty
}

fn bph_combo_label(item: BphComboItem) -> String {
    match item {
        BphComboItem::Auto => "Auto".to_string(),
        BphComboItem::Free => "Free".to_string(),
        BphComboItem::Value(n) => n.to_string(),
        BphComboItem::Other => "Other…".to_string(),
    }
}

/// BPH combo (behavior contract): Auto/Free/the 5 common beat rates send
/// their `BphMode` directly; `Other…` reveals the M3 validated free-text
/// `Fixed` field (`bph_other_field`), which alone routes through
/// `to_bph_mode` since it's the only fallible case.
fn bph_combo(
    ui: &mut egui::Ui,
    palette: &Palette,
    controls: &mut ControlsState,
    engine: &Engine,
    selection: &mut BphComboItem,
) {
    ui.label(muted_label(palette, "BPH"));
    egui::ComboBox::from_id_salt("chrona_toolbar_bph")
        .selected_text(bph_combo_label(*selection))
        .show_ui(ui, |ui| {
            for item in bph_combo_items() {
                let is_selected = *selection == item;
                if ui
                    .selectable_label(is_selected, bph_combo_label(item))
                    .clicked()
                    && !is_selected
                {
                    apply_bph_selection(controls, engine, selection, item);
                }
            }
        });

    if *selection == BphComboItem::Other {
        bph_other_field(ui, palette, controls, engine);
    }
}

/// Applies one `BphComboItem` selection: `Auto`/`Free`/`Value(n)` all send
/// their `BphMode` straight to the engine (behavior contract: "the 5 fixed
/// values in the combo send `BphMode::Fixed(n)` directly"); `Other` primes
/// `controls.bph_mode_ui` with a starting value for `bph_other_field`
/// without sending anything itself (there's nothing valid to send until the
/// field's edited — same as the M3 selector's own "Fixed" row).
fn apply_bph_selection(
    controls: &mut ControlsState,
    engine: &Engine,
    selection: &mut BphComboItem,
    item: BphComboItem,
) {
    *selection = item;
    match item {
        BphComboItem::Auto => {
            controls.bph_mode_ui = BphModeUi::Auto;
            engine.send(ControlMsg::SetBphMode(chrona_dsp::BphMode::Auto));
        }
        BphComboItem::Free => {
            controls.bph_mode_ui = BphModeUi::Free;
            engine.send(ControlMsg::SetBphMode(chrona_dsp::BphMode::Free));
        }
        BphComboItem::Value(n) => {
            controls.bph_mode_ui = BphModeUi::Fixed(n.to_string());
            engine.send(ControlMsg::SetBphMode(chrona_dsp::BphMode::Fixed(n)));
        }
        BphComboItem::Other => {
            if !matches!(controls.bph_mode_ui, BphModeUi::Fixed(_)) {
                controls.bph_mode_ui = BphModeUi::Fixed("28800".to_string());
            }
        }
    }
}

/// The M3 validated free-text `Fixed` field (red outline while
/// unparseable), shown inline only while `Other…` is selected. Identical
/// validation/red-outline behavior to the old `bph_mode_selector`, just
/// restyled onto `palette.rec` instead of a bare `Color32::RED`.
fn bph_other_field(
    ui: &mut egui::Ui,
    palette: &Palette,
    controls: &mut ControlsState,
    engine: &Engine,
) {
    let BphModeUi::Fixed(text) = &mut controls.bph_mode_ui else {
        return;
    };
    let invalid = text.trim().parse::<u32>().is_err();
    let mut changed = false;
    ui.scope(|ui| {
        if invalid {
            let stroke = egui::Stroke::new(1.0, palette.rec);
            ui.visuals_mut().widgets.inactive.bg_stroke = stroke;
            ui.visuals_mut().widgets.hovered.bg_stroke = stroke;
            ui.visuals_mut().widgets.active.bg_stroke = stroke;
        }
        changed = ui
            .add(
                egui::TextEdit::singleline(text)
                    .desired_width(60.0)
                    .font(egui::FontId::monospace(13.0)),
            )
            .changed();
    });
    if changed && let Some(mode) = to_bph_mode(&controls.bph_mode_ui) {
        engine.send(ControlMsg::SetBphMode(mode));
    }
}

/// Averaging combo (behavior contract: 10 s / 30 s / 60 s — spec §14
/// deviation #1 drops the mockup's 120 s). Not persisted, same as M3.
fn averaging_combo(
    ui: &mut egui::Ui,
    palette: &Palette,
    controls: &mut ControlsState,
    engine: &Engine,
) {
    ui.label(muted_label(palette, "Averaging"));
    egui::ComboBox::from_id_salt("chrona_toolbar_averaging")
        .selected_text(format!("{:.0} s", controls.averaging))
        .show_ui(ui, |ui| {
            for opt in averaging_options() {
                let is_selected = controls.averaging == opt;
                if ui
                    .selectable_label(is_selected, format!("{opt:.0} s"))
                    .clicked()
                    && !is_selected
                {
                    controls.averaging = opt;
                    engine.send(ControlMsg::SetAveraging(opt));
                }
            }
        });
}

/// `cal {ppm:+.1} ppm` — a click target opening a compact ppm editor (spec
/// §14 deviation #10, pre-review fix: the mockup renders this as static
/// text, but the app keeps it editable — manual-protocol B.7 and per-device
/// calibration depend on in-app entry). The popup reuses the M3
/// `ppm_control` DragValue verbatim (range/step/`SetPpm`/`device_ppm`
/// persistence — see `git show 1bbd3ba:crates/chrona-app/src/ui/
/// controls.rs`), just moved behind a click instead of always visible.
/// `egui::Popup::from_toggle_button_response` handles the open/close state
/// (toggling on click) and, via `CloseOnClickOutside`, closing on an
/// outside click; Esc-closing is built into `Popup::show` unconditionally.
/// Returns `true` iff `config.device_ppm` changed this frame (same
/// debounced-dirty contract the old `ppm_control` had).
fn cal_ppm_control(
    ui: &mut egui::Ui,
    palette: &Palette,
    controls: &mut ControlsState,
    engine: &Engine,
    config: &mut ConfigStore,
) -> bool {
    let button_resp = ui
        .add(
            egui::Button::new(
                egui::RichText::new(format!("cal {:+.1} ppm", controls.ppm))
                    .font(egui::FontId::monospace(12.0))
                    .color(palette.faint),
            )
            .frame(false),
        )
        .on_hover_text("Timebase calibration — click to edit");

    egui::Popup::from_toggle_button_response(&button_resp)
        // Defensive stability (T6 review rider): `Popup` has no `id_salt`
        // builder (that's `ComboBox`'s API) — its own default id is
        // `button_resp.id.with("popup")`, and `button_resp.id` is itself an
        // auto-id derived from call order within the toolbar row. An
        // explicit `.id(...)` pins the popup's open/close memory to a fixed
        // key, decoupled from that positional id entirely (T6 review traced
        // the default to be safe today — banner renders after the toolbar
        // closure, combo interactions force-close popups — but a fixed id
        // costs nothing and removes the dependency on that reasoning
        // continuing to hold as the toolbar evolves).
        .id(egui::Id::new("chrona_cal_ppm"))
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            ui.set_min_width(150.0);
            ui.label(muted_label(palette, "Timebase calibration"));
            let changed = ui
                .add(
                    egui::DragValue::new(&mut controls.ppm)
                        .range(-500.0..=500.0)
                        .speed(0.1)
                        .suffix(" ppm"),
                )
                .changed();
            if !changed {
                return false;
            }
            engine.send(ControlMsg::SetPpm(controls.ppm));
            match current_device_name(controls) {
                Some(name) => {
                    config.device_ppm.insert(name, controls.ppm);
                    true
                }
                None => false,
            }
        })
        .map(|inner| inner.inner)
        .unwrap_or(false)
}

fn outline_button(ui: &mut egui::Ui, palette: &Palette, text: &str) -> egui::Response {
    ui.add(
        egui::Button::new(egui::RichText::new(text).color(palette.text))
            .fill(egui::Color32::TRANSPARENT)
            .stroke(egui::Stroke::new(1.0, palette.border2)),
    )
}

fn theme_toggle_button(
    ui: &mut egui::Ui,
    palette: &Palette,
    theme: &mut Theme,
    config: &mut ConfigStore,
) {
    let label = match theme {
        Theme::Dark => "☾ Dark",
        Theme::Light => "☀ Light",
    };
    let resp = outline_button(ui, palette, label).on_hover_text("Toggle light/dark");
    if resp.clicked() {
        *theme = match *theme {
            Theme::Dark => Theme::Light,
            Theme::Light => Theme::Dark,
        };
        theme::apply_style(ui.ctx(), *theme);
        config.theme = Some(theme::theme_config_value(*theme).to_string());
        persist_config_now(config);
    }
}

/// "Open recording…" (M3's rfd flow, moved here from the session panel
/// verbatim, behavior contract).
fn open_recording_button(
    ui: &mut egui::Ui,
    palette: &Palette,
    engine: &Engine,
    session: &mut SessionPanelState,
) {
    let resp = outline_button(ui, palette, "Open recording…");
    if resp.clicked()
        && let Some(path) = rfd::FileDialog::new()
            .add_filter("WAV", &["wav"])
            .pick_file()
    {
        session.replay_name = Some(
            path.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string()),
        );
        engine.send(ControlMsg::SwitchSource(SourceSpec::ReplayFile { path }));
    }
}

/// Export button: enabled iff `export_enabled` (a watch selected AND it has
/// recorded sessions), disabled otherwise with the behavior contract's
/// tooltip. A click only sets `export_requested`; `ChronaApp::run_export`
/// (M4a Task 9) consumes it on the next frame.
fn export_button(
    ui: &mut egui::Ui,
    palette: &Palette,
    selected_watch: Option<&str>,
    history: &HistoryIndex,
    export_requested: &mut bool,
) {
    let has_sessions = selected_watch.is_some_and(|w| history.for_watch(w).next().is_some());
    let enabled = export_enabled(selected_watch, has_sessions);
    ui.add_enabled_ui(enabled, |ui| {
        let resp = outline_button(ui, palette, "Export report");
        let resp = if enabled {
            resp
        } else {
            resp.on_disabled_hover_text("Select a watch with recorded sessions")
        };
        if resp.clicked() {
            *export_requested = true;
        }
    });
}

/// Borrowed, per-frame context `record_stop_button` needs beyond its own
/// leading arguments: bundled purely to keep the argument count sane
/// (`clippy::too_many_arguments` — empirically 8 params without this, one
/// over the 7-arg default threshold: T10 review rider, verified by
/// temporarily removing the old `#[allow]` and running clippy), same shape
/// as `ToolbarCtx`/`StripCtx`.
struct RecordButtonCtx<'a> {
    session: &'a mut SessionPanelState,
    recordings_dir: &'a Path,
    selected_watch: &'a Option<String>,
    selected_position: &'a str,
}

/// Record/Stop: a custom-painted button (not `egui::Button`, so the
/// leading dot can be positioned exactly) — accent fill + accent-ink text
/// idle, `palette.rec` fill + white text while recording, with a leading
/// dot (circle idle, pulsing rounded square while recording — mockup's
/// `recordDotStyle`). The pulse paints the dot's alpha from
/// `ui.input(|i| i.time)`'s sine (behavior contract) and keeps repainting
/// at 10 Hz only while recording, so the animation runs without spinning
/// the CPU the rest of the time.
fn record_stop_button(
    ui: &mut egui::Ui,
    palette: &Palette,
    engine: &Engine,
    recording: bool,
    ctx: RecordButtonCtx<'_>,
) {
    let label = if recording { "Stop" } else { "Record" };
    let font = egui::FontId::new(13.0, egui::FontFamily::Name(theme::FAMILY_SEMIBOLD.into()));
    let fg = if recording {
        palette.rec_ink
    } else {
        palette.accent_ink
    };
    let bg = if recording {
        palette.rec
    } else {
        palette.accent
    };

    let galley = ui.painter().layout_no_wrap(label.to_string(), font, fg);
    let dot_d = 8.0_f32;
    let gap = 8.0_f32;
    let pad_h = 14.0_f32;
    let pad_v = 7.0_f32;
    let content_h = galley.size().y.max(dot_d);
    let size = egui::vec2(
        pad_h * 2.0 + dot_d + gap + galley.size().x,
        pad_v * 2.0 + content_h,
    );

    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());

    if ui.is_rect_visible(rect) {
        let bg = if response.hovered() {
            bg.gamma_multiply(1.1)
        } else {
            bg
        };
        ui.painter()
            .rect_filled(rect, egui::CornerRadius::same(8), bg);

        let dot_center = egui::pos2(rect.left() + pad_h + dot_d / 2.0, rect.center().y);
        let alpha = if recording {
            let t = ui.input(|i| i.time);
            let s = (t * std::f64::consts::TAU / 1.4).sin();
            0.3 + 0.7 * (0.5 + 0.5 * s)
        } else {
            1.0
        };
        let dot_color = theme::with_alpha(fg, (alpha * 255.0) as u8);
        if recording {
            let dot_rect = egui::Rect::from_center_size(dot_center, egui::vec2(dot_d, dot_d));
            ui.painter()
                .rect_filled(dot_rect, egui::CornerRadius::same(2), dot_color);
        } else {
            ui.painter()
                .circle_filled(dot_center, dot_d / 2.0, dot_color);
        }

        let text_pos = egui::pos2(
            dot_center.x + dot_d / 2.0 + gap,
            rect.center().y - galley.size().y / 2.0,
        );
        ui.painter().galley(text_pos, galley, fg);
    }

    if recording {
        ui.ctx().request_repaint_after(Duration::from_millis(100));
    }

    if response.clicked() {
        if recording {
            engine.send(ControlMsg::StopRecording);
            ctx.session.recording_started_at = None;
        } else {
            // "created on first record" (M3 behavior contract) —
            // idempotent, best-effort: a failure here just means the
            // subsequent StartRecording fails too and surfaces its own
            // error banner.
            let _ = std::fs::create_dir_all(ctx.recordings_dir);
            engine.send(ControlMsg::StartRecording {
                dir: ctx.recordings_dir.to_path_buf(),
                meta_position: Some(ctx.selected_position.to_string()),
                watch: ctx.selected_watch.clone(),
            });
            ctx.session.recording_started_at = Some(Instant::now());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn averaging_options_table() {
        assert_eq!(averaging_options(), [10.0, 30.0, 60.0]);
    }

    #[test]
    fn bph_combo_items_order() {
        let items = bph_combo_items();
        assert_eq!(items[0], BphComboItem::Auto);
        assert_eq!(items[1], BphComboItem::Free);
        assert_eq!(items[2], BphComboItem::Value(18_000));
        assert_eq!(items[3], BphComboItem::Value(21_600));
        assert_eq!(items[4], BphComboItem::Value(25_200));
        assert_eq!(items[5], BphComboItem::Value(28_800));
        assert_eq!(items[6], BphComboItem::Value(36_000));
        assert_eq!(items[7], BphComboItem::Other);
    }

    #[test]
    fn export_enabled_truth_table() {
        assert!(export_enabled(Some("Seiko 5"), true));
        assert!(!export_enabled(Some("Seiko 5"), false));
        assert!(!export_enabled(None, true));
        assert!(!export_enabled(None, false));
    }
}
