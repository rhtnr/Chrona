# M4 Trust Features Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Land the M4 trust features — debt queue, calibration wizard + NTP cross-check, scope view, Mic Doctor signal-analysis core, pattern legend, local-time display — everything headless-verifiable, per the M4 design spec.

**Architecture:** DSP additions (parabolic core, doctor analysis, beat_scope, synth knobs) stay pure and synth-tested; engine gains four bounded features (idle-survival source, NTP skew sampler, doctor capture, scope gating) without touching the M3 threading model; UI adds three modals/panels in the M4a conventions. Two new dependencies, both deliberate (spec §8).

**Tech Stack:** unchanged + `jiff` (chrona-app, local time), `assert_no_alloc` (chrona-audio, debug-gated use).

**Spec:** docs/superpowers/specs/2026-09-07-m4-trust-features-design.md (this milestone's decisions) arguing from docs/superpowers/specs/2026-08-20-chrona-timegrapher-design.md §3.2–3.4/§6 (binding). M4a UI conventions per docs/superpowers/specs/2026-08-23-ui-redesign-design.md.

## Global Constraints

- Gates for EVERY task: `cargo test --workspace` (0 failures), `cargo fmt --all` then `-- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, frozen headless pin `cargo run -p chrona-app -- --simulate --rate 12 --beat-error 0.8 --amplitude 270 --headless-seconds 45` → exactly `TIER 3 · full  rate +12.0 s/d ⚠ uncal  beat error 0.8 ms  amplitude 270°`, exit 0. Tasks touching `chrona-dsp` ADDITIONALLY run `cargo test -p chrona-dsp --release -- --ignored` (3 pass).
- Frozen decisions (AGENTS.md) hold: period tolerances, octave constants, native-rate edges, is_finite-before-clamp, no real-mic streams in tests, byte-exact presenter pins.
- Honesty (§3.1): never a fabricated number; "not run"/"—" + reason everywhere below capability.
- All UI colors via `theme::Palette`; app compiles AND runs after every task; report-integrity: TDD claims must match the transcript.
- The M4a accumulator/chart/watch surfaces are regression-frozen: their tests must stay green untouched unless a task names them.

---

### Task 1: `parabolic3` core unification (chrona-dsp)

**Files:** Create: `crates/chrona-dsp/src/interp.rs`. Modify: `crates/chrona-dsp/src/lib.rs` (mod), `autocorr.rs`, `fold.rs`, `cal.rs`.

**Interfaces (produced):** `pub(crate) fn parabolic3(a: f64, b: f64, c: f64) -> f64` — vertex offset in `[-0.5, 0.5]`-ish of the parabola through `(-1,a),(0,b),(1,c)`: `0.5·(a−c)/(a−2b+c)`, with the degenerate/denominator guard UNIFIED ONLY IF the three existing copies agree.

- [ ] **Step 1: Read all three implementations FIRST** — `cal.rs:34 parabolic3`, `autocorr.rs:50 parabolic_offset` (slice-indexed, has a clamp; see its `parabolic_clamp_engages_on_pathological_shape` test), `fold.rs:31 circular_parabolic` (circular indexing). Write down, in the module doc of `interp.rs`, exactly what differs (indexing, clamping, degenerate handling). The core takes the three RAW values; each site keeps a thin wrapper preserving its indexing and any site-specific clamp EXACTLY. If the copies' degenerate-denominator behavior differs, the core is the pure formula and each wrapper keeps its own guard.
- [ ] **Step 2: Failing test** in interp.rs: `parabolic3(0.0, 1.0, 0.0) == 0.0`; `parabolic3(0.5, 1.0, 0.9)` ≈ `0.5·(0.5−0.9)/(0.5−2+0.9)` = `0.3(3)` (compute the literal in the test); symmetric-flip antisymmetry `parabolic3(a,b,c) == -parabolic3(c,b,a)` for a sample triple.
- [ ] **Step 3: Implement + rewire the three sites.** ZERO behavior change allowed: the entire fast suite AND the release invariant net prove it (that's the point of doing this refactor under the stress matrix's protection).
- [ ] **Step 4: Gates incl. release `--ignored`.** **Step 5: Commit** `refactor(dsp): unify parabolic vertex interpolation into one core`

---

### Task 2: DSP test-pattern debt — collect-then-assert sweeps + fold empty-bin epsilon (chrona-dsp)

**Files:** Modify: `crates/chrona-dsp/src/metrics.rs` (sweep tests), `crates/chrona-dsp/src/fold.rs`.

- [ ] **Step 1:** Rewrite `amplitude_sweep_meets_the_bar` and `beat_error_sweep_meets_the_bar` (find exact names; metrics.rs tests) to collect `(case, err)` failures into a Vec and assert emptiness at the end printing ALL failures — behavior-identical pass criteria.
- [ ] **Step 2 (RED first):** fold empty-bin test: construct a fold at a period that is an exact integer number of env samples (e.g. period_env = 250.0 exactly with 250 bins-worth of data) such that some of the 512 bins receive zero samples; assert `fold_envelope`'s output has no NaN/inf and `contrast` is finite and ≤ a sane cap (e.g. < 1e6). Expected today: absurd contrast (~3.2e11 per m2-followups) → RED.
- [ ] **Step 3:** Fix: empty bins get the mean of their two nearest non-empty neighbors (circular); document. The real 48 kHz pipeline is unaffected (fractional periods fill all bins) — the fast suite + release net prove no regression.
- [ ] **Step 4: Gates incl. release.** **Step 5: Commit** `fix(dsp): fold empty-bin fill; collect-then-assert sweeps`

---

### Task 3: Engine survives total startup failure (idle source)

**Files:** Modify: `crates/chrona-app/src/engine.rs`, `crates/chrona-app/tests/engine.rs`.

**Interfaces:** internal `SourceRuntime::Idle` variant — delivers no samples, `sample_rate_hz() = 48_000.0` (analyzer built against it stays inert), kind maps to the last-attempted SourceKind.

- [ ] **Step 1 (failing test):** `engine_survives_total_startup_failure_and_recovers`: `Engine::start_with_turbo(Mic { device_id: Some("chrona-bogus-device") }, …)`; within 10 s a snapshot carries an Error banner (existing behavior); THEN send `SwitchSource(Simulate{rate:12,…})` and within 60 s a snapshot reaches Tier 3 — impossible today (the loop is dead), so RED.
- [ ] **Step 2:** Restructure `engine_loop`'s initial-build failure arms (both source-build and analyzer-build, incl. the mic-fallback double-failure) to fall through into the main loop with `SourceRuntime::Idle` + the Error banner retained in every published snapshot until a successful SwitchSource/apply_config replaces it. `run_headless` untouched (it keeps its exit-2 semantics — verify with the existing flags tests). Auto-finalize/Drop paths must tolerate Idle (no writer can exist).
- [ ] **Step 3: Gates** (this is engine-threading-adjacent: re-run the full engine test file 3× locally to shake flakiness). **Step 4: Commit** `fix(app): engine loop survives total startup failure with an idle source`

---

### Task 4: Tee write-error surfacing + overrun re-baseline (engine + session + controls)

**Files:** Modify: `crates/chrona-session/src/writer.rs` (test seam), `crates/chrona-app/src/engine.rs`, `crates/chrona-app/src/ui/controls.rs`, `crates/chrona-app/tests/engine.rs`.

**Interfaces:** `SessionWriter` gains `#[cfg(test)] pub fail_push_after: Option<u64>` (per spec §2.3 — but note the seam must be reachable from chrona-app's tests: `cfg(test)` in chrona-session is NOT visible to chrona-app's test build ⇒ use a cargo feature instead: `chrona-session` feature `test-fault-injection`, enabled by chrona-app's `[dev-dependencies]` entry; the field is `pub` under that feature only). `CountWatermark { last: u64, last_hit: Option<Instant>, window: Duration }` in controls.rs — generalizes ClipTracker; ClipTracker becomes `CountWatermark` with a 5 s window; overruns get their own instance (5 s window, re-baselined on source rebuild via the existing rebuild path that resets the clip tracker — find it and mirror).

- [ ] **Step 1 (failing tests):** (a) engine test: start Simulate turbo, arm the fault seam via a writer constructed with `fail_push_after: Some(4800)` — the engine's StartRecording path constructs the writer internally, so the seam is threaded via `ControlMsg::StartRecording` only in test builds? NO — simplest honest seam: a feature-gated `EngineConfig` test knob is ugly; instead put the injection on the SESSION side keyed by env var under the feature: `CHRONA_TEST_FAIL_PUSH_AFTER` read in `SessionWriter::create` when the feature is on. Test sets the env var, records, asserts within 5 s: Warn banner containing "recording write failed", `snapshot().recording == None`, no panic. (b) controls test: `CountWatermark` overrun case — count jumps 0→3 → nonzero window value; quiet 5 s → 0; rebuild reset → 0 immediately.
- [ ] **Step 2: Implement.** Engine push-error arm: drop writer (best-effort finalize ignored-with-note), clear recording_path, set Warn banner. pick_banner's overrun branch consumes the watermark value instead of the raw cumulative count.
- [ ] **Step 3: Gates.** **Step 4: Commit** `fix(app): surface recording write failures; overrun banner re-baselines`

---

### Task 5: Replay-path alignment + assert_no_alloc + spec text syncs

**Files:** Modify: `crates/chrona-session/src/replay.rs`, `crates/chrona-app/src/engine.rs` (consume), `crates/chrona-audio/Cargo.toml` + `crates/chrona-audio/src/capture.rs`, `docs/superpowers/specs/2026-08-20-chrona-timegrapher-design.md`, `docs/superpowers/specs/2026-08-23-ui-redesign-design.md`.

- [ ] **Step 1:** `chrona_session::replay`'s sidecar application replaces hard-Err-on-out-of-range with the engine's clamp semantics (lift 10–90, ppm finite ±500; reuse/move the sanitize fns — they live in chrona-app/ui/controls.rs; move the two pure fns INTO chrona-session (`pub fn sanitize_lift/sanitize_ppm`) and re-export/consume from chrona-app so one copy exists). Engine keeps calling its own path but through the moved fns. Existing replay tests updated to expect clamping (find the hard-Err tests and flip them deliberately, documenting each).
- [ ] **Step 2:** `assert_no_alloc = "1"` as a normal chrona-audio dependency; the cpal data-callback body wrapped in `assert_no_alloc::assert_no_alloc(|| { … })` under `#[cfg(debug_assertions)]` (release: the closure runs unwrapped). One unit test (debug-only) asserting a deliberate alloc inside `assert_no_alloc` panics — proving the guard is armed in the test profile.
- [ ] **Step 3:** Spec syncs (docs): 2026-08-20 spec §4 crossbeam line → std::sync::mpsc (changelog line); 2026-08-23 spec §7 ordering fallback text + §5 "Select watch" + two §14 entries (items 11/12).
- [ ] **Step 4: Gates.** **Step 5: Commit** `refactor: unify sidecar sanitization; debug no-alloc guard; spec text syncs`

---

### Task 6: Local-time display via jiff

**Files:** Modify: `crates/chrona-app/Cargo.toml` (+jiff), `crates/chrona-app/src/ui/history_ui.rs` (`session_time_label`), `crates/chrona-app/src/presenter/format.rs` (`day_label`), call sites in history_ui.

**Interfaces:** both fns gain a `tz: &jiff::tz::TimeZone` parameter; production call sites pass a `TimeZone` resolved ONCE in `ChronaApp::new` via `jiff::tz::TimeZone::system()` and stored on the app (re-resolving per frame is waste; OS tz changes mid-session are out of scope — doc it).

- [ ] **Step 1 (failing tests):** with `TimeZone::get("Asia/Kolkata")`: `session_time_label(86_400*20_000 + 9*3600, Some(126.0), &tz)` renders the +05:30-shifted HH:MM ("14:30" for 09:00 UTC) — compute exact literals in the test; `day_label` Today/Yesterday flip across a local midnight that differs from the UTC midnight (pick an instant that is "today" in UTC but "yesterday" in Kolkata — assert the local answer). Keep one UTC-zone test pinning old behavior continuity.
- [ ] **Step 2:** Implement; remove the UTC-approximation doc comments; update the m4a-followups top item with a DISCHARGED annotation.
- [ ] **Step 3: Gates.** **Step 4: Commit** `feat(app): local-time history display via jiff`

---

### Task 7: NTP/system-clock cross-check (engine)

**Files:** Modify: `crates/chrona-app/src/engine.rs`, `crates/chrona-app/tests/engine.rs`, `crates/chrona-app/src/ui/toolbar.rs` (cal popup line).

**Interfaces:** `HealthView` gains `pub clock_skew: Option<ClockSkew>` with `pub struct ClockSkew { pub ppm: f64, pub span_s: f64 }`. Internal `SkewTracker`: `push(wall_elapsed_s: f64, audio_delivered_s: f64)` 1/s samples (mic source only); linear regression slope of (audio − wall) vs wall → ppm ×1e6; publishes only once span ≥ 120 s; `reset()` on source rebuild. Pure struct, unit-testable without an engine.

- [ ] **Step 1 (failing tests):** SkewTracker: synthetic series with slope +25e-6 and ±2 ms jitter over 300 samples → ppm within ±0.5 of 25; below-120 s → None; reset clears. Engine test: Simulate source snapshots carry `clock_skew: None` always (spec: synthetic delivery).
- [ ] **Step 2:** Implement (regression = standard least squares; reuse nothing from chrona-dsp — this is app-side). Wire: mic pull path samples once per second (wall via `Instant` since tracker start, audio via frames/sr_nominal). Toolbar cal popup gains the faint line `system-clock cross-check: {ppm:+.1} ppm over {span} min` + "use as correction" button (sends SetPpm(measured) + persists via the existing device_ppm path) — shown only when `clock_skew` is Some; a doc comment notes the mic-only/120 s semantics.
- [ ] **Step 3: Gates.** **Step 4: Commit** `feat(app): system-clock skew cross-check with explicit adopt`

---

### Task 8: Calibration wizard

**Files:** Create: `crates/chrona-app/src/ui/cal_wizard.rs`. Modify: `crates/chrona-app/src/app.rs`, `crates/chrona-app/src/ui/toolbar.rs` (entry point: a "Calibrate…" item inside the cal popup), `ui/mod.rs`.

**State machine (binding):** `enum CalWizard { Intro, Capturing { started: Instant, path: PathBuf }, Analyzing { rx: mpsc::Receiver<Result<QuartzCalResult, CalError>> }, Result { outcome: Result<QuartzCalResult, CalError> } }` on `ChronaApp` as `Option<CalWizard>`. Capture = the EXISTING StartRecording tee into a temp dir (`std::env::temp_dir().join("chrona-cal")`); minimum 300 s enforced (Analyze button disabled with countdown until then; recommended 600–900 s shown); Cancel any time → StopRecording + temp file cleanup. Analyze → StopRecording, then a one-shot `std::thread` reads the WAV (`chrona_session` reader) and runs `chrona_dsp::cal::calibrate_quartz`, sending the result; UI polls `rx.try_recv()` each frame with a spinner. Result screen: ppm ± residual (fields from `QuartzCalResult` — read cal.rs for exact names), the reference-quartz caveat sentence, per-CalError plain-language advice (NoTicks → placement/gain; Unstable → temperature/movement; BadInput → internal), "Save for <device>" (device_ppm path, same as popup) / "Discard". Temp WAV+sidecar deleted on every exit path (Drop-guard struct).

- [ ] **Step 1 (failing tests):** pure helpers: `cal_advice(&CalError) -> &'static str` non-empty per variant; `min_capture_remaining(started, now) -> Option<Duration>` math; analysis-stage integration test WITHOUT the UI: synthesize_quartz (~330 s at 48 kHz is ~63 MB of f32 — use 8 kHz? calibrate_quartz needs real sr; use the shortest duration cal.rs's own tests use — READ THEM and reuse their parameters) → write WAV via SessionWriter → run the wizard's analysis fn (`fn analyze_wav(path) -> Result<QuartzCalResult, CalError>`, the same fn the thread calls) → ppm within the cal module's own proven tolerance of the synthesized ppm.
- [ ] **Step 2:** Implement + wire. The wizard modal follows M4a modal conventions (overlay, card, Esc=Cancel with confirm-if-capturing).
- [ ] **Step 3: Gates + launch check.** **Step 4: Commit** `feat(app): quartz calibration wizard`

---

### Task 9: Synth AGC knobs + Mic Doctor analysis core (chrona-dsp)

**Files:** Create: `crates/chrona-dsp/src/doctor.rs`. Modify: `crates/chrona-dsp/src/synth.rs`, `crates/chrona-dsp/src/lib.rs`.

**Interfaces (produced):**
```rust
// synth.rs — SynthConfig gains (defaults preserving current output BIT-EXACTLY):
pub agc_depth: f64,        // 0.0 = off (default); 0..1 post-tick gain dip depth
pub agc_recovery_s: f64,   // exponential recovery tau (default 0.05)
pub hum_level: f64,        // linear amplitude of the existing hum_hz sine (find how
                           // hum_hz is applied today; if a level already exists under
                           // another name, REUSE it and skip this field)
// doctor.rs — all pure:
pub struct SilenceReport { pub noise_floor_dbfs: f64, pub hum: Option<HumReport> }
pub struct HumReport { pub freq_hz: f64, pub strength_db: f64 } // strength over local floor
pub fn silence_report(samples: &[f32], sample_rate_hz: f64) -> SilenceReport;
pub struct TickReport { pub tier: Tier, pub band_snr_db: Option<f64>, pub clipped: u64 }
pub fn tick_report(samples: &[f32], sample_rate_hz: f64) -> TickReport; // throwaway Analyzer inside
pub struct AgcReport { pub agc_db: Option<f64>, pub gate_db: Option<f64> } // detected depths
pub fn agc_report(samples: &[f32], sample_rate_hz: f64) -> AgcReport; // internally: envelope +
    // detected events (throwaway Analyzer), then gap-RMS series correlated against a
    // post-tick dip/recovery template; gate = floor collapse ≥20 dB between ticks
pub struct DoctorScore { pub score: u8, pub advice: Vec<&'static str> }
pub fn doctor_score(silence: Option<&SilenceReport>, tick: Option<&TickReport>, agc: Option<&AgcReport>) -> DoctorScore;
```
Hum detection: rFFT of the (windowed) silence buffer; candidate bins at 50/60 Hz ±1 Hz and their 2nd/3rd harmonics; hum reported when the fundamental+harmonics exceed the local median floor by ≥12 dB. Scoring: start 100; −20 clipping, −15 AGC, −25 gate, −10 hum ≥20 dB, −10 noise floor > −50 dBFS, −30 tier < T2; advice strings from the binding spec §3.2 list (placement, gain, wired mic, earbuds-on-crown, piezo pickup), ranked by deduction size. Exact constants are THIS plan's values — binding.

- [ ] **Step 1 (failing tests, synth-driven):** synth-with-defaults byte-equivalence pin (generate one seed with old defaults at BASE vs new struct defaults — same output; do this by asserting the new fields' defaults are the no-op values and one golden checksum test of a short buffer); `agc_report` on synth agc_depth 0.4 → agc_db Some(≈ −4.4 dB ± 2), on defaults → None, gate synth (agc_depth 0.99, recovery 5 s → floor collapse) → gate_db Some; `silence_report` on synth noise + hum_hz 50/hum strong → HumReport at 50 ± 1 Hz, on hum-free → None; `tick_report` on a Tier-3 synth → tier T3, snr Some; `doctor_score` table: all-good → ≥ 90 with empty-ish advice; gated signal → ≤ 45 with the gate advice ranked first.
- [ ] **Step 2: Implement.** **Step 3: Gates incl. release `--ignored` + the synth byte-equivalence pin is load-bearing (every prior proven number depends on synth defaults not moving).**
- [ ] **Step 4: Commit** `feat(dsp): mic-doctor analysis core; synth AGC/hum knobs`

---

### Task 10: Doctor capture plumbing + Doctor panel UI

**Files:** Modify: `crates/chrona-app/src/engine.rs` (`ControlMsg::DoctorCapture { seconds: f64 }` → engine buffers the next N seconds of pushed samples, then attaches `doctor_capture: Option<Arc<Vec<f32>>>` to ONE snapshot and clears), `tests/engine.rs`. Create: `crates/chrona-app/src/ui/doctor.rs` (panel). Modify: `app.rs`, `toolbar.rs` (a "Mic Doctor" button), `ui/mod.rs`.

**Panel:** modal with three step rows (Silence 5 s / Tick 10 s / AGC 10 s — AGC reuses the tick capture when run back-to-back? NO — keep independent captures, simpler and honest), each: Run button → sends DoctorCapture → progress → on capture arrival runs the Task-9 pure fn on a worker thread (mpsc one-shot like the wizard) → pass/warn/fail row. Score + advice section renders `doctor_score` over whatever has run ("not run" rows excluded from scoring — the fn's Option params). Honesty: steps never auto-run; stale results marked with a "re-run after changing setup" hint once any config/source changes (clear results on SwitchSource).

- [ ] **Step 1 (failing tests):** engine: DoctorCapture on Simulate turbo → within deadline a snapshot carries `doctor_capture` of len ≈ seconds×sr (±1 chunk), and the NEXT snapshot carries None (one-shot); two overlapping requests → second replaces first (documented). UI pure helpers: step-row model fn (report → Pass/Warn/Fail + text) with a small table test.
- [ ] **Step 2: Implement + wire.** **Step 3: Gates + launch check.** **Step 4: Commit** `feat(app): mic doctor panel with engine capture plumbing`

---

### Task 11: `beat_scope` analyzer API + engine gating (chrona-dsp + engine)

**Files:** Modify: `crates/chrona-dsp/src/analyzer.rs`, `crates/chrona-app/src/engine.rs`, `tests/engine.rs`.

**Interfaces:**
```rust
// chrona-dsp
pub struct BeatScope { pub t_drop_corr_s: f64, pub parity: Parity,
    pub samples: Vec<f32>,        // ~25 ms of filtered native-rate signal, decimated ×4
    pub sample_step_s: f64,       // 4.0 / sr_eff
    pub window_start_rel_drop_s: f64, // window start relative to the drop edge (≈ −0.010)
    pub t_unlock_rel_s: Option<f64>, pub snr_db: Option<f64> }
pub fn beat_scope(&self, n: usize) -> Vec<BeatScope>; // newest ≤ min(n,16) events whose
    // raw window [drop−10ms, drop+15ms] is FULLY inside the raw ring; older/evicted
    // omitted (never padded). &self — read-only over rings + cache events.
// engine: ControlMsg::SetScope(bool); EngineSnapshot.scope: Option<Vec<BeatScope>> (Some iff on)
```

- [ ] **Step 1 (failing tests, dsp):** on a Tier-3 synth analyzer: `beat_scope(8)` returns 8 windows; each window's max-abs sample sits within ±2 ms of `−window_start_rel_drop_s` (the drop lands where the event said); `t_unlock_rel_s` matches the event's `t_unlock_s − t_drop_edge_s` within 0.1 ms; decimation length == (0.025·sr/4).round() ±1; parity alternates; eviction: after pushing 40 more seconds, requesting 16 only returns windows whose data still exists (assert all returned windows satisfy the containment invariant — no zeros run at the edges).
- [ ] **Step 2 (engine):** SetScope gating test on Simulate turbo: scope None by default; SetScope(true) → next snapshots carry Some(non-empty) at Tier 3; SetScope(false) → None again.
- [ ] **Step 3: Implement** (dsp first, then engine wiring — the per-tick call is `analyzer.beat_scope(16)` only when on). **Step 4: Gates incl. release.** **Step 5: Commit** `feat(dsp,app): per-beat scope extraction with engine gating`

---

### Task 12: Scope view UI

**Files:** Create: `crates/chrona-app/src/ui/scope.rs`. Modify: `app.rs` (collapsible section between amplitude strip and bottom grid; open-state drives SetScope), `ui/mod.rs`.

- [ ] **Step 1 (failing tests):** pure geometry: `scope_trace_points(scope: &BeatScope, rect) -> Vec<Pos2>`-shaped mapper (x from sample index·step, y from amplitude normalized to the window's own max-abs — per-trace normalization, doc-commented as display-only) with a round-trip test; marker-x mapper for unlock/drop; stack layout fn (`n` traces → n rects with 4 px gaps, newest first) row-count pin (≤ 8 rendered).
- [ ] **Step 2:** Implement: collapsing header "Scope" (open→`SetScope(true)`, close→false — trace the state change to the ControlMsg in a comment); per-trace: parity-tinted line (tick/accent), drop marker (grid0 vertical + "drop" faint label on the first trace only), unlock marker when Some (good color + "unlock"), right-aligned `snr N dB` faint mono; empty state "scope needs detected beats"; header legend. Chart-panel styling (chartbg/border/12px) per M4a conventions.
- [ ] **Step 3: Gates + launch check.** **Step 4: Commit** `feat(app): stacked per-beat scope view`

---

### Task 13: Classic-pattern legend modal

**Files:** Create: `crates/chrona-app/src/ui/patterns.rs`. Modify: `crates/chrona-app/src/ui/charts.rs` (a "?" button in the beat-trace header row opening it), `ui/mod.rs`, `app.rs` (modal state).

**Content (binding copy — write exactly, tune only for typos):** six rows, each schematic + text:
1. Level line — "Healthy and on rate. The flatter, the closer to 0 s/d."
2. Sloped line — "Running fast (rising) or slow (falling). Adjust the regulator."
3. Two parallel bands — "Beat error: tick and tock are unevenly spaced. Adjust the stud/collet, not the regulator."
4. Wavy line — "Periodic fault — suggests a damaged tooth or bent pivot in the gear train; the wave period hints at which wheel."
5. Scattered dots, no line — "Weak or dirty signal — suggests a dirty escapement, low amplitude, or poor pickup. Check placement and run the Mic Doctor."
6. Sudden vertical jumps — "Sudden rate steps — suggests rebanking, a knock, or the hairspring touching. Re-test after winding fully."

**Schematics:** pure fns `fn pattern_points(kind: PatternKind, rect: Rect) -> Vec<Vec<Pos2>>` (polyline groups) — level/slope/bands/wavy(sine)/scatter(deterministic pseudo-random from a fixed seed table)/jumps(step function); painted as tick-colored dots or 1.5 px lines on a mini chartbg card ~120×48.

- [ ] **Step 1 (failing tests):** `pattern_points` pins: level → 1 group, constant y; bands → 2 groups with constant y-gap; wavy → y oscillates with ≥ 3 sign changes of dy; jumps → exactly 2 discontinuities > 30% of height; scatter → deterministic (same output twice).
- [ ] **Step 2:** Implement modal (M4a conventions; opened from the beat-trace header "?" placed after the legend). The wording above is the pinned copy — a `PATTERNS` const with a completeness test (6 rows, non-empty, "suggests"/"check" hedging present in rows 4–6: assert the strings contain "suggests" where specified — overclaim guard).
- [ ] **Step 3: Gates + launch check.** **Step 4: Commit** `feat(app): classic tape-pattern legend`

---

### Task 14: Docs — protocol, README, AGENTS, followups discharge

**Files:** Modify: `docs/manual-testing.md`, `README.md`, `AGENTS.md`, `docs/superpowers/notes/m3-followups.md`, `docs/superpowers/notes/m4a-followups.md`.

- [ ] **Step 1 (ground every line in code, cite file:line in the report):** manual-testing.md — A gains: A.7 scope check (open Scope while simulate runs → stacked beats, drop/unlock markers, SNR readouts) and A.8 patterns legend (beat-trace "?" → six patterns render). B gains: B.8 Mic Doctor (run all three steps against the real mic; expect a score and ranked advice; silence test in a quiet room should NOT report hum unless there is some) and B.9 calibration wizard (real quartz watch ≥ 5 min → ppm ± residual plausible vs the CLI path; save; ⚠ uncal drops) — B.7's CLI wording becomes the alternative path. A note that history times are local now. README: trust-features paragraph. AGENTS: extend the DSP-release-gate line to name doctor/scope/synth-knob files; note the synth-defaults byte-equivalence pin as frozen. Followups: DISCHARGED annotations for every §2 debt item + m4a #1 (local time), with commit refs.
- [ ] **Step 2: Gates (docs safety).** **Step 3: Commit** `docs: M4 trust-features protocol and followups discharge`

---

## Self-review notes (writing-plans checklist)

- Spec coverage: §2.1→T1, §2.2→T3, §2.3/2.4→T4, §2.5/2.6/2.9→T5, §2.7/2.8→T2, §3→T7/T8, §4→T11/T12, §5→T9/T10, §6→T13, §7→T6, §8→T5/T6 (deps land with their consumers), §9→T14, §10 recorded (no tasks — M5 items), §11 distributed. No gaps.
- Type consistency: `QuartzCalResult`/`CalError` (T8) match cal.rs's real names; `BeatScope` (T11) consumed by T12 by that name; `ClockSkew` (T7) UI line in T7 itself; `SilenceReport/TickReport/AgcReport/DoctorScore` (T9) consumed by T10; `CountWatermark` (T4) internal to controls.
- Placeholders: none — constants, thresholds, copy, and test expectations are stated. Two investigate-first steps (T1 semantics survey, T9 hum-level reuse check) are deliberate evidence-before-code instructions, not gaps.
- Ordering: T1/T2 (dsp) → T3/T4/T5/T6 (independent) → T7 → T8 → T9 → T10 → T11 → T12 → T13 → T14. Only real edges: T9→T10, T11→T12; T8 uses no T7 output.
