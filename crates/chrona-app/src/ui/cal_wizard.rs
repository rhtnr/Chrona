//! Quartz calibration wizard (M4 Task 8, binding spec §3.4): the guided
//! in-app flow around the proven ±0.5 ppm `chrona_dsp::cal::calibrate_quartz`
//! math. Entry point is the toolbar's cal-ppm popup's "Calibrate…" row
//! (`ui::toolbar::cal_ppm_control`).
//!
//! **State machine (binding, per the task brief):** `CalWizard` has exactly
//! the four variants below; `ChronaApp` owns it as `Option<CalWizard>`, plus
//! two small pieces of state this module keeps OUTSIDE the enum on purpose:
//! `confirm_cancel` (a UI-only "are you sure" flag, only meaningful while
//! `Capturing`) and the Drop-guard that deletes the temp WAV
//! (`TempCaptureGuard`, see its doc comment). Both live as their own
//! `ChronaApp` fields rather than inside a `CalWizard` variant because a
//! `Capturing -> Analyzing -> Result` transition replaces the whole enum
//! value each time — a field tucked inside one variant wouldn't survive
//! that move, but both of these need to.
//!
//! The pure helpers (`cal_advice`, `min_capture_remaining`, `analyze_wav`)
//! are TDD'd first, below; the egui-facing rendering that follows is
//! exercised only by `cargo build` + the workspace test suite (no window in
//! CI) — same split as every other M4a modal (`ui::modals`, `ui::toolbar`,
//! `ui::strip`, `ui::history_ui`).

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

use eframe::egui;

use chrona_dsp::cal::{CalError, QuartzCalResult, calibrate_quartz};
use chrona_session::ConfigStore;

use crate::engine::{ControlMsg, Engine, EngineSnapshot};
use crate::presenter::{SignalClass, signal_meter};
use crate::theme::{self, Palette};
use crate::ui::controls::{ControlsState, current_device_name};
use crate::ui::strip::rec_elapsed_label;
use crate::ui::toolbar::{apply_and_persist_ppm, persist_config_now};

/// Minimum capture length `chrona_dsp::cal::calibrate_quartz` accepts.
/// Mirrors that module's own private `MIN_SECONDS` constant — duplicated
/// rather than imported, since it isn't part of `chrona_dsp::cal`'s public
/// surface and this task's global constraint keeps `chrona-dsp` untouched.
const MIN_CAPTURE_S: f64 = 300.0;

/// How many times `analyze_wav` retries *opening* the just-stopped
/// recording before giving up, and the delay between attempts — see
/// `analyze_wav`'s doc comment for the race this absorbs. 20 × 100 ms = 2 s
/// worst case.
const OPEN_RETRY_ATTEMPTS: u32 = 20;
const OPEN_RETRY_DELAY: Duration = Duration::from_millis(100);

/// How long Phase 1 waits for `EngineSnapshot::recording` to confirm a
/// `StartCapture` before giving up and surfacing an honest failure, rather
/// than silently counting down a full 5-minute capture against a recording
/// that never actually started (final-review IMPORTANT-2: e.g.
/// `ControlMsg::StartRecording` was refused because the input source was
/// idle, or the engine thread never got a chance to publish a confirming
/// snapshot). The engine's own snapshot-publish latency is ~100 ms (see
/// `CalWizard::Capturing`'s doc comment) — 3 s is a 30x margin over that,
/// comfortably longer than any normal confirm but far short of making a
/// stuck wizard indistinguishable from a slow one.
const CAPTURE_CONFIRM_GRACE_S: f64 = 3.0;

// ---------------------------------------------------------------------
// Pure helpers — TDD'd in `tests` below.
// ---------------------------------------------------------------------

/// Plain-language next-step advice for each `CalError` variant (HONESTY
/// requirement: the wizard never shows a fabricated ppm on failure, always a
/// concrete, honest next step). PURE.
pub fn cal_advice(err: &CalError) -> &'static str {
    match err {
        CalError::TooShort { .. } => {
            "The capture ended before the minimum 5 minutes of quartz tick was recorded. \
             Start a new capture and let it run at least 5 minutes (10\u{2013}15 recommended) \
             before analyzing."
        }
        CalError::NoTicks => {
            "No 1 Hz quartz tick was found in the recording. Clamp a quartz watch directly \
             to the mic pickup, raise the input gain, and re-record in a quiet room."
        }
        CalError::Unstable { .. } => {
            "The fit was unstable, most likely temperature drift or the watch/mic moving \
             during capture. Re-record in a quieter, temperature-stable setup and don't \
             disturb the pickup while it runs."
        }
        CalError::NotQuartz { .. } => {
            "This looks like a mechanical watch, not quartz — the wizard needs a quartz watch \
             (one tick per second). Rate measurement of mechanical watches is the main \
             screen's job."
        }
        CalError::BadInput { .. } => {
            "Internal error while analyzing the capture. Try recording again; if this keeps \
             happening, it's likely a bug worth reporting."
        }
    }
}

/// Time remaining before `calibrate_quartz`'s minimum capture length is met,
/// or `None` once it is (the Capturing screen's Analyze button is enabled
/// exactly when this returns `None`, and shows the countdown otherwise).
/// PURE.
pub fn min_capture_remaining(started: Instant, now: Instant) -> Option<Duration> {
    let min = Duration::from_secs_f64(MIN_CAPTURE_S);
    let elapsed = now.saturating_duration_since(started);
    if elapsed >= min {
        None
    } else {
        Some(min - elapsed)
    }
}

