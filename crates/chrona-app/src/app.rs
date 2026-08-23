//! The eframe application shell. Owns a live `Engine`, seeded from CLI flags
//! and the persisted `ConfigStore`, and renders the redesigned toolbar
//! (`ui::toolbar::toolbar_row`, M4a Task 6) + health banner, the position/
//! session strip and metric cards (`ui::strip`/`ui::cards`, M4a Task 7),
//! above the beat-trace/amplitude charts (`ui::charts::charts_section`,
//! M4a Task 8) that replaced M3's paper tape, and — completing the
//! mockup's layout — the session-history/position-comparison cards
//! (`ui::history_ui::bottom_grid`, M4a Task 9) below them, plus the
//! Export-report flow (`run_export`) and the app-side notice banner that
//! reports its outcome.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use chrona_session::ConfigStore;
use eframe::egui;

use crate::AppFlags;
use crate::engine::{BannerSeverity, ControlMsg, Engine, EngineConfig, HealthView, SourceSpec};
use crate::export::{ReportInput, default_report_name, render_report};
use crate::history::{HistoryIndex, POSITIONS, PositionRates, SessionEntry, position_rates};
use crate::presenter::{AmpAccum, BeatAccum, TraceView};
use crate::theme::{self, Theme};
use crate::ui::{
    AddWatchModalState, BottomGridCtx, BphComboItem, BphModeUi, ChartsCtx, ChartsUiState,
    ClipTracker, ControlsState, HelpTopicId, SessionPanelState, StripCtx, ToolbarCtx, ToolbarState,
    bottom_grid, charts_section, default_recordings_dir, metrics_cards, pick_banner,
    position_strip, render_help_modal, resolve_app_banner, toolbar_row,
};

/// `ConfigStore::save` debounce (behavior contract: save at most once per
/// second).
const CONFIG_SAVE_DEBOUNCE: Duration = Duration::from_secs(1);
/// How often a persistent mic capture error retries `SwitchSource`
/// (behavior contract).
const MIC_RETRY_INTERVAL: Duration = Duration::from_secs(3);

