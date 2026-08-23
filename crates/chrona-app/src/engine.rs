//! Engine thread: DSP pipeline, capture/simulate/replay sources, and
//! snapshot publishing (spec §7/§9). Stubbed in the M3 scaffold — T7 lands
//! the real thread body (`Engine`, `EngineSnapshot`, `SourceSpec`, etc.).

use crate::AppFlags;

/// Headless entry point (the scripted/E2E hook). Once T7 wires the engine
/// this runs the simulated source for `seconds` of audio with no window and
/// prints the final metrics, exiting 0. This task's stub proves the flag
/// surface without a real engine: it reports that and exits 2.
pub fn run_headless(_flags: &AppFlags, _seconds: f64) -> i32 {
    eprintln!("engine not wired");
    2
}
