# Chrona

A cross-platform software timegrapher in Rust: listen to a mechanical watch and read its
rate (s/day), beat error (ms), and amplitude (°) — with honest, signal-aware confidence
instead of fabricated numbers on weak microphones.

Status: M1 (headless pipeline). Design: `docs/superpowers/specs/2026-08-20-chrona-timegrapher-design.md`.

## Try it

```sh
# Generate a synthetic 21,600 bph watch running +12.5 s/d fast, then analyze it:
cargo run -p chrona-cli -- synth test.wav --bph 21600 --rate 12.5
cargo run -p chrona-cli -- analyze test.wav

# Analyze a real recording (auto-detects beat rate):
cargo run -p chrona-cli -- analyze my_watch.wav --json
```

Rate readings carry an `[uncalibrated timebase]` badge until you supply `--ppm` — consumer
audio clocks are off by up to ±100 ppm and 1 s/day is only 11.6 ppm; calibration tooling
arrives in a later milestone.

## License

MIT OR Apache-2.0. The DSP approach follows the published literature (see the spec's
references); no GPL code is used.
