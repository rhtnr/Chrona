//! cpal input streams behind a lock-free ring (spec §4). `CaptureStream::start`
//! negotiates a device config, spins up a cpal input stream, and hands back a
//! ring-buffer consumer plus a live health handle.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use thiserror::Error;

use crate::devices::{self, choose_config};
use crate::monoize::average_into;

/// Negotiated stream parameters, reported back to the caller once capture starts.
#[derive(Debug, Clone)]
pub struct StreamInfo {
    pub sample_rate_hz: f64,
    pub channels: u16,
    pub device_name: String,
}

#[derive(Debug, Error)]
pub enum CaptureError {
    #[error("no input device available")]
    NoDevice,
    #[error("input device not found: {id}")]
    DeviceNotFound { id: String },
    #[error("unsupported capture configuration: {detail}")]
    Unsupported { detail: String },
    #[error("audio backend error: {detail}")]
    Backend { detail: String },
}

#[derive(Debug, Default)]
struct HealthInner {
    overruns: AtomicU64,
    callbacks: AtomicU64,
    last_error: Mutex<Option<String>>,
}

/// Live counters for a running (or stopped) capture stream. Cheaply `Clone`
/// (an `Arc` handle): the data callback holds one clone to bump atomics, the
/// error callback holds another to record the last error, and callers hold
/// the `CaptureStream::health` clone to read both from the UI thread.
///
/// The inner `Mutex` guards only `last_error`, and only the error callback
/// and UI reads ever touch it — the data callback never locks anything.
#[derive(Debug, Clone)]
pub struct CaptureHealth {
    inner: Arc<HealthInner>,
}

impl CaptureHealth {
    fn new() -> Self {
        CaptureHealth {
            inner: Arc::new(HealthInner::default()),
        }
    }

    pub fn overruns(&self) -> u64 {
        self.inner.overruns.load(Ordering::Relaxed)
    }

    pub fn callbacks(&self) -> u64 {
        self.inner.callbacks.load(Ordering::Relaxed)
    }

    pub fn last_error(&self) -> Option<String> {
        self.inner
            .last_error
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn bump_callback(&self) {
        self.inner.callbacks.fetch_add(1, Ordering::Relaxed);
    }

    fn add_overrun(&self, dropped: u64) {
        self.inner.overruns.fetch_add(dropped, Ordering::Relaxed);
    }

    fn set_error(&self, message: String) {
        let mut guard = self
            .inner
            .last_error
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *guard = Some(message);
    }
}

/// A running capture stream: negotiated config, the ring-buffer consumer to
/// drain, and a live health handle. Dropping it stops the underlying cpal
/// stream.
pub struct CaptureStream {
    pub info: StreamInfo,
    pub consumer: rtrb::Consumer<f32>,
    pub health: CaptureHealth,
    _stream: cpal::Stream,
}

impl Drop for CaptureStream {
    fn drop(&mut self) {
        // cpal streams stop when dropped on every backend, but pausing first
        // makes the stop explicit rather than relying on that implicitly.
        // Best-effort: we're tearing down, so a pause failure is not actionable.
        let _ = self._stream.pause();
    }
}

impl CaptureStream {
    /// Start capturing from `device_id` (or the host default if `None`) into
    /// a ring buffer sized for `ring_seconds` of audio at the negotiated rate.
    ///
    /// Negotiation order: 48 kHz mono f32, then 44.1 kHz f32, then the
    /// device's own default f32 config (falling back further to the widest
    /// f32 range the device reports if even the default isn't f32).
    /// Multi-channel devices are mono-ized (averaged) in the callback. `f32`
    /// only in M3 — a device with no f32 input config at all is
    /// `Unsupported` (i16 conversion is M4+ YAGNI).
    pub fn start(
        device_id: Option<&str>,
        ring_seconds: f64,
    ) -> Result<CaptureStream, CaptureError> {
        let host = cpal::default_host();
        let device = match device_id {
            Some(id) => devices::find_by_id(&host, id)
                .ok_or_else(|| CaptureError::DeviceNotFound { id: id.to_string() })?,
            None => host.default_input_device().ok_or(CaptureError::NoDevice)?,
        };

        let (rate_hz, channels) = negotiate(&device)?;
        let device_name = device
            .description()
            .map(|d| d.name().to_string())
            .unwrap_or_else(|_| "unknown".to_string());

        let ring_len = ((rate_hz * ring_seconds).round() as usize).max(1);
        let (mut producer, consumer) = rtrb::RingBuffer::new(ring_len);
        let health = CaptureHealth::new();

        // Reserved once, up front: `average_into`'s clear()+extend fills back
        // into this capacity every callback without ever reallocating, since
        // no single cpal buffer callback should exceed this many mono
        // samples per channel.
        let mut scratch: Vec<f32> = Vec::with_capacity(8_192);

        let config = cpal::StreamConfig {
            channels,
            sample_rate: rate_hz as u32,
            buffer_size: cpal::BufferSize::Default,
        };

        let data_health = health.clone();
        let error_health = health.clone();
        let stream = device
            .build_input_stream(
                config,
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    // M4 Task 5 (spec §2.6): guards the real-time audio
                    // callback against accidental allocation in debug
                    // builds. See `lib.rs`'s `ALLOC_GUARD` doc comment for
                    // where this is actually armed (chrona-audio's own
                    // debug test binary only) versus merely present; and
                    // `process_callback`'s own tests for the direct proof.
                    #[cfg(debug_assertions)]
                    assert_no_alloc::assert_no_alloc(|| {
                        process_callback(
                            data,
                            channels as usize,
                            &mut scratch,
                            &mut producer,
                            &data_health,
                        );
                    });
                    #[cfg(not(debug_assertions))]
                    process_callback(
                        data,
                        channels as usize,
                        &mut scratch,
                        &mut producer,
                        &data_health,
                    );
                },
                move |err: cpal::Error| error_health.set_error(err.to_string()),
                None,
            )
            .map_err(|e| CaptureError::Backend {
                detail: e.to_string(),
            })?;
        stream.play().map_err(|e| CaptureError::Backend {
            detail: e.to_string(),
        })?;

