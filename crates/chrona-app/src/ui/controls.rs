//! Controls state + shared helpers, consumed by `ui::toolbar::toolbar_row`
//! (M4a Task 6, which replaced the M3 controls row this module used to
//! render): the BPH mode decision function, health-banner precedence,
//! `ConfigStore`-seeded per-control state, and the device picker (still
//! rendered here verbatim — "existing device list/refresh logic" per the
//! Task 6 brief — and called from the toolbar). The pure decision functions
//! (`to_bph_mode`, `pick_banner`) are TDD'd first, below; `device_picker`'s
//! egui-facing rendering comes after and is exercised only by `cargo build`
//! + the workspace test suite (no window in CI).

use std::time::{Duration, Instant};

use chrona_audio::DeviceInfo;
use chrona_session::ConfigStore;
use eframe::egui;

use crate::engine::{
    Banner, BannerSeverity, ControlMsg, Engine, HealthView, SourceKind, SourceSpec,
};

/// BPH selector state. `Fixed` carries a free-text buffer rather than a
/// parsed `u32` so a partially-typed (or momentarily invalid) value isn't
/// clobbered on every keystroke — see `to_bph_mode`.
#[derive(Debug, Clone, PartialEq)]
pub enum BphModeUi {
    Auto,
    Free,
    Fixed(String),
}

/// Maps the controls-row BPH selector to the DSP-facing `BphMode`. PURE.
/// `Fixed`'s free-text buffer is the only fallible case: unparseable (or
/// empty) input has no valid `BphMode` — the behavior contract is that the
/// caller shows this as a red outline and sends nothing, rather than
/// falling back to a guessed default.
pub fn to_bph_mode(ui: &BphModeUi) -> Option<chrona_dsp::BphMode> {
    match ui {
        BphModeUi::Auto => Some(chrona_dsp::BphMode::Auto),
        BphModeUi::Free => Some(chrona_dsp::BphMode::Free),
        BphModeUi::Fixed(text) => text
            .trim()
            .parse::<u32>()
            .ok()
            .map(chrona_dsp::BphMode::Fixed),
    }
}

/// `HealthView::silent_for_s` at or above this, on a Mic source, shows the
/// "no signal" banner (behavior contract).
const SILENT_BANNER_S: f64 = 3.0;

/// Picks at most one banner to show, in strict precedence order:
/// `engine_banner` (T7 thread faults / recording notices) > mic capture
/// error > silence (Mic only) > clipping > overruns. Never more than one at
/// a time (controller ruling).
///
/// `engine_banner`, when present, is returned as-is (severity included) —
/// the engine already classified it per the spec §4 mapping. Of the banners
/// `pick_banner` itself derives from `h`: `last_error` (a dead/failing
/// capture stream) is `BannerSeverity::Error` (spec §4: "stream/config/fault
/// errors = Error" — M3's semantics, where the last_error banner outranks
/// and covers the dead-stream freeze, depend on it reading as an error);
/// silence, clipping, and overruns are all `BannerSeverity::Warn` (spec §4
/// deliberately reclassifies these — clipping in particular — as transient
/// health notices, not faults).
///
/// PURE: all temporal/stateful judgment — "clipped within the last 5s"
/// tolerating counter resets (see `ClipTracker`), debounced retries — is the
/// caller's job before calling this; `h.clipped` here is taken as-is.
pub fn pick_banner(
    engine_banner: Option<&Banner>,
    h: &HealthView,
    kind: SourceKind,
) -> Option<Banner> {
    if let Some(b) = engine_banner {
        return Some(b.clone());
    }
    if let Some(msg) = &h.last_error {
        return Some(Banner {
            severity: BannerSeverity::Error,
            text: format!("audio error: {msg} (reconnecting)"),
        });
    }
    if kind == SourceKind::Mic && h.silent_for_s >= SILENT_BANNER_S {
        return Some(Banner {
            severity: BannerSeverity::Warn,
            text: "no signal — check mic permission / Windows Settings → Privacy → Microphone"
                .to_string(),
        });
    }
    if h.clipped > 0 {
        return Some(Banner {
            severity: BannerSeverity::Warn,
            text: "input clipping — reduce gain".to_string(),
        });
    }
    if h.overruns > 0 {
        return Some(Banner {
            severity: BannerSeverity::Warn,
            text: format!("{} buffer overrun(s) — audio briefly dropped", h.overruns),
        });
    }
    None
}

