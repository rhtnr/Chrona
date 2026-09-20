//! Mic Doctor panel (M4 Task 10, binding spec §3.2): the guided in-app flow
//! around the Task 9 analysis core (`chrona_dsp::doctor`) — three short
//! checks (Silence 5s / Tick 10s / AGC 10s) against whatever source is
//! currently active, each Run button sending `ControlMsg::DoctorCapture`
//! and, once the engine delivers the buffer, handing it to the matching
//! `chrona_dsp::doctor` fn on a worker thread (mirrors `ui::cal_wizard`'s
//! own Capturing -> Analyzing handoff — see that module's doc comment).
//! Entry point is the toolbar's top-level "Mic Doctor" button, beside "Open
//! recording…"/"Export report" (`ui::toolbar`'s `mic_doctor_button`) — a
//! pre-review fix moved it out of the cal-ppm popup, since Calibrate is
//! genuinely a calibration setting but the Doctor is a distinct, first-class
//! flow (first-launch auto-run lands in M5) that deserves its own button.
//!
//! **Honesty (binding):** a step that hasn't run shows "not run", never a
//! guess — `DoctorPanel`'s three report fields are `Option`, and nothing
//! populates them except a completed capture's own analysis. Only ONE
//! capture may be in flight at a time (`DoctorPanel::pending`); the other
//! two steps' Run buttons disable while it runs. A source switch
//! invalidates whatever was measured against the OLD setup — `render_
//! doctor_panel` compares `EngineSnapshot::source_generation` every frame
//! (even while closed) and clears every report (plus abandons any pending
//! capture) the moment it changes, so reopening the panel after a
//! `SwitchSource` never shows a stale reading. The score + advice section
//! (`doctor_score`) renders only once at least one step has run, and scores
//! ONLY the reports that have (its `Option` params) — a step nobody ran
//! contributes no penalty and no advice.
//!
//! **Cancel (fix round 1, post-review):** a step's capture can never reach
//! `EngineSnapshot::doctor_capture` at all if the active source runs out
//! before the requested length — most concretely, a `Replay` source
//! (which doesn't loop, unlike `Simulate`) with less audio remaining than
//! the step needs. Before this fix, that left the panel stuck showing
//! "Capturing…" forever, with the other two Run buttons disabled, until a
//! `SwitchSource` (which clears `pending` as a side effect of the stale-
//! setup guard above) or an app restart — `cal_wizard` has exactly this
//! escape-hatch affordance on its own Capturing screen, which this panel
//! was missing. Every pending row now offers an instant Cancel (no confirm
//! dialog, unlike the wizard's multi-minute capture — these are 5–10s, so
//! an accidental cancel costs nothing) that clears `DoctorPanel::pending`.
//! The ENGINE has no matching "cancel" message: cancelling is purely a
//! panel-side decision to stop caring about a capture it already asked
//! for. The engine may still eventually deliver that buffer (or, on a live
//! source, a NEW request the panel sends afterward simply supersedes it —
//! "second replaces first", `ControlMsg::DoctorCapture`'s own doc
//! comment) — safety rests on `pending_capturing_step` (below): Phase 1
//! only ever consumes a `doctor_capture` arrival when `pending` is
//! `Some(Capturing{..})` for a matching step, so a cancelled request's
//! buffer, however late it arrives, is silently ignored — never
//! fabricated into a report.
//!
//! The pure helpers below (`silence_verdict`, `tick_verdict`, `agc_verdict`,
//! `doctor_capture_remaining`) are TDD'd first; the egui-facing rendering
//! that follows is exercised only by `cargo build` + the workspace test
//! suite (no window in CI) — same split as every other M4a modal
//! (`ui::cal_wizard`, `ui::modals`, `ui::toolbar`, `ui::strip`).

use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

use eframe::egui;

use chrona_dsp::Tier;
use chrona_dsp::doctor::{
    AgcReport, SilenceReport, TickReport, agc_report, doctor_score, silence_report, tick_report,
};

use crate::engine::{ControlMsg, Engine, EngineSnapshot};
use crate::theme::{self, Palette};

/// Step capture durations (task brief: "Silence 5 s / Tick 10 s / AGC
/// 10 s"). Independent captures per step — brief: "AGC reuses the tick
/// capture when run back-to-back? NO — keep independent captures, simpler
/// and honest."
const SILENCE_CAPTURE_S: f64 = 5.0;
const TICK_CAPTURE_S: f64 = 10.0;
const AGC_CAPTURE_S: f64 = 10.0;

// ---------------------------------------------------------------------
// Pure helpers — TDD'd in `tests` below.
// ---------------------------------------------------------------------

