//! The eframe application shell. Owns a live `Engine`, seeded from CLI flags
//! and the persisted `ConfigStore`, and renders the instrument view (T8)
//! behind a top controls row + health banner strip (T9).

use std::time::{Duration, Instant};

use chrona_session::ConfigStore;
use eframe::egui;

use crate::AppFlags;
use crate::engine::{ControlMsg, Engine, EngineConfig, HealthView, SourceSpec};
use crate::ui::{Banner, ClipTracker, ControlsState, TapeUiState, controls_row, pick_banner};

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
    /// Set the moment the config first goes dirty (not refreshed on every
    /// subsequent edit), so a continuous drag still saves within a bounded
    /// ~1s window instead of never catching up — cleared once saved.
    dirty_since: Option<Instant>,
    clip_tracker: ClipTracker,
    next_mic_retry: Option<Instant>,
}

impl ChronaApp {
    pub fn new(cc: &eframe::CreationContext<'_>, flags: AppFlags) -> Self {
        let config = ConfigStore::load_default();
        let controls = ControlsState::from_config(&config);

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
        ChronaApp {
            engine,
            tape_ui: TapeUiState::default(),
            controls,
            config,
            dirty_since: None,
            clip_tracker: ClipTracker::default(),
            next_mic_retry: None,
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

        egui::Panel::top("chrona_top").show(ui, |ui| {
            ui.heading("Chrona");
            if controls_row(ui, &mut self.controls, &self.engine, &mut self.config) {
                self.dirty_since.get_or_insert_with(Instant::now);
            }
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
        let banner = pick_banner(
            snap.error_banner.as_deref(),
            &health_for_banner,
            snap.source_kind,
        );

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
    let (color, text) = match banner {
        Banner::Error(s) => (egui::Color32::from_rgb(140, 30, 30), s.as_str()),
        Banner::Warning(s) => (egui::Color32::from_rgb(140, 100, 20), s.as_str()),
        Banner::Info(s) => (egui::Color32::from_gray(55), s.as_str()),
    };
    egui::Frame::new()
        .fill(color)
        .inner_margin(6.0)
        .show(ui, |ui| {
            ui.label(egui::RichText::new(text).color(egui::Color32::WHITE));
        });
    ui.add_space(4.0);
}