        Ok(CaptureStream {
            info: StreamInfo {
                sample_rate_hz: rate_hz,
                channels,
                device_name,
            },
            consumer,
            health,
            _stream: stream,
        })
    }
}

/// The data-callback body: mono-average `data` into `scratch`, then drain
/// `scratch` into the ring, counting anything the ring couldn't hold as an
/// overrun. Split out of the cpal closure in `CaptureStream::start` (M4
/// Task 5) for two reasons: it's the exact call that closure now wraps in
/// the debug-only `assert_no_alloc` guard, and — the reason it's worth
/// naming — it's directly callable from a test with a synthetic
/// ring/health pair, no cpal `Device`/`Stream` (no audio hardware) needed
/// at all. `scratch`/`producer`/`health` are all long-lived, reused every
/// call (never rebuilt here) — matching `CaptureStream::start`'s own
/// once-per-stream construction of each — which is exactly what makes the
/// steady-state body allocation-free: `average_into`'s `clear()`+`extend`
/// fills `scratch`'s pre-reserved capacity in place (see its own doc
/// comment), and `rtrb`'s ring never allocates after construction either.
fn process_callback(
    data: &[f32],
    channels: usize,
    scratch: &mut Vec<f32>,
    producer: &mut rtrb::Producer<f32>,
    health: &CaptureHealth,
) {
    health.bump_callback();
    average_into(data, channels, scratch);
    let requested = scratch.len().min(producer.slots());
    let written = match producer.write_chunk_uninit(requested) {
        Ok(chunk) => chunk.fill_from_iter(scratch.iter().copied()),
        Err(_) => 0,
    };
    let dropped = scratch.len() - written;
    if dropped > 0 {
        health.add_overrun(dropped as u64);
    }
}

/// Pick a negotiation target for `device` from its reported f32 input ranges.
fn negotiate(device: &cpal::Device) -> Result<(f64, u16), CaptureError> {
    let f32_ranges: Vec<cpal::SupportedStreamConfigRange> = device
        .supported_input_configs()
        .map_err(|e| CaptureError::Backend {
            detail: e.to_string(),
        })?
        .filter(|range| range.sample_format() == cpal::SampleFormat::F32)
        .collect();

    let Some(widest) = f32_ranges.first().copied() else {
        return Err(CaptureError::Unsupported {
            detail: "device offers no f32 input configuration".to_string(),
        });
    };

    let mut candidates: Vec<(f64, u16)> = Vec::new();
    for range in &f32_ranges {
        if range.contains_rate(48_000) {
            candidates.push((48_000.0, range.channels()));
        }
        if range.contains_rate(44_100) {
            candidates.push((44_100.0, range.channels()));
        }
    }

    let default = device
        .default_input_config()
        .ok()
        .filter(|c| c.sample_format() == cpal::SampleFormat::F32)
        .map(|c| (c.sample_rate() as f64, c.channels()))
        .unwrap_or((widest.max_sample_rate() as f64, widest.channels()));

    Ok(choose_config(&candidates, default))
}

