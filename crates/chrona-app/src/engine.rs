//! Engine thread: DSP pipeline, capture/simulate/replay sources, and
//! snapshot publishing (spec §7/§9).
//!
//! One `std::thread` owns everything DSP-adjacent: the active source (mic
//! consumer / simulate generator / replay data), the `Analyzer`, an
//! optional recording `SessionWriter`, and the input side of a
//! `triple_buffer::TripleBuffer<EngineSnapshot>`. The UI thread never
//! touches the `Analyzer` directly — see [`Engine::snapshot`] for why that
//! matters. Control flows in one direction, UI → engine, over an
//! `std::sync::mpsc` channel; state flows the other way, engine → UI, over
//! the triple buffer, at ~10 Hz.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use chrona_audio::{CaptureStream, StreamInfo};
use chrona_dsp::synth::{SynthConfig, synthesize};
use chrona_dsp::{Analyzer, AnalyzerConfig, AnalyzerError, BphMode, MetricsSnapshot, TapeEvent};
use chrona_session::{SessionMeta, SessionReader, SessionWriter};
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
    },
    StopRecording,
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

/// Live health counters, folded into every published snapshot.
#[derive(Debug, Clone, Default)]
pub struct HealthView {
    pub overruns: u64,
    pub clipped: u64,
    pub silent_for_s: f64,
    pub last_error: Option<String>,
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
    pub error_banner: Option<String>,
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
                        error_banner: Some(format!("analysis thread fault: {detail}")),
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
}

