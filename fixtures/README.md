# Chrona fixture corpus

Regression recordings + expectations for `chrona verify fixtures/`.

- One `<name>.json` expectation per recording (schema in the CLI: `commands.rs::Expectation`);
  `file` is relative to this directory.
- Committed WAVs: ≤ 60 s, 16-bit PCM, mono. Name as
  `<movement>_<position>_<mic>.wav`, e.g. `eta2824_dial_up_contact_mic.wav`.
- Real recordings land from M2 onward (spec §10): several movements × positions × mic
  types, including deliberately bad laptop-mic captures and a quartz reference for
  calibration tests. Where a hardware timegrapher reading exists, record it in the
  expectation and note machine + settings in a comment field.
- Synthetic cases don't get committed — `chrona synth` regenerates them (see
  `crates/chrona-cli/tests/`).
