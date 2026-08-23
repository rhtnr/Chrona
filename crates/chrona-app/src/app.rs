//! The eframe application shell. Owns a live `Engine`, seeded from CLI flags
//! and the persisted `ConfigStore`, and renders the redesigned toolbar
//! (`ui::toolbar::toolbar_row`, M4a Task 6) + health banner, the position/
//! session strip and metric cards (`ui::strip`/`ui::cards`, M4a Task 7),
//! above the M3 paper tape (`ui::instrument_view`) — the tape still carries
//! its own M4a redesign in a later task.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use chrona_session::ConfigStore;
use eframe::egui;

use crate::AppFlags;
use crate::engine::{
    Banner, BannerSeverity, ControlMsg, Engine, EngineConfig, HealthView, SourceSpec,
};
use crate::history::{HistoryIndex, POSITIONS};
use crate::presenter::{AmpAccum, BeatAccum, TraceView};
use crate::theme::{self, Theme};
use crate::ui::{
    AddWatchModalState, BphComboItem, ClipTracker, ControlsState, HelpTopicId, SessionPanelState,
    StripCtx, TapeUiState, ToolbarCtx, ToolbarState, default_recordings_dir, metrics_cards,
    pick_banner, position_strip, render_help_modal, toolbar_row,
};

/// `ConfigStore::save` debounce (behavior contract: save at most once per
/// second).
const CONFIG_SAVE_DEBOUNCE: Duration = Duration::from_secs(1);
/// How often a persistent mic capture error retries `SwitchSource`
/// (behavior contract).
const MIC_RETRY_INTERVAL: Duration = Duration::from_secs(3);

pub struct ChronaApp {
    engine: Engine,
    tape_ui: TapeUiState,
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
    /// snapshot alongside the M3 tape below, spanning up to
    /// `presenter::HORIZON_S` rather than the analyzer's own ~32 s ring.
    /// Rendering still reads the old `tape_ui`/M3 tape until Task 8's
    /// chart painters land.
    beat_accum: BeatAccum,
    amp_accum: AmpAccum,
    /// The beat-trace chart's pan/zoom/follow state (design spec §10).
    /// Not yet read anywhere else — the chart's pan/zoom controls land in
    /// a later M4a task (Task 8).
    #[allow(dead_code)]
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
    /// whether the selected watch has any recorded sessions; the history
    /// table and position-comparison card land in a later M4a task.
    history: HistoryIndex,
    /// Toolbar-owned state (M4a Task 6) that doesn't belong on
    /// `ControlsState`/`ConfigStore`: the selected watch (seeded from, and
    /// kept mirrored into, `config.last_watch`), the active BPH combo row,
    /// the add-watch modal, and the export-report request flag the
    /// toolbar's Export button sets — nothing consumes it yet (see the
    /// `export_requested` stub in `ChronaApp::ui`; Task 9 wires the real
    /// export).
    toolbar: ToolbarState,
    /// Index into `history::POSITIONS` — the position selected in the
    /// redesigned segmented control (M4a Task 7; replaces the old M3
    /// session-panel free-text position field). Threaded into the
    /// toolbar's Record button as `meta_position` (`ui::toolbar::
    /// record_stop_button`) and, later, into Task 9's position-comparison
    /// highlight. Chosen as a `usize` index (rather than storing the code
    /// `&'static str` itself) so it stays trivially `Copy`/`Default`-able
    /// and index-aligned with `history::PositionRates::by_code`, the same
    /// convention that struct already uses. Defaults to `0` (`"DU"`).
    selected_position: usize,
    /// Which metric's help modal is open, if any (M4a Task 7); `None` most
    /// of the time. Set from a metrics card's "?" button
    /// (`ui::cards::metrics_cards`'s return value) and cleared by
    /// `ui::modals::render_help_modal` (✕, click-outside, or Esc).
    open_help: Option<HelpTopicId>,
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
            tape_ui: TapeUiState::default(),
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
}

impl eframe::App for ChronaApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // The only sanctioned way to read live metrics from the UI thread —
        // see `Engine::snapshot`'s doc comment for why `current_metrics`
        // itself must never be called here.
        let snap = self.engine.snapshot();

        // M4a beat-trace feed (design spec §10/§11): runs alongside the M3
        // tape below every frame, independent of whether anything paints
        // from it yet (Task 8). `bph_nominal`/`amplitude_deg` come straight
        // from the snapshot's metrics; a metrics-less snapshot feeds `None`
        // into both, which `BeatAccum`/`AmpAccum` already treat as "append
        // nothing" rather than fabricating a beat or amplitude point.
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
            if let Some(banner) = &banner {
                render_banner(ui, palette, banner);
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
            // Task 9 wires the real export flow; for now this just clears
            // the flag `ui::toolbar::toolbar_row`'s Export button sets so
            // a stale request isn't reprocessed on a later frame.
            self.toolbar.export_requested = false;
        }

        self.maybe_retry_mic(snap.health.last_error.is_some());
        self.maybe_save_config();

        egui::CentralPanel::default().show(ui, |ui| {
            crate::ui::instrument_view(ui, &snap, &mut self.tape_ui);
        });
    }
}

/// Paints one banner (M4a Task 6 severity → color mapping, spec §4): `Info`
/// is `palette.accent` text, `Warn` is `palette.warnfg` text on a
/// `palette.warnbg`-filled strip, `Error` is `palette.rec` text — replacing
/// M3's flat red/amber/gray fills for every severity.
fn render_banner(ui: &mut egui::Ui, palette: &theme::Palette, banner: &Banner) {
    match banner.severity {
        BannerSeverity::Warn => {
            egui::Frame::new()
                .fill(palette.warnbg)
                .corner_radius(6)
                .inner_margin(6.0)
                .show(ui, |ui| {
                    ui.label(egui::RichText::new(&banner.text).color(palette.warnfg));
                });
        }
        BannerSeverity::Info => {
            ui.label(egui::RichText::new(&banner.text).color(palette.accent));
        }
        BannerSeverity::Error => {
            ui.label(egui::RichText::new(&banner.text).color(palette.rec));
        }
    }
    ui.add_space(4.0);
}