pub struct ChronaApp {
    engine: Engine,
    /// The charts section's own UI state (M4a Task 8): currently just the
    /// wrap-band selection — see `ui::charts::ChartsUiState`. Replaces M3's
    /// `TapeUiState`.
    charts_ui: ChartsUiState,
    controls: ControlsState,
    config: ConfigStore,
    /// The active theme, applied to `cc.egui_ctx` at startup and flipped by
    /// the toolbar's theme-toggle button (M4a Task 6:
    /// `ui::toolbar::toolbar_row`), which re-applies it via
    /// `theme::apply_style` and persists the choice into `config.theme`
    /// immediately (not the debounced save the continuous-drag controls
    /// use).
    theme: Theme,
    /// M4a beat-trace accumulators (design spec §10/§11): fed from every
    /// snapshot, spanning up to `presenter::HORIZON_S` rather than the
    /// analyzer's own ~32 s ring. Read each frame by
    /// `ui::charts::charts_section`.
    beat_accum: BeatAccum,
    amp_accum: AmpAccum,
    /// The beat-trace chart's pan/zoom/follow state (design spec §10),
    /// mutated by `ui::charts::charts_section`'s wheel/drag interactions
    /// and its `+`/`−`/`Fit`/`Live` controls.
    trace_view: TraceView,
    /// Set the moment the config first goes dirty (not refreshed on every
    /// subsequent edit), so a continuous drag still saves within a bounded
    /// ~1s window instead of never catching up — cleared once saved.
    dirty_since: Option<Instant>,
    clip_tracker: ClipTracker,
    next_mic_retry: Option<Instant>,
    session: SessionPanelState,
    /// `<platform data dir>/chrona/recordings` (T10); falls back to a
    /// relative `recordings/` dir on the rare platform `directories` can't
    /// resolve (same graceful-fallback shape as `ConfigStore::load_default`
    /// uses for the config dir).
    recordings_dir: PathBuf,
    /// The session-history index (spec §7), scanned once from
    /// `recordings_dir` at startup and kept current afterwards by
    /// `ui::strip::position_strip`'s upsert-on-finalize hook. Read by the
    /// toolbar's Export button (M4a Task 6: `export_enabled`) to gate on
    /// whether the selected watch has any recorded sessions, by
    /// `run_export` to build the report, and by `ui::history_ui::
    /// bottom_grid` (M4a Task 9) for the session-history table and
    /// position-comparison card.
    history: HistoryIndex,
    /// Toolbar-owned state (M4a Task 6) that doesn't belong on
    /// `ControlsState`/`ConfigStore`: the selected watch (seeded from, and
    /// kept mirrored into, `config.last_watch`), the active BPH combo row,
    /// the add-watch modal, and the export-report request flag the
    /// toolbar's Export button sets — consumed each frame by `run_export`
    /// (M4a Task 9).
    toolbar: ToolbarState,
    /// Index into `history::POSITIONS` — the position selected in the
    /// redesigned segmented control (M4a Task 7; replaces the old M3
    /// session-panel free-text position field). Threaded into the
    /// toolbar's Record button as `meta_position` (`ui::toolbar::
    /// record_stop_button`) and into the position-comparison card's accent
    /// highlight (M4a Task 9: `ui::history_ui::comparison_rows`). Chosen
    /// as a `usize` index (rather than storing the code `&'static str`
    /// itself) so it stays trivially `Copy`/`Default`-able and
    /// index-aligned with `history::PositionRates::by_code`, the same
    /// convention that struct already uses. Defaults to `0` (`"DU"`).
    selected_position: usize,
    /// Which metric's help modal is open, if any (M4a Task 7); `None` most
    /// of the time. Set from a metrics card's "?" button
    /// (`ui::cards::metrics_cards`'s return value) and cleared by
    /// `ui::modals::render_help_modal` (✕, click-outside, or Esc).
    open_help: Option<HelpTopicId>,
    /// A transient app-side notice (M4a Task 9, spec §9) — currently only
    /// the Export-report success/failure outcome (`run_export`) — shown in
    /// the same banner strip as the engine's own banner, but ONLY when the
    /// engine has none to show this frame
    /// (`ui::controls::resolve_app_banner`: engine truth always outranks a
    /// UI notice, since a live fault must never be silently covered by
    /// e.g. "report saved").
    ///
    /// Cleared on the next user action: `ChronaApp::ui` reads `ui.input(|i|
    /// i.pointer.any_click())` once per frame and clears this field before
    /// any button handling runs that frame, so a notice set LATER in that
    /// SAME frame (e.g. by the very click that triggered the export)
    /// survives — only a DIFFERENT, later frame's click wipes a stale one.
    /// This is intentionally coarser than "cleared only by a click that
    /// sends a `ControlMsg` or opens a dialog" (it also clears on, say, a
    /// metrics-card "?" click): auditing every individual button handler
    /// for this one slot isn't worth the risk of missing one and leaving a
    /// stale banner stuck forever.
    app_banner: Option<(BannerSeverity, String)>,
}