/// Whether Phase 1 should confirm `candidate` (`EngineSnapshot::recording`,
/// borrowed via `.as_deref()`) as THIS wizard capture's own file, rather
/// than some other in-flight recording that merely happens to be visible in
/// the same snapshot tick. Closes the stale-snapshot race final review
/// found (CRITICAL-1): `EngineSnapshot` publishes on a fixed tick, up to
/// ~100 ms behind any single `ControlMsg`, so the FIRST `Some(path)` seen
/// after `StartCapture` isn't guaranteed to be the wizard's own — if the
/// user already had a SESSION recording running (or starts one in that same
/// window), that recording's path could be confirmed instead, wrapped in
/// `TempCaptureGuard`, and DELETED on wizard exit. A session recording is
/// always written to the user's configured recordings directory, never to
/// `dir` (the wizard's own `std::env::temp_dir().join(TEMP_CAPTURE_SUBDIR)`
/// — see `WizardAction::StartCapture`), so confirming only a path inside
/// `dir` closes the race: a foreign path is simply ignored, and Phase 1
/// keeps waiting for the real one. `None` (no snapshot yet) is also
/// rejected — nothing to confirm. Uses `Path::starts_with`, which compares
/// whole path COMPONENTS, not a raw string prefix, so a sibling directory
/// that merely shares `dir`'s string prefix is correctly rejected too.
/// PURE.
pub fn accepts_capture_path(dir: &Path, candidate: Option<&Path>) -> bool {
    candidate.is_some_and(|p| p.starts_with(dir))
}

/// Whether Phase 1 should give up waiting for a `StartCapture` to be
/// confirmed by a snapshot: true once `elapsed` (time since `Capturing`'s
/// `started`) has passed `CAPTURE_CONFIRM_GRACE_S` with no path confirmed
/// yet. `path_confirmed` makes a successful (if slow) confirm immune to a
/// LATER timeout check — once the wizard's own capture path is known, the
/// capture is real and counting down toward `min_capture_remaining`
/// normally, however long the confirm itself took. PURE.
pub fn capture_confirm_timed_out(elapsed: Duration, path_confirmed: bool) -> bool {
    !path_confirmed && elapsed.as_secs_f64() >= CAPTURE_CONFIRM_GRACE_S
}

/// Reads `path`'s WAV (via `chrona_session::SessionReader`) and runs
/// `calibrate_quartz` against it, using the file's ACTUAL sample rate —
/// never an assumed 48 kHz, since a mic can negotiate a different rate.
/// This is the exact fn both the background analysis thread (spawned from
/// `apply_wizard_action`'s `WizardAction::Analyze` arm, below) and this
/// module's own integration test call.
///
/// Retries the *open* step (only) a bounded number of times:
/// `ControlMsg::StopRecording` (sent right before the thread is spawned —
/// see `WizardAction::Analyze`) is drained asynchronously by the engine
/// thread, so the WAV may not be finalized (RIFF header closed, sidecar
/// rewritten) the instant this fn first runs. Retrying here — rather than growing the
/// binding state machine an extra "waiting to confirm stop" variant —
/// absorbs that ordering race without it ever surfacing as a spurious,
/// dishonest `CalError` for a capture that was actually fine. Once the file
/// opens, `calibrate_quartz`'s own result (`Ok` or any `CalError`) is
/// returned as-is with no further retrying: a real
/// `NoTicks`/`Unstable`/`TooShort` is deterministic given the same bytes, so
/// retrying it would only add latency, never a different answer.
fn analyze_wav(path: &Path) -> Result<QuartzCalResult, CalError> {
    let mut last_open_err = String::new();
    for attempt in 0..OPEN_RETRY_ATTEMPTS {
        match chrona_session::SessionReader::open(path) {
            Ok(reader) => return calibrate_quartz(&reader.samples, reader.sample_rate_hz),
            Err(e) => {
                last_open_err = e.to_string();
                if attempt + 1 < OPEN_RETRY_ATTEMPTS {
                    std::thread::sleep(OPEN_RETRY_DELAY);
                }
            }
        }
    }
    Err(CalError::BadInput {
        reason: format!("failed to read capture: {last_open_err}"),
    })
}

// ---------------------------------------------------------------------
// State machine (binding) + rendering. Exercised by `cargo build` + the
// workspace test suite; no window in CI, so nothing below is unit-tested
// directly — see the module doc comment.
// ---------------------------------------------------------------------

