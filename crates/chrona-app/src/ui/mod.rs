//! egui views: instrument numerals/tape (T8), the redesigned toolbar (M4a
//! Task 6, `toolbar`) plus its add-watch modal (`modals`), controls state
//! shared with the toolbar (`controls`), and the record & replay panel
//! (T10, `session_panel`).

mod controls;
mod instrument;
mod modals;
mod session_panel;
mod toolbar;

pub use controls::{BphModeUi, ClipTracker, ControlsState, pick_banner, to_bph_mode};
pub use instrument::{TapeUiState, dot_to_screen, instrument_view};
pub use modals::AddWatchModalState;
pub use session_panel::{
    SessionPanelState, default_recordings_dir, rec_elapsed_label, session_panel,
};
pub use toolbar::{BphComboItem, ToolbarCtx, ToolbarState, toolbar_row};
