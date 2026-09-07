//! Engine thread: DSP pipeline, capture/simulate/replay sources, and
//! snapshot publishing (spec §7/§9).
//!
//! One `std::thread` owns everything DSP-adjacent: the active source (mic
//! consumer / simulate generator / replay data — or, when every way of
//! starting one has fatally failed, an idle placeholder that keeps the
//! thread alive for a later recovery; see `SourceRuntime::Idle`), the
//! `Analyzer`, an optional recording `SessionWriter`, and the input side of
//! a `triple_buffer::TripleBuffer<EngineSnapshot>`. The UI thread never
//! touches the `Analyzer` directly — see [`Engine::snapshot`] for why that
//! matters. Control flows in one direction, UI → engine, over an
//! `std::sync::mpsc` channel; state flows the other way, engine → UI, over
//! the triple buffer, at ~10 Hz.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use chrona_audio::{CaptureStream, StreamInfo};
use chrona_dsp::synth::{SynthConfig, synthesize};
use chrona_dsp::{
    Analyzer, AnalyzerConfig, AnalyzerError, BeatScope, BphMode, MetricsSnapshot, TapeEvent, Tier,
};
use chrona_session::{
    SIDECAR_SCHEMA_VERSION, SessionMeta, SessionReader, SessionSummary, SessionWriter,
    sanitize_lift, sanitize_ppm,
};
use eframe::egui;

use crate::AppFlags;
use crate::presenter::{format_amplitude, format_beat_error, format_rate, tier_label};

/// Thread-body tick length (spec: "loop every ~50 ms"; `recv_timeout` on the
/// control channel doubles as the tick). Also the Simulate source's
/// per-tick chunk length.
const TICK_S: f64 = 0.05;
/// Publish a snapshot (and request a repaint) every *2nd* tick: at the
/// ~50 ms tick cadence that's ~10 Hz, matching the spec.
const SNAPSHOT_EVERY: u32 = 2;
/// Silence gate for `HealthView::silent_for_s`: RMS below this over the
/// trailing window (see `SilenceTracker`) counts as silence.
const SILENCE_RMS: f64 = 1e-5;
/// Trailing window `SilenceTracker` measures RMS over. A single ~50 ms
/// chunk's own RMS is too spiky to threshold directly — a ticking watch's
/// drop pulses are loud but brief, separated by mostly-quiet gaps, so a
/// single-chunk threshold would "reset" on every tick and never accumulate
/// real silence time.
const SILENCE_WINDOW_S: f64 = 3.0;
/// Simulate source: pre-synthesize this many seconds once at source build
/// and loop it (brief: "simpler and adequate"). The loop seam is a single
/// discontinuous sample once per lap; at 60 s/lap that's a one-beat glitch
/// far too infrequent to move the tiered quality gates, and inaudible to
/// the analyzer's averaging window.
const SIM_LOOP_S: f64 = 60.0;
/// Replay pushes the recorded file in 1 s chunks per tick (spec) rather
/// than at real-time (50 ms/tick) pace — a deliberate fast-forward so
/// reviewing a session doesn't take as long as recording it did.
const REPLAY_CHUNK_S: f64 = 1.0;
/// Mic source ring-buffer depth: headroom against a stalled tick (e.g. a
/// slow control-message batch, or the OS briefly starving this thread)
/// before `chrona_audio` starts counting overruns.
const MIC_RING_S: f64 = 5.0;
/// `SourceRuntime::Idle`'s fixed sample rate (spec/M4 T3). Any real
/// negotiated rate would do — this is never fed samples — but 48 kHz
/// matches `CaptureStream`'s own first-choice negotiation target
/// (`chrona_audio::capture::negotiate`) and comfortably clears every
/// DSP-side Nyquist requirement (spec's 3 kHz high-pass among them), so an
/// `Analyzer` can always be built against it regardless of *why* the engine
/// ended up idle.
const IDLE_SAMPLE_RATE_HZ: f64 = 48_000.0;
/// System-clock skew cross-check (binding spec §3.4): sample `SkewTracker`
/// at this cadence, on the mic path only.
const SKEW_SAMPLE_INTERVAL_S: f64 = 1.0;
/// Minimum regression span `SkewTracker::skew` requires before it publishes
/// anything (binding spec §3.4 — a short window's slope estimate is too
/// noisy to trust, let alone show as a one-click "correction"). Display-
/// plus-explicit-adopt only, never silently applied — see `SkewTracker`'s
/// doc comment.
const SKEW_MIN_SPAN_S: f64 = 120.0;
/// `ControlMsg::DoctorCapture`'s clamp range (M4 Task 10, defensive): the
/// Mic Doctor panel only ever sends its own fixed 5/10/10s step durations,
/// so neither bound is expected to bite in practice — this exists purely
/// so a malformed/future caller can't request a near-zero (dishonest,
/// under-evidenced) or unbounded (memory-blowup) capture.
const MIN_DOCTOR_CAPTURE_S: f64 = 1.0;
const MAX_DOCTOR_CAPTURE_S: f64 = 30.0;

// ---------------------------------------------------------------------
// Public control-plane types (spec §7): what the UI sends the engine, and
// what the engine publishes back.
// ---------------------------------------------------------------------

/// Which input source the engine should run from. `SwitchSource` carries
/// one of these to hot-swap sources at runtime.
#[derive(Debug, Clone)]
pub enum SourceSpec {
    Mic {
        device_id: Option<String>,
    },
    Simulate {
        rate: f64,
        beat_error: f64,
        amplitude: f64,
        snr: f64,
    },
    ReplayFile {
        path: PathBuf,
    },
}

/// UI → engine control messages, drained at the top of every tick.
#[derive(Debug, Clone)]
pub enum ControlMsg {
    SetLift(f64),
    SetAveraging(f64),
    SetBphMode(BphMode),
    SetPpm(f64),
    SwitchSource(SourceSpec),
    StartRecording {
        dir: PathBuf,
        meta_position: Option<String>,
        /// The watch under test (M4a: watch list), stored verbatim into the
        /// sidecar's `SessionMeta::watch`.
        watch: Option<String>,
    },
    StopRecording,
    /// M4 Task 10 (Mic Doctor, binding spec §3.2): buffer the next `seconds`
    /// (clamped to `[MIN_DOCTOR_CAPTURE_S, MAX_DOCTOR_CAPTURE_S]`) of PUSHED
    /// samples from whatever source is currently active — Mic, Simulate, or
    /// Replay alike (`pull_tick` tees every source arm into it the same way
    /// it already tees into a recording `SessionWriter`). `Idle` pulls no
    /// samples at all, so a request against it simply never completes — an
    /// honest "no data yet", never a fabricated one.
    ///
    /// One-shot: the completed buffer is attached to EXACTLY the next
    /// published `EngineSnapshot::doctor_capture`, then cleared back to
    /// `None` on every publish after that (`engine_loop`'s `doctor_ready.
    /// take()`). Overlapping requests: a second `DoctorCapture` arriving
    /// before the first has completed (or been delivered) discards whatever
    /// the first had accumulated/finished and restarts against its own
    /// `seconds` — see `engine_loop`'s handler, the one place this is
    /// applied.
    DoctorCapture {
        seconds: f64,
    },
    /// M4 Task 11 (spec §6): toggles the "Scope" view. `true` makes every
    /// subsequent publish call `Analyzer::beat_scope` once per tick and
    /// attach its result to `EngineSnapshot::scope`; `false` reverts that
    /// field to `None`. No analyzer rebuild — purely a per-tick gate the UI
    /// flips when it opens/closes the Scope panel (Task 12).
    SetScope(bool),
    Shutdown,
}

/// The `Analyzer` knobs the engine owns. Any change rebuilds the `Analyzer`
/// from scratch (`Analyzer::new` — period/state reset; acceptable per spec
/// §6, the UI shows the reset naturally). This includes `ppm_correction`:
/// hot-swapping ppm without a full reset is possible in principle (it's a
/// linear correction applied post-hoc to period estimates) but that's an M4
/// nicety, not an M3 requirement — every config change rebuilds alike here.
#[derive(Debug, Clone, Copy)]
pub struct EngineConfig {
    pub lift_angle_deg: f64,
    pub averaging_s: f64,
    pub bph_mode: BphMode,
    pub ppm_correction: f64,
}

/// Which kind of source produced the latest `EngineSnapshot`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SourceKind {
    #[default]
    Simulate,
    Mic,
    Replay,
}

/// One system-clock skew reading (binding spec §3.4's NTP cross-check):
/// `SkewTracker`'s fitted regression slope, in parts per million, and how
/// much wall-clock span backs it. Display-plus-explicit-adopt only — see
/// `SkewTracker`'s doc comment for the full contract this never violates on
/// its own.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClockSkew {
    pub ppm: f64,
    pub span_s: f64,
}

/// Live health counters, folded into every published snapshot.
#[derive(Debug, Clone, Default)]
pub struct HealthView {
    pub overruns: u64,
    pub clipped: u64,
    pub silent_for_s: f64,
    pub last_error: Option<String>,
    /// The current system-clock skew cross-check reading (binding spec
    /// §3.4), or `None` before enough mic-path span has accumulated (see
    /// `SkewTracker`) — and always `None` on Simulate/Replay/Idle, whose
    /// delivery isn't real audio hardware to cross-check against.
    pub clock_skew: Option<ClockSkew>,
}

/// How urgently a [`Banner`] should read (spec §4). `Info` for notices that
/// don't need alarm styling (e.g. "recording stopped: source changed",
/// replay's "using recorded settings…"), `Warn` for recoverable/degraded-
/// but-continuing conditions (`pick_banner`'s silence/clipping/overruns
/// notices, plus "saved input device unavailable … using default input"),
/// `Error` for genuine failures/faults (every `failed to *` / `invalid *`
/// string, the thread-panic banner, and `pick_banner`'s mic-capture-error
/// banner — a dead/failing stream is a fault, not a transient notice).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BannerSeverity {
    Info,
    Warn,
    Error,
}

/// A ready-to-render banner: severity plus its already-formatted text.
#[derive(Debug, Clone, PartialEq)]
pub struct Banner {
    pub severity: BannerSeverity,
    pub text: String,
}