/// The calibration wizard's state machine (binding, per the task brief).
/// Owned by `ChronaApp` as `Option<CalWizard>` — `None` means the wizard
/// isn't open.
pub enum CalWizard {
    /// What/why + "Start capture".
    Intro,
    /// Recording via the engine's existing tee into a temp WAV. `started`
    /// is when the capture began (drives the elapsed clock, the
    /// `min_capture_remaining` countdown, and the `CAPTURE_CONFIRM_GRACE_S`
    /// give-up timer); `path` is the temp WAV's path — empty until
    /// `render_cal_wizard` fills it in from `EngineSnapshot::recording` once
    /// the engine confirms it. It can't be predicted client-side instead
    /// (the engine timestamps the filename itself, from its own thread,
    /// when it processes `StartRecording`).
    ///
    /// `dir` is the capture directory `WizardAction::StartCapture` computed
    /// (`std::env::temp_dir().join(TEMP_CAPTURE_SUBDIR)`), carried alongside
    /// `path` so Phase 1 can guard against a race final review caught
    /// (CRITICAL-1): `EngineSnapshot` publishes on a fixed tick, up to
    /// ~100 ms behind any single `ControlMsg`, so the FIRST `Some(path)`
    /// seen after `StartCapture` isn't necessarily the wizard's own file —
    /// if the user already had a SESSION recording running (or starts one
    /// in that same window) before the wizard's own capture is confirmed,
    /// that recording's path would be confirmed instead, wrapped in
    /// `TempCaptureGuard`, and DELETED (WAV + sidecar) the moment the
    /// wizard exits. `accepts_capture_path` confirms a snapshot path only
    /// when it falls inside `dir`, so a foreign path is ignored (Phase 1
    /// just keeps waiting) rather than adopted. See also `ui::toolbar`'s
    /// `cal_ppm_control`, which closes the other half of this same race by
    /// disabling "Calibrate…" while a recording is already in progress —
    /// so the wizard can't even open mid-session-recording in the first
    /// place.
    Capturing {
        started: Instant,
        path: PathBuf,
        dir: PathBuf,
    },
    /// `StopRecording` was sent and a background thread is running
    /// `analyze_wav` on the finalized capture; `rx` is polled once per
    /// frame via `try_recv` (`render_cal_wizard`'s Phase 2, below).
    Analyzing {
        rx: Receiver<Result<QuartzCalResult, CalError>>,
    },
    /// The analysis finished — by this point the temp WAV is already
    /// deleted (design spec §3: "deleted after analysis regardless of
    /// outcome" — see `render_cal_wizard`'s Phase 2). `Ok` shows the
    /// measured ppm + residual and "Save for <device>"/"Discard"; `Err`
    /// shows `cal_advice` and "Try again"/"Close".
    Result {
        outcome: Result<QuartzCalResult, CalError>,
    },
}

/// Deletes the wizard's temp WAV+JSON sidecar when dropped — the Drop-guard
/// the brief calls for. Kept as its own `ChronaApp` field (`cal_capture_
/// guard`), NOT nested inside a `CalWizard` variant: a `Capturing ->
/// Analyzing` transition replaces the whole enum value, which would drop a
/// nested guard prematurely and delete the file the analysis thread is
/// about to read. Being a plain `ChronaApp` field instead means it's also
/// dropped — and cleans up — when `ChronaApp` itself drops (e.g. normal app
/// shutdown mid-wizard), with no extra code needed there.
///
/// **Caveat, stated honestly:** `Drop` is a Rust-level scope-exit
/// mechanism, not a process-external guarantee — it cannot run on an
/// OS-level crash, `kill -9`, or power loss. A hard crash mid-capture can
/// still leave a `chrona-cal` temp file behind; the OS temp directory is
/// expected to be cleared periodically regardless, so this is a documented
/// limitation, not a silently-broken promise.
pub struct TempCaptureGuard {
    wav_path: PathBuf,
}

impl TempCaptureGuard {
    fn new(wav_path: PathBuf) -> TempCaptureGuard {
        TempCaptureGuard { wav_path }
    }
}

impl Drop for TempCaptureGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.wav_path);
        let _ = std::fs::remove_file(self.wav_path.with_extension("json"));
    }
}

/// The subdirectory of `std::env::temp_dir()` every capture records into
/// (brief: `std::env::temp_dir().join("chrona-cal")`).
const TEMP_CAPTURE_SUBDIR: &str = "chrona-cal";

/// Borrowed, per-frame context `render_cal_wizard` needs beyond the
/// wizard's own owned state (`wizard`/`confirm_cancel`/`guard`, passed as
/// separate `&mut` params so `ChronaApp` can hold each as its own field —
/// see `CalWizard`'s and `TempCaptureGuard`'s doc comments for why they
/// can't just live inside one bundled struct). Same per-frame-context
/// bundling shape as `ToolbarCtx`/`StripCtx`.
pub struct CalWizardCtx<'a> {
    pub engine: &'a Engine,
    pub snap: &'a EngineSnapshot,
    pub controls: &'a mut ControlsState,
    pub config: &'a mut ConfigStore,
}

/// What a screen's buttons (or Esc/click-outside) decided this frame.
/// Rendering only ever produces one of these — turning it into an actual
/// mutation of `*wizard`/`*confirm_cancel`/`*guard` is `apply_wizard_action`
/// below, applied only AFTER the immutable borrow rendering takes of the
/// current `CalWizard` value has ended (several of these transitions, e.g.
/// `StartCapture`, replace `*wizard` outright, which can't happen while
/// something still borrows the old value).
enum WizardAction {
    None,
    StartCapture,
    AskCancelCapture,
    DismissCancelConfirm,
    ConfirmCancelCapture,
    Analyze,
    /// Discard/Close/Esc/click-outside — every exit path that does NOT
    /// save a ppm. Always stops any in-flight recording (a harmless no-op
    /// if nothing was recording — `ControlMsg::StopRecording` against an
    /// idle engine is already a normal path) and drops the capture guard.
    CloseWithoutSaving,
    /// Result(Err)'s "Try again": back to `Intro`, no cleanup needed (the
    /// temp WAV was already deleted the moment analysis finished).
    TryAgain,
    /// Result(Ok)'s "Save for <device>", carrying the ppm to persist.
    Save(f64),
}