/// A step's Pass/Warn/Fail verdict — a UI presentation judgment call over a
/// `chrona_dsp::doctor` report, NOT the same thing as `doctor_score`'s own
/// scoring (see each verdict fn's doc comment: some thresholds happen to
/// numerically match `doctor_score`'s own constants, chosen independently,
/// never imported from it — `chrona_dsp::doctor::doctor_score`'s own doc
/// comment has ITS constants).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepVerdict {
    Pass,
    Warn,
    Fail,
}

/// Silence-step Pass ceiling: at/below this noise floor (dBFS), with no hum
/// detected, reads as a clean recording environment. Coincides with
/// `doctor_score`'s own `-50.0` noise-floor deduction threshold — chosen
/// independently as a UI presentation band, not imported from it.
const SILENCE_FLOOR_PASS_DBFS: f64 = -50.0;
/// Silence-step Fail gate: hum at/above this many dB over the local floor
/// reads as a real interference problem, not just a trace pickup.
/// Coincides with `doctor_score`'s own `20.0` hum-strength deduction
/// threshold — chosen independently as a UI presentation band, not
/// imported from it.
const SILENCE_HUM_FAIL_DB: f64 = 20.0;

/// Silence step (spec §3.2 step 1) verdict + display text. PURE.
///
/// - **Fail**: hum detected at ≥ `SILENCE_HUM_FAIL_DB` over the local
///   floor.
/// - **Warn**: floor above `SILENCE_FLOOR_PASS_DBFS`, OR hum detected but
///   under the Fail gate.
/// - **Pass**: floor at/below `SILENCE_FLOOR_PASS_DBFS` AND no hum
///   detected.
pub fn silence_verdict(report: &SilenceReport) -> (StepVerdict, String) {
    let mut text = format!("noise floor {:.1} dBFS", report.noise_floor_dbfs);
    let verdict = match &report.hum {
        Some(hum) => {
            text += &format!(", {:.0} Hz hum +{:.1} dB", hum.freq_hz, hum.strength_db);
            if hum.strength_db >= SILENCE_HUM_FAIL_DB {
                StepVerdict::Fail
            } else {
                StepVerdict::Warn
            }
        }
        None if report.noise_floor_dbfs <= SILENCE_FLOOR_PASS_DBFS => StepVerdict::Pass,
        None => StepVerdict::Warn,
    };
    (verdict, text)
}

/// Tick step (spec §3.2 step 2) verdict + display text. PURE.
///
/// - **Fail**: `tier` is `None` — the throwaway analyzer produced no
///   metrics at all (honestly represented as the ABSENCE of a tier, never
///   guessed at — see `TickReport::tier`'s own doc comment).
/// - **Warn**: `Some(Tier::T1)` — a defensible rate estimate, but no beat
///   error yet.
/// - **Pass**: `Some(Tier::T2)` or `Some(Tier::T3)`.
pub fn tick_verdict(report: &TickReport) -> (StepVerdict, String) {
    let (verdict, mut text): (StepVerdict, String) = match report.tier {
        None => (StepVerdict::Fail, "no signal".to_string()),
        Some(Tier::T1) => (StepVerdict::Warn, "Tier 1".to_string()),
        Some(Tier::T2) => (StepVerdict::Pass, "Tier 2".to_string()),
        Some(Tier::T3) => (StepVerdict::Pass, "Tier 3".to_string()),
    };
    if let Some(snr) = report.band_snr_db {
        text += &format!(", {snr:.1} dB SNR");
    }
    if report.clipped > 0 {
        text += &format!(", {} clipped samples", report.clipped);
    }
    (verdict, text)
}

/// AGC/gate step (spec §3.2 step 3) verdict + display text. PURE.
///
/// - **Fail**: `gate_db` is `Some` (floor never recovered — a suppressor).
/// - **Warn**: `agc_db` is `Some` (a recovering dip — AGC pumping), and no
///   gate.
/// - **Pass**: neither detected. (`agc_report` never returns both `Some`
///   simultaneously — see its own doc comment — but gate is checked first
///   regardless, defensively.)
pub fn agc_verdict(report: &AgcReport) -> (StepVerdict, String) {
    match (report.gate_db, report.agc_db) {
        (Some(g), _) => (
            StepVerdict::Fail,
            format!("gate/suppressor detected, {g:.1} dB floor collapse"),
        ),
        (None, Some(a)) => (
            StepVerdict::Warn,
            format!("AGC pumping detected, {a:.1} dB dip"),
        ),
        (None, None) => (
            StepVerdict::Pass,
            "no AGC or gate pumping detected".to_string(),
        ),
    }
}

