//! Presenters: pure functions turning DSP/engine data into display-ready
//! tape geometry and formatted strings (no `egui` dependency, TDD'd on their
//! own).

mod format;
mod tape;

pub use format::{format_amplitude, format_beat_error, format_rate, source_label, tier_label};
pub use tape::{TapeDot, TapeParams, tape_dots};