/// Renders the calibration wizard when `*wizard` is `Some` (no-op
/// otherwise): overlay (`palette.overlay` at the same 140-alpha every M4a
/// modal uses) plus a centered card, one screen per `CalWizard` variant —
/// same M4a modal conventions (`ui::modals`) as every other overlay in this
/// app. Esc/click-outside close the wizard, EXCEPT while `Capturing`, where
/// they ask for confirmation first (`*confirm_cancel`) rather than silently
/// discarding an in-progress capture (the one addition the brief calls for
/// beyond the standard three-way close contract).
///
/// Three phases, each re-borrowing `*wizard` fresh so a later phase is free
/// to replace it outright (Rust won't allow reassigning `*wizard` while an
/// earlier phase's borrow of the old value is still alive):
/// 1. Reconcile `Capturing`'s `path` from `wctx.snap.recording`, subject to
///    `accepts_capture_path`'s own-directory guard (CRITICAL-1) — a
///    confirmed path outside the wizard's own capture dir is ignored, not
///    adopted; and `capture_confirm_timed_out`'s give-up timeout
///    (IMPORTANT-2) — no valid path confirmed within
///    `CAPTURE_CONFIRM_GRACE_S` gives up honestly instead of counting down a
///    phantom 5-minute capture.
/// 2. Poll `Analyzing`'s `rx`; on completion, delete the temp capture and
///    transition to `Result`.
/// 3. Render whatever `*wizard` now holds, collect a `WizardAction`, and
///    apply it via `apply_wizard_action`.
pub fn render_cal_wizard(
    ctx: &egui::Context,
    palette: &Palette,
    wizard: &mut Option<CalWizard>,
    confirm_cancel: &mut bool,
    guard: &mut Option<TempCaptureGuard>,
    wctx: CalWizardCtx<'_>,
) {
    if wizard.is_none() {
        *confirm_cancel = false; // stays clean for the next time the wizard opens
        return;
    }

    // Phase 1: reconcile the Capturing path from the snapshot — see
    // `CalWizard::Capturing`'s doc comment for why this can't just be
    // predicted client-side instead, and for the stale-snapshot race
    // `accepts_capture_path` guards against (CRITICAL-1).
    if let Some(CalWizard::Capturing { started, path, dir }) = wizard.as_mut()
        && path.as_os_str().is_empty()
    {
        if accepts_capture_path(dir, wctx.snap.recording.as_deref()) {
            let confirmed = wctx
                .snap
                .recording
                .clone()
                .expect("accepts_capture_path only returns true when recording is Some");
            *path = confirmed.clone();
            if guard.is_none() {
                *guard = Some(TempCaptureGuard::new(confirmed));
            }
        } else if capture_confirm_timed_out(started.elapsed(), false) {
            // IMPORTANT-2: give up honestly rather than count down a
            // phantom 5-minute capture against a recording that never
            // started. Defensively stop any recording anyway — if the
            // engine actually did start one just as this timeout fired (the
            // race is generous but not impossible), this ensures it doesn't
            // run forever unattended with nothing left to stop it.
            wctx.engine.send(ControlMsg::StopRecording);
            *wizard = Some(CalWizard::Result {
                outcome: Err(CalError::BadInput {
                    reason: "recording never started \u{2014} check the input source".to_string(),
                }),
            });
        }
    }

    // Phase 2: poll the analysis thread. "The temp WAV is deleted after
    // analysis regardless of outcome" (design spec §3) — both arms below
    // clear `*guard` unconditionally.
    let mut analysis_finished: Option<Result<QuartzCalResult, CalError>> = None;
    if let Some(CalWizard::Analyzing { rx }) = wizard.as_mut() {
        analysis_finished = match rx.try_recv() {
            Ok(outcome) => Some(outcome),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err(CalError::BadInput {
                reason: "analysis thread ended unexpectedly".to_string(),
            })),
        };
    }
    if let Some(outcome) = analysis_finished {
        *guard = None;
        *wizard = Some(CalWizard::Result { outcome });
    }

    // Phase 3: render.
    let Some(state) = wizard.as_mut() else {
        return; // unreachable in practice — phases 1/2 never clear `*wizard`
    };

    let screen = ctx.input(|i| i.content_rect());
    let scrim_layer = egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("chrona_cal_wizard_scrim"),
    );
    ctx.layer_painter(scrim_layer).rect_filled(
        screen,
        0.0,
        theme::with_alpha(palette.overlay, 140),
    );

    let is_capturing = matches!(state, CalWizard::Capturing { .. });
    let (_, signal_label, signal_class) = signal_meter(wctx.snap.metrics.as_ref());
    let device_label = current_device_name(wctx.controls);

    let mut action = WizardAction::None;
    let card_response = egui::Area::new(egui::Id::new("chrona_cal_wizard_card"))
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(palette.panel)
                .stroke(egui::Stroke::new(1.0, palette.border2))
                .corner_radius(14.0)
                .inner_margin(22.0)
                .show(ui, |ui| {
                    ui.set_width(420.0_f32.min(screen.width() - 48.0 - 44.0));
                    action = match &*state {
                        CalWizard::Intro => render_intro(ui, palette),
                        CalWizard::Capturing { started, .. } => render_capturing(
                            ui,
                            palette,
                            *started,
                            *confirm_cancel,
                            signal_label,
                            signal_class,
                        ),
                        CalWizard::Analyzing { .. } => render_analyzing(ui, palette),
                        CalWizard::Result { outcome } => {
                            render_result(ui, palette, outcome, device_label.as_deref())
                        }
                    };
                });
        })
        .response;

    let clicked_outside = ctx.input(|i| i.pointer.any_click())
        && ctx
            .input(|i| i.pointer.interact_pos())
            .is_some_and(|pos| !card_response.rect.contains(pos));
    let esc = ctx.input(|i| i.key_pressed(egui::Key::Escape));
    if matches!(action, WizardAction::None) && (clicked_outside || esc) {
        action = if is_capturing {
            WizardAction::AskCancelCapture
        } else {
            WizardAction::CloseWithoutSaving
        };
    }

    apply_wizard_action(action, wizard, confirm_cancel, guard, wctx);
}

