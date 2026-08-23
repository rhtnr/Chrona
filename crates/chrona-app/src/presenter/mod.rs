//! Presenters: pure functions turning DSP/engine data into display-ready
//! tape/trace geometry and formatted strings (no `egui` dependency, TDD'd
//! on their own).

mod format;
mod tape;
mod trace;

pub use format::{
    day_label, format_amplitude, format_beat_error, format_bph_grouped, format_rate, source_label,
    tier_label,
};
pub use tape::{TapeDot, TapeParams, tape_dots};
pub use trace::{
    AmpAccum, BeatAccum, BeatPoint, GAP_BREAK_S, HORIZON_S, SPAN_DEFAULT_S, SPAN_MAX_S, SPAN_MIN_S,
    SignalClass, TraceView, Trend, WRAP_PRESETS_MS, amp_range, signal_meter, trend, wrap_signed,
    x_step_s, y_frac, y_step_ms,
};