// Module-level `debug_assertions` gate (not just per-`#[test]` fn): every
// test in this module exists to exercise the alloc guard, which is itself
// only meaningfully testable in debug (see the tests' own doc comments) —
// gating here once, rather than on each fn, also avoids an unused-import
// warning on `use super::*` in a hypothetical `cargo test --release` run.
#[cfg(all(test, debug_assertions))]
mod tests {
    use super::*;

    /// M4 Task 5: proves the no-alloc guard is actually armed in
    /// chrona-audio's own debug test binary (`lib.rs`'s `ALLOC_GUARD`,
    /// `cfg(all(test, debug_assertions))`) — a deliberate allocation inside
    /// `assert_no_alloc` is detected.
    ///
    /// This does NOT use `#[should_panic]`. Read from `assert_no_alloc`'s
    /// own source (`~/.cargo/registry/.../assert_no_alloc-1.1.2/src/
    /// lib.rs`) before writing this: with only the crate's default
    /// features (`disable_release`), a violation calls
    /// `std::alloc::handle_alloc_error`, which *aborts the whole process*
    /// — not a catchable `panic!`. Using that default here would take down
    /// the entire test binary (every other test running in the same
    /// process) the moment this one ran, not fail this test cleanly. The
    /// `warn_debug` feature (this crate's `[dev-dependencies]` entry —
    /// unioned in for test builds only, see Cargo.toml's comment) swaps
    /// that abort for a counted, in-process-observable violation instead
    /// (`assert_no_alloc::violation_count()`), which is what's actually
    /// safe to assert on in a test — and is a strictly more precise proof
    /// than "did not panic" would have been anyway.
    #[test]
    fn assert_no_alloc_flags_a_forbidden_allocation() {
        assert_no_alloc::reset_violation_count();
        assert_no_alloc::assert_no_alloc(|| {
            let v: Vec<u8> = Vec::with_capacity(64); // deliberate alloc
            std::hint::black_box(v);
        });
        assert!(
            assert_no_alloc::violation_count() > 0,
            "a Vec::with_capacity(64) inside assert_no_alloc should have tripped the guard"
        );
    }

    /// M4 Task 5: the real callback body, fed a synthetic chunk under the
    /// guard, does not allocate. Mirrors `CaptureStream::start`'s own
    /// recipe (ring buffer + a pre-reserved scratch buffer, both built
    /// once and reused) without any cpal `Device`/`Stream` — hardware-free,
    /// same spirit as this crate's other pure-logic tests
    /// (`devices.rs`/`monoize.rs`). Reads `violation_count()` rather than
    /// relying on "did not panic" for the same reason as the test above.
    #[test]
    fn process_callback_does_not_allocate_in_steady_state() {
        let (mut producer, consumer) = rtrb::RingBuffer::<f32>::new(4_096);
        let mut scratch: Vec<f32> = Vec::with_capacity(8_192); // pre-reserved, as in `start`
        let health = CaptureHealth::new();

        // Two-channel interleaved input, mirroring a real stereo device.
        let data: Vec<f32> = (0..2_048).map(|i| (i % 7) as f32 * 0.01).collect();

        assert_no_alloc::reset_violation_count();
        assert_no_alloc::assert_no_alloc(|| {
            process_callback(&data, 2, &mut scratch, &mut producer, &health);
        });
        assert_eq!(
            assert_no_alloc::violation_count(),
            0,
            "process_callback's steady-state path must not allocate"
        );

        assert_eq!(health.callbacks(), 1);
        assert_eq!(scratch.len(), 1_024, "1024 stereo frames averaged to mono");
        assert_eq!(
            consumer.slots(),
            1_024,
            "all 1024 mono samples fit in the 4096-capacity ring"
        );
    }
}
