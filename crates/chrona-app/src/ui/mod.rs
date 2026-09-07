//! egui views: the metric cards (M4a Task 7, `cards`), the beat-trace/
//! amplitude charts (M4a Task 8, `charts`) that replaced M3's paper-tape
//! instrument view, the redesigned toolbar (M4a Task 6, `toolbar`) plus its
//! add-watch and metric-help modals (`modals`), controls state shared with
//! the toolbar (`controls`), the position/session strip (M4a Task 7,
//! `strip`), the Mic Doctor panel (M4 Task 10, `doctor`), the collapsible
//! per-beat Scope view (M4 Task 12, `scope`), and the classic tape-pattern
//! legend modal (M4 Task 13, `patterns`).

mod cal_wizard;
mod cards;
mod charts;
mod controls;
mod doctor;
mod history_ui;
mod modals;
mod patterns;
mod scope;
mod strip;
mod toolbar;

pub use cal_wizard::{CalWizard, CalWizardCtx, TempCaptureGuard, render_cal_wizard};
pub use cards::metrics_cards;
pub use charts::{ChartsCtx, ChartsUiState, charts_section};
pub use controls::{
    BphModeUi, ControlsState, CountWatermark, WATERMARK_WINDOW, pick_banner, resolve_app_banner,
    to_bph_mode,
};
pub use doctor::{DoctorCtx, DoctorPanel, render_doctor_panel};
pub use history_ui::{
    BottomGridCtx, ComparisonRow, bottom_grid, comparison_rows, session_time_label,
};
pub use modals::{AddWatchModalState, HelpTopicId, render_help_modal};
pub use patterns::render_pattern_legend_modal;
pub use scope::{ScopeCtx, scope_section};
pub use strip::{
    SessionPanelState, StripCtx, default_recordings_dir, position_strip, rec_elapsed_label,
};
pub use toolbar::{BphComboItem, ToolbarCtx, ToolbarState, toolbar_row};