/// A `Capturing` step's "live countdown vs the requested seconds" — `None`
/// once the requested span has elapsed (the row then shows a "finishing
/// up…" state instead of a countdown; the ACTUAL end is always the
/// engine's own `EngineSnapshot::doctor_capture` arrival, never this fn —
/// see `PendingCapture::Capturing`'s doc comment). PURE, same shape as
/// `ui::cal_wizard::min_capture_remaining`.
pub fn doctor_capture_remaining(started: Instant, seconds: f64, now: Instant) -> Option<Duration> {
    let total = Duration::from_secs_f64(seconds.max(0.0));
    let elapsed = now.saturating_duration_since(started);
    if elapsed >= total {
        None
    } else {
        Some(total - elapsed)
    }
}

// ---------------------------------------------------------------------
// State machine + rendering. Exercised by `cargo build` + the workspace
// test suite; no window in CI, so nothing below is unit-tested directly —
// see the module doc comment.
// ---------------------------------------------------------------------

/// Which Mic Doctor step (spec §3.2 steps 1–3) a `PendingCapture` is
/// running, and which of `DoctorPanel`'s three report fields a finished
/// `StepReport` belongs in. `Copy` — cheap to carry through render/apply
/// without an extra borrow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DoctorStep {
    Silence,
    Tick,
    Agc,
}

impl DoctorStep {
    fn label(self) -> &'static str {
        match self {
            DoctorStep::Silence => "Silence",
            DoctorStep::Tick => "Tick",
            DoctorStep::Agc => "AGC",
        }
    }

    fn capture_seconds(self) -> f64 {
        match self {
            DoctorStep::Silence => SILENCE_CAPTURE_S,
            DoctorStep::Tick => TICK_CAPTURE_S,
            DoctorStep::Agc => AGC_CAPTURE_S,
        }
    }
}

/// A finished worker-thread analysis, tagged by which step produced it —
/// the payload `PendingCapture::Analyzing`'s channel carries.
enum StepReport {
    Silence(SilenceReport),
    Tick(TickReport),
    Agc(AgcReport),
}

/// The one capture allowed in flight at a time (binding constraint: Run
/// buttons for the OTHER two steps disable while this is `Some`).
/// `Capturing` mirrors the engine's own accumulation with a live countdown
/// (`doctor_capture_remaining`, driven by `Instant::now()` each frame —
/// same `request_repaint_after` idiom `ui::cal_wizard::render_capturing`
/// uses) — the countdown reaching zero is only a DISPLAY hint ("finishing
/// up…"), never what actually ends this state; only the engine's own
/// `EngineSnapshot::doctor_capture` arrival does. `Analyzing` is the
/// worker-thread handoff, polled via `try_recv` — same one-shot-thread-
/// plus-mpsc-channel shape as `ui::cal_wizard`'s own `CalWizard::
/// Analyzing`.
enum PendingCapture {
    Capturing {
        step: DoctorStep,
        started: Instant,
        seconds: f64,
        /// The `ControlMsg::DoctorCapture::id` this request was sent with —
        /// Phase 1 adopts only a snapshot buffer echoing this exact id (see
        /// `pending_capturing_step`), which is what makes the engine's
        /// LATCHED delivery safe: a still-latched buffer from a cancelled
        /// or older request can never be misattributed to this step.
        id: u64,
    },
    Analyzing {
        step: DoctorStep,
        rx: Receiver<StepReport>,
    },
}

/// The step a `doctor_capture` arrival tagged `buffer_id` should be handed
/// off to this frame, if any — `None` whenever `pending` isn't `Some(
/// Capturing{..})` OR the ids don't match. The state cases cover: nothing
/// pending at all, a step already past capturing and into `Analyzing`, and
/// — the case fix round 1 (post-review) adds Cancel for — right after a
/// Cancel cleared `pending` back to `None`. The ID case closes what the
/// final review called the cancel-then-run-other misattribution window:
/// with the engine's LATCHED delivery, a buffer from a cancelled/older
/// request can still be sitting on every snapshot when a NEW step starts
/// capturing — the id mismatch keeps this fn returning `None` for it, so
/// the new step only ever adopts its own audio.
///
/// This is the ENTIRE safety argument for Cancel being a purely panel-side
/// decision with no matching engine-side "cancel" message: a cancelled
/// request's buffer, however long the engine latches it, arrives to find
/// this fn returning `None` and is silently ignored by
/// `render_doctor_panel`'s Phase 1 — never fabricated into a report for a
/// step nobody is waiting on. PURE, tested in `tests` below.
fn pending_capturing_step(pending: &Option<PendingCapture>, buffer_id: u64) -> Option<DoctorStep> {
    match pending {
        Some(PendingCapture::Capturing { step, id, .. }) if *id == buffer_id => Some(*step),
        _ => None,
    }
}