/// Resolves which banner the transient app-side slot (M4a Task 9:
/// `ChronaApp::app_banner` — currently only the Export-report success/
/// failure notice) shows this frame, against the engine-truth banner
/// already resolved by `pick_banner` (live health/capture faults, explicit
/// engine notices — spec §4). Engine truth always outranks a UI notice
/// (behavior contract): a real fault must never be silently covered on
/// screen by e.g. "report saved", so `app_banner` is only ever returned
/// when `engine_banner_present` is `false` this frame. PURE — takes
/// presence as a plain `bool` rather than `Banner` itself so this has no
/// dependency on the engine banner's own fields, just its precedence.
pub fn resolve_app_banner(
    engine_banner_present: bool,
    app_banner: Option<&(BannerSeverity, String)>,
) -> Option<&(BannerSeverity, String)> {
    if engine_banner_present {
        None
    } else {
        app_banner
    }
}

/// Trailing window `ClipTracker` treats a clip as still "current" for.
const CLIP_WINDOW_S: f64 = 5.0;

/// Turns `HealthView::clipped`'s raw, ever-increasing-until-rebuild counter
/// into a "clipping happened recently" signal for `pick_banner`'s clipped-
/// count check, so a single clipped sample early in a session doesn't leave
/// the banner stuck on forever (controller ruling: "delta over last 5s").
/// The analyzer rebuilds (resetting the counter to 0) on any config change;
/// a decrease from the last-seen count re-baselines rather than
/// underflowing or misreading the reset itself as a fresh clip.
#[derive(Debug, Default)]
pub struct ClipTracker {
    last_count: u64,
    last_clip_at: Option<std::time::Instant>,
}

impl ClipTracker {
    /// Feeds this frame's raw cumulative count; returns the windowed count
    /// to hand `pick_banner` (0, or a nonzero placeholder while still
    /// inside the trailing window).
    pub fn observe(&mut self, count: u64, now: std::time::Instant) -> u64 {
        if count > self.last_count {
            self.last_clip_at = Some(now);
        }
        self.last_count = count;
        match self.last_clip_at {
            Some(t) if now.duration_since(t).as_secs_f64() < CLIP_WINDOW_S => 1,
            _ => 0,
        }
    }
}

// ---------------------------------------------------------------------
// ControlsState: the controls row's own knobs, seeded from `ConfigStore`.
// ---------------------------------------------------------------------

/// Controls-row UI state: the device list + selection, and the DSP knobs
/// the row exposes. `app.rs`'s `update` loop is the only place that turns a
/// change here into an `Engine::send` call or a debounced config save.
#[derive(Debug, Clone)]
pub struct ControlsState {
    pub devices: Vec<DeviceInfo>,
    pub selected_device: Option<String>,
    pub lift: f64,
    pub averaging: f64,
    pub bph_mode_ui: BphModeUi,
    pub ppm: f64,
    pub last_refresh: Option<Instant>,
}

/// Poll interval for the device combo's auto-refresh (behavior contract:
/// cpal has no hot-plug events, so this is the only way new/removed devices
/// show up).
const DEVICE_POLL_INTERVAL: Duration = Duration::from_secs(2);

/// Config files are hand-editable; TOML has literal `inf`/`nan`. Gate on
/// finiteness first (f64::clamp propagates NaN), then clamp to the same
/// ranges the sidecar-restore path enforces (engine.rs).
fn sanitize_lift(lift: f64) -> f64 {
    if lift.is_finite() {
        lift.clamp(10.0, 90.0)
    } else {
        52.0
    }
}
fn sanitize_ppm(ppm: f64) -> f64 {
    if ppm.is_finite() {
        ppm.clamp(-500.0, 500.0)
    } else {
        0.0
    }
}