/// Everything the UI thread needs to render one frame, published by the
/// engine thread over a `triple_buffer` at ~10 Hz. Cheap to clone (small
/// `Copy` metrics plus a tape `Vec` that's bounded to ~32 s of events by
/// the analyzer itself).
#[derive(Debug, Clone, Default)]
pub struct EngineSnapshot {
    pub metrics: Option<MetricsSnapshot>,
    pub tape: Vec<TapeEvent>,
    pub stream: Option<StreamInfo>,
    pub source_kind: SourceKind,
    pub replay_done: bool,
    pub health: HealthView,
    pub recording: Option<PathBuf>,
    pub banner: Option<Banner>,
    /// M4 Task 10 (Mic Doctor): the completed `ControlMsg::DoctorCapture`
    /// buffer, `(samples, sample_rate_hz)` — bundled as one tuple rather
    /// than a parallel `doctor_capture_sr` field so the two can never
    /// desync (e.g. a stale sample rate surviving next to a `None`
    /// buffer). `Arc`, not a plain `Vec`, so cloning a snapshot (every
    /// `publish`, and `Engine::snapshot()`'s own `.clone()`) shares the one
    /// allocation instead of deep-copying up to 30s of audio. `Some` on
    /// EXACTLY the one snapshot published right after the requested length
    /// was reached, `None` on every other — see `ControlMsg::
    /// DoctorCapture`'s doc comment for the full one-shot contract. Relies
    /// on `publish`'s existing `request_repaint()` (fired on every publish,
    /// unconditionally) to give the UI a real chance to observe that one
    /// snapshot rather than skip over it.
    pub doctor_capture: Option<(Arc<Vec<f32>>, f64)>,
    /// M4 Task 11 (spec §6, "Scope" view): the newest ≤16 per-beat waveform
    /// windows, refreshed on every publish while the view is open —
    /// `Some` (possibly empty, e.g. before the analyzer has ever folded)
    /// whenever `ControlMsg::SetScope(true)` is the last one received,
    /// `None` otherwise (unlike `doctor_capture`, this is NOT one-shot: it
    /// stays populated for as long as the view is on). Computed by
    /// `Analyzer::beat_scope`, called once per publish tick only while on
    /// — cheap (≤16 windows of ~300 native-rate `f32` samples each). At
    /// ~16×300×4 B ≈ 20 KB when on, this is a real but small addition to
    /// every publish's cost: `publish`'s own `snap.clone()` (for `last_
    /// snapshot`) doubles it to ~40 KB/publish at the ~10 Hz snapshot
    /// cadence — still negligible next to `doctor_capture`'s up-to-
    /// several-MB buffers.
    pub scope: Option<Vec<BeatScope>>,
    /// M4 Task 10: bumped once for every successful `ControlMsg::
    /// SwitchSource` (never on startup, never on a config-only change —
    /// see `engine_loop`'s `SwitchSource` handler, the only place this
    /// changes). The Mic Doctor panel's own "stale setup" honesty signal:
    /// it remembers the value it last saw and clears its step results (and
    /// abandons any pending capture) the moment this changes, since a
    /// different source invalidates whatever it measured against the old
    /// one.
    pub source_generation: u64,
}

// ---------------------------------------------------------------------
// Engine: the public handle. Owns the control-channel sender and the
// triple_buffer output side; the DSP thread owns everything else.
// ---------------------------------------------------------------------

/// Handle to the running DSP thread. `Engine::snapshot` is the *only*
/// sanctioned way for the UI thread to read live metrics — never call
/// `chrona_dsp::Analyzer::current_metrics` from the UI thread. It's cheap
/// in steady state (~3-5 ms) but spikes to ~88 ms on the ~1-in-10 polls
/// that trigger a refold (measured in T2); on the UI thread that's a
/// dropped frame. The engine thread absorbs that cost once per tick and
/// publishes the result; `snapshot()` only clones the small struct it last
/// wrote.
pub struct Engine {
    tx: mpsc::Sender<ControlMsg>,
    output: triple_buffer::Output<EngineSnapshot>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Engine {
    /// Starts the engine thread paced at real time (~50 ms/tick).
    pub fn start(
        initial: SourceSpec,
        config: EngineConfig,
        egui_ctx: Option<egui::Context>,
    ) -> Engine {
        Engine::start_with_turbo(initial, config, egui_ctx, false)
    }

    /// Starts the engine thread. `turbo=true` is a test-only knob: it
    /// replaces the ~50 ms/tick `recv_timeout` pacing with a non-blocking
    /// `try_recv`, so the thread generates and pushes audio as fast as the
    /// CPU allows instead of at simulated real time. Tick size and the ~10
    /// Hz (in ticks, not wall time) snapshot cadence are unchanged — this
    /// only removes the wall-clock wait, so tests reach steady state (e.g.
    /// Tier 3) in well under a second instead of tens of seconds.
    pub fn start_with_turbo(
        initial: SourceSpec,
        config: EngineConfig,
        egui_ctx: Option<egui::Context>,
        turbo: bool,
    ) -> Engine {
        let (tx, rx) = mpsc::channel();
        let (buf_input, buf_output) = triple_buffer::TripleBuffer::default().split();

        let handle = std::thread::Builder::new()
            .name("chrona-engine".to_string())
            .spawn(move || {
                let mut buf_input = buf_input;
                // Mirrors the last snapshot `engine_loop` published, kept
                // outside the panicking closure (like `buf_input`) so a
                // caught panic can still report truthful context — source
                // kind, recording path, stream info — instead of resetting
                // everything to `Default` in the fault banner.
                let mut last_snapshot = EngineSnapshot::default();
                let ctx_for_loop = egui_ctx.clone();
                // Panic containment (spec §9): the DSP thread must never
                // silently vanish and leave the UI showing stale data with
                // no explanation. `&mut buf_input` / `&mut last_snapshot`
                // are the only borrowed (non-owned) captures here —
                // AssertUnwindSafe is the standard escape hatch for exactly
                // this shape (see `std::panic::catch_unwind`'s own docs
                // example).
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    engine_loop(
                        initial,
                        config,
                        ctx_for_loop,
                        turbo,
                        rx,
                        &mut buf_input,
                        &mut last_snapshot,
                    );
                }));
                if let Err(payload) = result {
                    let detail = panic_payload_message(&payload);
                    eprintln!("chrona engine thread panicked: {detail}");
                    buf_input.write(EngineSnapshot {
                        banner: Some(Banner {
                            severity: BannerSeverity::Error,
                            text: format!("analysis thread fault: {detail}"),
                        }),
                        ..last_snapshot
                    });
                    if let Some(ctx) = &egui_ctx {
                        ctx.request_repaint();
                    }
                }
            })
            .expect("spawn chrona engine thread");

        Engine {
            tx,
            output: buf_output,
            handle: Some(handle),
        }
    }

    /// Reads the latest published snapshot (non-blocking; never touches the
    /// DSP thread's state directly — see the type-level doc comment).
    pub fn snapshot(&mut self) -> EngineSnapshot {
        self.output.read().clone()
    }

    /// Sends a control message. Best-effort: if the engine thread has
    /// already exited (e.g. after a panic) the message is silently
    /// dropped — the next `snapshot()` will show why.
    pub fn send(&self, msg: ControlMsg) {
        let _ = self.tx.send(msg);
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        let _ = self.tx.send(ControlMsg::Shutdown);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn panic_payload_message(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic payload".to_string()
    }
}

// ---------------------------------------------------------------------
// SourceRuntime: the live, owned state behind whichever `SourceSpec` is
// currently active. Internal — the engine thread's own bookkeeping.
// ---------------------------------------------------------------------

enum SourceRuntime {
    Mic(CaptureStream),
    Simulate {
        buffer: Vec<f32>,
        pos: usize,
        sample_rate_hz: f64,
    },
    Replay {
        samples: Vec<f32>,
        pos: usize,
        sample_rate_hz: f64,
        done: bool,
    },
    /// A live-nothing placeholder `engine_loop`'s initial-build fallback
    /// runs on once every way of starting a real source (or its `Analyzer`)
    /// has fatally failed — see the fall-through arms in `engine_loop`
    /// below (T3, M4: this used to be a `return`, killing the thread).
    /// `pull_tick` pulls zero samples for it, forever: no data ever reaches
    /// the `Analyzer`, the `SilenceTracker`, or a `SessionWriter` (which is
    /// why `ControlMsg::StartRecording`'s handler refuses to create one
    /// while idle — recording silence under a name implying real capture
    /// would be a lie). A fixed `IDLE_SAMPLE_RATE_HZ`, rather than carrying
    /// no rate at all, means a real `Analyzer` still gets built against it —
    /// every downstream invariant ("there is always a current `Analyzer`",
    /// config changes rebuild it, etc.) stays intact; Idle just never feeds
    /// it anything, rather than every caller needing to special-case an
    /// `Option<Analyzer>`. The carried `SourceKind` is whatever was last
    /// *attempted* (not necessarily built) — see `spec_kind` — so
    /// `EngineSnapshot::source_kind` (the UI's device-picker context) still
    /// reflects what the user was trying to run, instead of resetting to
    /// `SourceKind::default()` out of nowhere. The only way out is a
    /// successful `SwitchSource`.
    ///
    /// Banner lifecycle: the fatal-fallthrough arms set an
    /// `Error`-severity `banner` alongside the `Idle` runtime; nothing
    /// while idle ever clears it back to `None` — `apply_config`'s
    /// `source_is_idle` gate refuses to (see its doc comment), the
    /// silence/clipping/overrun paths above never fire (no samples ever
    /// pulled), and `SwitchSource`'s own Err arms only ever *replace* it
    /// with another `Error` banner, never clear it. The one path that does
    /// clear it — `SwitchSource` actually succeeding — is also the only
    /// path that ever moves `source` off `Idle`, so the two stay
    /// correctly in lockstep. On the UI side, `ui::controls::pick_banner`
    /// checks the engine's `banner` first and returns it immediately when
    /// present, before even looking at health counters — so this banner is
    /// guaranteed to be what's rendered while idle regardless of anything
    /// else in the published snapshot.
    Idle(SourceKind),
}

