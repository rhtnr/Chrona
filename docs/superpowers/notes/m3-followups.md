# M3 follow-ups (from the M3 final whole-branch review, 2026-08-23)

Everything here was reviewed and deliberately deferred — none of it blocked the M3 merge.
Feed into M4 planning (spec §12.4: Mic Doctor, calibration wizard UI, scope view,
positions summary/export). Ordered by priority.

## M4 first-week debt (named tickets, in order)

1. **`parabolic3` unification — fourth deferral, first in line.** Triplicated in
   autocorr/events/cal since M2; promote one `pub(crate)` copy. The M3 plan ruled it out
   of the T11 batch ("no excuses the fourth time" was M2's wording; this is the fourth
   time).
   **DISCHARGED by M4 (Task 1, commits a5fdb14 + 42a215f):** one shared
   `pub(crate) fn parabolic3` core landed in `chrona-dsp/src/interp.rs`, consumed by
   `autocorr.rs`/`fold.rs`/`cal.rs` (a5fdb14) and, once Step 1's survey turned up a
   FOURTH byte-identical copy the M2 debt note itself had dropped, `events.rs` too
   (42a215f) — five deferrals in, zero copies left. Byte-equivalence proven by the
   release-mode stress matrix (`--ignored`) passing unchanged plus a direct unit test on
   the core; see task-1-report.md.
2. **Engine survives total startup failure.** M3's fix wave added config sanitization and
   a default-input fallback, but if the *retry* also fails (no working input device at
   all, or out-of-domain `--simulate` values) the engine thread still publishes its fault
   banner and exits — the UI runs but every control send goes to a dead channel until app
   restart. Keep the loop alive with a degraded/no-op source so `SwitchSource` can
   recover. (Final-review finding 2, robust form; minimum form shipped in M3.)
   **DISCHARGED by M4 (Task 3, commit fa502f9):** a total startup failure (initial build
   AND the default-input retry both fail) now leaves the engine loop alive on a
   `SourceRuntime::Idle` no-op source — publishes empty snapshots plus the Error banner,
   and a subsequent `SwitchSource`/config message can still recover, headlessly tested
   (bogus-device start → Error banner → `SwitchSource(Simulate)` recovers to Tier 3). See
   `engine.rs`'s `SourceRuntime::Idle` and task-3-report.md.
3. **Typed severity on `EngineSnapshot` banners.** All `engine_banner` strings render
   error-red today — including the informational "replay: using recorded settings (…)"
   line a protocol-A.5 tester sees. One `enum BannerSeverity { Info, Warn, Error }` field
   fixes replay-info tone, "recording stopped" notice tone, and the red-tinted overrun
   case at once. (T10 deferred minor, priority raised by the final review.)
   **DISCHARGED by M4a (Task 3, commit b1afab3):** `BannerSeverity { Info, Warn, Error }`
   landed on `Banner`/`EngineSnapshot` exactly as described; the replay-info and
   recording-stopped banners now render `Info` (accent-colored, not red) and only
   genuine faults render `Error` (red) — see `engine.rs`'s `Banner`/`BannerSeverity` and
   `app.rs::render_banner`.
4. **Collect-then-assert sweep refactor** (M2 carryover) — the inline-assert loop pattern
   masks later sweep points on first failure; applies to amplitude_sweep,
   beat_error_sweep, and future sweeps.
   **DISCHARGED by M4 (Task 2, commit de376b3):** `amplitude_sweep_meets_the_bar` and
   `beat_error_sweep_meets_the_bar` (`chrona-dsp/src/metrics.rs`) now collect
   `(case, failure)` pairs and assert emptiness once at the end instead of asserting
   inline mid-loop; same cases, same thresholds.
5. **`fold_envelope` empty-bin epsilon** (M2 carryover) — near-integer env-sample folding
   periods leave zero bins → absurd contrast; not reachable through the real 48 kHz
   pipeline but the function is public.
   **DISCHARGED by M4 (Task 2, commit de376b3):** empty bins are now filled with the
   circular mean of their nearest non-empty neighbors (`chrona-dsp/src/fold.rs`),
   dropping contrast from ~1e12 to ~10 on the pathological period a new regression test
   constructs directly; every pre-existing fold test (incl. the release-mode
   octave-guard stress matrix) passes unchanged, proving no real-pipeline regression.

## App/engine debt from the final review

