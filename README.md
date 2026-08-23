# Chrona

A cross-platform software timegrapher in Rust: listen to a mechanical watch and read its
rate (s/day), beat error (ms), and amplitude (°) — with honest, signal-aware confidence
instead of fabricated numbers on weak microphones.

Status: M3 (live app with microphone input, record/replay, real-time UI). Design: docs/superpowers/specs/2026-08-20-chrona-timegrapher-design.md.

## App

Live app (microphone), or a hardware-free demo:

```sh
cargo run -p chrona-app
cargo run -p chrona-app -- --simulate --rate 12 --beat-error 0.8
```

Before any release, run the complete manual test protocol in docs/manual-testing.md.

## Try it

```sh
# Generate a synthetic 21,600 bph watch running +12.5 s/d fast, then analyze it:
cargo run -p chrona-cli -- synth test.wav --bph 21600 --rate 12.5
cargo run -p chrona-cli -- analyze test.wav

# Analyze a real recording (auto-detects beat rate):
cargo run -p chrona-cli -- analyze my_watch.wav --json

# Full metrics on a synthetic watch with beat error:
cargo run -p chrona-cli -- synth demo.wav --rate 12 --beat-error 0.8 --amplitude 270
cargo run -p chrona-cli -- analyze demo.wav --lift 52

# Calibrate your sound card against any quartz watch (record ≥ 5 minutes):
cargo run -p chrona-cli -- calibrate quartz.wav
```

Rate readings carry an `[uncalibrated timebase]` badge until you supply `--ppm` — measure
yours with `chrona calibrate` against any quartz watch (consumer audio clocks are off by
up to ±100 ppm; 1 s/day is only 11.6 ppm).

Metrics are tiered by signal quality (spec §3.1): Tier 1 = rate only, Tier 2 adds beat error, Tier 3 adds amplitude — weak signals show "—" with the reason instead of fabricated numbers.

## License

MIT OR Apache-2.0. The DSP approach follows the published literature (see the spec's
references); no GPL code is used.