impl SourceRuntime {
    /// Builds the runtime for `spec`. For every arm but `ReplayFile` this
    /// leaves `config` untouched and returns `None` alongside the runtime.
    ///
    /// `ReplayFile` is the exception: if the recording carries a JSON
    /// sidecar (`chrona_session::SessionMeta`, written by `StartRecording`
    /// below), this applies its `lift_angle_deg`/`ppm_correction`/
    /// `bph_mode` into `*config` *before returning* — the caller's
    /// subsequent `build_analyzer(&config, ..)` then builds the replay's
    /// analyzer from the recorded settings, not whatever the UI happened to
    /// have live. This mirrors `chrona_session::replay::replay`'s restore
    /// semantics (fall back to `AnalyzerConfig::default()` is the caller's
    /// existing behavior for a bare WAV, since a missing/unparseable
    /// sidecar is `None` here same as there) so replaying a file through
    /// this app reproduces what the `chrona` CLI's `replay` subcommand
    /// would report on the same file. The `Some(String)` return is an info
    /// message describing the *applied* (post-clamp) values, for the caller
    /// to publish as a banner; UI config changes made during replay still
    /// work as normal afterwards (they rebuild the analyzer same as any
    /// other change).
    ///
    /// The sidecar's `lift_angle_deg`/`ppm_correction` are clamped rather
    /// than applied raw: a hand-edited (or otherwise corrupted) sidecar
    /// could carry a `lift_angle_deg` outside `Analyzer::new`'s `[10, 90]`
    /// domain, which — applied raw — would leave `*config` holding that bad
    /// value even though the caller's subsequent `build_analyzer` rejects
    /// it (this function only ever fails on `SessionReader::open`, never on
    /// the config it writes). Every later config change starts from that
    /// same poisoned `*config` (see `apply_config`'s `let mut candidate =
    /// *config`), so it would keep failing validation — for the *same*
    /// reason — until the user happened to fix the one bad field. Clamping
    /// here means `*config` is always left analyzer-valid.
    ///
    /// M4 Task 5: this clamp now calls `chrona_session::{sanitize_lift,
    /// sanitize_ppm}` — the same fns `chrona_session::replay::replay`'s own
    /// sidecar handling and `ui::controls::ControlsState::from_config` use
    /// — instead of hand-rolling the bounds again a third time. This stays
    /// defense-in-depth alongside those (not a replacement for them): the
    /// engine's live replay path below doesn't go through
    /// `chrona_session::replay::replay` at all, so it needs its own clamp
    /// regardless of what that module does.
    fn build(
        spec: &SourceSpec,
        config: &mut EngineConfig,
    ) -> Result<(SourceRuntime, Option<String>), String> {
        match spec {
            SourceSpec::Mic { device_id } => CaptureStream::start(device_id.as_deref(), MIC_RING_S)
                .map(|s| (SourceRuntime::Mic(s), None))
                .map_err(|e| format!("microphone: {e}")),
            SourceSpec::Simulate {
                rate,
                beat_error,
                amplitude,
                snr,
            } => {
                let cfg = SynthConfig {
                    duration_s: SIM_LOOP_S,
                    rate_s_per_day: *rate,
                    beat_error_ms: *beat_error,
                    amplitude_deg: *amplitude,
                    snr_db: *snr,
                    seed: 1,
                    ..SynthConfig::default()
                };
                let sample_rate_hz = cfg.sample_rate_hz;
                synthesize(&cfg)
                    .map(|buffer| {
                        (
                            SourceRuntime::Simulate {
                                buffer,
                                pos: 0,
                                sample_rate_hz,
                            },
                            None,
                        )
                    })
                    .map_err(|e| format!("simulate: {e}"))
            }
            SourceSpec::ReplayFile { path } => SessionReader::open(path)
                .map(|r| {
                    let info = r.meta.as_ref().map(|m| {
                        if let Some(mode) = parse_sidecar_bph_mode(&m.bph_mode) {
                            config.bph_mode = mode;
                        }
                        // Clamped, not applied raw — see this fn's doc
                        // comment. `sanitize_lift` clamps a finite
                        // out-of-domain value to the nearest bound; a `NaN`
                        // (or, per further checking during Task 5, even a
                        // huge-exponent overflow-to-±infinity numeral) lift
                        // can't survive the JSON round trip in the first
                        // place — `serde_json` rejects an out-of-f64-range
                        // numeral as a parse error rather than saturating
                        // it, so `Some(m)` here always already carries a
                        // finite value — but going through the shared fn
                        // (rather than a bare `.clamp(10.0, 90.0)`) means
                        // this stays correct even if that ever changes.
                        let lift = sanitize_lift(m.lift_angle_deg);
                        config.lift_angle_deg = lift;
                        // ppm has no analyzer-side range (`Analyzer::new`
                        // only requires it finite), so an out-of-[-500,500]
                        // value wouldn't wedge the config the way an
                        // out-of-range lift would — this clamp is UI/sanity
                        // consistency with `sanitize_ppm`'s own range, not a
                        // build-safety fix. `sanitize_ppm` handles the
                        // finite (the only reachable, per the lift comment
                        // above) branch; a non-finite input — unreachable
                        // via a real sidecar today, but kept for the same
                        // future-proofing reason as `lift` above — still
                        // deliberately leaves `config.ppm_correction`
                        // untouched (no sensible clamp target for it, and
                        // `sanitize_ppm`'s own non-finite default of 0.0
                        // would poison the live config with a value the
                        // user never asked for) rather than delegating that
                        // branch too.
                        let ppm = if m.ppm_correction.is_finite() {
                            sanitize_ppm(m.ppm_correction)
                        } else {
                            config.ppm_correction
                        };
                        config.ppm_correction = ppm;
                        format!("replay: using recorded settings (lift {lift:.1}°, ppm {ppm:.1})")
                    });
                    (
                        SourceRuntime::Replay {
                            samples: r.samples,
                            pos: 0,
                            sample_rate_hz: r.sample_rate_hz,
                            done: false,
                        },
                        info,
                    )
                })
                .map_err(|e| format!("replay {}: {e}", path.display())),
        }
    }

    fn sample_rate_hz(&self) -> f64 {
        match self {
            SourceRuntime::Mic(s) => s.info.sample_rate_hz,
            SourceRuntime::Simulate { sample_rate_hz, .. } => *sample_rate_hz,
            SourceRuntime::Replay { sample_rate_hz, .. } => *sample_rate_hz,
            SourceRuntime::Idle(_) => IDLE_SAMPLE_RATE_HZ,
        }
    }

    fn kind(&self) -> SourceKind {
        match self {
            SourceRuntime::Mic(_) => SourceKind::Mic,
            SourceRuntime::Simulate { .. } => SourceKind::Simulate,
            SourceRuntime::Replay { .. } => SourceKind::Replay,
            SourceRuntime::Idle(kind) => *kind,
        }
    }

    fn stream_info(&self) -> Option<StreamInfo> {
        match self {
            SourceRuntime::Mic(s) => Some(s.info.clone()),
            _ => None,
        }
    }

    fn device_name(&self) -> String {
        match self {
            SourceRuntime::Mic(s) => s.info.device_name.clone(),
            SourceRuntime::Simulate { .. } => "simulate".to_string(),
            SourceRuntime::Replay { .. } => "replay".to_string(),
            SourceRuntime::Idle(_) => "idle".to_string(),
        }
    }

    fn replay_done(&self) -> bool {
        matches!(self, SourceRuntime::Replay { done: true, .. })
    }
}

/// Maps a `SourceSpec` to the `SourceKind` it would build (mirrors
/// `SourceRuntime::kind`'s own mapping one level up, before anything has
/// necessarily built) — used by `engine_loop`'s initial-build fallback to
/// give `SourceRuntime::Idle` a sensible `kind` even when nothing actually
/// built, so `EngineSnapshot::source_kind` still reflects what was
/// attempted rather than resetting to `SourceKind::default()`.
fn spec_kind(spec: &SourceSpec) -> SourceKind {
    match spec {
        SourceSpec::Mic { .. } => SourceKind::Mic,
        SourceSpec::Simulate { .. } => SourceKind::Simulate,
        SourceSpec::ReplayFile { .. } => SourceKind::Replay,
    }
}

/// Rolling ~`SILENCE_WINDOW_S` RMS window backing `HealthView::silent_for_s`.
#[derive(Debug, Default)]
struct SilenceTracker {
    window: VecDeque<(f64, usize)>, // (sum of squares, sample count) per pushed chunk
    energy: f64,
    samples: usize,
    silent_for_s: f64,
}

impl SilenceTracker {
    fn push(&mut self, chunk: &[f32], sample_rate_hz: f64) {
        if chunk.is_empty() {
            return;
        }
        let energy: f64 = chunk.iter().map(|&s| (s as f64) * (s as f64)).sum();
        self.window.push_back((energy, chunk.len()));
        self.energy += energy;
        self.samples += chunk.len();
        let cap = (SILENCE_WINDOW_S * sample_rate_hz) as usize;
        while self.samples > cap {
            let Some((e, c)) = self.window.pop_front() else {
                break;
            };
            self.energy -= e;
            self.samples -= c;
        }
        let rms = (self.energy / self.samples.max(1) as f64).sqrt();
        let dur_s = chunk.len() as f64 / sample_rate_hz;
        if rms < SILENCE_RMS {
            self.silent_for_s += dur_s;
        } else {
            self.silent_for_s = 0.0;
        }
    }
}

/// System-clock skew regression (binding spec §3.4's NTP cross-check):
/// least-squares slope of `audio_delivered_s − wall_elapsed_s` (accumulated
/// drift, s) against `wall_elapsed_s` (elapsed wall time, s), fed by `push`
/// once per second on the mic path only — `engine_loop` samples
/// `wall_elapsed_s` from an `Instant` and `audio_delivered_s` from the mic
/// stream's own delivered-frame count divided by its nominal sample rate
/// (`SKEW_SAMPLE_INTERVAL_S`). The fitted slope, ×1e6, is the skew in ppm:
/// a sound card whose crystal runs fast relative to the system clock
/// (itself "whatever disciplines it, typically NTP" — never assumed
/// correct on its own) drifts `audio_delivered_s` away from `wall_elapsed_s`
/// linearly over time, and that's exactly the drift this regression
/// recovers. `skew()` withholds a reading until `SKEW_MIN_SPAN_S` of span
/// has accumulated — too short a window is too noisy to trust.
///
/// **Display-plus-explicit-adopt only** (binding spec §3.4/§10 deviation
/// #4): nothing in this type, or in what feeds it, ever writes back into
/// `EngineConfig` on its own. The only path from a reading to an applied
/// correction is a human clicking the toolbar cal popup's "use as
/// correction" button (`ui/toolbar.rs`), which sends an explicit `SetPpm`.
///
/// Pure and engine-independent — unit-tested on its own, without a running
/// engine (see this module's `tests`).
///
/// Numerical care: every sample's `wall_elapsed_s` is re-centered on the
/// FIRST pushed sample (`origin`) before folding it into the running
/// regression sums, i.e. every sample after the first folds in `dx =
/// wall_elapsed_s - origin`, not the raw value (shifting `x` by a constant
/// doesn't change a least-squares slope — only the intercept, which nothing
/// here reads). The textbook one-pass slope formula's denominator (`nΣx² -
/// (Σx)²`) loses precision as `x` grows large relative to its own spread;
/// re-centering keeps every accumulated sum bounded by the session's own
/// length (since `reset` — see below — is the only thing that ever moves
/// `origin`) rather than by whatever convention a caller's `wall_elapsed_s`
/// happens to use, so this stays well-conditioned even across an
/// hours-long uninterrupted mic session.
#[derive(Debug, Default)]
struct SkewTracker {
    /// The first pushed sample's `wall_elapsed_s` — the regression's `x`
    /// origin (see the type doc comment). `None` until the first `push`.
    origin: Option<f64>,
    n: u64,
    sum_dx: f64,
    sum_dx2: f64,
    sum_dy: f64,
    sum_dxdy: f64,
    /// The most recent sample's `dx` (`wall_elapsed_s - origin`). Since
    /// `origin` is itself the first sample, this doubles as the regression's
    /// span so far — reused directly as `ClockSkew::span_s`, no separate
    /// bookkeeping needed.
    last_dx: f64,
}

