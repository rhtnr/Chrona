# M2 follow-ups (from the M2 final whole-branch review, 2026-08-22)

Everything here was reviewed and deliberately deferred — none of it blocked the M2 merge.
Feed into M3 planning. Ordered by the final reviewer's priority.

## M3 first-week debt (named tickets, in order)

1. **Octave-guard stress matrix (T10-I1).** Commit the seed × toc_gain × SNR false-positive
   stress matrix as a slow/nightly test driven through the full Analyzer (real period
   estimates). The probe constants (1.8× floor, ±NBINS/16 window, 0.5 cluster factor) are
   **FROZEN until this exists**. Context: three defenses recorded zero false halvings
   (120-case exact-period stress + one committed real-estimate e2e case), but a false
   halve on 14400/18000/21600/36000 watches doubles back ONTO the snap table — a
   confident wrong-BPH display, not an honest no-rate. The 1.126× true-positive margin is
   contrast-based and swings under sub-ppm period changes — platform/FP drift will show
   as a loud declined-halve test failure (acceptable, noisy).
2. **`current()` performance before the live loop.** ~6.4 MB allocation + full
   matched-filter re-extraction per call; `&self` blocks scratch reuse. M3's UI polls at
   ~10 Hz — make this incremental/reusing before wiring it.
3. **`rate_source` Option-ization** in M3's breaking-change window (currently non-Option,
   reads `PeriodSlope` beside `rate_s_per_day: None`; doc comment added in M2).
4. **`parabolic3` unification** — now triplicated (autocorr/events/cal). Promote one
   `pub(crate)` copy. Flagged three times; "no excuses the fourth time".
5. **Collect-then-assert sweep refactor** — the inline-assert loop pattern masks later
   sweep points on first failure (this hid a real 330° amplitude failure in M2); applies
   to amplitude_sweep, beat_error_sweep, and future sweeps.
6. **`extract_events` env_start_abs removal** (vestigial since the T9 native-rate edge fix)
   + its misleading placeholder comment.
7. **fold_envelope empty-bin epsilon** — near-integer env-sample folding periods leave
   zero bins → contrast ≈3.2e11 and unreliable significant_clusters at the top of the BPH
   table (probed NOT reachable through the real pipeline at 48 kHz — real fractional
   periods fill the bins — but the function is public).

## Smaller M3 debt

- `clipped_samples` unreported in the `no_beat` CLI report (needs an Analyzer accessor) —
  the too-hot-input case is exactly where it's diagnostic.
- One forced-Tier-1 snapshot test pinning the PeriodSlope rate value (the 30 dB grid now
  exercises the regression source).
- Dedicated test for verify's defaulted-tolerance path (expect_* present, tol_* absent →
  0.15 ms / 10.0° defaults).
- CalError::Unstable/BadInput unit tests (needs a contrived noisy-but-ticking fixture).
- Cal seed tick unrefined vs parabolic (≈0.01 ppm effect).
- "Period σ: 0.0 µs" formatting for sub-0.05 µs values (M1-era).
- Test-strength polish: octave pinning test asserts !halved but not gate-engagement;
  probe-ratio margin at toc_gain 0.6 is 1.02 vs the 1.8 threshold (behavioral pin, not a
  sensitivity pin).
- T8 thin-data test covers the toc-deficient guard branch only; det threshold 1e-9 is
  absolute (fine for windowed indices, not general).
- T5 refactors: cluster_centroids/significant_clusters dedup, named probe fn, dead factor
  param, double sort.
- Misc accepted characteristics: synth ppm just above −1e6 → huge finite loop (dev tool);
  fold guard rejects (0,1] periods beyond doc wording; no overflow guards on internal
  ring arithmetic; build_template redundant first guard; half_height_edge unreachable
  else-arm; 1 ms unlock-window margin (spec constant, degrades safely).

## Corpus notes

- When real recordings land: include at least one 44.1 kHz fixture and one high-BPH
  (43200/72000) fixture — every M2 number was proven at synthetic 48 kHz except the final
  review's two ad-hoc probes (44.1 kHz off-table honesty; high-BPH health).
- The spec's §5.5 changelog line references "plan T9 fix round" informally (the fix-round
  record lives in git history and the M2 plan; no standalone doc).
