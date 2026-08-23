# M3 follow-ups (from the M3 final whole-branch review, 2026-08-23)

Everything here was reviewed and deliberately deferred — none of it blocked the M3 merge.
Feed into M4 planning (spec §12.4: Mic Doctor, calibration wizard UI, scope view,
positions summary/export). Ordered by priority.

## M4 first-week debt (named tickets, in order)

1. **`parabolic3` unification — fourth deferral, first in line.** Triplicated in
   autocorr/events/cal since M2; promote one `pub(crate)` copy. The M3 plan ruled it out
   of the T11 batch ("no excuses the fourth time" was M2's wording; this is the fourth
   time).
2. **Engine survives total startup failure.** M3's fix wave added config sanitization and
   a default-input fallback, but if the *retry* also fails (no working input device at
   all, or out-of-domain `--simulate` values) the engine thread still publishes its fault
   banner and exits — the UI runs but every control send goes to a dead channel until app
   restart. Keep the loop alive with a degraded/no-op source so `SwitchSource` can
   recover. (Final-review finding 2, robust form; minimum form shipped in M3.)
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
5. **`fold_envelope` empty-bin epsilon** (M2 carryover) — near-integer env-sample folding
   periods leave zero bins → absurd contrast; not reachable through the real 48 kHz
   pipeline but the function is public.

## App/engine debt from the final review

- **Recording tee swallows write errors** — `let _ = writer.push(...)`: disk-full mid-
  recording silently loses samples until finalize's error surfaces. Surface a Warn banner
  on first push failure.
- **Overrun Info banner latches forever** once `overruns > 0` (cumulative counter, no
  re-baseline). Give it a ClipTracker-style window or a re-baseline on source rebuild.
- **Two replay paths, divergent semantics**: `chrona_session::replay` applies sidecar
  values raw (out-of-range lift → hard Err) while the engine path clamps/tolerates; the
  app only uses the engine path, and `chrona_session::replay` is otherwise library/test
  only. Align them (probably: move the clamping into chrona-session and have the engine
  consume it).
- **Sidecar schema vs spec §8**: the spec lists "computed per-beat event table + metrics
  timeline" and "device ID/name" in the sidecar; schema_version 1 stores neither table
  and only the device *name*. Deliberate (replay recomputes from the WAV; exports are M4)
  but previously unrecorded — recorded here. Revisit when M4 adds positions
  summary/export.
- **Spec §6 classic-pattern legend** (wavy = gear fault etc.) — help overlay not built,
  and it was missing from M3's recorded-simplifications list until the final review.
- **Spec §9 `assert_no_alloc`** for the audio callback — not wired; the no-alloc
  discipline is manual + documented only. Add the debug-build guard in M4.
- **Spec §4 says crossbeam** for the control channel; `std::sync::mpsc` shipped
  (plan-specified, works fine). Amend the spec line in M4's first spec touch.
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