/// The Mic Doctor panel's own state: whether it's currently shown, each
/// step's completed report (`None` = "not run" — honesty gate), and the one
/// pending capture, if any. Kept as a plain `ChronaApp` field (not
/// `Option<DoctorPanel>`): unlike `CalWizard`, closing this panel discards
/// nothing (a capture keeps running via the engine regardless of whether
/// this is being rendered this frame — see `render_doctor_panel`'s doc
/// comment) and step results are deliberately NOT cleared on close, so
/// reopening later shows what was already measured instead of forcing a
/// re-run.
#[derive(Default)]
pub struct DoctorPanel {
    open: bool,
    silence: Option<SilenceReport>,
    tick: Option<TickReport>,
    agc: Option<AgcReport>,
    pending: Option<PendingCapture>,
    /// The `EngineSnapshot::source_generation` last observed — see the
    /// module doc comment's honesty "stale setup" rule.
    last_source_generation: u64,
    /// Monotonic counter for `ControlMsg::DoctorCapture::id` — incremented
    /// before each Run so every request this panel ever sends carries a
    /// distinct id (`Default` starts at 0; the first request is 1).
    next_capture_id: u64,
}

impl DoctorPanel {
    /// Opens the panel (toolbar's top-level "Mic Doctor" button —
    /// `ChronaApp::ui` consumes `ToolbarState::doctor_requested` the same
    /// "set on click, consumed next frame" way it consumes `cal_wizard_
    /// requested`).
    pub fn open(&mut self) {
        self.open = true;
    }
}

/// Borrowed, per-frame context `render_doctor_panel` needs beyond the
/// panel's own owned state — same per-frame-context bundling shape as
/// `CalWizardCtx`/`ToolbarCtx`/`StripCtx`.
pub struct DoctorCtx<'a> {
    pub engine: &'a Engine,
    pub snap: &'a EngineSnapshot,
}

/// What a screen's buttons (or Esc/click-outside) decided this frame —
/// same shape as `ui::cal_wizard`'s own `WizardAction`.
enum DoctorAction {
    None,
    Run(DoctorStep),
    /// Fix round 1 (post-review): abandon whatever's pending (`Capturing`
    /// or `Analyzing`, either — no need to distinguish, since only the
    /// pending step's own row ever shows a Cancel button) — see the module
    /// doc comment's "Cancel" section.
    Cancel,
    Close,
}