impl ChronaApp {
    pub fn new(cc: &eframe::CreationContext<'_>, flags: AppFlags) -> Self {
        let config = ConfigStore::load_default();
        let controls = ControlsState::from_config(&config);
        let toolbar = ToolbarState {
            selected_watch: config.last_watch.clone(),
            bph_selection: BphComboItem::Auto,
            add_watch_modal: AddWatchModalState::default(),
            export_requested: false,
        };

        let theme = theme::theme_from_config(config.theme.as_deref());
        theme::install_fonts(&cc.egui_ctx);
        theme::apply_style(&cc.egui_ctx, theme);

        let initial = if flags.simulate {
            SourceSpec::Simulate {
                rate: flags.rate,
                beat_error: flags.beat_error,
                amplitude: flags.amplitude,
                snr: flags.snr,
            }
        } else {
            SourceSpec::Mic {
                device_id: controls.selected_device.clone(),
            }
        };
        let engine_config = EngineConfig {
            lift_angle_deg: controls.lift,
            averaging_s: controls.averaging,
            bph_mode: chrona_dsp::BphMode::Auto,
            ppm_correction: controls.ppm,
        };
        let engine = Engine::start(initial, engine_config, Some(cc.egui_ctx.clone()));
        let recordings_dir =
            default_recordings_dir().unwrap_or_else(|| PathBuf::from("recordings"));
        // Best-effort at startup (spec §7: "scan `*.json` sidecars at
        // startup"): a directory that doesn't exist yet (no recording has
        // ever been made) scans to an empty index, not an error.
        let history = HistoryIndex::scan(&recordings_dir);
        ChronaApp {
            engine,
            charts_ui: ChartsUiState::default(),
            controls,
            config,
            theme,
            beat_accum: BeatAccum::default(),
            amp_accum: AmpAccum::default(),
            trace_view: TraceView::default(),
            dirty_since: None,
            clip_tracker: ClipTracker::default(),
            next_mic_retry: None,
            session: SessionPanelState::default(),
            recordings_dir,
            history,
            toolbar,
            selected_position: 0,
            open_help: None,
            app_banner: None,
        }
    }

    /// Debounced `ConfigStore::save` — see `dirty_since`'s doc comment.
    fn maybe_save_config(&mut self) {
        let Some(since) = self.dirty_since else {
            return;
        };
        if since.elapsed() < CONFIG_SAVE_DEBOUNCE {
            return;
        }
        if let Err(e) = self.config.save() {
            eprintln!("chrona: failed to save config: {e}");
        }
        self.dirty_since = None;
    }

    /// Every `MIC_RETRY_INTERVAL` while the mic stream has a live error,
    /// re-attempts the same device (behavior contract: "the app sends
    /// SwitchSource retry every 3 s"). Self-terminating: a successful
    /// reconnect gets a fresh `CaptureHealth` with `last_error = None`, so
    /// `has_error` goes false on the next snapshot and the timer resets.
    fn maybe_retry_mic(&mut self, has_error: bool) {
        if !has_error {
            self.next_mic_retry = None;
            return;
        }
        let now = Instant::now();
        if self.next_mic_retry.is_some_and(|t| now < t) {
            return;
        }
        self.engine.send(ControlMsg::SwitchSource(SourceSpec::Mic {
            device_id: self.controls.selected_device.clone(),
        }));
        self.next_mic_retry = Some(now + MIC_RETRY_INTERVAL);
    }

    /// Runs the Export-report flow (M4a Task 9, spec §9), once
    /// `toolbar.export_requested` was set this frame
    /// (`ui::toolbar::export_button`, gated by `ui::toolbar::
    /// export_enabled` — a watch must already be selected with at least
    /// one recorded session, so `selected_watch` below is always `Some`
    /// in practice; the early return is defensive, not expected): a
    /// native save dialog seeded with `export::default_report_name`, then
    /// `export::render_report` + `std::fs::write` for the selected
    /// watch's report. A cancelled dialog (`save_file()` returns `None`)
    /// is silent — same as `ui::toolbar::open_recording_button`'s
    /// cancelled pick — never an error banner. Sets `self.app_banner` on
    /// both real outcomes: `Info` with "report saved to <file name>", or
    /// `Error` with the write failure's message — never the engine's own
    /// banner slot (see `app_banner`'s doc comment for why the two stay
    /// separate).
    fn run_export(&mut self) {
        let Some(watch) = self.toolbar.selected_watch.clone() else {
            return;
        };
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let Some(path) = rfd::FileDialog::new()
            .set_file_name(default_report_name(&watch, now))
            .save_file()
        else {
            return;
        };

        let rates = position_rates(&self.history, &watch);
        let sessions: Vec<&SessionEntry> = self.history.for_watch(&watch).collect();
        let bph_mode = bph_mode_report_label(&self.controls.bph_mode_ui);
        let input = ReportInput {
            watch: &watch,
            app_version: env!("CARGO_PKG_VERSION"),
            lift_deg: self.controls.lift,
            bph_mode: &bph_mode,
            averaging_s: self.controls.averaging,
            ppm: self.controls.ppm,
            rates: &rates,
            sessions: &sessions,
            exported_unix_s: now,
        };
        let html = render_report(&input);
        self.app_banner = Some(match std::fs::write(&path, html) {
            Ok(()) => {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.display().to_string());
                (BannerSeverity::Info, format!("report saved to {name}"))
            }
            Err(e) => (BannerSeverity::Error, format!("failed to save report: {e}")),
        });
    }
}

