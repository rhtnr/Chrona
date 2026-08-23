//! Chrona live-app scaffold (M3): eframe window bootstrap, CLI flags, and the
//! module skeleton for the presenter/engine/ui layers built out in later tasks.
//! `src/main.rs` is a thin binary crate root that uses this library.

pub mod app;
pub mod engine;
pub mod presenter;
pub mod ui;

use clap::Parser;

/// CLI surface for the `chrona-app` binary. Window sizing/placement is
/// implicit (eframe defaults); these flags select and configure the input
/// source and, with `--headless-seconds`, drive the engine without opening a
/// window — the scripted/E2E hook. `--headless-seconds` requires
/// `--simulate` in M3 (mic/replay headless runs are out of scope).
#[derive(Parser)]
#[command(name = "chrona-app", version, about = "Chrona live timegrapher")]
pub struct AppFlags {
    /// Drive the engine from a synthetic watch instead of microphone input.
    #[arg(long)]
    pub simulate: bool,
    /// Simulated rate error, s/day (positive = fast). Requires --simulate.
    #[arg(long, default_value_t = 0.0, allow_negative_numbers = true)]
    pub rate: f64,
    /// Simulated beat error, ms. Requires --simulate.
    #[arg(long, default_value_t = 0.0, allow_negative_numbers = true)]
    pub beat_error: f64,
    /// Simulated lift amplitude, degrees. Requires --simulate.
    #[arg(long, default_value_t = 270.0)]
    pub amplitude: f64,
    /// Simulated signal-to-noise ratio, dB. Requires --simulate.
    #[arg(long, default_value_t = 30.0, allow_negative_numbers = true)]
    pub snr: f64,
    /// Run headless for N simulated seconds, print the final metrics, and
    /// exit. Requires --simulate; validated in `main` before anything else.
    #[arg(long)]
    pub headless_seconds: Option<f64>,
}
