//! The eframe application shell. M3 scaffold: owns the CLI flags and a live
//! `Engine`, seeded from them, and renders placeholder panels; T8-T10 add
//! the real instrument/control views and layout on top of the snapshots
//! already flowing here.

use crate::AppFlags;
use crate::engine::{Engine, EngineConfig, SourceSpec};
use crate::ui::TapeUiState;
use eframe::egui;

pub struct ChronaApp {
    flags: AppFlags,
    engine: Engine,
    tape_ui: TapeUiState,
}

impl ChronaApp {
    pub fn new(cc: &eframe::CreationContext<'_>, flags: AppFlags) -> Self {
        let initial = if flags.simulate {
            SourceSpec::Simulate {
                rate: flags.rate,
                beat_error: flags.beat_error,
                amplitude: flags.amplitude,
                snr: flags.snr,
            }
        } else {
            SourceSpec::Mic { device_id: None }
        };
        // Defaults matching `chrona_dsp::AnalyzerConfig::default()`; T9
        // wires these up to real controls.
        let config = EngineConfig {
            lift_angle_deg: 52.0,
            averaging_s: 30.0,
            bph_mode: chrona_dsp::BphMode::Auto,
            ppm_correction: 0.0,
        };
        let engine = Engine::start(initial, config, Some(cc.egui_ctx.clone()));
        ChronaApp {
            flags,
            engine,
            tape_ui: TapeUiState::default(),
        }
    }
}

impl eframe::App for ChronaApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // The only sanctioned way to read live metrics from the UI thread —
        // see `Engine::snapshot`'s doc comment for why `current_metrics`
        // itself must never be called here.
        let snap = self.engine.snapshot();
        egui::Panel::top("chrona_top").show(ui, |ui| {
            ui.heading("Chrona — M3 scaffold");
            let source = if self.flags.simulate {
                "simulate"
            } else {
                "microphone"
            };
            ui.label(format!("Source: {source}"));
            // T9 owns the real controls row and fault-banner rendering
            // here; a fault snapshot's `error_banner` is otherwise left
            // untouched by this scaffold.
        });
        egui::CentralPanel::default().show(ui, |ui| {
            crate::ui::instrument_view(ui, &snap, &mut self.tape_ui);
        });
    }
}