impl SkewTracker {
    /// Folds in one `(wall_elapsed_s, audio_delivered_s)` sample. See the
    /// type doc comment for the regression this feeds and the `origin`
    /// re-centering.
    fn push(&mut self, wall_elapsed_s: f64, audio_delivered_s: f64) {
        let origin = *self.origin.get_or_insert(wall_elapsed_s);
        let dx = wall_elapsed_s - origin;
        let dy = audio_delivered_s - wall_elapsed_s;
        self.n += 1;
        self.sum_dx += dx;
        self.sum_dx2 += dx * dx;
        self.sum_dy += dy;
        self.sum_dxdy += dx * dy;
        self.last_dx = dx;
    }

    /// The fitted skew, or `None` before `SKEW_MIN_SPAN_S` of span has
    /// accumulated (binding spec §3.4: too short a window to trust) — see
    /// the type doc comment for the "display-plus-explicit-adopt only"
    /// contract this backs. Also `None` in the (practically unreachable
    /// once the span gate is cleared) degenerate case of every sample
    /// sharing the same `wall_elapsed_s`, which would otherwise divide by
    /// zero.
    fn skew(&self) -> Option<ClockSkew> {
        if self.last_dx < SKEW_MIN_SPAN_S {
            return None;
        }
        let n = self.n as f64;
        let denom = n * self.sum_dx2 - self.sum_dx * self.sum_dx;
        if denom == 0.0 {
            return None;
        }
        let slope = (n * self.sum_dxdy - self.sum_dx * self.sum_dy) / denom;
        Some(ClockSkew {
            ppm: slope * 1e6,
            span_s: self.last_dx,
        })
    }

    /// Clears all accumulated state — called on every source rebuild (same
    /// site `silence` resets at, in `engine_loop`'s `SwitchSource` success
    /// arm): a regression that straddled two different sources (or two
    /// different mic devices, with their own independent clocks) would be
    /// measuring nothing meaningful.
    fn reset(&mut self) {
        *self = SkewTracker::default();
    }
}

/// Read `n` samples starting at `*pos` from `buffer`, wrapping around when
/// the read crosses the end. Assumes `n <= buffer.len()` (true for every
/// caller: `n` is one tick's worth of audio, `buffer` is a 60 s loop).
fn read_looping(buffer: &[f32], pos: &mut usize, n: usize) -> Vec<f32> {
    let len = buffer.len();
    let mut out = Vec::with_capacity(n);
    let end = *pos + n;
    if end <= len {
        out.extend_from_slice(&buffer[*pos..end]);
    } else {
        out.extend_from_slice(&buffer[*pos..]);
        out.extend_from_slice(&buffer[..end - len]);
    }
    *pos = end % len;
    out
}

fn sim_chunk_len(sample_rate_hz: f64) -> usize {
    ((TICK_S * sample_rate_hz).round() as usize).max(1)
}

fn build_analyzer(config: &EngineConfig, sample_rate_hz: f64) -> Result<Analyzer, AnalyzerError> {
    Analyzer::new(AnalyzerConfig {
        sample_rate_hz,
        bph_mode: config.bph_mode,
        ppm_correction: config.ppm_correction,
        lift_angle_deg: config.lift_angle_deg,
        averaging_s: config.averaging_s,
    })
}

/// Builds the `Analyzer` `SourceRuntime::Idle` runs with — called only from
/// `engine_loop`'s initial-build fallback, once every other option (a real
/// source, or an `Analyzer` built against its real negotiated rate) has
/// already failed.
///
/// `IDLE_SAMPLE_RATE_HZ` already rules out a pathological source sample
/// rate (e.g. an oddball Mic negotiation too low for the spec's 3 kHz
/// high-pass) as a reason this could still fail — so the only remaining way
/// it can is `*config` itself being invalid (e.g. a corrupted persisted
/// `lift_angle_deg`, read at app startup independently of which source was
/// being attempted). In that case `*config` is reset to the same
/// known-valid literals `run_headless` and
/// `chrona_dsp::AnalyzerConfig::default()` both use, for the same reason
/// `SourceRuntime::build`'s Replay-sidecar clamp exists (see its doc
/// comment): every later `apply_config`/`SwitchSource` call starts from
/// `*config`, so leaving it poisoned here would keep failing validation
/// "for the same reason" until the user happened to fix that exact field.
/// These literals are unconditionally analyzer-valid at
/// `IDLE_SAMPLE_RATE_HZ`, so the second attempt can never fail — this
/// function never fails.
fn build_idle_analyzer(config: &mut EngineConfig) -> Analyzer {
    build_analyzer(config, IDLE_SAMPLE_RATE_HZ).unwrap_or_else(|_| {
        *config = EngineConfig {
            lift_angle_deg: 52.0,
            averaging_s: 30.0,
            bph_mode: BphMode::Auto,
            ppm_correction: 0.0,
        };
        build_analyzer(config, IDLE_SAMPLE_RATE_HZ)
            .expect("EngineConfig defaults at IDLE_SAMPLE_RATE_HZ are always analyzer-valid")
    })
}

fn bph_mode_to_string(mode: BphMode) -> String {
    match mode {
        BphMode::Auto => "auto".to_string(),
        BphMode::Free => "free".to_string(),
        BphMode::Fixed(n) => n.to_string(),
    }
}

/// Maps a live `MetricsSnapshot` into the sidecar's stop-time
/// `SessionSummary` (M4a). `duration_s` is always left at `0.0` here — see
/// `stop_summary`'s doc comment for why.
fn summary_from(m: &MetricsSnapshot) -> SessionSummary {
    let tier = match m.tier {
        Tier::T1 => "T1",
        Tier::T2 => "T2",
        Tier::T3 => "T3",
    };
    SessionSummary {
        tier: tier.to_string(),
        rate_s_per_day: m.rate_s_per_day,
        beat_error_ms: m.beat_error_ms,
        amplitude_deg: m.amplitude_deg,
        bph_detected: Some(m.bph_detected),
        duration_s: 0.0,
    }
}

/// Builds the `SessionSummary` every finalized recording gets (M4a: engine-
/// owned invariant — every writer that stops, however it stopped
/// (StopRecording, an auto-finalize on SwitchSource/StartRecording, or
/// Shutdown), gets a summary; the UI never has to remember to ask for one).
/// `last_metrics` is the engine's most recently published `MetricsSnapshot`
/// — independent of any one recording, so a session that ends before the
/// analyzer ever locks reports tier `"none"` with every metric absent,
/// rather than omitting the summary altogether. `duration_s` is always left
/// at `0.0` here — `SessionWriter::finalize_with` (chrona-session T2)
/// overwrites it from the actual recorded sample count, never trusted from
/// the caller.
fn stop_summary(last_metrics: &Option<MetricsSnapshot>) -> SessionSummary {
    match last_metrics {
        Some(m) => summary_from(m),
        None => SessionSummary {
            tier: "none".to_string(),
            rate_s_per_day: None,
            beat_error_ms: None,
            amplitude_deg: None,
            bph_detected: None,
            duration_s: 0.0,
        },
    }
}

/// Parses a sidecar's `bph_mode` string (`"auto"`, `"free"`, or a numeric
/// BPH — `bph_mode_to_string`'s inverse) into a `BphMode`. Mirrors
/// `chrona_session::replay`'s own `parse_bph_mode` (itself mirroring the
/// CLI's `parse_bph_mode` in `chrona-cli/src/commands.rs`) — duplicated
/// rather than imported because that parser is a private implementation
/// detail of the `replay` module, not part of `chrona_session`'s public
/// surface. Unlike `chrona_session::replay::replay` (which hard-fails the
/// whole replay on an unparseable sidecar value), this returns `None` and
/// lets the caller leave the current `bph_mode` untouched — consistent with
/// `chrona_session::sidecar`'s own "absent/unparseable is not a failure"
/// rule for a JSON sidecar that parsed as valid `SessionMeta` but happens to
/// carry a corrupted `bph_mode` string.
fn parse_sidecar_bph_mode(s: &str) -> Option<BphMode> {
    match s {
        "auto" => Some(BphMode::Auto),
        "free" => Some(BphMode::Free),
        n => n.parse::<u32>().ok().map(BphMode::Fixed),
    }
}

/// Applies a config change atomically: builds the candidate `Analyzer`
/// first, and only commits it (and the new config value) if that succeeds.
/// A rejected candidate (e.g. lift angle out of [10, 90]) leaves both the
/// running analyzer and `config` untouched, rather than corrupting `config`
/// with a value that never actually took effect.
///
/// `source_is_idle` gates whether this touches `banner` at all (T3, M4):
/// while `SourceRuntime::Idle`, the published Error banner is reporting
/// "there is no active source" — a fact no config tweak can change, only a
/// successful `SwitchSource` can — so it must survive here regardless of
/// whether this particular candidate happened to validate. `config` and
/// `analyzer` still update normally either way: rebuilding the (unfed, so
/// harmless) idle `Analyzer` keeps it in sync with the user's latest knobs
/// for whenever a real source does arrive, same as it would for a live one.
fn apply_config(
    config: &mut EngineConfig,
    analyzer: &mut Analyzer,
    sample_rate_hz: f64,
    source_is_idle: bool,
    banner: &mut Option<Banner>,
    mutate: impl FnOnce(&mut EngineConfig),
) {
    let mut candidate = *config;
    mutate(&mut candidate);
    match build_analyzer(&candidate, sample_rate_hz) {
        Ok(a) => {
            *config = candidate;
            *analyzer = a;
            if !source_is_idle {
                *banner = None;
            }
        }
        Err(e) => {
            if !source_is_idle {
                *banner = Some(Banner {
                    severity: BannerSeverity::Error,
                    text: format!("invalid config: {e}"),
                })
            }
        }
    }
}

/// Writes `snap` to the triple_buffer, mirrors it into `last_snapshot` (see
/// its doc comment on the spawn closure for why), and requests a repaint.
/// The single publish path for `engine_loop`, so `last_snapshot` can never
/// drift from what was actually last written.
fn publish(
    buf_input: &mut triple_buffer::Input<EngineSnapshot>,
    last_snapshot: &mut EngineSnapshot,
    egui_ctx: &Option<egui::Context>,
    snap: EngineSnapshot,
) {
    *last_snapshot = snap.clone();
    buf_input.write(snap);
    if let Some(ctx) = egui_ctx {
        ctx.request_repaint();
    }
}

