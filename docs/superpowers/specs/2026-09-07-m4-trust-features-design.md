# Chrona M4 — Trust Features (Design)

**Status:** draft for user review. Implements the binding spec's M4 slice (§3.2 core,
§3.3, §3.4, §6 scope view + pattern legend) **split by verifiability**: everything
headless-testable lands now; Mic Doctor's OS/platform layer (Windows effects
enumeration, programmatic gain control, volume-change listeners, guided first-launch
flow polish) is **M5**, where the user's real-hardware iteration drives it. Also
discharges the accumulated debt queue (m3/m4a followups).

**Authority:** `2026-08-20-chrona-timegrapher-design.md` remains binding; this document
selects and details its M4 items and records deviations in §10. The M4a UI conventions
(palette, cards, modals, banner severities, honesty states) carry over unchanged.

## 1. Scope summary

IN: debt queue (§2) · calibration wizard + NTP cross-check (§3) · scope view (§4) ·
Mic Doctor signal-analysis core (§5) · classic-pattern legend (§6) · local-time display
(§7) · new-dependency decisions (§8) · protocol/docs updates (§9).
OUT (→ M5): Mic Doctor steps 4–5's OS checks, gain writeback, effects/Bluetooth
detection, first-launch auto-run; raw WASAPI path; positions beyond what M4a shipped.

## 2. Debt queue (first-week, in order)

1. **`parabolic3` unification** (fifth deferral — leads the milestone). One
   `pub(crate) fn parabolic3(y: [f64; 3]) -> f64` in `chrona-dsp` (new `interp.rs` or
   in `autocorr.rs`), consumed by autocorr/events/cal; byte-equivalent behavior proven
   by keeping all existing tests green PLUS a direct unit test; the three local copies
   deleted. DSP is touched ⇒ the release-mode invariant net (`--ignored`) is a gate for
   every M4 task that follows in the same files.
