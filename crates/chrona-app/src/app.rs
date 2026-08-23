//! The eframe application shell. Owns a live `Engine`, seeded from CLI flags
//! and the persisted `ConfigStore`, and renders the instrument view (T8)
//! behind a top controls row + health banner strip (T9) and a record &
//! replay panel (T10).

use std::path::PathBuf;
use std::time::{Duration, Instant};

use chrona_session::ConfigStore;
use eframe::egui;

use crate::AppFlags;
use crate::engine::{
    Banner, BannerSeverity, ControlMsg, Engine, EngineConfig, HealthView, SourceSpec,
};
use crate::history::HistoryIndex;
use crate::presenter::{AmpAccum, BeatAccum, TraceView};
use crate::theme::{self, Theme};
use crate::ui::{
    ClipTracker, ControlsState, SessionPanelState, TapeUiState, controls_row,
    default_recordings_dir, pick_banner, session_panel,
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
    /// The active theme, applied to `cc.egui_ctx` at startup. Not yet read
    /// anywhere else — the toolbar toggle that flips it and re-applies via
    /// `theme::apply_style` lands in a later M4a task (spec §1).
    #[allow(dead_code)]
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
    /// a later M4a task (Task 7/8), same as `theme` above.
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
    /// `session_panel`'s upsert-on-finalize hook (which is the only place
    /// this is read so far — nothing yet renders its contents; the history
    /// table and position-comparison card land in a later M4a task,
    /// Task 9).
    history: HistoryIndex,
}

impl ChronaApp {
    pub fn new(cc: &eframe::CreationContext<'_>, flags: AppFlags) -> Self {
        let config = ConfigStore::load_default();
        let controls = ControlsState::from_config(&config);

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

        egui::Panel::top("chrona_top").show(ui, |ui| {
            ui.heading("Chrona");
            if controls_row(ui, &mut self.controls, &self.engine, &mut self.config) {
                self.dirty_since.get_or_insert_with(Instant::now);
            }
            ui.separator();
            session_panel(
                ui,
                &mut self.session,
                &self.engine,
                &snap,
                &self.recordings_dir,
                &self.controls.selected_device,
                &mut self.history,
            );
        });

        self.maybe_retry_mic(snap.health.last_error.is_some());
        self.maybe_save_config();

        // Clip windowing (controller ruling: "delta over last 5s",
        // tolerating the counter resetting on analyzer rebuild) happens
        // here, outside the pure `pick_banner`; the windowed count is
        // spliced into a copy of the snapshot's health before deciding the
        // banner.
        let windowed_clipped = self
            .clip_tracker
            .observe(snap.health.clipped, Instant::now());
        let health_for_banner = HealthView {
            clipped: windowed_clipped,
            ..snap.health.clone()
        };
        let banner = pick_banner(snap.banner.as_ref(), &health_for_banner, snap.source_kind);

        egui::CentralPanel::default().show(ui, |ui| {
            if let Some(banner) = &banner {
                render_banner(ui, banner);
            }
            crate::ui::instrument_view(ui, &snap, &mut self.tape_ui);
        });
    }
}

/// Paints one banner as a filled, full-width strip above the instrument.
fn render_banner(ui: &mut egui::Ui, banner: &Banner) {
    let color = match banner.severity {
        BannerSeverity::Error => egui::Color32::from_rgb(140, 30, 30),
        BannerSeverity::Warn => egui::Color32::from_rgb(140, 100, 20),
        BannerSeverity::Info => egui::Color32::from_gray(55),
    };
    egui::Frame::new()
        .fill(color)
        .inner_margin(6.0)
        .show(ui, |ui| {
            ui.label(egui::RichText::new(&banner.text).color(egui::Color32::WHITE));
        });
    ui.add_space(4.0);
}