/// Pulls one tick's worth of audio from `source` into `analyzer`, teeing it
/// to `writer` if recording, to `doctor_capture` if a Mic Doctor capture is
/// accumulating (M4 Task 10), and folding it into the silence tracker.
/// Returns the number of samples pulled (0 for a starved mic, or a replay
/// that has already finished) and, if the tee to `writer` failed this tick
/// (M4 Task 4 — e.g. a full disk), that write error's message: state the
/// caller (`engine_loop`, the only owner of `recording_path`/`banner`) still
/// needs to turn into the write-failure banner and a dropped writer.
fn pull_tick(
    source: &mut SourceRuntime,
    analyzer: &mut Analyzer,
    writer: &mut Option<SessionWriter>,
    doctor_capture: &mut Option<DoctorCaptureState>,
    silence: &mut SilenceTracker,
) -> (usize, Option<String>) {
    match source {
        SourceRuntime::Mic(stream) => {
            let avail = stream.consumer.slots();
            if avail == 0 {
                return (0, None);
            }
            let Ok(chunk) = stream.consumer.read_chunk(avail) else {
                return (0, None);
            };
            let (a, b) = chunk.as_slices();
            analyzer.push_samples(a);
            analyzer.push_samples(b);
            let write_error = push_tee(writer, a).or_else(|| push_tee(writer, b));
            doctor_tee(doctor_capture, a);
            doctor_tee(doctor_capture, b);
            let sr = stream.info.sample_rate_hz;
            silence.push(a, sr);
            silence.push(b, sr);
            let n = a.len() + b.len();
            chunk.commit_all();
            (n, write_error)
        }
        SourceRuntime::Simulate {
            buffer,
            pos,
            sample_rate_hz,
        } => {
            let n = sim_chunk_len(*sample_rate_hz);
            let chunk = read_looping(buffer, pos, n);
            analyzer.push_samples(&chunk);
            let write_error = push_tee(writer, &chunk);
            doctor_tee(doctor_capture, &chunk);
            silence.push(&chunk, *sample_rate_hz);
            (chunk.len(), write_error)
        }
        SourceRuntime::Replay {
            samples,
            pos,
            sample_rate_hz,
            done,
        } => {
            if *done {
                return (0, None);
            }
            let n = ((REPLAY_CHUNK_S * *sample_rate_hz).round() as usize).max(1);
            let end = (*pos + n).min(samples.len());
            let chunk = &samples[*pos..end];
            analyzer.push_samples(chunk);
            let write_error = push_tee(writer, chunk);
            doctor_tee(doctor_capture, chunk);
            silence.push(chunk, *sample_rate_hz);
            let pulled = chunk.len();
            *pos = end;
            if *pos >= samples.len() {
                *done = true;
            }
            (pulled, write_error)
        }
        // Deliberately a no-op (spec/M4 T3): no samples, so nothing reaches
        // `analyzer`, `writer`, `doctor_capture`, or `silence` — see
        // `SourceRuntime::Idle`'s doc comment for why that's exactly the
        // point (a silent `SilenceTracker` would otherwise eventually raise
        // its own silence banner on top of the real Error banner explaining
        // why there's no source at all; a Doctor capture against Idle
        // simply never completes, for the same reason).
        SourceRuntime::Idle(_) => (0, None),
    }
}

/// Tees `samples` to `writer` if one is currently recording. `None` when not
/// recording, or the push succeeded; `Some(message)` on a write failure —
/// `pull_tick`'s callers each stop at the first failure (mirroring what a
/// broken/full-disk writer would do on a second write anyway) rather than
/// attempting a same-tick second push against a writer already known to be
/// broken. Leaves `writer` itself untouched either way — dropping it is
/// `engine_loop`'s job, alongside the `recording_path`/`banner` state that
/// lives there too (see `pull_tick`'s doc comment).
fn push_tee(writer: &mut Option<SessionWriter>, samples: &[f32]) -> Option<String> {
    writer.as_mut()?.push(samples).err().map(|e| e.to_string())
}

/// M4 Task 10: in-progress accumulation for `ControlMsg::DoctorCapture` — a
/// one-shot buffer of the next N seconds of PUSHED samples, from ANY source
/// (`pull_tick` tees every arm into it, mirroring `push_tee`). `target_len`
/// and `sample_rate_hz` are both fixed ONCE at request time (`engine_loop`'s
/// `ControlMsg::DoctorCapture` handler), from the source's sample rate at
/// that moment — mirrors `StartRecording`'s own `meta.sample_rate_hz`
/// snapshot. A capture never straddles a sample-rate change mid-flight: a
/// `SwitchSource` while accumulating discards this state outright (honesty
/// — see `EngineSnapshot::source_generation`'s doc comment) rather than let
/// a buffer silently splice old-source audio with new-source audio.
struct DoctorCaptureState {
    buffer: Vec<f32>,
    target_len: usize,
    sample_rate_hz: f64,
}

/// Tees `samples` into `capture`'s buffer if one is accumulating, stopping
/// (but not truncating the chunk that crosses the line) once the buffer
/// reaches its target length — this fn's caller (`engine_loop`, right after
/// `pull_tick`) tolerates that overshoot, up to one tick's worth of samples
/// (task brief: "±1 chunk"), so trimming here would only lose the tick/AGC
/// analyzers' own trailing context for no honesty benefit.
fn doctor_tee(capture: &mut Option<DoctorCaptureState>, samples: &[f32]) {
    if samples.is_empty() {
        return;
    }
    if let Some(state) = capture
        && state.buffer.len() < state.target_len
    {
        state.buffer.extend_from_slice(samples);
    }
}

// ---------------------------------------------------------------------
// The thread body itself (spec's numbered loop).
// ---------------------------------------------------------------------