fn apply_wizard_action(
    action: WizardAction,
    wizard: &mut Option<CalWizard>,
    confirm_cancel: &mut bool,
    guard: &mut Option<TempCaptureGuard>,
    wctx: CalWizardCtx<'_>,
) {
    match action {
        WizardAction::None => {}
        WizardAction::StartCapture => {
            let dir = std::env::temp_dir().join(TEMP_CAPTURE_SUBDIR);
            let _ = std::fs::create_dir_all(&dir);
            wctx.engine.send(ControlMsg::StartRecording {
                dir: dir.clone(),
                meta_position: None,
                watch: None,
            });
            *wizard = Some(CalWizard::Capturing {
                started: Instant::now(),
                path: PathBuf::new(),
                dir,
            });
        }
        WizardAction::AskCancelCapture => *confirm_cancel = true,
        WizardAction::DismissCancelConfirm => *confirm_cancel = false,
        WizardAction::ConfirmCancelCapture => {
            wctx.engine.send(ControlMsg::StopRecording);
            *confirm_cancel = false;
            *guard = None;
            *wizard = None;
        }
        WizardAction::Analyze => {
            if let Some(CalWizard::Capturing { path, .. }) = wizard.take() {
                wctx.engine.send(ControlMsg::StopRecording);
                let (tx, rx) = mpsc::channel();
                std::thread::spawn(move || {
                    let _ = tx.send(analyze_wav(&path));
                });
                *wizard = Some(CalWizard::Analyzing { rx });
            }
        }
        WizardAction::CloseWithoutSaving => {
            wctx.engine.send(ControlMsg::StopRecording);
            *confirm_cancel = false;
            *guard = None;
            *wizard = None;
        }
        WizardAction::TryAgain => *wizard = Some(CalWizard::Intro),
        WizardAction::Save(ppm) => {
            wctx.controls.ppm = ppm;
            apply_and_persist_ppm(wctx.engine, wctx.config, wctx.controls, ppm);
            // IMPORTANT-1: `apply_and_persist_ppm` alone only updates the
            // in-memory `ConfigStore` + the live engine — without this, the
            // measured ppm was lost the moment the app quit.
            persist_config_now(wctx.config);
            *guard = None;
            *wizard = None;
        }
    }
}

fn outline_button(ui: &mut egui::Ui, palette: &Palette, text: &str) -> egui::Response {
    ui.add(
        egui::Button::new(egui::RichText::new(text).color(palette.text))
            .fill(egui::Color32::TRANSPARENT)
            .stroke(egui::Stroke::new(1.0, palette.border2)),
    )
}

fn heading(ui: &mut egui::Ui, palette: &Palette, text: &str) {
    ui.label(
        egui::RichText::new(text)
            .font(egui::FontId::new(
                16.0,
                egui::FontFamily::Name(theme::FAMILY_SEMIBOLD.into()),
            ))
            .color(palette.text),
    );
}

fn signal_hint_color(palette: &Palette, class: SignalClass) -> egui::Color32 {
    match class {
        SignalClass::Good => palette.good,
        SignalClass::Warn => palette.warnfg,
        SignalClass::Faint => palette.faint,
    }
}

/// Rounds a `Duration` UP to the nearest whole second, for a "time
/// remaining" countdown display that should never show "0s" while the
/// Analyze button is still disabled.
fn ceil_secs(d: Duration) -> u64 {
    d.as_secs() + u64::from(d.subsec_nanos() > 0)
}