/// Renders the Mic Doctor panel. Phases 1/2 (engine polling / worker-thread
/// handoff) run UNCONDITIONALLY, even while `panel.open` is `false` — a
/// capture already requested keeps progressing via the engine regardless of
/// whether the panel is on screen this frame (there is nothing to "cancel":
/// no file, no OS resource, just an in-memory buffer the engine accumulates
/// either way), so this never pauses/loses a result just because the modal
/// was closed. Phase 3 (the actual overlay/card) only runs when `panel.
/// open`.
///
/// The honesty "stale setup" guard (module doc comment) also runs
/// unconditionally, for the same reason: a `SwitchSource` while the panel
/// is closed must still be reflected the next time it opens.
pub fn render_doctor_panel(
    ctx: &egui::Context,
    palette: &Palette,
    panel: &mut DoctorPanel,
    dctx: DoctorCtx<'_>,
) {
    if panel.last_source_generation != dctx.snap.source_generation {
        panel.last_source_generation = dctx.snap.source_generation;
        panel.silence = None;
        panel.tick = None;
        panel.agc = None;
        panel.pending = None;
    }

    // Phase 1: a Capturing step whose buffer just arrived hands off to a
    // worker thread running the matching `chrona_dsp::doctor` fn — mirrors
    // `ui::cal_wizard`'s own Capturing -> Analyzing handoff. Also the
    // "ignore a cancelled request's late buffer" guard — see `pending_
    // capturing_step`'s doc comment.
    if let Some((buffer_id, samples, sample_rate_hz)) = dctx.snap.doctor_capture.clone()
        && let Some(step) = pending_capturing_step(&panel.pending, buffer_id)
    {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let report = match step {
                DoctorStep::Silence => {
                    StepReport::Silence(silence_report(&samples, sample_rate_hz))
                }
                DoctorStep::Tick => StepReport::Tick(tick_report(&samples, sample_rate_hz)),
                DoctorStep::Agc => StepReport::Agc(agc_report(&samples, sample_rate_hz)),
            };
            let _ = tx.send(report);
        });
        panel.pending = Some(PendingCapture::Analyzing { step, rx });
    }

    // Phase 2: poll the analysis thread.
    let mut finished: Option<StepReport> = None;
    let mut abandon = false;
    if let Some(PendingCapture::Analyzing { rx, .. }) = &panel.pending {
        match rx.try_recv() {
            Ok(report) => finished = Some(report),
            Err(TryRecvError::Empty) => {}
            // Never fabricate a report — abandon back to "not run" instead.
            Err(TryRecvError::Disconnected) => abandon = true,
        }
    }
    if let Some(report) = finished {
        match report {
            StepReport::Silence(r) => panel.silence = Some(r),
            StepReport::Tick(r) => panel.tick = Some(r),
            StepReport::Agc(r) => panel.agc = Some(r),
        }
        panel.pending = None;
    } else if abandon {
        panel.pending = None;
    }

    if !panel.open {
        return;
    }

    let screen = ctx.input(|i| i.content_rect());
    let scrim_layer = egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("chrona_doctor_scrim"),
    );
    ctx.layer_painter(scrim_layer).rect_filled(
        screen,
        0.0,
        theme::with_alpha(palette.overlay, 140),
    );

    let now = Instant::now();
    let mut action = DoctorAction::None;
    let card_response = egui::Area::new(egui::Id::new("chrona_doctor_card"))
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
                    action = render_doctor_card(ui, palette, panel, now);
                });
        })
        .response;

    let clicked_outside = ctx.input(|i| i.pointer.any_click())
        && ctx
            .input(|i| i.pointer.interact_pos())
            .is_some_and(|pos| !card_response.rect.contains(pos));
    let esc = ctx.input(|i| i.key_pressed(egui::Key::Escape));
    if matches!(action, DoctorAction::None) && (clicked_outside || esc) {
        action = DoctorAction::Close;
    }

    apply_doctor_action(action, panel, dctx.engine);

    // Keep the countdown/spinner advancing without user input — same
    // repaint-while-active idiom as `ui::cal_wizard::render_capturing`/
    // `render_analyzing`.
    if panel.pending.is_some() {
        ctx.request_repaint_after(Duration::from_millis(100));
    }
}

fn apply_doctor_action(action: DoctorAction, panel: &mut DoctorPanel, engine: &Engine) {
    match action {
        DoctorAction::None => {}
        DoctorAction::Run(step) => {
            // Defensive: the Run button is already disabled whenever a
            // capture is pending (see `render_step_row`) — this just
            // refuses to double-start if that's ever bypassed.
            if panel.pending.is_none() {
                let seconds = step.capture_seconds();
                panel.next_capture_id += 1;
                let id = panel.next_capture_id;
                engine.send(ControlMsg::DoctorCapture { seconds, id });
                panel.pending = Some(PendingCapture::Capturing {
                    step,
                    started: Instant::now(),
                    seconds,
                    id,
                });
            }
        }
        // No message to the engine — see the module doc comment's
        // "Cancel" section for why clearing `pending` here is the whole
        // fix, and `pending_capturing_step`'s doc comment for why a late
        // buffer from the abandoned request can never resurface as a
        // fabricated report.
        DoctorAction::Cancel => panel.pending = None,
        DoctorAction::Close => panel.open = false,
    }
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

fn outline_button(ui: &mut egui::Ui, palette: &Palette, text: &str) -> egui::Response {
    ui.add(
        egui::Button::new(egui::RichText::new(text).color(palette.text))
            .fill(egui::Color32::TRANSPARENT)
            .stroke(egui::Stroke::new(1.0, palette.border2)),
    )
}

/// Rounds a `Duration` UP to the nearest whole second — same "never show
/// 0s while still waiting" idiom as `ui::cal_wizard::ceil_secs`.
fn ceil_secs(d: Duration) -> u64 {
    d.as_secs() + u64::from(d.subsec_nanos() > 0)
}

fn verdict_label(v: StepVerdict) -> &'static str {
    match v {
        StepVerdict::Pass => "Pass",
        StepVerdict::Warn => "Warn",
        StepVerdict::Fail => "Fail",
    }
}

fn verdict_color(palette: &Palette, v: StepVerdict) -> egui::Color32 {
    match v {
        StepVerdict::Pass => palette.good,
        StepVerdict::Warn => palette.warnfg,
        StepVerdict::Fail => palette.rec,
    }
}

