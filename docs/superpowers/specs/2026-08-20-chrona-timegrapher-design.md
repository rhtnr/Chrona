# Chrona — Cross-Platform Software Timegrapher: Design Specification

- **Date:** 2026-08-20
- **Status:** Approved design (egui native GUI, full tiered metric scope)
- **Repo:** empty at time of writing; this spec is the project's founding document

## 1. Overview

Chrona is a desktop timegrapher written in Rust for macOS, Windows, and Linux. It listens
to a mechanical watch through any microphone — laptop built-in, external USB, or a piezo
contact pickup — and displays, live:

- **Rate** in seconds/day
- **Beat error** in milliseconds
- **Amplitude** in degrees (given a lift angle)
- **Beat rate** (BPH), auto-detected
- The classic scrolling **paper-tape trace** (two-color tic/toc dot plot)
- A **per-beat scope view** of the tick waveform (Witschi-style diagnostics)

Its differentiators against existing software timegraphers:

1. **Honest, tiered output.** Metrics appear only when the signal supports them, each with
   a confidence badge. A weak laptop-mic signal yields a trustworthy rate instead of a
   full panel of fabricated numbers.
2. **Mic Doctor.** A setup assistant that measures the input chain (noise floor, clipping,
   AGC/noise-suppression artifacts, Bluetooth transport) and tells the user exactly what
   to fix, per OS, with a setup score.
3. **First-class timebase calibration.** Consumer audio clocks are off by up to ±100 ppm;
   1 s/day is only 11.6 ppm. Chrona treats the ADC clock as uncalibrated until corrected
   and says so on screen.

Benchmark target: match or beat the Weishi No. 1000 machine's published resolution
(rate ±999 s/d at 1 s/d, amplitude 100–360° at 1°, beat error 0–9.9 ms at 0.1 ms,
averaging 2–60 s) on software features; approach its accuracy after calibration.

### Non-goals (v1)

- No ML models (deferred to v2; see §13)
- No quartz-watch *measurement* mode (quartz is used only as a calibration reference;
  inhibition-type quartz needs different techniques)
- No tuning-fork (Accutron) support
- No mobile targets, no cloud features, no Bluetooth microphone support (actively refused
  with an explanation — HFP/SCO codecs cap audio at 8–16 kHz and destroy tick energy)
- No per-caliber database beyond a small lift-angle preset list

## 2. Domain background (what the DSP must know)

### 2.1 The tick is three pulses

Each beat of a Swiss lever escapement produces a cluster of three micro-impacts
(Witschi training documentation):

| Pulse | Event | Property | Used for |
|---|---|---|---|
| 1 | **Unlocking** — impulse pin strikes pallet fork | temporally precise, quiet | rate, beat error timestamps |
| 2 | **Impulse** — escape tooth slides on pallet face | irregular | nothing |
| 3 | **Drop/locking** — tooth lands + lever hits banking pin | loudest | detection anchor, amplitude endpoint |

The unlocking→drop spacing Δt spans the balance's traversal of the **lift angle** and is
amplitude-dependent, ranging roughly 5–15 ms at common beat rates. Individual impacts are
broadband clicks exciting mechanical resonances; useful energy for detection sits above
~1–3 kHz (tg high-passes at 3 kHz; contact-pickup chains use ~1–11 kHz; air-mic guidance
favors 10–20 kHz).

### 2.2 Beat arithmetic

For a movement rated at `bph` beats per hour:

- beat period `T_beat = 3600 / bph` seconds (28,800 bph → 125 ms)
- full oscillation `T_osc = 2 · T_beat = 7200 / bph` seconds
- oscillator frequency in Hz = `bph / 7200`

**Auto-detect BPH set** (Weishi + tg union): 12000, 14400, 17280, 18000, 19800, 21600,
25200, 28800, 36000, 43200, 72000. **Manual/vintage list** additionally includes the
Weishi manual's low-beat set (3600 … 21600 oddballs such as 12342, 14040, 16200, 17280).
A **free mode** accepts any detected period in the 12000–72000 bph band without snapping.

### 2.3 Metric definitions