fn render_intro(ui: &mut egui::Ui, palette: &Palette) -> WizardAction {
    let mut action = WizardAction::None;
    heading(ui, palette, "Quartz calibration");
    ui.add_space(6.0);
    ui.label(
        egui::RichText::new(
            "Clamp any quartz watch to the mic pickup. Chrona regresses the watch's 1 Hz \
             tick against a perfect grid to measure this audio device's clock error.",
        )
        .size(13.0)
        .color(palette.muted),
    );
    ui.add_space(6.0);
    ui.label(
        egui::RichText::new(
            "Record for at least 5 minutes \u{2014} 10\u{2013}15 minutes gives the best fit.",
        )
        .size(13.0)
        .color(palette.muted),
    );
    ui.add_space(14.0);
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        if ui
            .add(
                egui::Button::new(
                    egui::RichText::new("Start capture")
                        .color(palette.accent_ink)
                        .strong(),
                )
                .fill(palette.accent),
            )
            .clicked()
        {
            action = WizardAction::StartCapture;
        }
        if outline_button(ui, palette, "Cancel").clicked() {
            action = WizardAction::CloseWithoutSaving;
        }
    });
    action
}

fn render_capturing(
    ui: &mut egui::Ui,
    palette: &Palette,
    started: Instant,
    confirm_cancel: bool,
    signal_label: &'static str,
    signal_class: SignalClass,
) -> WizardAction {
    let mut action = WizardAction::None;

    if confirm_cancel {
        heading(ui, palette, "Discard this capture?");
        ui.add_space(6.0);
        ui.label(
            egui::RichText::new(
                "The recording so far will be stopped and deleted. This can't be undone.",
            )
            .size(13.0)
            .color(palette.muted),
        );
        ui.add_space(14.0);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .add(
                    egui::Button::new(
                        egui::RichText::new("Discard capture")
                            .color(palette.rec_ink)
                            .strong(),
                    )
                    .fill(palette.rec),
                )
                .clicked()
            {
                action = WizardAction::ConfirmCancelCapture;
            }
            if outline_button(ui, palette, "Keep capturing").clicked() {
                action = WizardAction::DismissCancelConfirm;
            }
        });
        return action;
    }

    heading(ui, palette, "Capturing\u{2026}");
    ui.add_space(6.0);
    let elapsed = started.elapsed();
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(format!("\u{25cf} {}", rec_elapsed_label(elapsed.as_secs())))
                .monospace()
                .size(14.0)
                .color(palette.rec),
        );
        ui.label(
            egui::RichText::new(format!("Signal: {signal_label}"))
                .size(12.0)
                .color(signal_hint_color(palette, signal_class)),
        );
    });
    ui.add_space(4.0);
    ui.label(
        egui::RichText::new("10\u{2013}15 minutes gives the best fit.")
            .size(12.0)
            .color(palette.muted),
    );
    ui.add_space(14.0);

    let remaining = min_capture_remaining(started, Instant::now());
    let can_analyze = remaining.is_none();
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.add_enabled_ui(can_analyze, |ui| {
            let resp = ui.add(
                egui::Button::new(
                    egui::RichText::new("Analyze")
                        .color(palette.accent_ink)
                        .strong(),
                )
                .fill(palette.accent),
            );
            let resp = match remaining {
                Some(r) => resp.on_disabled_hover_text(format!(
                    "Available in {}",
                    rec_elapsed_label(ceil_secs(r))
                )),
                None => resp,
            };
            if resp.clicked() {
                action = WizardAction::Analyze;
            }
        });
        if outline_button(ui, palette, "Cancel").clicked() {
            action = WizardAction::AskCancelCapture;
        }
    });
    if let Some(r) = remaining {
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(format!(
                "Analyze available in {}",
                rec_elapsed_label(ceil_secs(r))
            ))
            .size(12.0)
            .color(palette.faint),
        );
    }
    // The elapsed clock / countdown need to keep advancing even with no
    // user input — same repaint-while-active idiom as
    // `ui::toolbar::record_stop_button`'s pulsing dot.
    ui.ctx().request_repaint_after(Duration::from_millis(250));
    action
}

fn render_analyzing(ui: &mut egui::Ui, palette: &Palette) -> WizardAction {
    let mut action = WizardAction::None;
    ui.horizontal(|ui| {
        ui.add(egui::Spinner::new().color(palette.accent));
        heading(ui, palette, "Analyzing capture\u{2026}");
    });
    ui.add_space(6.0);
    ui.label(
        egui::RichText::new("This usually takes a few seconds.")
            .size(13.0)
            .color(palette.muted),
    );
    ui.add_space(14.0);
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        if outline_button(ui, palette, "Cancel").clicked() {
            action = WizardAction::CloseWithoutSaving;
        }
    });
    ui.ctx().request_repaint_after(Duration::from_millis(100));
    action
}