fn engine_loop(
    initial: SourceSpec,
    initial_config: EngineConfig,
    egui_ctx: Option<egui::Context>,
    turbo: bool,
    rx: mpsc::Receiver<ControlMsg>,
    buf_input: &mut triple_buffer::Input<EngineSnapshot>,
    last_snapshot: &mut EngineSnapshot,
) {
    let mut config = initial_config;
    // Declared before the initial source build (rather than alongside
    // `writer`/`recording_path` below, as originally) so a successful
    // Replay build with a sidecar can hand its "using recorded settings"
    // info message straight to it — see `SourceRuntime::build`'s doc
    // comment. No initializer: the match below either assigns it (`Ok`) or
    // diverges (`Err` returns), so every path past it is initialized —
    // an eager `= None` here would just be dead-and-overwritten.
    let mut banner: Option<Banner>;
    let mut source = match SourceRuntime::build(&initial, &mut config) {
        Ok((s, info)) => {
            banner = info.map(|text| Banner {
                severity: BannerSeverity::Info,
                text,
            });
            s
        }
        Err(e) => {
            // A saved-but-now-unplugged input device is a common,
            // recoverable failure — retry once against the default input
            // rather than leaving the whole app inert (dead control
            // channel: no device picker, nothing) until restart. A Mic
            // build failure never mutates `config` (only Replay does — see
            // `SourceRuntime::build`'s doc comment), so retrying against
            // the same `config` is safe. This is deliberately a single
            // retry, not an unbounded one — if it ALSO fails, the
            // fall-through below (T3, M4) keeps the engine alive on an
            // `Idle` source rather than exiting, so a later `SwitchSource`
            // can still recover.
            if matches!(initial, SourceSpec::Mic { device_id: Some(_) }) {
                match SourceRuntime::build(&SourceSpec::Mic { device_id: None }, &mut config) {
                    Ok((s, _info)) => {
                        banner = Some(Banner {
                            severity: BannerSeverity::Warn,
                            text: format!(
                                "saved input device unavailable ({e}) — using default input"
                            ),
                        });
                        s
                    }
                    Err(e2) => {
                        // Total startup failure: both the requested device
                        // and the default-input retry failed. Previously
                        // this published the fault and `return`ed, killing
                        // the thread — the UI was left wired to a dead
                        // control channel until restart. Falls through to
                        // `SourceRuntime::Idle` instead: the loop stays
                        // alive, this banner is retained (see
                        // `apply_config`'s `source_is_idle` gate for how it
                        // survives later config tweaks) until a successful
                        // `SwitchSource` replaces it.
                        banner = Some(Banner {
                            severity: BannerSeverity::Error,
                            text: format!("failed to start: {e}; default input also failed: {e2}"),
                        });
                        SourceRuntime::Idle(spec_kind(&initial))
                    }
                }
            } else {
                // No source at all (not a Mic-with-explicit-device spec, so
                // no retry applies) — same idle fall-through as the
                // mic-double-failure arm above, for the same reason.
                banner = Some(Banner {
                    severity: BannerSeverity::Error,
                    text: format!("failed to start: {e}"),
                });
                SourceRuntime::Idle(spec_kind(&initial))
            }
        }
    };
    let mut analyzer = match build_analyzer(&config, source.sample_rate_hz()) {
        Ok(a) => a,
        Err(e) => {
            // The source (real, or already `Idle` from the arm above) is
            // set, but the `Analyzer` isn't — same idle fall-through, one
            // level up: retain an Error banner and swap `source` for an
            // `Idle` runtime carrying its own last-attempted kind (a no-op
            // if it's already `Idle`). `build_idle_analyzer` can't fail
            // (see its doc comment), so — unlike the two arms above —
            // there's no further failure mode to handle here.
            banner = Some(Banner {
                severity: BannerSeverity::Error,
                text: format!("invalid initial config: {e}"),
            });
            source = SourceRuntime::Idle(source.kind());
            build_idle_analyzer(&mut config)
        }
    };

    let mut writer: Option<SessionWriter> = None;
    let mut recording_path: Option<PathBuf> = None;
    // M4 Task 10 (Mic Doctor): `doctor_capture` accumulates the in-flight
    // `ControlMsg::DoctorCapture` request, if any; `doctor_ready` holds a
    // just-completed one until the next `publish` attaches it to exactly
    // one `EngineSnapshot` and clears it via `.take()` — see both fields'
    // doc comments on `EngineSnapshot`/`DoctorCaptureState`.
    let mut doctor_capture: Option<DoctorCaptureState> = None;
    let mut doctor_ready: Option<(Arc<Vec<f32>>, f64)> = None;
    // M4 Task 11 (spec §6): whether the "Scope" view is open — see
    // `ControlMsg::SetScope`'s doc comment. Not reset on `SwitchSource`
    // (same as `lift_angle_deg`/`averaging_s`): it's a UI-level display
    // toggle independent of which source is active, and `beat_scope` on a
    // freshly-rebuilt (cache-less) `Analyzer` already degrades to an empty
    // `Vec` honestly on its own.
    let mut scope_on = false;
    // M4 Task 10: bumped on every successful `SwitchSource` below — see
    // `EngineSnapshot::source_generation`'s doc comment.
    let mut source_generation: u64 = 0;
    let mut silence = SilenceTracker::default();
    // System-clock skew cross-check (binding spec §3.4, M4 Task 7):
    // `skew`'s own regression origin is internal (see `SkewTracker`'s doc
    // comment), but computing its `wall_elapsed_s` input each tick needs a
    // fixed zero point of our own — `skew_epoch`, reset alongside `skew`
    // itself on every source rebuild below so the two can never drift out
    // of sync. `mic_frames_delivered` is the matching audio-side counter
    // (total mic-path frames pulled since the same reset), divided by the
    // mic's nominal sample rate to get `audio_delivered_s`. `last_skew_
    // sample_at` paces `skew.push` to ~once per `SKEW_SAMPLE_INTERVAL_S`
    // (mirrors `ui::controls::device_picker`'s own `Instant`-polling idiom)
    // — ticks run at ~50 ms, far faster than the 1 Hz this cross-check
    // needs.
    let mut skew = SkewTracker::default();
    let mut skew_epoch = Instant::now();
    let mut mic_frames_delivered: u64 = 0;
    let mut last_skew_sample_at: Option<Instant> = None;
    // Mirrors the most recently published `MetricsSnapshot` (updated
    // alongside every publish below), independent of any one recording —
    // this is what every finalize path (`stop_summary`) summarizes at the
    // moment it fires. `None` until the analyzer has ever detected a
    // signal.
    let mut last_metrics: Option<MetricsSnapshot> = None;
    let mut tick: u32 = 0;
    // I2: pull_tick/publish are gated on real elapsed time (non-turbo) so a
    // burst of control messages (e.g. a T9 slider drag, which can fire many
    // SetLift messages between two real ticks) can't inflate the
    // audio/publish cadence — see the loop body below.
    let tick_duration = Duration::from_secs_f64(TICK_S);
    let mut next_tick = Instant::now() + tick_duration;

    'outer: loop {
        // 1. Drain control messages. Non-turbo waits only until the next
        // tick boundary (not a flat 50 ms) so it can't overshoot it; turbo
        // never waits. Either way this drains ALL currently-queued
        // messages, not just one — a burst (e.g. a slider drag) is fully
        // absorbed here without affecting how often step 2/3 below runs.
        // Both branches treat a disconnected sender as an implicit
        // Shutdown (the sender is gone without sending one explicitly) so
        // the thread doesn't spin forever on a dead channel.
        let first = if turbo {
            match rx.try_recv() {
                Ok(m) => Some(m),
                Err(mpsc::TryRecvError::Empty) => None,
                Err(mpsc::TryRecvError::Disconnected) => Some(ControlMsg::Shutdown),
            }
        } else {
            let wait = next_tick.saturating_duration_since(Instant::now());
            match rx.recv_timeout(wait) {
                Ok(m) => Some(m),
                Err(mpsc::RecvTimeoutError::Timeout) => None,
                Err(mpsc::RecvTimeoutError::Disconnected) => Some(ControlMsg::Shutdown),
            }
        };
        let mut inbox: Vec<ControlMsg> = first.into_iter().collect();
        while let Ok(m) = rx.try_recv() {
            inbox.push(m);
        }

        for msg in inbox {
            match msg {
                ControlMsg::Shutdown => {
                    if let Some(w) = writer.take() {
                        let _ = w.finalize_with(Some(stop_summary(&last_metrics)));
                    }
                    break 'outer;
                }
                ControlMsg::SetLift(v) => apply_config(
                    &mut config,
                    &mut analyzer,
                    source.sample_rate_hz(),
                    matches!(source, SourceRuntime::Idle(_)),
                    &mut banner,
                    |c| c.lift_angle_deg = v,
                ),
                ControlMsg::SetAveraging(v) => apply_config(
                    &mut config,
                    &mut analyzer,
                    source.sample_rate_hz(),
                    matches!(source, SourceRuntime::Idle(_)),
                    &mut banner,
                    |c| c.averaging_s = v,
                ),
                ControlMsg::SetBphMode(m) => apply_config(
                    &mut config,
                    &mut analyzer,
                    source.sample_rate_hz(),
                    matches!(source, SourceRuntime::Idle(_)),
                    &mut banner,
                    |c| c.bph_mode = m,
                ),
                ControlMsg::SetPpm(v) => apply_config(
                    &mut config,
                    &mut analyzer,
                    source.sample_rate_hz(),
                    matches!(source, SourceRuntime::Idle(_)),
                    &mut banner,
                    |c| c.ppm_correction = v,
                ),
                ControlMsg::SwitchSource(spec) => match SourceRuntime::build(&spec, &mut config) {
                    Ok((new_source, info)) => {
                        let sr = new_source.sample_rate_hz();
                        match build_analyzer(&config, sr) {
                            Ok(a) => {
                                source = new_source;
                                analyzer = a;
                                silence = SilenceTracker::default();
                                // System-clock skew (M4 Task 7): a fresh
                                // source (even a different device on the
                                // same Mic spec) has its own independent
                                // clock — a regression straddling the old
                                // and new source would be measuring
                                // nothing meaningful. Reset in lockstep
                                // with `skew` itself — see the `skew_epoch`
                                // doc comment above this loop.
                                skew.reset();
                                skew_epoch = Instant::now();
                                mic_frames_delivered = 0;
                                last_skew_sample_at = None;
                                // M4 Task 10: a capture spanning the switch
                                // would silently splice old-source audio
                                // with new-source audio — discard rather
                                // than publish a buffer that misrepresents
                                // either setup (see `DoctorCaptureState`'s
                                // doc comment). `source_generation` is
                                // `ui::doctor`'s own signal to clear its
                                // step results and abandon a pending Run —
                                // see `EngineSnapshot::source_generation`'s
                                // doc comment.
                                doctor_capture = None;
                                doctor_ready = None;
                                source_generation += 1;
                                // A mid-recording switch must not keep
                                // teeing the old source's (now
                                // semantically wrong, possibly
                                // different-rate) audio into the WAV —
                                // stop and finalize it, and say so. When
                                // nothing was recording, a successful
                                // switch clears any stale error as usual
                                // (this banner would otherwise be a false
                                // "recording stopped" notice every time the
                                // source changes) — except a Replay source
                                // with a sidecar, which reports its
                                // restored settings here instead of a bare
                                // `None`.
                                if let Some(w) = writer.take() {
                                    let _ = w.finalize_with(Some(stop_summary(&last_metrics)));
                                    recording_path = None;
                                    banner = Some(Banner {
                                        severity: BannerSeverity::Info,
                                        text: "recording stopped: source changed".to_string(),
                                    });
                                } else {
                                    banner = info.map(|text| Banner {
                                        severity: BannerSeverity::Info,
                                        text,
                                    });
                                }
                            }
                            Err(e) => {
                                banner = Some(Banner {
                                    severity: BannerSeverity::Error,
                                    text: format!("invalid config for new source: {e}"),
                                })
                            }
                        }
                    }
                    Err(e) => {
                        banner = Some(Banner {
                            severity: BannerSeverity::Error,
                            text: format!("failed to switch source: {e}"),
                        })
                    }
                },
                ControlMsg::StartRecording {
                    dir,
                    meta_position,
                    watch,
                } => {
                    // T3 (M4) ruling: refuse to record while idle rather
                    // than silently creating a writer that would capture
                    // nothing but silence under a filename implying real
                    // audio. No writer can exist yet in this state — Idle
                    // is only ever entered by the initial-build fallback
                    // above, *before* `writer` is declared below, and
                    // `SwitchSource` never sets `source` to `Idle` (only
                    // this function's own initial-build region does) — so
                    // unlike the live-source path there's never an old
                    // writer here to finalize.
                    if matches!(source, SourceRuntime::Idle(_)) {
                        recording_path = None;
                        banner = Some(Banner {
                            severity: BannerSeverity::Error,
                            text: "failed to start recording: no active source — fix the \
                                   input first"
                                .to_string(),
                        });
                        continue;
                    }
                    if let Some(w) = writer.take() {
                        let _ = w.finalize_with(Some(stop_summary(&last_metrics)));
                    }
                    let started_unix_s = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0);
                    let meta = SessionMeta {
                        // Inert: `write_sidecar` (chrona-session) stamps
                        // every on-disk sidecar with `SIDECAR_SCHEMA_VERSION`
                        // unconditionally, regardless of what this literal
                        // carries — set to the real constant anyway so this
                        // in-memory value is never a lie in its own right.
                        schema_version: SIDECAR_SCHEMA_VERSION,
                        device_name: source.device_name(),
                        sample_rate_hz: source.sample_rate_hz(),
                        ppm_correction: config.ppm_correction,
                        lift_angle_deg: config.lift_angle_deg,
                        bph_mode: bph_mode_to_string(config.bph_mode),
                        position: meta_position,
                        started_unix_s,
                        app_version: env!("CARGO_PKG_VERSION").to_string(),
                        watch,
                        summary: None,
                    };
                    let path = dir.join(format!(
                        "chrona-{}.wav",
                        chrona_session::civil::timestamp_compact(started_unix_s)
                    ));
                    match SessionWriter::create(&dir, meta) {
                        Ok(w) => {
                            writer = Some(w);
                            recording_path = Some(path);
                            banner = None;
                        }
                        Err(e) => {
                            // A failed start while already recording would
                            // otherwise leave a stale `Some(path)` here
                            // beside `writer = None` — the `writer.take()`
                            // above already finalized and dropped the old
                            // writer, but this branch never overwrote the
                            // old `recording_path`.
                            recording_path = None;
                            banner = Some(Banner {
                                severity: BannerSeverity::Error,
                                text: format!("failed to start recording: {e}"),
                            });
                        }
                    }
                }
                ControlMsg::StopRecording => {
                    if let Some(w) = writer.take()
                        && let Err(e) = w.finalize_with(Some(stop_summary(&last_metrics)))
                    {
                        banner = Some(Banner {
                            severity: BannerSeverity::Error,
                            text: format!("failed to finalize recording: {e}"),
                        });
                    }
                    recording_path = None;
                }
                ControlMsg::DoctorCapture { seconds } => {
                    // Defensive clamp (task brief) — see MIN/MAX_DOCTOR_
                    // CAPTURE_S's own doc comment.
                    let seconds = seconds.clamp(MIN_DOCTOR_CAPTURE_S, MAX_DOCTOR_CAPTURE_S);
                    let sr = source.sample_rate_hz();
                    let target_len = ((seconds * sr).round() as usize).max(1);
                    doctor_capture = Some(DoctorCaptureState {
                        buffer: Vec::with_capacity(target_len),
                        target_len,
                        sample_rate_hz: sr,
                    });
                    // "Second replaces first" (task brief) covers a
                    // not-yet-delivered completed capture too, not just an
                    // in-progress accumulation — a fresh request supersedes
                    // either.
                    doctor_ready = None;
                }
                ControlMsg::SetScope(on) => scope_on = on,
            }
        }

        // 2/3. Pull audio and (every ~10 Hz) publish — gated on wall-clock
        // time actually reaching `next_tick` in non-turbo mode (turbo
        // always fires, matching its existing "as fast as possible"
        // semantics). Without this gate, a burst of control messages above
        // would make `recv_timeout` above return near-instantly on every
        // iteration, running this block far faster than the intended ~50
        // ms/tick pace — over-generating Simulate/Replay audio and
        // over-triggering publishes for as long as the burst lasted.
        if turbo || Instant::now() >= next_tick {
            let (n, write_error) = pull_tick(
                &mut source,
                &mut analyzer,
                &mut writer,
                &mut doctor_capture,
                &mut silence,
            );

            // M4 Task 10: once the in-progress capture's buffer has reached
            // its target length, hand it off to `doctor_ready` (`Arc`'d
            // here, once, so every later `EngineSnapshot::clone()` shares
            // this one allocation rather than deep-copying it) — the next
            // publish below attaches it to exactly that one snapshot, via
            // `doctor_ready.take()`.
            if doctor_capture
                .as_ref()
                .is_some_and(|c| c.buffer.len() >= c.target_len)
            {
                let finished = doctor_capture
                    .take()
                    .expect("is_some_and above just proved this is Some");
                doctor_ready = Some((Arc::new(finished.buffer), finished.sample_rate_hz));
            }

            // System-clock skew cross-check (binding spec §3.4, M4 Task 7):
            // mic path only — Simulate/Replay/Idle's delivery isn't real
            // audio hardware to cross-check against (see `SkewTracker`'s
            // doc comment), so `skew` is simply never fed on those sources,
            // which keeps `skew.skew()` reading `None` for them below (the
            // regression never accumulates a span in the first place).
            if matches!(source, SourceRuntime::Mic(_)) {
                mic_frames_delivered += n as u64;
                let now = Instant::now();
                let due = last_skew_sample_at
                    .is_none_or(|t| now.duration_since(t).as_secs_f64() >= SKEW_SAMPLE_INTERVAL_S);
                if due {
                    let wall_elapsed_s = now.duration_since(skew_epoch).as_secs_f64();
                    let audio_delivered_s = mic_frames_delivered as f64 / source.sample_rate_hz();
                    skew.push(wall_elapsed_s, audio_delivered_s);
                    last_skew_sample_at = Some(now);
                }
            }

            if let Some(e) = write_error {
                // M4 Task 4: the tee to `writer` failed (e.g. a full disk).
                // Drop it — best-effort finalize; a second error here would
                // just be noise on top of the one already driving this
                // banner, so it's ignored — clear the recording path, and
                // surface a Warn banner (spec §4: a write failure is a
                // recoverable notice, not a fault).
                if let Some(w) = writer.take() {
                    let _ = w.finalize();
                }
                recording_path = None;
                banner = Some(Banner {
                    severity: BannerSeverity::Warn,
                    text: format!("recording write failed: {e} — recording stopped"),
                });
            }

            tick += 1;
            if tick.is_multiple_of(SNAPSHOT_EVERY) {
                let metrics = analyzer.current_metrics();
                last_metrics = metrics;
                let tape = analyzer.tape_events();
                // M4 Task 11: only pay for `beat_scope`'s ring copies while
                // the Scope view is actually open — see `ControlMsg::
                // SetScope`'s and `EngineSnapshot::scope`'s doc comments.
                let scope = scope_on.then(|| analyzer.beat_scope(16));
                let health = HealthView {
                    overruns: match &source {
                        SourceRuntime::Mic(s) => s.health.overruns(),
                        _ => 0,
                    },
                    clipped: analyzer.clipped_samples(),
                    silent_for_s: silence.silent_for_s,
                    last_error: match &source {
                        SourceRuntime::Mic(s) => s.health.last_error(),
                        _ => None,
                    },
                    clock_skew: skew.skew(),
                };
                publish(
                    buf_input,
                    last_snapshot,
                    &egui_ctx,
                    EngineSnapshot {
                        metrics,
                        tape,
                        stream: source.stream_info(),
                        source_kind: source.kind(),
                        replay_done: source.replay_done(),
                        health,
                        recording: recording_path.clone(),
                        banner: banner.clone(),
                        doctor_capture: doctor_ready.take(),
                        source_generation,
                        scope,
                    },
                );
            }

            if !turbo {
                next_tick += tick_duration;
                // Catch up without spiraling: a long stall (GC-style
                // pause, OS starving this thread, a debugger breakpoint)
                // would otherwise leave `next_tick` far in the past, and
                // the loop would immediately re-enter this block on every
                // subsequent iteration to "catch up" — doing so by
                // bursting through the whole backlog only makes the
                // thread fall further behind while it works through it.
                // Past 5 ticks behind, give up catching up and resume
                // pacing from now instead.
                if Instant::now() > next_tick + tick_duration * 5 {
                    next_tick = Instant::now() + tick_duration;
                }
            }
        }
    }
}

