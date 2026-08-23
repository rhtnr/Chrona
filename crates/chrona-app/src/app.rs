//! The eframe application shell. M3 scaffold: owns the CLI flags (T7 seeds
//! the engine's initial source/config from them) and renders placeholder
//! panels; T7 adds a live `Engine`, T8-T10 add the real instrument/control
//! views and layout.

use crate::AppFlags;
use eframe::egui;

pub struct ChronaApp {
    flags: AppFlags,
}

impl ChronaApp {
    pub fn new(_cc: &eframe::CreationContext<'_>, flags: AppFlags) -> Self {
        ChronaApp { flags }
    }
}

impl eframe::App for ChronaApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::Panel::top("chrona_top").show(ui, |ui| {
            ui.heading("Chrona — M3 scaffold");
        });
        egui::CentralPanel::default().show(ui, |ui| {
            ui.label("Instrument view placeholder (wired in T8)");
            ui.separator();
            ui.label("Controls placeholder (wired in T9)");
            ui.separator();
            let source = if self.flags.simulate {
                "simulate"
            } else {
                "microphone"
            };
            ui.label(format!("Source: {source} (engine wired in T7)"));
        });
    }
}
