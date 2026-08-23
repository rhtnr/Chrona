//! egui views: instrument numerals/tape (T8), controls (T9), and the
//! record & replay panel (T10).

mod controls;
mod instrument;
mod session_panel;

pub use controls::{BphModeUi, ClipTracker, ControlsState, controls_row, pick_banner, to_bph_mode};
pub use instrument::{TapeUiState, dot_to_screen, instrument_view};
pub use session_panel::{
    SessionPanelState, default_recordings_dir, rec_elapsed_label, session_panel,
};
