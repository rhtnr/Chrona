//! egui views: instrument numerals/tape (T8), controls (T9), and app
//! layout/menu chrome (T10).

mod controls;
mod instrument;

pub use controls::{
    Banner, BphModeUi, ClipTracker, ControlsState, controls_row, pick_banner, to_bph_mode,
};
pub use instrument::{TapeUiState, dot_to_screen, instrument_view};