- **Recording tee swallows write errors** — `let _ = writer.push(...)`: disk-full mid-
  recording silently loses samples until finalize's error surfaces. Surface a Warn banner
  on first push failure.
  **DISCHARGED by M4 (Task 4, commit d9c2d99):** the first `writer.push` failure now
  drops the writer (best-effort finalize attempted), clears `recording`, and surfaces a
  Warn banner ("recording write failed: … — recording stopped") — never the Error-tier
  fault banner. Armed via a test-only `chrona-session` feature
  (`test-fault-injection` + `CHRONA_TEST_FAIL_PUSH_AFTER`, since POSIX writes to an
  already-unlinked fd still succeed, making external fault injection non-portable here);
  a follow-up rider (confirmed GREEN in M4 Task 7, see task-7-report.md) proved a fresh
  `StartRecording` after the failure starts cleanly, not just fails cleanly once. See
  `crates/chrona-app/tests/engine.rs`'s
  `recording_write_failure_surfaces_warn_banner_and_stops_recording`.
- **Overrun Info banner latches forever** once `overruns > 0` (cumulative counter, no
  re-baseline). Give it a ClipTracker-style window or a re-baseline on source rebuild.
  **DISCHARGED by M4 (Task 4, commit d9c2d99):** `ClipTracker` generalized into
  `CountWatermark` (a watermark reset on source rebuild + a 5 s quiet re-baseline
  window), reused by both the clip counter and the overrun counter — see `controls.rs`'s
  `CountWatermark`/`WATERMARK_WINDOW`.
- **Two replay paths, divergent semantics**: `chrona_session::replay` applies sidecar
  values raw (out-of-range lift → hard Err) while the engine path clamps/tolerates; the
  app only uses the engine path, and `chrona_session::replay` is otherwise library/test
  only. Align them (probably: move the clamping into chrona-session and have the engine
  consume it).
  **DISCHARGED by M4 (Task 5, commit 598d3e8):** `sanitize_lift`/`sanitize_ppm` moved
  into a new `chrona_session::sanitize` module (one implementation instead of three
  near-copies); `chrona_session::replay`'s sidecar handling now clamps through it (via a
  new `resolve_ppm_lift` helper) instead of hard-`Err`ing the whole replay over one
  out-of-range field, and `engine.rs`'s own `ReplayFile` arm — which does NOT go through
  `chrona_session::replay` at all, so keeps its own clamp as defense-in-depth — now calls
  the same shared `sanitize_lift`/`sanitize_ppm` rather than hand-rolling the bounds a
  third time. Exactly the "move the clamping into chrona-session" resolution this item
  proposed. See task-5-report.md's "Step 1: replay-path alignment".
- **Sidecar schema vs spec §8**: the spec lists "computed per-beat event table + metrics
  timeline" and "device ID/name" in the sidecar; schema_version 1 stores neither table
  and only the device *name*. Deliberate (replay recomputes from the WAV; exports are M4)
  but previously unrecorded — recorded here. Revisit when M4 adds positions
  summary/export.
- **Spec §6 classic-pattern legend** (wavy = gear fault etc.) — help overlay not built,
  and it was missing from M3's recorded-simplifications list until the final review.
  **DISCHARGED by M4 (Task 13, commit dd75b06):** a "Classic tape patterns" help modal,
  opened from the beat-trace header's "?" button, renders all six canonical shapes
  (level / sloped / two parallel bands / wavy / scattered / sudden jumps) each with a
  small painter-drawn schematic and the spec's meaning, hedged per the M4 design doc's
  "must not overclaim diagnosis" instruction. See `ui/patterns.rs`.