// ---------------------------------------------------------------------
// Headless entry point.
// ---------------------------------------------------------------------

/// Headless entry point (spec: the scripted/E2E hook, and the CLI's own
/// `--headless-seconds` smoke test). Drives a `Simulate` source through
/// `seconds` of generated audio and prints the final metrics.
///
/// This reuses the exact tick-level plumbing the live `Engine` thread uses
/// (`SourceRuntime`, `pull_tick`) so it stays a faithful smoke test of the
/// real pipeline — but it runs synchronously on the calling thread, with no
/// spawned thread, control channel, or triple_buffer. A one-shot CLI report
/// has no live UI to publish to, so none of that machinery earns its cost
/// here, and being fully synchronous means `seconds` unambiguously means
/// "generate this much simulated audio" with no wall-clock pacing or turbo
/// knob to reason about.
pub fn run_headless(flags: &AppFlags, seconds: f64) -> i32 {
    let spec = SourceSpec::Simulate {
        rate: flags.rate,
        beat_error: flags.beat_error,
        amplitude: flags.amplitude,
        snr: flags.snr,
    };
    let mut config = EngineConfig {
        lift_angle_deg: 52.0,
        averaging_s: 30.0,
        bph_mode: BphMode::Auto,
        ppm_correction: 0.0,
    };
    // Headless is Simulate-only (validated in `main`), so `SourceRuntime::
    // build`'s Replay-sidecar restore never fires here — `config` is passed
    // `&mut` only because the signature is shared with the live engine
    // loop's Replay path.
    let mut source = match SourceRuntime::build(&spec, &mut config) {
        Ok((s, _info)) => s,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let mut analyzer = match build_analyzer(&config, source.sample_rate_hz()) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let mut writer: Option<SessionWriter> = None;
    // Headless never sends `ControlMsg::DoctorCapture` (no control channel
    // at all here) — a fixed `None` that's never touched, purely to match
    // `pull_tick`'s signature.
    let mut doctor_capture: Option<DoctorCaptureState> = None;
    let mut silence = SilenceTracker::default();

    let ticks = ((seconds / TICK_S).ceil() as u64).max(1);
    for _ in 0..ticks {
        pull_tick(
            &mut source,
            &mut analyzer,
            &mut writer,
            &mut doctor_capture,
            &mut silence,
        );
    }

    match analyzer.current_metrics() {
        Some(m) => {
            println!(
                "{}  rate {}  beat error {}  amplitude {}",
                tier_label(m.tier),
                format_rate(m.rate_s_per_day, m.calibrated),
                format_beat_error(m.beat_error_ms),
                format_amplitude(m.amplitude_deg, m.quality.amplitude_gate),
            );
            0
        }
        None => {
            eprintln!("no metrics: signal never detected");
            2
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_looping_wraps_at_the_buffer_end() {
        let buffer: Vec<f32> = (0..10).map(|i| i as f32).collect();
        let mut pos = 8;
        let out = read_looping(&buffer, &mut pos, 4);
        assert_eq!(out, vec![8.0, 9.0, 0.0, 1.0]);
        assert_eq!(pos, 2);
    }

    #[test]
    fn read_looping_stays_in_bounds_without_wrap() {
        let buffer: Vec<f32> = (0..10).map(|i| i as f32).collect();
        let mut pos = 2;
        let out = read_looping(&buffer, &mut pos, 3);
        assert_eq!(out, vec![2.0, 3.0, 4.0]);
        assert_eq!(pos, 5);
    }

    #[test]
    fn silence_tracker_accumulates_and_resets() {
        let sr = 100.0; // trivially small rate: window cap = 300 samples
        let mut s = SilenceTracker::default();
        let quiet = vec![0.0f32; 50];
        s.push(&quiet, sr);
        assert!(s.silent_for_s > 0.0);
        let loud = vec![1.0f32; 50];
        s.push(&loud, sr);
        assert_eq!(s.silent_for_s, 0.0, "a loud chunk resets the run");
    }

    #[test]
    fn silence_tracker_does_not_flicker_between_regular_ticks() {
        // A per-chunk-only (unwindowed) RMS check would reset
        // `silent_for_s` to 0 on every tick-containing chunk and then
        // immediately start re-accumulating during the quiet gap before the
        // next one — for a watch ticking every ~125 ms, that means it would
        // never read more than a fraction of a second of "silence" even
        // though the watch never actually stopped. The trailing window
        // must instead keep `silent_for_s` pinned at 0 throughout, since
        // some tick energy is always present somewhere in the last 3 s.
        let sr = 1_000.0;
        let mut s = SilenceTracker::default();
        for i in 0..40 {
            // 25 ms chunks; every 5th one carries a moderate "tick" burst
            // in its first 5 samples (a ~125 ms tick spacing), the rest are
            // pure silence.
            let mut chunk = vec![0.0f32; 25];
            if i % 5 == 0 {
                chunk.iter_mut().take(5).for_each(|v| *v = 0.05);
            }
            s.push(&chunk, sr);
            assert_eq!(s.silent_for_s, 0.0, "flickered silent at chunk {i}");
        }
    }

    #[test]
    fn build_idle_analyzer_heals_a_poisoned_config_to_safe_defaults() {
        // A config that's invalid regardless of sample rate (averaging_s
        // outside AnalyzerConfig's [2, 60]) must not survive into `config`
        // — every later apply_config/SwitchSource call starts from
        // `*config`, so leaving it poisoned would keep failing "for the
        // same reason" until the user happened to fix that exact field.
        let mut config = EngineConfig {
            lift_angle_deg: 52.0,
            averaging_s: 999.0,
            bph_mode: BphMode::Fixed(21_600),
            ppm_correction: 5.0,
        };
        let _ = build_idle_analyzer(&mut config); // must not panic
        assert!(
            build_analyzer(&config, IDLE_SAMPLE_RATE_HZ).is_ok(),
            "healed config must be analyzer-valid"
        );
        assert_eq!(config.averaging_s, 30.0);
        assert_eq!(config.lift_angle_deg, 52.0);
        assert_eq!(config.bph_mode, BphMode::Auto);
        assert_eq!(config.ppm_correction, 0.0);
    }

    #[test]
    fn build_idle_analyzer_preserves_an_already_valid_config() {
        // The common case: the source failed (or its sample rate was the
        // problem), not the config — the user's real lift/averaging/bph/ppm
        // settings must survive into the idle state untouched, so a later
        // successful SwitchSource picks them back up rather than silently
        // reverting to defaults.
        let mut config = EngineConfig {
            lift_angle_deg: 44.0,
            averaging_s: 10.0,
            bph_mode: BphMode::Fixed(21_600),
            ppm_correction: 5.0,
        };
        let original = config;
        let _ = build_idle_analyzer(&mut config);
        assert_eq!(config.lift_angle_deg, original.lift_angle_deg);
        assert_eq!(config.averaging_s, original.averaging_s);
        assert_eq!(config.bph_mode, original.bph_mode);
        assert_eq!(config.ppm_correction, original.ppm_correction);
    }

    #[test]
    fn replay_source_restores_sidecar_settings_into_config() {
        // Synthesizes a tiny session via chrona_session (a handful of
        // silent samples — this test is about the sidecar-restore wiring,
        // not analyzer output) with lift/ppm/bph values that all differ
        // from the config it's replayed against, then confirms
        // `SourceRuntime::build` overwrites `config` with the recorded
        // values and returns the matching info message — T10's port of
        // `chrona_session::replay::replay`'s restore semantics (T7-I3).
        let dir = tempfile::tempdir().unwrap();
        let meta = SessionMeta {
            schema_version: 1,
            device_name: "TestMic".to_string(),
            sample_rate_hz: 48_000.0,
            ppm_correction: 7.5,
            lift_angle_deg: 44.0,
            bph_mode: "21600".to_string(),
            position: None,
            started_unix_s: 1_700_000_000,
            app_version: "test".to_string(),
            watch: None,
            summary: None,
        };
        let mut w = SessionWriter::create(dir.path(), meta).unwrap();
        w.push(&vec![0.0f32; 4_800]).unwrap();
        let wav_path = w.finalize().unwrap();

        let mut config = EngineConfig {
            lift_angle_deg: 52.0,
            averaging_s: 30.0,
            bph_mode: BphMode::Auto,
            ppm_correction: 0.0,
        };
        let (source, info) =
            SourceRuntime::build(&SourceSpec::ReplayFile { path: wav_path }, &mut config)
                .expect("replay build succeeds");
        assert_eq!(source.kind(), SourceKind::Replay);
        assert_eq!(config.lift_angle_deg, 44.0);
        assert_eq!(config.ppm_correction, 7.5);
        assert_eq!(config.bph_mode, BphMode::Fixed(21_600));
        assert_eq!(
            info.as_deref(),
            Some("replay: using recorded settings (lift 44.0°, ppm 7.5)")
        );
    }

    #[test]
    fn replay_source_clamps_out_of_range_sidecar_lift() {
        // A hand-edited (or otherwise corrupted) sidecar can carry a
        // lift_angle_deg outside Analyzer::new's [10, 90] domain. Applied
        // raw, `config` would end up holding that bad value even though
        // `SourceRuntime::build` itself only ever fails on
        // `SessionReader::open` (never on the config it writes) — the
        // caller's later `build_analyzer` would then reject it, leaving
        // `config` wedged (see `SourceRuntime::build`'s doc comment).
        // Confirms the clamp lands (5.0 -> 10.0), the info banner reports
        // the applied (post-clamp) value rather than the raw 5.0, and the
        // clamped config is actually analyzer-buildable.
        let dir = tempfile::tempdir().unwrap();
        let meta = SessionMeta {
            schema_version: 1,
            device_name: "TestMic".to_string(),
            sample_rate_hz: 48_000.0,
            ppm_correction: 0.0,
            lift_angle_deg: 5.0,
            bph_mode: "auto".to_string(),
            position: None,
            started_unix_s: 1_700_000_000,
            app_version: "test".to_string(),
            watch: None,
            summary: None,
        };
        let mut w = SessionWriter::create(dir.path(), meta).unwrap();
        w.push(&vec![0.0f32; 4_800]).unwrap();
        let wav_path = w.finalize().unwrap();

        let mut config = EngineConfig {
            lift_angle_deg: 52.0,
            averaging_s: 30.0,
            bph_mode: BphMode::Auto,
            ppm_correction: 0.0,
        };
        let (source, info) =
            SourceRuntime::build(&SourceSpec::ReplayFile { path: wav_path }, &mut config)
                .expect("replay build succeeds even with an out-of-range sidecar lift");
        assert_eq!(source.kind(), SourceKind::Replay);
        assert_eq!(
            config.lift_angle_deg, 10.0,
            "clamped into Analyzer::new's [10, 90] domain"
        );
        assert_eq!(
            info.as_deref(),
            Some("replay: using recorded settings (lift 10.0°, ppm 0.0)"),
            "banner reports the applied (post-clamp) value, not the raw 5.0"
        );
        assert!(
            build_analyzer(&config, source.sample_rate_hz()).is_ok(),
            "clamped config must be analyzer-valid — the whole point of clamping"
        );
    }

    #[test]
    fn replay_source_without_sidecar_leaves_config_untouched() {
        // A recording whose sidecar is missing (removed here after writing,
        // to reuse SessionWriter rather than pull in hound directly — same
        // "<stem>.json next to the WAV" convention `chrona_session::sidecar`
        // documents) must not perturb `config` at all, and reports no info
        // message — mirrors `chrona_session::sidecar`'s own "absent sidecar
        // is not a failure" rule.
        let dir = tempfile::tempdir().unwrap();
        let meta = SessionMeta {
            schema_version: 1,
            device_name: "TestMic".to_string(),
            sample_rate_hz: 48_000.0,
            ppm_correction: 7.5,
            lift_angle_deg: 44.0,
            bph_mode: "21600".to_string(),
            position: None,
            started_unix_s: 1_700_000_000,
            app_version: "test".to_string(),
            watch: None,
            summary: None,
        };
        let mut w = SessionWriter::create(dir.path(), meta).unwrap();
        w.push(&vec![0.0f32; 4_800]).unwrap();
        let wav_path = w.finalize().unwrap();
        std::fs::remove_file(wav_path.with_extension("json")).unwrap();

        let mut config = EngineConfig {
            lift_angle_deg: 52.0,
            averaging_s: 30.0,
            bph_mode: BphMode::Auto,
            ppm_correction: 0.0,
        };
        let (source, info) =
            SourceRuntime::build(&SourceSpec::ReplayFile { path: wav_path }, &mut config)
                .expect("replay build succeeds");
        assert_eq!(source.kind(), SourceKind::Replay);
        assert_eq!(config.lift_angle_deg, 52.0);
        assert_eq!(config.ppm_correction, 0.0);
        assert_eq!(config.bph_mode, BphMode::Auto);
        assert_eq!(info, None);
    }

    // -------------------------------------------------------------------
    // SkewTracker (M4 Task 7 — binding spec §3.4's NTP cross-check).
    // -------------------------------------------------------------------

    /// Deterministic, bounded ±2 ms jitter (no real RNG needed): a sine
    /// whose period (2π/0.7 ≈ 9 samples) doesn't evenly divide the 300-
    /// sample test span, so it doesn't correlate suspiciously well with the
    /// linear trend by construction alone. Verified numerically before
    /// writing these tests (not just assumed): at this exact frequency the
    /// jitter biases the recovered slope by under 0.01 ppm — nowhere near
    /// the ±0.5 ppm tolerance below (a scan across other frequencies/phases
    /// found worst cases over 1 ppm, so this specific frequency choice is
    /// deliberate, not incidental).
    fn synthetic_jitter(i: u32) -> f64 {
        0.002 * (i as f64 * 0.7).sin()
    }

    /// Pushes `n` once-per-second samples (wall = 1.0, 2.0, ..., n) built
    /// from a target `slope` (fractional, e.g. 25e-6 for +25 ppm) plus
    /// `synthetic_jitter`.
    fn push_synthetic_series(t: &mut SkewTracker, n: u32, slope: f64) {
        for i in 1..=n {
            let wall = i as f64;
            t.push(wall, wall + wall * slope + synthetic_jitter(i));
        }
    }

    #[test]
    fn skew_tracker_recovers_a_positive_slope_despite_jitter() {
        let mut t = SkewTracker::default();
        push_synthetic_series(&mut t, 300, 25e-6);
        let skew = t.skew().expect("300s span clears the 120s gate");
        assert!(
            (skew.ppm - 25.0).abs() <= 0.5,
            "expected ppm within ±0.5 of 25.0, got {}",
            skew.ppm
        );
        assert!(
            (skew.span_s - 299.0).abs() < 1e-9,
            "span is the last sample's wall time minus the first's (300.0 - 1.0), got {}",
            skew.span_s
        );
    }

    #[test]
    fn skew_tracker_recovers_a_negative_slope_despite_jitter() {
        let mut t = SkewTracker::default();
        push_synthetic_series(&mut t, 300, -18e-6);
        let skew = t.skew().expect("300s span clears the 120s gate");
        assert!(
            (skew.ppm - (-18.0)).abs() <= 0.5,
            "expected ppm within ±0.5 of -18.0, got {}",
            skew.ppm
        );
    }

    #[test]
    fn skew_tracker_recovers_a_zero_slope_despite_jitter() {
        let mut t = SkewTracker::default();
        push_synthetic_series(&mut t, 300, 0.0);
        let skew = t.skew().expect("300s span clears the 120s gate");
        assert!(
            skew.ppm.abs() <= 0.5,
            "expected ppm within ±0.5 of 0.0, got {}",
            skew.ppm
        );
    }

    #[test]
    fn skew_tracker_withholds_below_the_120s_span_gate() {
        let mut t = SkewTracker::default();
        // 60 once-per-second samples starting at wall=1.0 span only 59s
        // (last sample 60.0 minus the first, 1.0) — below the gate.
        push_synthetic_series(&mut t, 60, 25e-6);
        assert_eq!(
            t.skew(),
            None,
            "a 59s span must not publish (binding spec §3.4: min 120s)"
        );
    }

    #[test]
    fn skew_tracker_publishes_at_exactly_the_120s_boundary() {
        let mut t = SkewTracker::default();
        t.push(0.0, 0.0);
        t.push(120.0, 120.0); // span exactly 120.0 — the documented ">= 120s"
        assert!(
            t.skew().is_some(),
            "span exactly 120s must already publish (gate is inclusive)"
        );
    }

    #[test]
    fn skew_tracker_reset_clears_accumulated_state() {
        let mut t = SkewTracker::default();
        push_synthetic_series(&mut t, 300, 25e-6);
        assert!(
            t.skew().is_some(),
            "sanity: a reading must be available before reset"
        );
        t.reset();
        assert_eq!(
            t.skew(),
            None,
            "reset must clear every accumulated regression sum"
        );
        // A fresh push after reset establishes its OWN origin/span from
        // scratch — proof this isn't just a lucky `None` from stale state
        // that happens to still fail the gate.
        t.push(500.0, 500.0);
        assert_eq!(
            t.skew(),
            None,
            "a single post-reset sample has zero span, regardless of its wall value"
        );
    }
}
