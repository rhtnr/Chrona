# M1 follow-ups (from the M1 final whole-branch review, 2026-08-21)

Everything here was reviewed and deliberately deferred — none of it blocks M1. Feed this
into M2 planning.

## Spec amendments to make (spec is the binding authority; code deviated for good reasons)

- **§5.2 envelope low-pass:** spec says "LP ~2–3 kHz"; that is incompatible with the ÷16
  decimation (post-decimation Nyquist at 48 kHz is 1.5 kHz). Code uses 1.5 kHz, which sits
  *at* Nyquist at 48 kHz and *above* it at 44.1 kHz (mild aliasing, provably harmless to
  the beat-lag autocorrelation at current bars). M2: parameterize cutoff =
  min(1500, 0.45·sr/16) and amend the spec range.
- **§5.3 divisor threshold:** spec says integer-divisor peaks "≥ 90 % of fundamental";
  shipped code uses 40 % with a beat-error-smearing argument (review-ratified). Amend spec.
- **§5.2 Hann edge taper:** dropped by the M1 plan (only mean removal is implemented).
  Accuracy bars pass with 30–100× margins regardless. Record as deliberate simplification
  or implement in M2.
- **§10 release-artifact CI job:** absent; schedule with M5 packaging.

## Known M1 limitations (documented, accepted)

- **Octave error under extreme tic/toc asymmetry** (autocorr.rs module docs): if the
  divisor walk fails, detected BPH halves. Rate stays numerically correct (scale-invariant);
  only the BPH label can mislabel (28800/36000/43200/72000 have in-table halves) or output
  is honestly suppressed. M2's phase-fold stage resolves definitively. Signature to watch:
  a healthy watch reading 14400.
- **σ-gate is the sole noise rejector** at ~3.7× margin on the synthetic noise seed. The
  M2 real-recording corpus is the true fix.
- No upper bound on `duration_s`/`bph` in `SynthConfig` validation (huge-but-finite values
  allocate huge buffers / run slow loops; not reachable from sane use).

## Code follow-ups (do when the file is next touched, or early M2)

- `period.rs`: add the invariant comment `tolerance·K_max < 0.5` (0.005·64 = 0.32) tying
  the per-cycle tolerance to the K cap; `.max(1)` on ring capacity; `#[derive(Debug)]` +
  doc comments on `PeriodEstimator`. **Do not widen the seed/walk tolerances citing the
  distance-to-confounder argument** — the nearest confounder is the drop×unlock satellite
  ~23 envelope samples away, beaten on *height* (5×), not distance.
- `autocorr.rs`: cache the FFT planner (rebuilt per call, ~2 MB — required before M3 live
  use); `Peak` derives; clamp-boundary + non-power-of-two-length tests.
- `envelope.rs`: cutoff parameterization + fix the "3 kHz Nyquist" comment (3 kHz is the
  post-decimation *rate*); `out.reserve()`.
- `wav.rs`: int-PCM read branch is untested — make it the first test the M2 fixture corpus
  adds (the entire real corpus is 16-bit PCM); add a 3-line header sanity guard
  (channels == 0, bits_per_sample == 0).
- `verify` (commands.rs): rework to FAIL-and-continue per expectation (currently aborts the
  whole run on the first `run_analyze` error, and that error lacks expectation-file
  context); assert the empty-dir notice text; fixed precision for `tol_rate` display.
- `synth.rs`: no unit test exercises `hum_hz: Some(_)` anywhere — add with M2 hum-detection
  work; consider NaN-proof `!(a >= b)` comparison forms as a house style.
- CI: consider `--locked` on cargo steps so CI can't drift from the committed lock.

## Test-coverage wishes (all minor)

- Exactly-1.5 % BPH snap boundary; a second beat-arithmetic value (e.g. 17280).
- Multi-hop divisor walk (800→400→200).
- Human-readable CLI output assertions (badge text, no-rate reasons).
- `beat_error_does_not_bias_the_period`: add `assert!(est.window_s >= 16.0)`.
