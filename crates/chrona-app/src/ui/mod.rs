//! egui views: the metric cards (M4a Task 7, `cards`), the paper-tape
//! instrument view (`instrument`), the redesigned toolbar (M4a Task 6,
//! `toolbar`) plus its add-watch and metric-help modals (`modals`),
//! controls state shared with the toolbar (`controls`), and the position/
//! session strip (M4a Task 7, `strip`).

mod cards;
mod controls;
mod instrument;
mod modals;
mod strip;
mod toolbar;

pub use cards::metrics_cards;
pub use controls::{BphModeUi, ClipTracker, ControlsState, pick_banner, to_bph_mode};
pub use instrument::{TapeUiState, dot_to_screen, instrument_view};
pub use modals::{AddWatchModalState, HelpTopicId, render_help_modal};
pub use strip::{
    SessionPanelState, StripCtx, default_recordings_dir, position_strip, rec_elapsed_label,
};
pub use toolbar::{BphComboItem, ToolbarCtx, ToolbarState, toolbar_row};
