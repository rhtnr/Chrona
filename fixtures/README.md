# Chrona fixture corpus

Regression recordings + expectations for `chrona verify fixtures/`.

## Recording a fixture (spec §10)

- 48 kHz mono, 60 s, 16-bit PCM (`--pcm16`-style format). Quiet room.
- Mic against the case back or crown; a piezo contact pickup reaches Tier 3,
  wired earbuds pressed to the crown work in a pinch, laptop mics usually give
  Tier 1-2. Deliberately-bad captures are valuable — record them too.
- Name as `<movement>_<position>_<mic>.wav`, e.g. `eta2824_dial_up_contact.wav`,
  positions DU/DD/CU/CD/CL/CR.
- If a hardware timegrapher reading exists (Weishi etc.), put its numbers in the
  expectation. Set `"lift"` to the movement's lift angle (default 52); other
  extra keys are ignored by the runner.
- A quartz-watch recording (≥ 5 min) doubles as the calibration reference:
  `chrona calibrate quartz.wav`, then record the mechanical fixtures and put the
  measured `ppm` in their expectations.

## Expectation schema (one JSON per recording; `file` relative to this dir)

```json
{
  "file": "eta2824_dial_up_contact.wav",
  "bph": "auto",
  "ppm": 12.5,
  "lift": 52.0,
  "expect_rate_s_per_day": 7.2,  "tol_rate": 1.0,
  "expect_beat_error_ms": 0.3,   "tol_beat_error": 0.15,
  "expect_amplitude_deg": 285.0, "tol_amplitude": 10.0
}
```

`bph` takes auto | free | a number. The beat-error and amplitude pairs are
optional — omit them for Tier-1 (weak-mic) fixtures; when the `expect_*`
half of a pair is given without its `tol_*`, the tolerance defaults to the
values shown above (0.15 ms / 10.0°). Synthetic cases are not
committed; `chrona synth` regenerates them (see `crates/chrona-cli/tests/`).