fn render_result(
    ui: &mut egui::Ui,
    palette: &Palette,
    outcome: &Result<QuartzCalResult, CalError>,
    device_label: Option<&str>,
) -> WizardAction {
    let mut action = WizardAction::None;
    match outcome {
        Ok(r) => {
            ui.label(
                egui::RichText::new(format!(
                    "{:+.1} ppm \u{00b1} {:.2} ppm",
                    r.ppm, r.residual_ppm
                ))
                .font(egui::FontId::new(
                    20.0,
                    egui::FontFamily::Name(theme::FAMILY_BOLD.into()),
                ))
                .color(palette.text),
            );
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(format!(
                    "{} events over {:.0}s of capture",
                    r.events, r.duration_s
                ))
                .size(12.0)
                .color(palette.muted),
            );
            ui.add_space(10.0);
            egui::Frame::new()
                .fill(palette.panel2)
                .corner_radius(10.0)
                .inner_margin(egui::Margin::symmetric(14, 12))
                .show(ui, |ui| {
                    ui.label(
                        egui::RichText::new(
                            "This calibration is only as accurate as the reference quartz \
                             watch itself \u{2014} typically within a few ppm \u{2014} unless \
                             you know its true rate and enter that separately.",
                        )
                        .size(12.0)
                        .color(palette.muted),
                    );
                });
            ui.add_space(14.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let save_label = match device_label {
                    Some(name) => format!("Save for {name}"),
                    None => "Save".to_string(),
                };
                ui.add_enabled_ui(device_label.is_some(), |ui| {
                    let resp = ui.add(
                        egui::Button::new(
                            egui::RichText::new(&save_label)
                                .color(palette.accent_ink)
                                .strong(),
                        )
                        .fill(palette.accent),
                    );
                    let resp = if device_label.is_some() {
                        resp
                    } else {
                        resp.on_disabled_hover_text(
                            "Select an input device to save a per-device calibration",
                        )
                    };
                    if resp.clicked() {
                        action = WizardAction::Save(r.ppm);
                    }
                });
                if outline_button(ui, palette, "Discard").clicked() {
                    action = WizardAction::CloseWithoutSaving;
                }
            });
        }
        Err(e) => {
            ui.label(
                egui::RichText::new("Calibration failed")
                    .font(egui::FontId::new(
                        16.0,
                        egui::FontFamily::Name(theme::FAMILY_SEMIBOLD.into()),
                    ))
                    .color(palette.rec),
            );
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new(e.to_string())
                    .size(12.0)
                    .color(palette.muted),
            );
            ui.add_space(8.0);
            ui.label(
                egui::RichText::new(cal_advice(e))
                    .size(13.0)
                    .color(palette.text),
            );
            ui.add_space(14.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add(
                        egui::Button::new(
                            egui::RichText::new("Try again")
                                .color(palette.accent_ink)
                                .strong(),
                        )
                        .fill(palette.accent),
                    )
                    .clicked()
                {
                    action = WizardAction::TryAgain;
                }
                if outline_button(ui, palette, "Close").clicked() {
                    action = WizardAction::CloseWithoutSaving;
                }
            });
        }
    }
    action
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrona_dsp::synth::synthesize_quartz;
    use chrona_session::{SessionMeta, SessionWriter};

    #[test]
    fn cal_advice_non_empty_per_variant() {
        let errs = [
            CalError::TooShort {
                seconds: 10.0,
                required: 300.0,
            },
            CalError::NoTicks,
            CalError::Unstable { residual_ppm: 5.0 },
            CalError::NotQuartz {
                off_gate_ratio: 5.0,
            },
            CalError::BadInput {
                reason: "x".to_string(),
            },
        ];
        for e in &errs {
            assert!(!cal_advice(e).is_empty(), "{e:?} advice must not be empty");
        }
    }

    /// Brief: "NoTicks -> placement/gain; Unstable -> temperature/movement;
    /// BadInput -> internal" — pins the advice text stays on-topic per
    /// variant, not just non-empty.
    #[test]
    fn cal_advice_is_topical_per_variant() {
        let no_ticks = cal_advice(&CalError::NoTicks).to_lowercase();
        assert!(
            no_ticks.contains("clamp")
                || no_ticks.contains("pickup")
                || no_ticks.contains("placement"),
            "NoTicks advice should mention placement: {no_ticks:?}"
        );
        assert!(
            no_ticks.contains("gain"),
            "NoTicks advice should mention gain: {no_ticks:?}"
        );

        let unstable = cal_advice(&CalError::Unstable { residual_ppm: 5.0 }).to_lowercase();
        assert!(
            unstable.contains("temperature"),
            "Unstable advice should mention temperature: {unstable:?}"
        );
        assert!(
            unstable.contains("mov"),
            "Unstable advice should mention movement: {unstable:?}"
        );

        let bad_input = cal_advice(&CalError::BadInput {
            reason: "x".to_string(),
        })
        .to_lowercase();
        assert!(
            bad_input.contains("internal"),
            "BadInput advice should mention 'internal': {bad_input:?}"
        );

        let not_quartz = cal_advice(&CalError::NotQuartz {
            off_gate_ratio: 5.0,
        })
        .to_lowercase();
        assert!(
            not_quartz.contains("mechanical"),
            "NotQuartz advice should mention 'mechanical': {not_quartz:?}"
        );
        assert!(
            not_quartz.contains("quartz"),
            "NotQuartz advice should mention 'quartz': {not_quartz:?}"
        );

        let too_short = cal_advice(&CalError::TooShort {
            seconds: 10.0,
            required: 300.0,
        })
        .to_lowercase();
        assert!(
            too_short.contains("5 min")
                || too_short.contains("300")
                || too_short.contains("minute"),
            "TooShort advice should mention the minimum duration: {too_short:?}"
        );
    }

    /// Final-review CRITICAL-1: `accepts_capture_path` guards Phase 1
    /// against confirming a snapshot path that belongs to some OTHER
    /// in-flight recording (most dangerously, the user's own session
    /// recording) rather than this wizard's own capture.
    #[test]
    fn accepts_capture_path_own_dir_and_foreign_and_none() {
        let dir = Path::new("/tmp/chrona-cal");
        let own = dir.join("chrona-1700000000.wav");
        let foreign = Path::new("/Users/example/recordings/chrona-1700000000.wav");
        assert!(
            accepts_capture_path(dir, Some(&own)),
            "a path inside the wizard's own capture dir must be accepted"
        );
        assert!(
            !accepts_capture_path(dir, Some(foreign)),
            "a path outside the wizard's capture dir (e.g. a session recording) \
             must be rejected"
        );
        assert!(
            !accepts_capture_path(dir, None),
            "no confirmed snapshot path yet must be rejected (keep waiting)"
        );
        // `Path::starts_with` matches whole components, not raw string
        // prefixes — a sibling directory that merely SHARES a string
        // prefix must still be rejected.
        let sibling_dir = Path::new("/tmp/chrona-cal-evil/chrona-1700000000.wav");
        assert!(
            !accepts_capture_path(dir, Some(sibling_dir)),
            "a string-prefix-only sibling directory must not be mistaken for \
             the real one"
        );
    }

    /// Final-review IMPORTANT-2: `capture_confirm_timed_out` is Phase 1's
    /// give-up signal for a `StartCapture` that the engine never confirms
    /// (e.g. `StartRecording` was refused because the input source was
    /// idle) — without it, the wizard silently counts down a full 5-minute
    /// phantom capture against nothing.
    #[test]
    fn capture_confirm_timed_out_grace_period() {
        assert!(
            !capture_confirm_timed_out(Duration::from_millis(0), false),
            "no time elapsed: keep waiting"
        );
        assert!(
            !capture_confirm_timed_out(Duration::from_millis(2_999), false),
            "1ms short of the grace period: keep waiting"
        );
        assert!(
            capture_confirm_timed_out(Duration::from_secs_f64(3.0), false),
            "exactly at the grace period with no path confirmed: give up"
        );
        assert!(
            capture_confirm_timed_out(Duration::from_millis(10_000), false),
            "well past the grace period with no path confirmed: give up"
        );
        assert!(
            !capture_confirm_timed_out(Duration::from_millis(10_000), true),
            "once a path IS confirmed, the grace period no longer applies — \
             immune"
        );
    }

    #[test]
    fn min_capture_remaining_math_and_boundary() {
        let t0 = Instant::now();
        assert_eq!(
            min_capture_remaining(t0, t0),
            Some(Duration::from_secs(300)),
            "no time elapsed: full 300s remaining"
        );
        assert_eq!(
            min_capture_remaining(t0, t0 + Duration::from_secs(299)),
            Some(Duration::from_secs(1)),
            "1s short of the minimum"
        );
        assert_eq!(
            min_capture_remaining(t0, t0 + Duration::from_millis(299_999)),
            Some(Duration::from_millis(1)),
            "1ms short of the minimum"
        );
        assert_eq!(
            min_capture_remaining(t0, t0 + Duration::from_secs(300)),
            None,
            "exactly at the minimum: Analyze must already be enabled, not Some(0)"
        );
        assert_eq!(
            min_capture_remaining(t0, t0 + Duration::from_secs(301)),
            None,
            "past the minimum: still enabled"
        );
    }

    /// Mirrors `chrona_dsp::cal::tests::recovers_injected_ppm_within_half_ppm`
    /// (`crates/chrona-dsp/src/cal.rs`) verbatim on parameters and tolerance:
    /// 320s @ 48kHz, 30dB SNR, seed 5, ppm_true = 50.0 ->
    /// `(ppm - 50.0).abs() <= 0.5`, `residual_ppm < 1.0`, `events >= 250`.
    /// The only difference is this test goes through a real WAV file on disk
    /// (written via `SessionWriter`, the same writer the engine's recording
    /// tee uses) and `analyze_wav` (this module's `SessionReader::open` +
    /// `calibrate_quartz` wrapper) instead of calling `calibrate_quartz` on
    /// the in-memory buffer directly — proving the wizard's own read path
    /// preserves cal.rs's proven accuracy end to end.
    #[test]
    fn analyze_wav_recovers_injected_ppm_like_cal_rs_own_test() {
        let ppm_true = 50.0;
        let sample_rate_hz = 48_000.0;
        let samples =
            synthesize_quartz(320.0, sample_rate_hz, ppm_true, 30.0, 5).expect("synthesize_quartz");

        let dir = tempfile::tempdir().unwrap();
        let meta = SessionMeta {
            schema_version: chrona_session::SIDECAR_SCHEMA_VERSION,
            device_name: "test".to_string(),
            sample_rate_hz,
            ppm_correction: 0.0,
            lift_angle_deg: 52.0,
            bph_mode: "auto".to_string(),
            position: None,
            started_unix_s: 1_700_000_000,
            app_version: "test".to_string(),
            watch: None,
            summary: None,
        };
        let mut writer = SessionWriter::create(dir.path(), meta).unwrap();
        writer.push(&samples).unwrap();
        let wav_path = writer.finalize().unwrap();

        let result = analyze_wav(&wav_path).unwrap_or_else(|e| panic!("{ppm_true}: {e}"));
        assert!(
            (result.ppm - ppm_true).abs() <= 0.5,
            "ppm {ppm_true}: got {}",
            result.ppm
        );
        assert!(
            result.residual_ppm < 1.0,
            "residual {}",
            result.residual_ppm
        );
        assert!(result.events >= 250, "events {}", result.events);
    }
}