2. **Engine survives total startup failure** (m3-followups #2, robust form): when the
   initial source build fails AND the default-input retry fails, the engine loop stays
   ALIVE with a `SourceRuntime::Idle` no-op source (publishes empty snapshots + the
   Error banner) so `SwitchSource`/config messages can still recover. `run_headless`
   unaffected. Tests: bogus-device engine start → snapshot has Error banner AND a
   subsequent `SwitchSource(Simulate)` recovers to Tier 3 (fully headless-testable —
   this was impossible to test before precisely because the loop died).
3. **Recording tee surfaces write errors**: first `writer.push` failure → Warn banner
   "recording write failed: <e> — recording stopped", writer dropped (finalize
   attempted best-effort), recording state cleared. External fault injection is not
   portable here (POSIX writes to an already-open fd succeed even after unlink), so
   `SessionWriter` gains a **test-only injection seam**: `#[cfg(test)]
   fail_push_after: Option<u64>` making `push` return an Err after N samples. The
   engine test records on Simulate with the seam armed and asserts the Warn banner +
   recording cleared + no panic.
4. **Overrun banner re-baselines** like ClipTracker: a watermark reset on source
   rebuild + a 5 s quiet window (mirror `ClipTracker`, generalize it to
   `CountWatermark` used by both). Tests mirror ClipTracker's.
5. **Replay path alignment**: sidecar-value clamping moves INTO `chrona_session::replay`
   (lift/ppm sanitized exactly like the engine path, with the engine consuming the
   session crate's result); `chrona_session::replay`'s hard-Err-on-weird-values
   semantics replaced by the tolerant clamp+note behavior. Engine keeps its own clamp
   as defense-in-depth. Existing tests updated; v1/v2 sidecars unaffected.
6. **`assert_no_alloc` in the audio callback** (debug builds only): dev-dependency-ish
   guard — `assert_no_alloc` crate behind `#[cfg(debug_assertions)]` in `chrona-audio`,
   wrapping the cpal data callback body. CI's debug test job exercises it via the
   existing synth-driven capture tests (no real mic needed — the callback discipline
   is what's guarded, and the simulate path doesn't use cpal, so the guard's real
   proof is the mic path at manual-protocol time + the wrapper's own unit test that
   an alloc inside `assert_no_alloc` panics in debug).
7. **Sweep refactor** (M2 carryover): amplitude_sweep/beat_error_sweep collect
   failures then assert (pattern already mandated by AGENTS.md).
8. **`fold_envelope` empty-bin epsilon** (M2 carryover): near-integer env-sample
   periods leave empty bins → fill with the neighbor-mean and cap contrast; guarded by
   a unit test at the pathological period; real-pipeline behavior unchanged (proven by
   the untouched fast suite + stress matrix).
9. **Spec text syncs**: §7 ordering fallback (started_unix_s → sidecar mtime), §5
   "Select watch" copy, §4 crossbeam→std::sync::mpsc line, M4a-§14 entries for the
   first two — one docs commit against the two spec files.

## 3. Calibration wizard + NTP cross-check (binding spec §3.4)

- **Wizard flow** (modal sequence, M4a modal conventions): intro (what/why, "clamp any
  quartz watch to the mic; ≥ 5 min, 10–15 recommended") → capture (records via the
  existing engine tee to a temp WAV; live elapsed + level; Cancel aborts cleanly;
  countdown to minimum) → analysis (runs `chrona_dsp::cal::calibrate_quartz` on the
  captured file OFF the UI thread — a one-shot `std::thread` with a result channel,
  engine untouched) → result (ppm ± residual, quality verdict per CalError variants;
  "Save for <device>" writes `device_ppm` exactly like the manual popup; "Discard").
  The temp WAV is deleted after analysis regardless of outcome.
- **Honesty:** the result screen shows the fit residual and the reference-quartz caveat
  (floor = the quartz's own error, per binding spec). `CalError::{NoTicks, Unstable,
  BadInput}` map to plain-language failures with next steps, never a fabricated ppm.
- **Headless testability:** the wizard's analysis/result/save stages are tested by
  feeding a `synthesize_quartz` WAV directly (capture stage skipped); the capture
  stage's engine plumbing is tested on the Simulate source (flow, cancel, temp-file
  cleanup — cal reporting NoTicks/Unstable on a watch signal is itself the
  honest-failure path under test).
- **NTP cross-check** (binding spec §3.4, display-only in v1): the engine samples
  `(SystemTime wall elapsed, frames_delivered / sr_nominal)` once per second on the
  mic source, linear-regresses skew over a growing window (min 120 s before showing
  anything), and publishes `clock_skew_ppm: Option<f64>` + fit span in the snapshot
  health. UI: a faint line in the cal popup — "system-clock cross-check: −3.2 ppm over
  14 min" with a "use as correction" button (explicit, never automatic). Resets on
  source rebuild. Simulate/replay sources publish None (their delivery is synthetic).
  Tests: regression math unit-tested with synthetic (t, frames) series incl. jitter;
  the 120 s gate; reset-on-rebuild.

## 4. Scope view (binding spec §6: "stacked per-beat waveform, ~20 ms, pulse markers")

- **Data path:** new `Analyzer::beat_scope(n) -> Vec<BeatScope>` — for the newest `n`
  (≤ 16) events, a ~25 ms window of the FILTERED native-rate signal centered on the
  drop edge, decimated ×4 for display (~300 points/beat), plus marker offsets
  (unlock, drop) in window-relative ms. Extraction reuses the raw ring; windows whose
  samples have been evicted are omitted (never zero-padded — honesty). Cost: ~16 ×
  1200-sample copies on demand.
- **Engine:** scope data is pulled only when the UI has the scope panel open — a
  `ControlMsg::SetScope(bool)`; when on, the engine attaches `Vec<BeatScope>` to each
  snapshot (10 Hz × ~5 KB — bounded). Default off.
- **UI:** a collapsible "Scope" section between the amplitude strip and the bottom
  grid: stacked traces (newest at top, up to 8 rendered), tick/tock tinted per parity,
  unlock/drop markers as thin vertical lines with labels, a per-beat SNR readout
  (from `BeatEvent::snr_db`). Empty state: "scope needs detected beats". The section
  header carries the legend.
- Tests: extraction windows/markers pinned against synth events (marker-to-edge
  distances match the analyzer's own event times); eviction omission; decimation
  length; the SetScope gating at the engine level (snapshot carries scope iff on).

## 5. Mic Doctor — signal-analysis core (binding spec §3.2 steps 1–3 + score; OS layer → M5)

New `chrona-dsp` module `doctor.rs` (pure, synth-testable) + a Doctor panel (opened
from the toolbar; M5 adds first-launch auto-run):

- **Silence test** (step 1): over a user-triggered 5 s capture: noise floor RMS (dBFS),
  spectrum via the existing FFT machinery; hum detection = peak lattice at 50/60 Hz ±
  harmonics exceeding the local floor by 12 dB → reported as "mains hum (50 Hz)" with
  strength. Pure fn: `fn silence_report(samples, sr) -> SilenceReport`.
- **Tick test** (step 2): 10 s with the watch in place → band SNR (reuse the analyzer's
  quality path on a throwaway Analyzer), clipping count, achieved tier. Pure wrapper.
- **AGC/pumping detection** (step 3): short-window RMS series of inter-tick gaps
  (gap = between detected events); AGC signature = gap-floor modulating in lockstep
  with the beat period (dip after each tick, exponential recovery — detected by
  correlating the gap-RMS series against a post-tick-indexed template); gate/suppressor
  signature = gap floor collapsing below (floor − 20 dB) between ticks.
  `fn agc_report(env, events, t_beat) -> AgcReport { agc: Option<Strength>, gate: Option<Strength> }`.
  **Synth gains an AGC simulation knob** (`SynthConfig::agc_depth/agc_recovery_s`,
  applied as a post-tick gain envelope) so both signatures have ground-truth tests —
  the same proven-by-synthesis pattern every DSP feature here uses.
- **Setup score + ranked fixes** (step 5, minus OS-specific fixes): pure scoring fn
  over the three reports → 0–100 + ranked advice strings from the binding spec's list
  (placement, gain, wired mic, piezo pickup). OS-specific advice slots exist but emit
  nothing until M5 fills the detectors.
- **Doctor panel UI:** three guided steps with run buttons, live progress, results as
  pass/warn/fail rows, the score, and the advice list. All state honesty-gated: a step
  not run shows "not run", never a guessed verdict. Capture uses the live mic source's
  existing stream (no second stream): the doctor observes the same sample feed via a
  short engine-side capture request (`ControlMsg::DoctorCapture { seconds }` → one-shot
  buffer attached to a snapshot when complete). The capture plumbing works on ANY
  source, so engine-level tests run it on Simulate (headless); the pure analysis fns
  test against synth with the new AGC/hum knobs.

## 6. Classic-pattern legend (binding spec §6, missing since M3)

A "Patterns" help modal (from the beat-trace header's "?"): 6 canonical tape patterns,
each a small painter-drawn schematic (pure geometry fns, testable) + one-line meaning:
straight level line (healthy, on-rate) · sloped line (fast/slow — slope direction per
our up-is-fast convention) · two parallel bands (beat error) · wavy/periodic (gear or
pivot fault — period hints at the wheel) · scattered/no line (dirty escapement or bad
signal — check Mic Doctor) · sudden vertical jumps (rebanking or external knock).
Content reviewed against the horology references in the binding spec; wording must not
overclaim diagnosis ("suggests", "check").

## 7. Local-time display (m4a-followups #1)

`session_time_label` and `day_label` switch to real local time via **jiff** (§8).
UTC-approximation doc comments removed; tests use fixed zones via jiff's TZ override
to stay deterministic in CI (`TZ` env or explicit `TimeZone` parameter — the fns take
an injected `TimeZone` so tests never depend on machine state; callers pass
`TimeZone::system()`).

## 8. New dependencies (deliberate, recorded)

- **jiff** (workspace: chrona-app) — local-time display (§7). Chosen over `time`
  (soundness-gated local_offset in threaded contexts) and `chrono` (heavier tree);
  pure Rust, bundled tzdb, injectable zones for tests.
- **assert_no_alloc** (chrona-audio, debug-only cfg) — §2.6.
- Nothing else. `rfd`, `hound`, `realfft` etc. unchanged. Every other §-item uses
  existing machinery.

## 9. Protocol & docs

`docs/manual-testing.md`: A gains a scope-view check (open Scope, see stacked beats
with markers) and a patterns-legend check; B gains the Doctor flow (silence + tick +
AGC steps against the real mic) and the calibration wizard against a real quartz watch
(this REPLACES the CLI-calibrate wording in B.7 — the CLI path remains and is
mentioned as the alternative); a note that history times are now local. README feature
list updated. AGENTS.md: doctor/scope pinned strings if any are load-bearing;
m3/m4a-followups discharge annotations for every §2 item.

## 10. Deviations & non-goals (recorded)

1. Binding spec §3.3's programmatic OS gain control and volume-change listeners → M5
   (platform layer). The headroom meter concept is folded into Doctor's tick test +
   the existing clip banner for now.
2. Binding spec §3.2 steps 4–5's OS checks (effects enumeration, Bluetooth refusal,
   input-volume listeners) → M5. The doctor's advice slots for them exist but stay
   empty.
3. First-launch auto-run of Mic Doctor → M5 (needs the OS layer to be worth running).
4. NTP cross-check is display-plus-explicit-adopt only (no silent override) — as the
   binding spec already mandates; the "NTP-disciplined" caveat is shown (we measure
   against the system clock, whatever disciplines it).
5. Corpus fixtures (44.1 kHz / high-BPH real recordings) remain blocked on real
   recordings existing; unchanged.

## 11. Testing summary

Everything in §2–§6 lands with headless tests (synth-driven where signal-dependent:
AGC knob, hum injection via synth noise + sine addition — synth gains
`mains_hum_hz/mains_hum_level` too). DSP-touching tasks re-run the release invariant
net. The frozen headless line and all M4a pins remain byte-identical. Platform/manual
surface: only the Doctor's real-mic value and the wizard's real-quartz value — the
protocol carries both.