impl SourceRuntime {
    fn build(spec: &SourceSpec) -> Result<SourceRuntime, String> {
        match spec {
            SourceSpec::Mic { device_id } => CaptureStream::start(device_id.as_deref(), MIC_RING_S)
                .map(SourceRuntime::Mic)
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
                    .map(|buffer| SourceRuntime::Simulate {
                        buffer,
                        pos: 0,
                        sample_rate_hz,
                    })
                    .map_err(|e| format!("simulate: {e}"))
            }
            SourceSpec::ReplayFile { path } => SessionReader::open(path)
                .map(|r| SourceRuntime::Replay {
                    samples: r.samples,
                    pos: 0,
                    sample_rate_hz: r.sample_rate_hz,
                    done: false,
                })
                .map_err(|e| format!("replay {}: {e}", path.display())),
        }
    }

    fn sample_rate_hz(&self) -> f64 {
        match self {
            SourceRuntime::Mic(s) => s.info.sample_rate_hz,
            SourceRuntime::Simulate { sample_rate_hz, .. } => *sample_rate_hz,
            SourceRuntime::Replay { sample_rate_hz, .. } => *sample_rate_hz,
        }
    }

    fn kind(&self) -> SourceKind {
        match self {
            SourceRuntime::Mic(_) => SourceKind::Mic,
            SourceRuntime::Simulate { .. } => SourceKind::Simulate,
            SourceRuntime::Replay { .. } => SourceKind::Replay,
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
        }
    }

    fn replay_done(&self) -> bool {
        matches!(self, SourceRuntime::Replay { done: true, .. })
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

fn bph_mode_to_string(mode: BphMode) -> String {
    match mode {
        BphMode::Auto => "auto".to_string(),
        BphMode::Free => "free".to_string(),
        BphMode::Fixed(n) => n.to_string(),
    }
}

/// Applies a config change atomically: builds the candidate `Analyzer`
/// first, and only commits it (and the new config value) if that succeeds.
/// A rejected candidate (e.g. lift angle out of [10, 90]) leaves both the
/// running analyzer and `config` untouched, rather than corrupting `config`
/// with a value that never actually took effect.
fn apply_config(
    config: &mut EngineConfig,
    analyzer: &mut Analyzer,
    sample_rate_hz: f64,
    error_banner: &mut Option<String>,
    mutate: impl FnOnce(&mut EngineConfig),
) {
    let mut candidate = *config;
    mutate(&mut candidate);
    match build_analyzer(&candidate, sample_rate_hz) {
        Ok(a) => {
            *config = candidate;
            *analyzer = a;
            *error_banner = None;
        }
        Err(e) => *error_banner = Some(format!("invalid config: {e}")),
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
/// to `writer` if recording and folding it into the silence tracker.
/// Returns the number of samples pulled (0 for a starved mic, or a replay
/// that has already finished).
fn pull_tick(
    source: &mut SourceRuntime,
    analyzer: &mut Analyzer,
    writer: &mut Option<SessionWriter>,
    silence: &mut SilenceTracker,
) -> usize {
    match source {
        SourceRuntime::Mic(stream) => {
            let avail = stream.consumer.slots();
            if avail == 0 {
                return 0;
            }
            let Ok(chunk) = stream.consumer.read_chunk(avail) else {
                return 0;
            };
            let (a, b) = chunk.as_slices();
            analyzer.push_samples(a);
            analyzer.push_samples(b);
            if let Some(w) = writer {
                let _ = w.push(a);
                let _ = w.push(b);
            }
            let sr = stream.info.sample_rate_hz;
            silence.push(a, sr);
            silence.push(b, sr);
            let n = a.len() + b.len();
            chunk.commit_all();
            n
        }
        SourceRuntime::Simulate {
            buffer,
            pos,
            sample_rate_hz,
        } => {
            let n = sim_chunk_len(*sample_rate_hz);
            let chunk = read_looping(buffer, pos, n);
            analyzer.push_samples(&chunk);
            if let Some(w) = writer {
                let _ = w.push(&chunk);
            }
            silence.push(&chunk, *sample_rate_hz);
            chunk.len()
        }
        SourceRuntime::Replay {
            samples,
            pos,
            sample_rate_hz,
            done,
        } => {
            if *done {
                return 0;
            }
            let n = ((REPLAY_CHUNK_S * *sample_rate_hz).round() as usize).max(1);
            let end = (*pos + n).min(samples.len());
            let chunk = &samples[*pos..end];
            analyzer.push_samples(chunk);
            if let Some(w) = writer {
                let _ = w.push(chunk);
            }
            silence.push(chunk, *sample_rate_hz);
            let pulled = chunk.len();
            *pos = end;
            if *pos >= samples.len() {
                *done = true;
            }
            pulled
        }
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
    let mut source = match SourceRuntime::build(&initial) {
        Ok(s) => s,
        Err(e) => {
            // No source at all — nothing to run. Publish the fault and
            // return; the thread exits cleanly (not a panic, so the
            // catch_unwind wrapper in `Engine::start_with_turbo` sees Ok
            // and does not double-publish).
            publish(
                buf_input,
                last_snapshot,
                &egui_ctx,
                EngineSnapshot {
                    error_banner: Some(format!("failed to start: {e}")),
                    ..Default::default()
                },
            );
            return;
        }
    };
    let mut analyzer = match build_analyzer(&config, source.sample_rate_hz()) {
        Ok(a) => a,
        Err(e) => {
            publish(
                buf_input,
                last_snapshot,
                &egui_ctx,
                EngineSnapshot {
                    error_banner: Some(format!("invalid initial config: {e}")),
                    ..Default::default()
                },
            );
            return;
        }
    };

    let mut writer: Option<SessionWriter> = None;
    let mut recording_path: Option<PathBuf> = None;
    let mut silence = SilenceTracker::default();
    let mut error_banner: Option<String> = None;
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
                        let _ = w.finalize();
                    }
                    break 'outer;
                }
                ControlMsg::SetLift(v) => apply_config(
                    &mut config,
                    &mut analyzer,
                    source.sample_rate_hz(),
                    &mut error_banner,
                    |c| c.lift_angle_deg = v,
                ),
                ControlMsg::SetAveraging(v) => apply_config(
                    &mut config,
                    &mut analyzer,
                    source.sample_rate_hz(),
                    &mut error_banner,
                    |c| c.averaging_s = v,
                ),
                ControlMsg::SetBphMode(m) => apply_config(
                    &mut config,
                    &mut analyzer,
                    source.sample_rate_hz(),
                    &mut error_banner,
                    |c| c.bph_mode = m,
                ),
                ControlMsg::SetPpm(v) => apply_config(
                    &mut config,
                    &mut analyzer,
                    source.sample_rate_hz(),
                    &mut error_banner,
                    |c| c.ppm_correction = v,
                ),
                ControlMsg::SwitchSource(spec) => match SourceRuntime::build(&spec) {
                    Ok(new_source) => {
                        let sr = new_source.sample_rate_hz();
                        match build_analyzer(&config, sr) {
                            Ok(a) => {
                                source = new_source;
                                analyzer = a;
                                silence = SilenceTracker::default();
                                // A mid-recording switch must not keep
                                // teeing the old source's (now
                                // semantically wrong, possibly
                                // different-rate) audio into the WAV —
                                // stop and finalize it, and say so. When
                                // nothing was recording, a successful
                                // switch clears any stale error as usual
                                // (this banner would otherwise be a false
                                // "recording stopped" notice every time
                                // the source changes).
                                if let Some(w) = writer.take() {
                                    let _ = w.finalize();
                                    recording_path = None;
                                    error_banner =
                                        Some("recording stopped: source changed".to_string());
                                } else {
                                    error_banner = None;
                                }
                            }
                            Err(e) => {
                                error_banner = Some(format!("invalid config for new source: {e}"))
                            }
                        }
                    }
                    Err(e) => error_banner = Some(format!("failed to switch source: {e}")),
                },
                ControlMsg::StartRecording { dir, meta_position } => {
                    if let Some(w) = writer.take() {
                        let _ = w.finalize();
                    }
                    let started_unix_s = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0);
                    let meta = SessionMeta {
                        schema_version: 1,
                        device_name: source.device_name(),
                        sample_rate_hz: source.sample_rate_hz(),
                        ppm_correction: config.ppm_correction,
                        lift_angle_deg: config.lift_angle_deg,
                        bph_mode: bph_mode_to_string(config.bph_mode),
                        position: meta_position,
                        started_unix_s,
                        app_version: env!("CARGO_PKG_VERSION").to_string(),
                    };
                    let path = dir.join(format!(
                        "chrona-{}.wav",
                        chrona_session::civil::timestamp_compact(started_unix_s)
                    ));
                    match SessionWriter::create(&dir, meta) {
                        Ok(w) => {
                            writer = Some(w);
                            recording_path = Some(path);
                            error_banner = None;
                        }
                        Err(e) => error_banner = Some(format!("failed to start recording: {e}")),
                    }
                }
                ControlMsg::StopRecording => {
                    if let Some(w) = writer.take()
                        && let Err(e) = w.finalize()
                    {
                        error_banner = Some(format!("failed to finalize recording: {e}"));
                    }
                    recording_path = None;
                }
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
            pull_tick(&mut source, &mut analyzer, &mut writer, &mut silence);

            tick += 1;
            if tick.is_multiple_of(SNAPSHOT_EVERY) {
                let metrics = analyzer.current_metrics();
                let tape = analyzer.tape_events();
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
                        error_banner: error_banner.clone(),
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
    let config = EngineConfig {
        lift_angle_deg: 52.0,
        averaging_s: 30.0,
        bph_mode: BphMode::Auto,
        ppm_correction: 0.0,
    };
    let mut source = match SourceRuntime::build(&spec) {
        Ok(s) => s,
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
    let mut silence = SilenceTracker::default();

    let ticks = ((seconds / TICK_S).ceil() as u64).max(1);
    for _ in 0..ticks {
        pull_tick(&mut source, &mut analyzer, &mut writer, &mut silence);
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
}