impl eframe::App for ChronaApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // The only sanctioned way to read live metrics from the UI thread —
        // see `Engine::snapshot`'s doc comment for why `current_metrics`
        // itself must never be called here.
        let snap = self.engine.snapshot();

        // M4a beat-trace feed (design spec §10/§11): runs every frame,
        // feeding the charts section painted below.
        // `bph_nominal`/`amplitude_deg` come straight from the snapshot's
        // metrics; a metrics-less snapshot feeds `None` into both, which
        // `BeatAccum`/`AmpAccum` already treat as "append nothing" rather
        // than fabricating a beat or amplitude point.
        self.beat_accum.extend(
            &snap.tape,
            snap.metrics.as_ref().and_then(|m| m.bph_nominal),
        );
        self.amp_accum.extend(
            self.beat_accum.newest_t(),
            snap.metrics.as_ref().and_then(|m| m.amplitude_deg),
        );

        // Clip windowing (controller ruling: "delta over last 5s",
        // tolerating the counter resetting on analyzer rebuild) happens
        // here, outside the pure `pick_banner`; the windowed count is
        // spliced into a copy of the snapshot's health before deciding the
        // banner. Computed before the top panel (not after, as M3 did) so
        // the banner can render inside it, directly below the toolbar (M4a
        // Task 6 behavior contract — a temporary placement; a later task
        // gives it a permanent home in the redesigned layout).
        let windowed_clipped = self
            .clip_tracker
            .observe(snap.health.clipped, Instant::now());
        let health_for_banner = HealthView {
            clipped: windowed_clipped,
            ..snap.health.clone()
        };
        let banner = pick_banner(snap.banner.as_ref(), &health_for_banner, snap.source_kind);

        // M4a Task 9: clear the transient app-side notice on the next user
        // action, before any of this frame's button handling runs — see
        // `app_banner`'s doc comment for the exact rule and why a click
        // anywhere (not just a narrower "sends a ControlMsg" set) is used.
        if ui.input(|i| i.pointer.any_click()) {
            self.app_banner = None;
        }

        egui::Panel::top("chrona_top").show(ui, |ui| {
            let selected_position_code = POSITIONS[self.selected_position].0;
            let dirty = toolbar_row(
                ui,
                &mut self.controls,
                &self.engine,
                &mut self.config,
                &mut self.theme,
                &mut self.toolbar,
                ToolbarCtx {
                    session: &mut self.session,
                    snap: &snap,
                    recordings_dir: &self.recordings_dir,
                    history: &self.history,
                    selected_position: selected_position_code,
                },
            );
            if dirty {
                self.dirty_since.get_or_insert_with(Instant::now);
            }

            let palette = theme::Palette::of(self.theme);
            let shown_app_banner = resolve_app_banner(banner.is_some(), self.app_banner.as_ref());
            match (&banner, shown_app_banner) {
                (Some(b), _) => render_banner(ui, palette, b.severity, &b.text),
                (None, Some((severity, text))) => render_banner(ui, palette, *severity, text),
                (None, None) => {}
            }

            ui.separator();
            position_strip(
                ui,
                palette,
                &mut self.selected_position,
                &mut self.session,
                StripCtx {
                    engine: &self.engine,
                    snap: &snap,
                    last_device: &self.controls.selected_device,
                    history: &mut self.history,
                },
            );

            ui.separator();
            let mut help_just_opened = false;
            if let Some(id) = metrics_cards(
                ui,
                palette,
                snap.metrics.as_ref(),
                self.toolbar.bph_selection,
            ) {
                self.open_help = Some(id);
                help_just_opened = true;
            }
            render_help_modal(ui.ctx(), palette, &mut self.open_help, help_just_opened);
        });

        if self.toolbar.export_requested {
            // Cleared up front so a stale request is never reprocessed on
            // a later frame, regardless of `run_export`'s outcome.
            self.toolbar.export_requested = false;
            self.run_export();
        }

        self.maybe_retry_mic(snap.health.last_error.is_some());
        self.maybe_save_config();

        egui::CentralPanel::default().show(ui, |ui| {
            let palette = theme::Palette::of(self.theme);
            charts_section(
                ui,
                palette,
                &mut self.trace_view,
                &mut self.charts_ui,
                ChartsCtx {
                    beat_accum: &self.beat_accum,
                    amp_accum: &self.amp_accum,
                    metrics: snap.metrics.as_ref(),
                },
            );

            // M4a Task 9: the session-history + position-comparison cards
            // that complete the mockup's layout below the amplitude strip.
            // `rates` is per the SELECTED watch (spec §8) — distinct from
            // the history card's own listing, which spans every watch (see
            // `ui::history_ui`'s module doc comment) — computed fresh each
            // frame from `self.history`, same "no caching" precedent as
            // `ui::toolbar::export_button`'s own `has_sessions` check. A
            // watch with no rated sessions (or no watch selected at all)
            // naturally yields the same all-`None`/no-spread result the
            // comparison card already renders as its own empty state.
            ui.add_space(8.0);
            let rates = match self.toolbar.selected_watch.as_deref() {
                Some(watch) => position_rates(&self.history, watch),
                None => PositionRates {
                    by_code: [None; 6],
                    spread: None,
                },
            };
            bottom_grid(
                ui,
                palette,
                BottomGridCtx {
                    history: &self.history,
                    session: &mut self.session,
                    engine: &self.engine,
                    rates: &rates,
                    selected_position: self.selected_position,
                },
            );
        });
    }
}