- **Spec §9 `assert_no_alloc`** for the audio callback — not wired; the no-alloc
  discipline is manual + documented only. Add the debug-build guard in M4.
  **DISCHARGED by M4 (Task 5, commit e572936):** the cpal data-callback body
  (`chrona-audio`'s `process_callback`) now runs inside `assert_no_alloc::
  assert_no_alloc(..)` under `cfg(debug_assertions)`; release builds call it unwrapped.
  Stated honestly, not oversold: `AllocDisabler` (the required global allocator for
  enforcement to actually fire) is installed only for `chrona-audio`'s OWN test binary
  (`cfg(all(test, debug_assertions))`) — installing it unconditionally would force every
  downstream debug binary (`chrona-app` included) onto this allocator via
  `#[global_allocator]`'s whole-program linkage. Everywhere else the guard is
  present-but-inert, not enforcing; a debug-gated `pub use assert_no_alloc::
  AllocDisabler` re-export documents the opt-in recipe for a consumer later. See
  `chrona-audio/src/lib.rs` and `capture.rs`, and task-5-report.md's "Step 2".
- **Spec §4 says crossbeam** for the control channel; `std::sync::mpsc` shipped
  (plan-specified, works fine). Amend the spec line in M4's first spec touch.
  **DISCHARGED by M4 (Task 5, commit c7e1f78):** the 2026-08-20 binding spec's §4 now
  reads `` `std::sync::mpsc` channel `` with an **Amended:** changelog line, matching
  `engine.rs`'s actual `use std::sync::mpsc;` since M3.
- **Mic-source startup fallback has no automated test** (would fire macOS TCC prompts
  during `cargo test`; suite is deliberately hardware-free). Covered by manual protocol
  B; an injectable device-enumeration seam would make it testable — consider with Mic
  Doctor.

## Accepted minors riding to M4 (from task reviews, re-confirmed by the final review)

- **DSP/analyzer**: span source `env_ring.len()` vs trimmed window (≤0.33 ms, pre-fill
  transient); `averaging_s > 32` silently ring-truncated (doc line); octave-flip latency
  ≤1 s (sanctioned refold cadence); `events::Parity` not re-exported at crate root;
  per-poll ~6 MB copy sits before the work gate (p50 4.37 ms has 2× margin).
- **Audio**: `widest` naming in devices.rs (first-f32-range, not widest); capture scratch
  hardcoded 8192 (one-time alloc on pathological backends).
- **Session**: same-second recording filename collision (two Start-Records inside one
  second); civil doc-comment nit.
- **Engine**: recording_path re-derivation duplicates the civil format at two sites;
  apply_config clears unrelated banners; dead stream freezes `silent_for_s` (last_error
  banner outranks it); unused pull_tick return; source build blocks the tick ≤ bounded
  load time; SourceKind enum order churn; fault snapshot carries last-known (not live)
  recording path; config-burst rebuilds don't coalesce; theoretical drain-all starvation;
  publish double-clone (~20 KB @ 10 Hz).
- **UI**: amplitude_cell wildcard arm absorbs future gate variants (drop wildcard if the
  enum grows); blank no-nominal tape lacks a caption (**DISCHARGED by M4a:** the
  redesigned beat-trace/amplitude charts always show an explicit empty-state caption —
  "no beat grid — select or detect a beat rate", "listening…", or "amplitude requires
  Tier 3 · <reason>" — see `ui/charts.rs`'s `empty_state` calls); per-metric confidence badges +
  signal/headroom meters are a recorded deliberate simplification → M4 Mic Doctor;
  ClipTracker same-poll clip+reset false-negative (count watermark; engine timestamps fix
  it); "System default" selection sends SetPpm(0.0) (code correct, wording was off);
  per-frame device-list clone; ~1-frame 00:00 flicker at Stop; no ppm clamp test on the
  sidecar path (lift twin is tested); README App-section label style; `format_rate` shows
  "-0.0 s/d" for tiny negative estimates (`{:+.1}` sign preservation — cosmetic).
- **CI**: `libgtk-3-dev` in the apt list likely redundant (harmless).

## Still-open M2 carryovers (see docs/superpowers/notes/m2-followups.md for full context)

- Cal seed tick unrefined vs parabolic (≈0.01 ppm effect).
- "Period σ: 0.0 µs" formatting for sub-0.05 µs values (M1-era).
- Octave pinning test asserts !halved but not gate-engagement; probe-ratio margin at
  toc_gain 0.6 is a behavioral pin, not a sensitivity pin.
- T8 thin-data test covers the toc-deficient guard branch only; det threshold 1e-9 is
  absolute.
- T5 refactors: cluster_centroids/significant_clusters dedup, named probe fn, dead factor
  param, double sort.
- The "misc accepted characteristics" list in m2-followups still stands as accepted.

## Corpus notes (unchanged from M2, still open)

- When real recordings land: at least one 44.1 kHz fixture and one high-BPH
  (43200/72000) fixture — all proven numbers are synthetic 48 kHz except two ad-hoc
  probes.

## Process notes

- **First real CI run: DISCHARGED 2026-08-23** (runs 32638681413 → 32639565470, all 4
  jobs green). Two adjustments were needed, both landed: the apt list gained
  `libpipewire-0.3-dev` (cpal 0.18 pulls the PipeWire host on Linux via libspa-sys),
  and the perf bar is 30 ms under `CI` (shared 2-core runners measured p50 12.9 ms vs
  4.4 ms local; the guarded regression class costs ~90 ms anywhere). The octave stress
  matrix passed on Linux — the platform-FP-drift failure M2 warned about did not occur.
- M4 chore: bump `actions/checkout@v4` → v5 (GitHub deprecation warnings for its
  Node 20 runtime on every job).
- Manual protocol (docs/manual-testing.md) is the acceptance gate for everything
  hardware-dependent: live mic, TCC permission, hot-unplug, per-device ppm persistence.