fn render_doctor_card(
    ui: &mut egui::Ui,
    palette: &Palette,
    panel: &DoctorPanel,
    now: Instant,
) -> DoctorAction {
    let mut action = DoctorAction::None;
    heading(ui, palette, "Mic Doctor");
    ui.add_space(6.0);
    ui.label(
        egui::RichText::new(
            "Three short checks against your current input \u{2014} run them whenever you \
             change your setup, before trusting a new reading.",
        )
        .size(13.0)
        .color(palette.muted),
    );
    ui.add_space(12.0);

    let steps = [DoctorStep::Silence, DoctorStep::Tick, DoctorStep::Agc];
    for step in steps {
        let result: Option<(StepVerdict, String)> = match step {
            DoctorStep::Silence => panel.silence.as_ref().map(silence_verdict),
            DoctorStep::Tick => panel.tick.as_ref().map(tick_verdict),
            DoctorStep::Agc => panel.agc.as_ref().map(agc_verdict),
        };
        let row_action = render_step_row(ui, palette, step, result, &panel.pending, now);
        if !matches!(row_action, DoctorAction::None) {
            action = row_action;
        }
        ui.add_space(8.0);
    }

    if panel.silence.is_some() || panel.tick.is_some() || panel.agc.is_some() {
        ui.separator();
        ui.add_space(8.0);
        render_score_section(ui, palette, panel);
    }

    ui.add_space(14.0);
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        if outline_button(ui, palette, "Close").clicked() {
            action = DoctorAction::Close;
        }
    });
    action
}

/// One step row: label + duration, a Run/Re-run button (or the live
/// progress state while this step is the pending one, or a disabled Run
/// while a DIFFERENT step is pending), and the result line — a Pass/Warn/
/// Fail badge + text once a report exists, or "not run" otherwise.
fn render_step_row(
    ui: &mut egui::Ui,
    palette: &Palette,
    step: DoctorStep,
    result: Option<(StepVerdict, String)>,
    pending: &Option<PendingCapture>,
    now: Instant,
) -> DoctorAction {
    let mut action = DoctorAction::None;

    // `true` while THIS row's step is the one currently accumulating
    // ("Capturing"); `false` while it's being analyzed on the worker
    // thread (`Analyzing` — shown as its own spinner state below) or not
    // pending at all.
    let capturing = match pending {
        Some(PendingCapture::Capturing {
            step: s,
            started,
            seconds,
            ..
        }) if *s == step => Some((*started, *seconds)),
        _ => None,
    };
    let analyzing =
        matches!(pending, Some(PendingCapture::Analyzing { step: s, .. }) if *s == step);
    let any_pending = pending.is_some();

    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(format!("{} ({:.0}s)", step.label(), step.capture_seconds()))
                .color(palette.text)
                .strong(),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if analyzing {
                // Call order [Cancel, Spinner, label] -> visual left-to-
                // right "Analyzing… [spinner] [Cancel]" (right_to_left
                // stacks first-call=rightmost).
                if outline_button(ui, palette, "Cancel").clicked() {
                    action = DoctorAction::Cancel;
                }
                ui.add(egui::Spinner::new().color(palette.accent));
                ui.label(
                    egui::RichText::new("Analyzing\u{2026}")
                        .size(12.0)
                        .color(palette.muted),
                );
            } else if let Some((started, seconds)) = capturing {
                // Fix round 1 (post-review): Cancel is the escape hatch
                // for a source that runs out before this step's requested
                // length — see the module doc comment's "Cancel" section.
                if outline_button(ui, palette, "Cancel").clicked() {
                    action = DoctorAction::Cancel;
                }
                let label = match doctor_capture_remaining(started, seconds, now) {
                    Some(r) => format!("Capturing\u{2026} {}s", ceil_secs(r)),
                    None => "Finishing up\u{2026}".to_string(),
                };
                ui.label(egui::RichText::new(label).size(12.0).color(palette.muted));
            } else {
                let label = if result.is_some() { "Re-run" } else { "Run" };
                ui.add_enabled_ui(!any_pending, |ui| {
                    let resp = ui.add(
                        egui::Button::new(
                            egui::RichText::new(label)
                                .color(palette.accent_ink)
                                .strong(),
                        )
                        .fill(palette.accent),
                    );
                    let resp = if any_pending {
                        resp.on_disabled_hover_text("Another check is already running")
                    } else {
                        resp
                    };
                    if resp.clicked() {
                        action = DoctorAction::Run(step);
                    }
                });
            }
        });
    });

    match &result {
        Some((verdict, text)) => {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(verdict_label(*verdict))
                        .strong()
                        .color(verdict_color(palette, *verdict)),
                );
                ui.label(egui::RichText::new(text).size(12.0).color(palette.muted));
            });
        }
        None if capturing.is_none() && !analyzing => {
            ui.label(
                egui::RichText::new("not run")
                    .size(12.0)
                    .color(palette.faint),
            );
        }
        None => {} // currently capturing/analyzing — no stale "not run" line
    }

    // Fix round 1 (post-review): honest, no-timeout-magic note about the
    // one way a capture can hang forever on its own — shown only while
    // actively `Capturing` (not `Analyzing`, which runs a fast in-memory
    // fn and isn't at risk of this). No attempt to detect or predict
    // whether THIS capture is doomed (that would need engine-side
    // signaling this task doesn't add) — just the plain fact plus the
    // escape hatch, always available while waiting.
    if capturing.is_some() {
        ui.label(
            egui::RichText::new(
                "A source with less audio remaining than this step needs will never \
                 finish on its own \u{2014} Cancel, then try a different source.",
            )
            .size(11.0)
            .color(palette.faint),
        );
    }

    action
}

