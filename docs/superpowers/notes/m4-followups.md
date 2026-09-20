# M4 follow-ups (from the M4 trust-features milestone, 2026-09-07)

Everything here was reviewed and deliberately deferred — none of it blocked the M4 merge.
M4's own debt queue (design doc `docs/superpowers/specs/2026-09-07-m4-trust-features-design.md`
§2 — `parabolic3` unification, engine-survives-total-startup-failure, the recording-tee/
overrun-banner/replay-path/`assert_no_alloc` carryovers from M3/M4a, the sweep refactor
and `fold_envelope` empty-bin carryovers from M2) is **fully discharged**; see
`m3-followups.md`'s "M4 first-week debt"/"App/engine debt from the final review" sections
and `m4a-followups.md`'s "Still first in line for M4 proper" section for the individual
DISCHARGED annotations and commits. This file records M4's OWN new debt, plus a pointer
to what's still open from earlier milestones. **M5 planning starts here.**

## Deferred minors (from task reviews and the milestone ledger)

1. **Cal-wizard cancel-before-first-snapshot stray-file gap.** `ui::cal_wizard`'s
   `TempCaptureGuard` (the Drop-guard that deletes the temp WAV+JSON sidecar) is only
   constructed once `EngineSnapshot::recording` confirms the capture's path — Phase 1 of
   `render_cal_wizard` sets `*guard = Some(TempCaptureGuard::new(confirmed.clone()))` only
   after `wctx.snap.recording` turns `Some`. If the user cancels (`WizardAction::
   ConfirmCancelCapture`) in the brief window before the engine's first publish tick
   confirms that path, `ControlMsg::StopRecording` still goes out (so the engine still
   finalizes a partial WAV+JSON), but `*guard` is still `None` at that moment — nothing
   is ever constructed to delete it. The temp file leaks into `<tempdir>/chrona-cal/`
   until the OS temp directory's own periodic cleanup reclaims it. Narrow window
   (roughly one engine tick), same class of gap `TempCaptureGuard`'s own doc comment
   already documents for a hard crash — but reachable by ordinary UI action, not just a
   crash. Fix: have `apply_wizard_action`'s `ConfirmCancelCapture`/`CloseWithoutSaving`
   arms fall back to deriving the expected path from the engine's recording-directory
   convention when `guard` is still `None`, or block Cancel until the path is confirmed.
   See `ui/cal_wizard.rs`'s Phase 1 (`render_cal_wizard`) and `WizardAction::
   ConfirmCancelCapture`'s handler in `apply_wizard_action`.
2. **`outline_button`/signal-color duplication.** The same frameless-outline button
   helper (`fn outline_button(ui, palette, text) -> egui::Response`, identical body) is
   defined three times: `ui/toolbar.rs`, `ui/doctor.rs`, `ui/cal_wizard.rs`. Similarly,
   the `SignalClass -> Color32` mapping is duplicated: `ui/cal_wizard.rs`'s
   `signal_hint_color` and `ui/strip.rs`'s own copy (both `SignalClass::Good =>
   palette.good` etc.). Each module's own doc comment cites `persist_config_now`
   (toolbar.rs) as local precedent for "each module owns its own tiny presentational
   helper" — a deliberate call at the time, not an oversight — but three copies of the
   same button and two copies of the same color match is worth a shared `ui`-level home
   if a fourth caller ever shows up.
3. **`beat_scope`'s ~9.7 ms ring-slack boundary-band test gap.** `Analyzer::beat_scope`
   omits (never pads) a window whose raw samples have already been evicted from
   `raw_ring` (`SampleRing::copy_range_abs` returning `false`) — this is exercised
   structurally (the all-or-nothing `copy_range_abs` primitive is independently tested,
   and `beat_scope`'s own eviction-omission test covers the case), but there's a ~9.7 ms
   band right at the ring's eviction boundary where a window could straddle
   "just barely still available" vs "just evicted" that no test names directly. A
   regression test against a deliberately shrunk ring would close it. See `Analyzer::
   beat_scope` and `SampleRing::copy_range_abs` in `chrona-dsp/src/analyzer.rs`.
4. **Mic Doctor's Analyzing-step disconnect shows no error message (cosmetic).**
   `render_doctor_panel`'s Phase 2 treats a disconnected analysis-thread channel
   (`TryRecvError::Disconnected`) as `abandon = true`, which just clears `panel.pending`
   back to `None` — the step's row then falls back to its ordinary "not run" state with
   no indication anything went wrong (contrast `cal_wizard`'s equivalent case, which
   surfaces a `CalError::BadInput { reason: "analysis thread ended unexpectedly" }` onto
   the Result screen). Only reachable if a `chrona_dsp::doctor` analysis fn itself
   panics, which none of Silence/Tick/AGC's pure fns are expected to do on real captured
   audio — cosmetic, not a correctness gap. See `ui/doctor.rs`'s `render_doctor_panel`
   Phase 2 (`Err(TryRecvError::Disconnected) => abandon = true`).
5. **The `-0.0 s/d` cosmetic — still open, pre-existing.** `format_rate`'s `{:+.1}`
   sign-preservation formats a tiny negative rate estimate (e.g. `-0.02`) as `"-0.0
   s/d"` rather than `"+0.0 s/d"` or `"0.0 s/d"`. Carried forward from M2/M3 (recorded
   in `m3-followups.md`'s "Accepted minors" list); M4 did not touch `presenter/
   format.rs`'s rate formatting, so it remains open.
6. **`AUTO_BPH`'s other 6 entries aren't in the NotQuartz matrix.** `chrona-dsp/src/
   bph.rs`'s `AUTO_BPH` lists 11 auto-detect beat rates; `cal.rs`'s
   `not_quartz_rejects_every_standard_bph` test (Task 8b) measures the off-gate ratio
   at only 5 of them (18 000/21 600/25 200/28 800/36 000 bph) — the other 6
   (12 000/14 400/17 280/19 800/43 200/72 000 bph) were never individually measured
   against `OFF_GATE_RATIO_MAX = 3.0`, though the tested 5 already show a ≥ 2×
   margin and there's no structural reason to expect the untested rates to behave
   differently (more beats/second only raises the off-gate count further above
   quartz's 0.0 floor). Widening the matrix to all 11 would close this without
   changing the threshold itself.
7. **Doctor capture generation tag.** `ui::doctor`'s `pending_capturing_step` matches an
   `EngineSnapshot::doctor_capture` arrival against whichever step `DoctorPanel::pending`
   currently names, not against a per-request identifier — there is none. A fast
   cancel-one-step-then-run-another sequence, both falling inside one ~100 ms
   `EngineSnapshot` publish tick, can theoretically have the CANCELLED step's buffer
   arrive after the NEW step's request is already `pending`, and get misattributed to
   the new step's analysis instead of silently ignored. `EngineSnapshot::
   source_generation` (`engine.rs`) already solves an analogous staleness problem for
   source switches — mirroring that same monotonic-counter shape onto
   `ControlMsg::DoctorCapture`/`EngineSnapshot::doctor_capture` (a per-request id the
   panel checks exactly, not just "is some step pending") would close this precisely
   instead of relying on the ~100 ms window being narrow in practice. See `ui/doctor.rs`'s
   module doc comment ("Cancel (fix round 1, post-review)") and `pending_capturing_step`.
   **DISCHARGED post-merge (commit `poll/latch fix`, 2026-09-20):** the first CI run
   proved the one-shot delivery window itself was missable under scheduler contention
   (macOS lost the race at 5 ms polling — and a stalled UI frame has the same exposure),
   so the fix went further than this item asked: `DoctorCapture` now carries the
   per-request `id`, delivery is LATCHED (attached to every snapshot until superseded or
   the source switches), and `pending_capturing_step` consumes only an exact id match —
   closing both the misattribution window described here AND the missed-delivery hang.
8. **Overrun banner always says "1 buffer overrun(s)".** `ui::controls::CountWatermark`
   (used for both the clip and overrun trackers, `app.rs`'s `clip_tracker`/
   `overrun_tracker`) returns a 0/1 "is this still current" placeholder, not a real
   count — but `pick_banner` (`ui/controls.rs`) formats that placeholder straight into
   `format!("{} buffer overrun(s) — audio briefly dropped", h.overruns)`, so the banner
   always reads "1 buffer overrun(s)" whenever it shows at all, never the actual
   windowed delta. Fix: either drop the number from the copy ("buffer overrun(s) —
   audio briefly dropped") or give `CountWatermark` a real windowed-delta count to
   return instead of the placeholder.
9. **Quartz-capture Signal reading stays "listening…" for the whole wizard capture.**
   `ui::cal_wizard`'s Capturing screen reuses `presenter::signal_meter` for its live
   signal label, same as the main window — but a quartz watch's 1 Hz tick is below
   every `AUTO_BPH` floor the signal-tier gates are tuned around, so the reading never
   leaves "listening…" even during a perfectly healthy 5+ minute capture. Honest (it
   really can't classify a 1 Hz source as a beat rate), but reads as anxious/stuck to a
   first-time user watching a static label for 5+ minutes. Fix (M5-shaped): either a
   wizard-specific sentence acknowledging this is expected for quartz, or a raw RMS
   level meter instead of the beat-rate-tiered signal label. See `ui/cal_wizard.rs`'s
   `render_capturing` and `presenter::signal_meter`.
10. **Doctor staleness hint on CONFIG changes.** `ui::doctor`'s `render_doctor_panel`
    clears all three step results (plus abandons any pending capture) when
    `EngineSnapshot::source_generation` changes — a SOURCE switch — but a CONFIG change
    (e.g. lift angle, BPH mode) while old Doctor results are still displayed neither
    clears them nor hints that they might now be stale. Recorded plan drift: worth a
    staleness indicator (or an explicit clear) the same way the source-generation guard
    already does for source changes. See `ui/doctor.rs`'s `render_doctor_panel` and its
    `last_source_generation` comparison.
11. **Replay-sourced calibration attribution.** The calibration wizard's
    `ControlMsg::StartRecording` (via `WizardAction::StartCapture`) captures from
    whatever source the engine currently has active — including `Replay`. Replaying a
    previously recorded quartz WAV through the wizard yields a real, correctly measured
    ppm, but the Result screen's "Save for <device>" attributes it to whichever
    physical device is currently selected in the toolbar's device combo, which may not
    be the device the replayed WAV was actually recorded on. Deliberate off-road use
    (nothing in the binding spec restricts the wizard to a live mic source), but the
    attribution is loose enough to be worth a documented caveat or an explicit
    replay-source warning in the wizard UI. See `ui::cal_wizard`'s `WizardAction::
    StartCapture` and `render_result`'s `device_label`.

## Still open from earlier milestones (untouched by M4)

`m3-followups.md`'s "Accepted minors riding to M4" and "Still-open M2 carryovers"
sections, and `m4a-followups.md`'s "final whole-branch review" items 2/4/5/6, all stand
unless annotated DISCHARGED in place. Notably still open, none picked up this milestone:

- Beat-trace x-gridlines anchor to absolute multiples of the step rather than `end`
  (m4a-followups item 2) — cosmetic ("now" label rarely renders), unrelated to any M4
  task's own files.
- Export "BPH mode" line echoes the raw Other-field buffer instead of the
  engine-effective mode (m4a-followups item 4) — `export.rs` untouched by M4.
- Free-mode amplitude strip still shows "listening…" forever instead of a Free-mode-
  specific "no beat grid" caption (m4a-followups item 5) — verified still true at
  `ui/charts.rs`'s amplitude-panel `newest_t` fallback; `charts.rs` gained the
  pattern-legend "?" button this milestone (Task 13) but that fallback branch itself
  wasn't touched.
- Toolbar wrap on narrow windows (accepted characteristic, both m3/m4a).
- Sidecar schema vs binding spec §8 (per-beat event table + device ID not stored) —
  still deliberate; still says "revisit when positions summary/export lands", which
  M4 did not add.
- Mic-source startup fallback has no automated test (m3-followups) — still no
  injectable device-enumeration seam. Mic Doctor (M4 Tasks 9/10) observes whatever
  source the engine already has running rather than adding one; the note that this
  might be "considered with Mic Doctor" didn't end up applying, since Doctor's capture
  path is source-agnostic by design (`ControlMsg::DoctorCapture` works against Mic,
  Simulate, or Replay alike).

## M5 planning start point

Binding spec §13 (v2 candidates) and the M4 design doc's §1 scope-summary OUT list /
§10 deviations record the same boundary, deliberately drawn at M4 planning time so the
DSP/UI-testable half of the trust-features work (calibration wizard, NTP cross-check,
scope view, Mic Doctor's signal-analysis core, pattern legend — all headless-testable,
all shipped in M4) could land without waiting on hardware-in-the-loop platform work.
Still to build, in M5:

- **Mic Doctor's OS-specific layer** (binding spec §3.2 steps 4-5, §3.3): Windows
  effects-enumeration (detect active AGC/NS/AEC on the stream, via the effects-discovery
  API), programmatic OS input-gain writeback (macOS
  `kAudioDevicePropertyVolumeScalar` / Windows `IAudioEndpointVolume` / Linux ALSA mixer
  capture elements) plus volume-change listeners that warn if another app moves the
  gain mid-session, and Bluetooth-transport detection + refusal (HFP/SCO codecs cap
  audio at 8-16 kHz and destroy tick energy). Doctor's advice slots for these exist
  today and emit nothing (`chrona-dsp/src/doctor.rs`'s OS-specific advice is M5-shaped
  but unbuilt) — see the M4 design doc §10 items 1-2.
- **First-launch auto-run of Mic Doctor** (binding spec §3.2's "runs on first launch and
  on demand" — only "on demand", via the toolbar button, shipped in M4). Needs the OS
  layer above to be worth running automatically (design doc §10 item 3).
- **Raw WASAPI side-path** (`AUDCLNT_STREAMOPTIONS_RAW` via the `wasapi` crate; cpal
  does not expose it) — binding spec §13, deferred until Windows effects detection
  shows real users hitting enhancements they can't disable through Settings.
- Corpus fixtures (44.1 kHz / high-BPH real recordings) remain blocked on real
  recordings existing — carried unchanged since M2/M3.
- Packaging/signing/first release itself (binding spec §11, milestone M5).

## Process notes

- **Report-integrity incident this milestone**: task-8b-report.md's RAW-gate summary
  stated "269 tests passed"; summing that same report's own per-binary numbers gives
  255 (98+0+14+2+6+1+0+2+7+4+97+24) — an arithmetic error in the rollup, not a
  fabricated or altered per-binary result. Same class as M4a's Task 5 report-integrity
  incident; ledgered rather than silently re-edited, since the worktree is ephemeral
  and the ledger is the record of what actually happened.
- **The milestone's marquee catch**: `calibrate_quartz` (pre-Task-8b) silently accepted
  a clean, synthetic MECHANICAL watch recording as a valid quartz reference — every
  standard BPH divides 1 Hz evenly, so the gated tracker phase-locked onto "the loudest
  beat nearest each integer second" exactly as it would a genuine 1 Hz quartz tick, and
  returned a plausible-but-wrong ppm (e.g. −1.05 ± 1.13 ppm) instead of refusing. Found
  during Task 8's own capture-plumbing test, not by design review. Closed by Task 8b's
  `NotQuartz` gate (`OFF_GATE_RATIO_MAX`, `chrona-dsp/src/cal.rs`) — the exact kind of
  silent-wrong-calibration risk the honesty rule (AGENTS.md) exists to prevent, and a
  reminder that a formula being internally self-consistent (a clean regression, a low
  residual) says nothing about whether its INPUT was the thing it assumed.
- Visual acceptance remains human-only for the Doctor/wizard/scope UI: no sandbox agent
  composited a window this milestone either. The manual protocol (A.7-A.8, B.8-B.9) is
  the visual gate.
