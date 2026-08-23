use chrona_app::{AppFlags, app, engine};
use clap::Parser;
use eframe::egui;

fn main() -> eframe::Result {
    let flags = AppFlags::parse();

    // Validated before anything else touches the engine or the window: M3
    // only supports headless runs against the simulated source.
    if flags.headless_seconds.is_some() && !flags.simulate {
        eprintln!("error: --headless-seconds requires --simulate");
        std::process::exit(1);
    }

    if let Some(secs) = flags.headless_seconds {
        std::process::exit(engine::run_headless(&flags, secs));
    }

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1_100.0, 700.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Chrona",
        options,
        Box::new(|cc| Ok(Box::new(app::ChronaApp::new(cc, flags)))),
    )
}