/// Paints one banner (M4a Task 6 severity → color mapping, spec §4): `Info`
/// is `palette.accent` text, `Warn` is `palette.warnfg` text on a
/// `palette.warnbg`-filled strip, `Error` is `palette.rec` text — replacing
/// M3's flat red/amber/gray fills for every severity. Takes severity/text
/// directly (M4a Task 9) rather than a `&Banner`, so the same paint code
/// serves both the engine's own banner AND the app-side notice — see
/// `app_banner`'s doc comment for how the two share this one strip without
/// ever showing simultaneously.
fn render_banner(
    ui: &mut egui::Ui,
    palette: &theme::Palette,
    severity: BannerSeverity,
    text: &str,
) {
    match severity {
        BannerSeverity::Warn => {
            egui::Frame::new()
                .fill(palette.warnbg)
                .corner_radius(6)
                .inner_margin(6.0)
                .show(ui, |ui| {
                    ui.label(egui::RichText::new(text).color(palette.warnfg));
                });
        }
        BannerSeverity::Info => {
            ui.label(egui::RichText::new(text).color(palette.accent));
        }
        BannerSeverity::Error => {
            ui.label(egui::RichText::new(text).color(palette.rec));
        }
    }
    ui.add_space(4.0);
}

/// "BPH mode" report field text (`export::ReportInput::bph_mode`): `Auto`/
/// `Free` verbatim, or the free-text `Fixed` buffer's trimmed content.
/// Reads `controls.bph_mode_ui` (the actual DSP-facing mode state) rather
/// than `toolbar.bph_selection` (just which combo row is highlighted) —
/// the more direct source, and it avoids reaching into `ui::toolbar`'s
/// private combo-label formatter for this one call site.
fn bph_mode_report_label(mode: &BphModeUi) -> String {
    match mode {
        BphModeUi::Auto => "Auto".to_string(),
        BphModeUi::Free => "Free".to_string(),
        BphModeUi::Fixed(text) => text.trim().to_string(),
    }
}