- **Rate (s/d):** from the measured beat period `T̂` vs nominal `T_nom`:
  `rate = 86400 · (T_nom − T̂) / T_nom` (positive = fast). `T̂` comes from a least-squares
  fit of unlocking timestamps against beat index (regression, not naive averaging —
  Watch-O-Scope's documented improvement).
- **Beat error (ms):** alternate intervals tic→toc (`t1`) and toc→tic (`t2`) measured at
  unlocking pulses; `BE = |t1 − t2| / 2`. Display 0.0–9.9 ms at 0.1 ms.
- **Amplitude (°):** with lift angle `L` and intra-beat unlocking→drop interval `Δt`:

  ```
  A = L / (2 · sin(π · Δt / T_osc))
  ```

  Validity gates (from tg): accept only 135° ≤ A ≤ 360°; require tic-side and toc-side
  amplitudes to agree within 60°; report their mean. Worked check: 28,800 bph, L = 52°,
  Δt = 7 ms → A ≈ 296°.
- **Lift angle:** default 52°; numeric input 10–90° plus presets (52° — ETA 2824/2892,
  7750, Sellita SW200/300, Rolex 31xx/32xx; 38° — Omega Co-Axial; 42/44/50/53/55° —
  assorted vintage). Wrong lift angle scales only amplitude, never rate/beat error; the
  UI says so.
- **Healthy reference ranges** (shown as context in UI): rate −5…+15 s/d, amplitude
  250–330° horizontal / 220–270° vertical, BE ≤ 0.5 ms; < 200° amplitude or > 2 ms BE
  flagged as service indicators; > 330° flagged as possible knocking/rebanking.

## 3. The four hard problems and their strategies

### 3.1 Weak signal (built-in mics) → tiered metrics

Rate does not require resolving individual ticks: the beat period is recoverable from
autocorrelation of the band-passed envelope over long windows even at very low per-tick
SNR. Chrona therefore reports in tiers:

| Tier | Shows | Requires |
|---|---|---|
| 0 | setup guidance only | nothing measurable |
| 1 | rate + BPH | stable autocorrelation period (σ gate, §5.3) |
| 2 | + beat error | per-beat onsets detected in a supermajority of beats with low jitter |
| 3 | + amplitude | unlocking pulse resolved within anchored beats; amplitude gates pass |

Initial tier thresholds (tunable against the fixture corpus, exposed in a debug panel):
Tier 2 when ≥ 60 % of expected beats yield onsets and onset jitter σ ≤ 0.5 ms;
Tier 3 when ≥ 50 % of anchored beats yield a gated unlocking pulse. Metrics below their
tier display "—" plus the limiting reason ("signal too weak for amplitude — try a contact
mic"). Every displayed metric carries a confidence badge derived from detection ratio,
jitter, and SNR.

### 3.2 Hostile OS processing → raw capture paths + Mic Doctor

Research-established facts driving this design:

- **macOS:** voice processing (AEC/NS/AGC, Voice Isolation mic modes) applies only to
  apps using the VoiceProcessingIO unit. cpal uses the plain HAL unit → Chrona's capture
  is unprocessed by the OS. Residual: modern MacBook built-in mics have always-on
  firmware-level noise reduction that no app can bypass → Mic Doctor steers users toward
  external/contact mics for Tier 3 work.
- **Windows:** driver "enhancements" (APOs) are the main threat. v1 approach: use the
  effects-discovery API to *detect* active AGC/NS/AEC on our stream and instruct the user
  to disable enhancements (Settings → Sound → device → Audio enhancements off). A raw
  side-path (`AUDCLNT_STREAMOPTIONS_RAW` via the `wasapi` crate — cpal does not expose
  it) is deferred to v1.x if detection shows it's needed in practice.
- **Linux:** PipeWire/PulseAudio apply no NS/AEC/AGC by default; nothing to bypass.
- **Bluetooth inputs:** detected via device transport/form-factor properties and name
  heuristics; refused with an explanation.
- **Speech-trained ML denoisers (RNNoise, OS voice isolation) are anti-features** for
  impulsive clicks — they remove exactly this signal class. Chrona never applies them and
  Mic Doctor treats their presence as a fault to fix.

**Mic Doctor** (guided flow, runs on first launch and on demand):
1. Silence test → noise floor RMS + spectrum; hum detection (50/60 Hz + harmonics)
2. Tick test with the watch in place → band SNR estimate, clipping check
3. AGC/pumping detection: track short-window RMS of inter-tick gaps; an AGC signature is
   a noise floor modulating in lockstep with the tick period (dip after each tick,
   exponential recovery between ticks); a gate/suppressor signature is the floor
   collapsing to near-silence between ticks
4. OS-level checks: input-volume property changed by another process (change listeners);
   Windows effects enumeration; Bluetooth transport
5. Output: setup score, ranked fixes ("disable enhancements", "lower input gain",
   "use wired mic", "try pressing wired earbuds against the crown", "a $10 piezo contact
   pickup reaches Tier 3")

### 3.3 Too-hot inputs → clipping handled as an analog problem

Digital attenuation after ADC clipping is useless (the information is gone). Chrona:

- Detects clipping two ways: runs of ≥ 3 consecutive samples at/above −0.1 dBFS, and
  amplitude-histogram pile-up at the rails (catches upstream clipping delivered below
  full scale)
- Shows a headroom meter with a target zone (peaks ≈ −12 to −6 dBFS)
- Adjusts OS input gain programmatically where the device exposes a writable control
  (macOS `kAudioDevicePropertyVolumeScalar`; Windows `IAudioEndpointVolume` on the
  capture endpoint; Linux ALSA mixer capture elements), otherwise walks the user through
  the OS setting
- Registers volume-change listeners and warns if another app moves the gain mid-session

### 3.4 Timebase error → calibration as a feature

The ADC crystal (up to ±100 ppm off, ~1 ppm/°C drift) bounds absolute accuracy;
1 s/d = 11.57 ppm. Professional machines use a ±0.3 ppm TCXO. Chrona:

- Stores a **per-device ppm correction** applied as a corrected sample rate
  (`sr_eff = sr_nominal · (1 + ppm/10⁶)`)
- **Quartz calibration wizard:** clamp/record any quartz watch (1 Hz stepper tick) for
  ~10–15 minutes; linear-regress phase drift → ppm; show fit residual; store per device.
  (tg's proven method; floor = the reference quartz's own error, a few ppm unless the
  user knows its true rate and enters it.)
- **Manual ppm / s-per-day entry** for users with a known-good reference
- **Continuous NTP cross-check:** count delivered frames against the NTP-disciplined
  system clock; display the regressed audio-clock skew as a sanity indicator (qtg's
  method; ~0.1 ppm residual achievable). v1 displays it and offers "use this as
  correction"; it never silently overrides the stored calibration.
- Rate display carries an **"uncalibrated timebase"** badge until a correction exists,
  with estimated worst-case systematic error spelled out

## 4. Architecture

Cargo workspace; strict dependency direction (GUI → session/audio → dsp; never reverse):

```
crates/
  chrona-dsp       Pure DSP + metrics. No audio I/O, no OS deps. push-samples API.
  chrona-audio     cpal capture, device enumeration/hot-plug polling, input-health
                   monitor (clipping/AGC/Bluetooth), per-OS gain control adapters.
  chrona-session   Recording (WAV + JSON sidecar), replay, reanalysis, CSV/PNG export,
                   config persistence (calibration store, settings).
  chrona-app       egui/eframe GUI.
  chrona-cli       Headless analysis of WAV files; dev harness and CI regression runner.
fixtures/          Real + synthetic WAV files with expected-metrics JSON.
docs/superpowers/specs/   This document and successors.
```

**Per-crate contract (what / how used / depends on):**

- `chrona-dsp` — feed `f32` mono samples at a declared rate, receive typed events
  (`PeriodEstimate`, `BeatEvent { kind: Tic|Toc, t_unlock, t_drop, snr }`,
  `MetricsSnapshot { rate, beat_error, amplitude, bph, tier, confidence, … }`).
  Deterministic; same samples in → same events out. Depends on rustfft/realfft, biquad,
  apodize only.
- `chrona-audio` — open/close named devices, deliver sample chunks + health events
  (clip, silence, disconnect, gain-changed, effects-detected). Depends on cpal (+ small
  per-OS property shims). No DSP knowledge.
- `chrona-session` — owns the on-disk formats and the calibration/config store
  (platform config dir, TOML). Depends on chrona-dsp types, hound, serde.
- `chrona-app` — wires the above; owns threads and UI state. Nothing below it knows egui.
- `chrona-cli` — same wiring minus GUI; guarantees the pipeline stays runnable headless.

**Threading model (real-time discipline):**

```
[cpal callback]  --rtrb SPSC ring-->  [DSP thread]  --triple_buffer-->  [egui UI]
 no alloc/locks                        chunked pipeline                  reads latest
 push samples,                         (§5), publishes                  snapshot each
 set atomic flags                      MetricsSnapshot,                 frame; repaint
 (clip/overrun)                        requests repaint                 on publish
```

Raw audio is teed from the DSP thread to the session recorder (buffered file writes off
the audio thread). UI → DSP control messages (BPH override, lift angle, reset) go over a
crossbeam channel.

## 5. DSP pipeline (`chrona-dsp`)

Baseline algorithm adopted from tg (field-proven, GPL — **algorithms reimplemented from
the published description/analysis, no code copied**; Chrona is MIT/Apache-2.0
dual-licensed), then extended with the matched-filter gate and tier system.

1. **Precondition:** DC removal; 2nd-order Butterworth high-pass at 3 kHz (biquad);
   optional low-pass ~16 kHz.
2. **Envelope:** full-wave rectify → low-pass (~2–3 kHz) → decimate. Mean removal + Hann
   edge taper per analysis window. Optional noise gate at 2× running median.
3. **Beat-period estimation:** over stepped windows (~4, 8, 16, 32 s — keep the longest
   whose fit passes), autocorrelation via FFT → power spectrum → IFFT; search the
   full-oscillation lag band (1–12 Hz oscillators); shape-validating peak detector;
   harmonic disambiguation via integer-divisor peaks (≥ 90 % of fundamental) and octave
   checks; iterative per-cycle refinement (±2 %). **σ gate:** accept when period σ <
   period/10⁴. Snap to the BPH table unless free mode.
4. **Phase fold / tic-toc separation:** fold the envelope at `T_osc`; stack cycles with a
   trimmed mean (drop the top quintile — impulse-noise robustness); the two folded maxima
   are the tic and toc **drop-pulse anchors**.
5. **Per-beat event extraction (matched-filter gate):**
   - Learn a **tick template** per anchor by phase-aligned trimmed-mean averaging of the
     band-passed signal (not just the envelope) across recent beats; refresh continuously
   - Within a phase-predicted gate around each expected anchor, cross-correlate template
     vs signal (FFT overlap); peak = drop time; **sub-sample refinement** by parabolic
     interpolation of the correlation peak (48 kHz sample = 20.8 µs; interpolation takes
     per-event resolution well below that, and regression over hundreds of beats averages
     the rest)
   - **Unlocking pulse:** search the `T_beat/8` window *before* each drop anchor on the
     smoothed (~1 ms) envelope; noise floor from the `T_beat/8` window *after* the peak;
     threshold `max(1 % of global max, 1.4 × noise)`, escalating ×1.4 while below 20 % of
     max; leading-edge timestamp
6. **Metrics (§2.3):** rate via least-squares over a sliding window of unlocking
   timestamps (window = averaging setting, 2–60 s, default 30 s); BE from unlocking
   asymmetry; amplitude from Δt with the 135–360° and 60°-agreement gates; all through
   the calibration correction (§3.4).
7. **Quality/tier engine:** detection ratio, onset jitter σ, per-beat SNR, clip counter,
   AGC score (§3.2) → tier + per-metric confidence (§3.1).

**Explicitly rejected:** speech-oriented denoisers (would delete ticks); resampling in
the capture path (capture at native device rate; rates other than 48 kHz are handled by
parameterizing the pipeline, not by resampling near impulses).

## 6. GUI (`chrona-app`, egui/eframe)

Single-window instrument layout, custom `Painter` for the tape (egui_plot only for
secondary charts). Continuous repaint driven by DSP-thread publishes.

- **Main instrument view:** large numerals (rate, BE, amplitude, BPH) each with
  confidence badge and "—/reason" states; lift-angle control with presets; averaging
  window control; position selector (DU, DD, CU, CD, CL, CR); signal meter + tier badge;
  headroom meter; calibration badge.
  **Paper tape:** x = beat index, y = unlocking-timestamp deviation vs the nominal grid,
  wrapped; alternate beats colored (tic/toc) → two traces; slope = rate, separation =
  beat error; ms gridlines; adjustable y-scale; classic pattern legend in a help overlay
  (clean parallel lines = healthy; wavy = gear-train fault; scatter = dirty/low
  amplitude; doubled dots = knocking).
- **Scope view:** stacked per-beat waveform (~20 ms window) with detected pulse markers —
  the diagnostic view separating Chrona from phone apps.
- **Mic Doctor:** §3.2 flow with per-OS instructions and setup score.
- **Calibration:** §3.4 wizard + manual entry + NTP skew readout.
- **Sessions:** list, record, replay (re-run pipeline from WAV), position-set summary
  (six-position table with per-position rate/amp/BE and max Δrate), CSV/PNG export.

## 7. Capture layer (`chrona-audio`)

- cpal 0.18 (features: `pipewire` on Linux); request 48 kHz mono f32, fall back
  44.1 kHz → device default; surface the negotiated rate to the DSP
- Device picker from `supported_input_configs()`; stable device IDs; hot-plug via slow
  polling (cpal has no notification API); stream `error_callback` kinds drive
  auto-reconnect with a UI banner
- Input-health monitor (clip flags, RMS, silence watchdog) computed in the DSP thread
  from raw chunks; silence + open-stream on Windows triggers the "check Settings →
  Privacy → Microphone → desktop apps" hint
- Per-OS shims behind traits: gain get/set + change listener (CoreAudio property /
  `IAudioEndpointVolume` / ALSA mixer); transport/form-factor query for Bluetooth
  detection; Windows effects enumeration (AGC/NS/AEC active?)

## 8. Sessions & persistence (`chrona-session`)

- **Recording:** 32-bit float WAV (hound) of the raw capture + JSON sidecar (schema
  versioned): device ID/name, negotiated sample rate, calibration ppm at record time,
  lift angle, BPH mode, position, start time (UTC), app version, and the computed
  per-beat event table + metrics timeline
- **Replay:** any WAV (± sidecar) re-runs the full pipeline — the debugging and
  regression workhorse
- **Config:** platform config dir (`directories`), TOML: per-device calibration,
  UI prefs, tier thresholds (debug)
- **Export:** per-beat CSV; PNG snapshot of tape + numerals

## 9. Error handling

| Failure | Behavior |
|---|---|
| Mic permission denied (macOS TCC) | Blocking explainer with per-OS re-enable steps |
| Stream opens but delivers silence (Windows privacy toggle) | Targeted hint after 3 s of digital silence |
| Device disconnect / xrun | Error-kind-driven reconnect loop + banner; session recording closed cleanly |
| Unsupported sample rate | Fallback chain, then explicit unsupported-device message |
| Clipping | Meter turns red + Mic Doctor prompt; events logged into session |
| No beat found | Tier 0 guidance (not an error): placement tips, Mic Doctor link |
| Implausible metrics (gates fail) | "—" with reason; never display gated-out values |
| Panic in DSP thread | Caught at thread boundary; UI shows fault + offers session save; audio thread never panics (no alloc/unwrap discipline + `assert_no_alloc` in debug) |

## 10. Testing strategy

- **Synthetic ground-truth generator** (in `chrona-dsp` dev-deps): parametric ticks
  (bph, rate offset, beat error, amplitude → Δt via the §2.3 formula inverted, pulse
  shapes as damped resonant bursts) + noise models (white/pink, hum, speech-band babble,
  simulated AGC pumping, clipping). Property tests: recovered rate within ±0.5 s/d, BE
  within ±0.1 ms, amplitude within ±5° at defined SNRs; tier engine degrades in the
  correct order as SNR falls.
- **Fixture corpus:** real recordings (several movements × positions × mic types,
  including deliberately bad laptop-mic captures + a quartz reference for the cal
  wizard), with expected-metrics JSON where a hardware timegrapher reading exists.
  `chrona-cli` runs the corpus in CI with tolerance assertions.
- **Unit tests** per stage (filters, envelope, autocorrelation peak logic, gates,
  formula edge cases: Δt→0, A at gate boundaries, missing pulses).
- **CI:** GitHub Actions, 3-OS build + test (DSP tests are pure and run everywhere);
  clippy + fmt; release-profile build artifact job.
- Manual test protocol per release: live run on macOS + Windows with built-in mic and a
  contact mic; calibration wizard against a quartz watch.

## 11. Packaging & distribution

- `cargo-packager`: macOS `.app`/`.dmg`, Windows NSIS installer, Linux AppImage/`.deb`
- macOS: `NSMicrophoneUsageDescription` in Info.plist; hardened runtime **with
  `com.apple.security.device.audio-input` entitlement** (omitting it silently kills mic
  input in notarized builds — documented trap); Developer ID + notarization for
  distribution; dev builds from a terminal inherit the terminal's mic permission
- Windows: unsigned v1 accepts SmartScreen "More info → Run anyway"; in-app hint for the
  global desktop-apps mic toggle
- Linux: AppImage first; Flatpak deferred (no audio portal yet; would need static
  pipewire socket grant)

## 12. Milestones (implementation plan will decompose these)

1. **M1 — Pipeline proof, headless:** workspace scaffold; synthetic generator; DSP
   stages 1–3 (period estimation) + rate; `chrona-cli analyze` on fixtures; CI
2. **M2 — Full metrics:** stages 4–7, tier engine, calibration math; corpus tolerances
   green
3. **M3 — Live app:** capture layer, threading, main instrument view + tape; record/replay
4. **M4 — Trust features:** Mic Doctor, calibration wizard, clipping/gain control,
   scope view, positions/export
5. **M5 — Ship:** packaging/signing, docs, first release

## 13. Deferred (v2 candidates)

- Tiny learned tick-detector (CNN via `tract`, ONNX from PyTorch) trained on the session
  corpus the app accumulates — only if the classic pipeline shows a real gap in noisy
  Tier 1→2 promotion
- WASAPI raw-mode side-path (`wasapi` crate) if Windows effects detection shows
  enhancements users can't disable
- Quartz-watch measurement mode; caliber database; Flatpak; localization

## 14. Key references

- tg (open-source timegrapher, algorithm baseline): https://github.com/vacaboja/tg · https://tg.ciovil.li/
- Witschi, *Measuring Technology and Troubleshooting for Watches* (tick physics, metrics, trace patterns): https://myretrowatches.co.uk/wp-content/uploads/2021/11/Witschi-Training-Course.pdf
- Watch-O-Scope (DIY hardware chain + least-squares rate, calibration): https://www.watchoscope.com/
- qtg (NTP-referenced clock-skew regression): https://github.com/simonArchipoff/qtg
- Weishi No. 1000 manual (spec benchmark, BPH tables): https://m.media-amazon.com/images/I/71jxE9rxr4L.pdf
- Amplitude formula worked example: https://calibercorner.com/amplitude/
- Windows audio effects/raw mode: https://learn.microsoft.com/en-us/windows-hardware/drivers/audio/audio-signal-processing-modes
- macOS voice processing opt-in (WWDC23): https://developer.apple.com/videos/play/wwdc2023/10235/
- cpal 0.18: https://github.com/RustAudio/cpal · rtrb: https://github.com/mgeier/rtrb
- Existing Rust timegraphers (competitive context): https://github.com/ettoreferranti/timegrapherQ · https://github.com/lponiatowski/timegrapher