impl ControlsState {
    /// Seeds initial controls state from a loaded `ConfigStore`: `lift`
    /// from `default_lift_deg`, `selected_device` from `last_device`, and
    /// `ppm` from that device's saved `device_ppm` entry (resolved id ->
    /// name against a fresh device enumeration, so the combo isn't empty on
    /// the very first frame either). `averaging`/`bph_mode_ui` start at the
    /// analyzer's own defaults since M3's `ConfigStore` doesn't persist
    /// either.
    ///
    /// `config.toml` is hand-editable, so `lift`/`ppm` are run through
    /// `sanitize_lift`/`sanitize_ppm` before use — an out-of-range or
    /// `inf`/`nan` value here would otherwise reach `app.rs`'s initial
    /// `EngineConfig` unclamped and fail `Analyzer::new`, leaving the
    /// engine thread dead before the UI ever gets a control channel to
    /// recover through (the bug this sanitization fixes).
    pub fn from_config(config: &ConfigStore) -> ControlsState {
        let devices = chrona_audio::list_input_devices();
        let selected_device = config.last_device.clone();
        let ppm = selected_device
            .as_ref()
            .and_then(|id| devices.iter().find(|d| &d.id == id))
            .and_then(|d| config.device_ppm.get(&d.name))
            .copied()
            .unwrap_or(0.0);
        ControlsState {
            devices,
            selected_device,
            lift: sanitize_lift(config.default_lift_deg),
            averaging: 30.0,
            bph_mode_ui: BphModeUi::Auto,
            ppm: sanitize_ppm(ppm),
            last_refresh: Some(Instant::now()),
        }
    }
}

/// Resolves the currently selected device id to its display name, for the
/// `device_ppm` config key (behavior contract: keyed by device name, not
/// the opaque id — a name survives round-tripping through the config file
/// more legibly, matching `ConfigStore`'s own convention). `pub(crate)`:
/// reused by the toolbar's ppm popup (M4a Task 6 pre-review fix) the same
/// way `device_picker` is.
pub(crate) fn current_device_name(controls: &ControlsState) -> Option<String> {
    controls
        .selected_device
        .as_ref()
        .and_then(|id| controls.devices.iter().find(|d| &d.id == id))
        .map(|d| d.name.clone())
}

/// True iff the *set* of device ids differs — used to gate replacing
/// `controls.devices` on a poll so an unchanged device list (the common
/// case, polled every 2s) doesn't flicker the open combo box.
fn device_ids_changed(old: &[DeviceInfo], new: &[DeviceInfo]) -> bool {
    let old_ids: std::collections::BTreeSet<&str> = old.iter().map(|d| d.id.as_str()).collect();
    let new_ids: std::collections::BTreeSet<&str> = new.iter().map(|d| d.id.as_str()).collect();
    old_ids != new_ids
}

// ---------------------------------------------------------------------
// Rendering: the device picker (called from `ui::toolbar::toolbar_row`).
// Exercised by `cargo build` + the workspace test suite; no window in CI,
// so nothing here is unit-tested directly — see the module doc comment.
// ---------------------------------------------------------------------

/// Refresh button + auto-refresh-every-2s, plus the device combo itself.
/// Selecting a device (or "System default") sends `SwitchSource`, persists
/// `last_device`, and applies + sends that device's saved ppm (behavior
/// contract). `pub(crate)`, not `pub`: reused by the toolbar (M4a Task 6)
/// but not otherwise part of this module's public surface.
pub(crate) fn device_picker(
    ui: &mut egui::Ui,
    controls: &mut ControlsState,
    engine: &Engine,
    config: &mut ConfigStore,
) -> bool {
    let now = Instant::now();
    let due = controls
        .last_refresh
        .is_none_or(|t| now.duration_since(t) >= DEVICE_POLL_INTERVAL);
    if ui
        .small_button("⟳")
        .on_hover_text("Refresh input devices")
        .clicked()
        || due
    {
        let fresh = chrona_audio::list_input_devices();
        if device_ids_changed(&controls.devices, &fresh) {
            controls.devices = fresh;
        }
        controls.last_refresh = Some(now);
    }

    let current_label = controls
        .selected_device
        .as_ref()
        .and_then(|id| controls.devices.iter().find(|d| &d.id == id))
        .map(|d| d.name.clone())
        .unwrap_or_else(|| "System default".to_string());

    let mut dirty = false;
    egui::ComboBox::from_label("Input")
        .selected_text(current_label)
        .show_ui(ui, |ui| {
            let is_default = controls.selected_device.is_none();
            if ui.selectable_label(is_default, "System default").clicked() && !is_default {
                select_device(controls, config, engine, None);
                dirty = true;
            }
            for d in controls.devices.clone() {
                let selected = controls.selected_device.as_deref() == Some(d.id.as_str());
                if ui.selectable_label(selected, &d.name).clicked() && !selected {
                    select_device(controls, config, engine, Some(d.id.clone()));
                    dirty = true;
                }
            }
        });
    dirty
}