/// Score + advice section (module doc comment: renders once ≥1 step has
/// run, over ONLY the reports that have — `doctor_score`'s own `Option`
/// params). Advice strings are rendered VERBATIM (spec pins) — only a
/// bullet prefix is added, never a rewrap/edit of the text itself.
fn render_score_section(ui: &mut egui::Ui, palette: &Palette, panel: &DoctorPanel) {
    let ds = doctor_score(
        panel.silence.as_ref(),
        panel.tick.as_ref(),
        panel.agc.as_ref(),
    );
    ui.label(
        egui::RichText::new(format!("Setup score: {}/100", ds.score))
            .font(egui::FontId::new(
                15.0,
                egui::FontFamily::Name(theme::FAMILY_SEMIBOLD.into()),
            ))
            .color(palette.text),
    );
    if ds.advice.is_empty() {
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new("No issues found in what you've checked so far.")
                .size(12.0)
                .color(palette.muted),
        );
    } else {
        for line in &ds.advice {
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(format!("\u{2022} {line}"))
                    .size(12.0)
                    .color(palette.text),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hum(freq_hz: f64, strength_db: f64) -> chrona_dsp::doctor::HumReport {
        chrona_dsp::doctor::HumReport {
            freq_hz,
            strength_db,
        }
    }

    #[test]
    fn silence_verdict_table() {
        let cases: [(SilenceReport, StepVerdict, &str); 6] = [
            (
                SilenceReport {
                    noise_floor_dbfs: -60.0,
                    hum: None,
                },
                StepVerdict::Pass,
                "noise floor -60.0 dBFS",
            ),
            (
                SilenceReport {
                    noise_floor_dbfs: -50.0,
                    hum: None,
                },
                StepVerdict::Pass,
                "noise floor -50.0 dBFS",
            ),
            (
                SilenceReport {
                    noise_floor_dbfs: -40.0,
                    hum: None,
                },
                StepVerdict::Warn,
                "noise floor -40.0 dBFS",
            ),
            (
                SilenceReport {
                    noise_floor_dbfs: -60.0,
                    hum: Some(hum(60.0, 15.0)),
                },
                StepVerdict::Warn,
                "noise floor -60.0 dBFS, 60 Hz hum +15.0 dB",
            ),
            (
                SilenceReport {
                    noise_floor_dbfs: -60.0,
                    hum: Some(hum(50.0, 20.0)),
                },
                StepVerdict::Fail,
                "noise floor -60.0 dBFS, 50 Hz hum +20.0 dB",
            ),
            (
                SilenceReport {
                    noise_floor_dbfs: -60.0,
                    hum: Some(hum(50.0, 25.0)),
                },
                StepVerdict::Fail,
                "noise floor -60.0 dBFS, 50 Hz hum +25.0 dB",
            ),
        ];
        for (report, want_verdict, want_text) in cases {
            let (verdict, text) = silence_verdict(&report);
            assert_eq!(verdict, want_verdict, "report {report:?}");
            assert_eq!(text, want_text, "report {report:?}");
        }
    }

    #[test]
    fn tick_verdict_table() {
        let cases: [(TickReport, StepVerdict, &str); 4] = [
            (
                TickReport {
                    tier: None,
                    band_snr_db: None,
                    clipped: 0,
                },
                StepVerdict::Fail,
                "no signal",
            ),
            (
                TickReport {
                    tier: Some(Tier::T1),
                    band_snr_db: Some(8.2),
                    clipped: 0,
                },
                StepVerdict::Warn,
                "Tier 1, 8.2 dB SNR",
            ),
            (
                TickReport {
                    tier: Some(Tier::T2),
                    band_snr_db: Some(15.0),
                    clipped: 3,
                },
                StepVerdict::Pass,
                "Tier 2, 15.0 dB SNR, 3 clipped samples",
            ),
            (
                TickReport {
                    tier: Some(Tier::T3),
                    band_snr_db: None,
                    clipped: 0,
                },
                StepVerdict::Pass,
                "Tier 3",
            ),
        ];
        for (report, want_verdict, want_text) in cases {
            let (verdict, text) = tick_verdict(&report);
            assert_eq!(verdict, want_verdict, "report {report:?}");
            assert_eq!(text, want_text, "report {report:?}");
        }
    }

    #[test]
    fn agc_verdict_table() {
        let cases: [(AgcReport, StepVerdict, &str); 4] = [
            (
                AgcReport {
                    agc_db: None,
                    gate_db: None,
                },
                StepVerdict::Pass,
                "no AGC or gate pumping detected",
            ),
            (
                AgcReport {
                    agc_db: Some(-4.4),
                    gate_db: None,
                },
                StepVerdict::Warn,
                "AGC pumping detected, -4.4 dB dip",
            ),
            (
                AgcReport {
                    agc_db: None,
                    gate_db: Some(-25.0),
                },
                StepVerdict::Fail,
                "gate/suppressor detected, -25.0 dB floor collapse",
            ),
            (
                // Defensive-only combination — `agc_report` never actually
                // returns both `Some` (see its own doc comment); gate must
                // still win if it ever did.
                AgcReport {
                    agc_db: Some(-4.4),
                    gate_db: Some(-25.0),
                },
                StepVerdict::Fail,
                "gate/suppressor detected, -25.0 dB floor collapse",
            ),
        ];
        for (report, want_verdict, want_text) in cases {
            let (verdict, text) = agc_verdict(&report);
            assert_eq!(verdict, want_verdict, "report {report:?}");
            assert_eq!(text, want_text, "report {report:?}");
        }
    }

    #[test]
    fn doctor_capture_remaining_math_and_boundary() {
        let t0 = Instant::now();
        assert_eq!(
            doctor_capture_remaining(t0, 10.0, t0),
            Some(Duration::from_secs(10)),
            "no time elapsed: full 10s remaining"
        );
        assert_eq!(
            doctor_capture_remaining(t0, 10.0, t0 + Duration::from_secs(9)),
            Some(Duration::from_secs(1)),
            "1s short of the requested span"
        );
        assert_eq!(
            doctor_capture_remaining(t0, 10.0, t0 + Duration::from_millis(9_999)),
            Some(Duration::from_millis(1)),
            "1ms short of the requested span"
        );
        assert_eq!(
            doctor_capture_remaining(t0, 10.0, t0 + Duration::from_secs(10)),
            None,
            "exactly at the requested span: must already be None, not Some(0)"
        );
        assert_eq!(
            doctor_capture_remaining(t0, 10.0, t0 + Duration::from_secs(11)),
            None,
            "past the requested span: still None"
        );
    }

    /// Fix round 1 (post-review): the entire safety argument for Cancel
    /// having no matching engine-side message — a late `doctor_capture`
    /// arrival is only ever consumed by `render_doctor_panel`'s Phase 1
    /// when this fn returns `Some`.
    #[test]
    fn pending_capturing_step_ignores_everything_but_a_matching_capturing_step() {
        assert_eq!(
            pending_capturing_step(&None, 7),
            None,
            "nothing pending (including right after a Cancel): ignore"
        );
        let capturing = Some(PendingCapture::Capturing {
            step: DoctorStep::Tick,
            started: Instant::now(),
            seconds: 10.0,
            id: 7,
        });
        assert_eq!(
            pending_capturing_step(&capturing, 7),
            Some(DoctorStep::Tick),
            "actively capturing Tick with the MATCHING id: consume for Tick"
        );
        assert_eq!(
            pending_capturing_step(&capturing, 6),
            None,
            "a still-latched buffer from an OLDER request (id mismatch): ignore — \
             the cancel-then-run-other misattribution guard"
        );
        let (_tx, rx) = mpsc::channel();
        assert_eq!(
            pending_capturing_step(
                &Some(PendingCapture::Analyzing {
                    step: DoctorStep::Agc,
                    rx,
                }),
                7
            ),
            None,
            "already past Capturing into Analyzing: ignore (nothing left to hand off)"
        );
    }
}