/// Applies a device selection: switches the live source, persists
/// `last_device`, and restores + sends that device's saved ppm (0.0, and
/// not persisted, for "System default" — there's no single concrete device
/// to attribute a saved calibration to).
fn select_device(
    controls: &mut ControlsState,
    config: &mut ConfigStore,
    engine: &Engine,
    device_id: Option<String>,
) {
    controls.selected_device = device_id.clone();
    engine.send(ControlMsg::SwitchSource(SourceSpec::Mic {
        device_id: device_id.clone(),
    }));
    config.last_device = device_id.clone();

    let ppm = device_id
        .as_ref()
        .and_then(|id| controls.devices.iter().find(|d| &d.id == id))
        .and_then(|d| config.device_ppm.get(&d.name))
        .copied()
        .unwrap_or(0.0);
    controls.ppm = ppm;
    engine.send(ControlMsg::SetPpm(ppm));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_bph_mode_table() {
        assert_eq!(
            to_bph_mode(&BphModeUi::Auto),
            Some(chrona_dsp::BphMode::Auto)
        );
        assert_eq!(
            to_bph_mode(&BphModeUi::Free),
            Some(chrona_dsp::BphMode::Free)
        );
        assert_eq!(
            to_bph_mode(&BphModeUi::Fixed("28800".to_string())),
            Some(chrona_dsp::BphMode::Fixed(28_800))
        );
        assert_eq!(
            to_bph_mode(&BphModeUi::Fixed(" 21600 ".to_string())),
            Some(chrona_dsp::BphMode::Fixed(21_600)),
            "surrounding whitespace from an edited text field is tolerated"
        );
        assert_eq!(
            to_bph_mode(&BphModeUi::Fixed("x".to_string())),
            None,
            "unparseable text has no BphMode"
        );
        assert_eq!(
            to_bph_mode(&BphModeUi::Fixed(String::new())),
            None,
            "empty buffer has no BphMode"
        );
        assert_eq!(
            to_bph_mode(&BphModeUi::Fixed("-5".to_string())),
            None,
            "BPH is unsigned; a negative reading is unparseable"
        );
    }

    #[test]
    fn pick_banner_precedence() {
        // A. engine_banner (T7 thread faults / recording notices) outranks
        // everything, even a simultaneous mic capture error — and is
        // returned as-is, severity included (the engine already classified
        // it per spec §4).
        let with_mic_error = HealthView {
            last_error: Some("permission denied".to_string()),
            ..HealthView::default()
        };
        let recording_stopped = Banner {
            severity: BannerSeverity::Info,
            text: "recording stopped: source changed".to_string(),
        };
        assert_eq!(
            pick_banner(Some(&recording_stopped), &with_mic_error, SourceKind::Mic),
            Some(recording_stopped.clone()),
            "engine_banner must win over a live mic error"
        );

        // B. Without an engine_banner, a mic capture error outranks
        // silence/clipping/overruns even when all are present at once, and
        // is itself Error severity (a dead/failing stream is a fault, not a
        // notice — unlike silence/clipping/overruns below, which are Warn).
        let everything_else_too = HealthView {
            last_error: Some("device disconnected".to_string()),
            silent_for_s: 10.0,
            clipped: 3,
            overruns: 2,
        };
        assert_eq!(
            pick_banner(None, &everything_else_too, SourceKind::Mic),
            Some(Banner {
                severity: BannerSeverity::Error,
                text: "audio error: device disconnected (reconnecting)".to_string(),
            }),
        );

        // C. Silence fires on Mic at the >= 3s threshold and still outranks
        // clipping/overruns.
        let silent_on_mic = HealthView {
            silent_for_s: 3.0,
            clipped: 1,
            overruns: 1,
            ..HealthView::default()
        };
        assert_eq!(
            pick_banner(None, &silent_on_mic, SourceKind::Mic),
            Some(Banner {
                severity: BannerSeverity::Warn,
                text: "no signal — check mic permission / Windows Settings → Privacy → Microphone"
                    .to_string(),
            }),
        );

        // D. Clipping outranks overruns.
        let clipping_and_overrunning = HealthView {
            clipped: 1,
            overruns: 5,
            ..HealthView::default()
        };
        assert_eq!(
            pick_banner(None, &clipping_and_overrunning, SourceKind::Mic),
            Some(Banner {
                severity: BannerSeverity::Warn,
                text: "input clipping — reduce gain".to_string(),
            }),
        );

        // E. Overruns alone are the lowest-precedence note.
        let overrunning_only = HealthView {
            overruns: 4,
            ..HealthView::default()
        };
        assert_eq!(
            pick_banner(None, &overrunning_only, SourceKind::Mic),
            Some(Banner {
                severity: BannerSeverity::Warn,
                text: "4 buffer overrun(s) — audio briefly dropped".to_string(),
            }),
        );

        // F. Replay/Simulate suppress the silence banner even when far past
        // threshold — T10's own Replay banner relies on this.
        let very_silent = HealthView {
            silent_for_s: 999.0,
            ..HealthView::default()
        };
        assert_eq!(pick_banner(None, &very_silent, SourceKind::Replay), None);
        assert_eq!(pick_banner(None, &very_silent, SourceKind::Simulate), None);

        // G. Fully healthy Mic source shows nothing.
        assert_eq!(
            pick_banner(None, &HealthView::default(), SourceKind::Mic),
            None
        );
    }

    #[test]
    fn resolve_app_banner_precedence() {
        let app = (
            BannerSeverity::Info,
            "report saved to report.html".to_string(),
        );
        assert_eq!(
            resolve_app_banner(true, Some(&app)),
            None,
            "engine truth outranks a UI notice, even when both are present"
        );
        assert_eq!(
            resolve_app_banner(false, Some(&app)),
            Some(&app),
            "no engine banner this frame: the app notice gets its turn"
        );
        assert_eq!(
            resolve_app_banner(false, None),
            None,
            "nothing to show when there's no app notice either"
        );
        assert_eq!(
            resolve_app_banner(true, None),
            None,
            "engine banner present and no app notice: still nothing from this slot"
        );
    }

    #[test]
    fn clip_tracker_windows_and_tolerates_resets() {
        let mut t = ClipTracker::default();
        let t0 = std::time::Instant::now();
        assert_eq!(t.observe(0, t0), 0, "no clipping yet");
        assert_eq!(t.observe(3, t0), 1, "count rose -> inside window");
        assert_eq!(
            t.observe(3, t0 + std::time::Duration::from_secs(4)),
            1,
            "still inside the 5s window with no new clips"
        );
        assert_eq!(
            t.observe(3, t0 + std::time::Duration::from_secs(6)),
            0,
            "window elapsed with no new clips"
        );
        // The analyzer rebuilding (any config change) resets the raw
        // counter to a lower value; this must not itself read as fresh
        // clipping, nor underflow.
        assert_eq!(
            t.observe(0, t0 + std::time::Duration::from_secs(6)),
            0,
            "reset to 0 must not itself count as new clipping"
        );
        assert_eq!(
            t.observe(1, t0 + std::time::Duration::from_secs(6)),
            1,
            "a genuine new clip right after a reset re-enters the window"
        );
    }

    #[test]
    fn sanitize_lift_clamps_finite_out_of_range_and_defaults_non_finite() {
        assert_eq!(sanitize_lift(500.0), 90.0);
        assert_eq!(sanitize_lift(5.0), 10.0);
        assert_eq!(sanitize_lift(f64::NAN), 52.0);
        assert_eq!(sanitize_lift(f64::INFINITY), 52.0);
        assert_eq!(sanitize_lift(52.0), 52.0);
    }

    #[test]
    fn sanitize_ppm_clamps_finite_out_of_range_and_defaults_non_finite() {
        assert_eq!(sanitize_ppm(f64::NAN), 0.0);
        assert_eq!(sanitize_ppm(f64::INFINITY), 0.0);
        assert_eq!(sanitize_ppm(9999.0), 500.0);
        assert_eq!(sanitize_ppm(-9999.0), -500.0);
        assert_eq!(sanitize_ppm(25.0), 25.0);
    }
}
