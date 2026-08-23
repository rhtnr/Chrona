# Chrona

A cross-platform software timegrapher in Rust: listen to a mechanical watch and read its
rate (s/day), beat error (ms), and amplitude (°) — with honest, signal-aware confidence
instead of fabricated numbers on weak microphones.

**Status:** milestones M1 (headless DSP pipeline), M2 (full metrics + calibration), M3
(live app: microphone capture, real-time instrument UI, record/replay), and M4a (redesigned
instrument UI: watch library, session history, position comparison, report export) are
complete. Still open from M4: Mic Doctor, calibration wizard UI, scope view.
The binding design document is
[docs/superpowers/specs/2026-08-20-chrona-timegrapher-design.md](docs/superpowers/specs/2026-08-20-chrona-timegrapher-design.md);
known debt and deferred work live in [docs/superpowers/notes/](docs/superpowers/notes/).

Runs on macOS, Windows, and Linux. Rust stable (pinned via `rust-toolchain.toml`).
On Linux you need ALSA plus the usual GUI development packages — the exact apt list is in
[.github/workflows/ci.yml](.github/workflows/ci.yml). On macOS the first live-mic run
triggers the system microphone-permission prompt (attributed to your terminal app).

## App

Live app (microphone), or a hardware-free demo:

```sh
cargo run -p chrona-app
cargo run -p chrona-app -- --simulate --rate 12 --beat-error 0.8 --amplitude 270
```

The redesigned instrument view shows four metric cards (rate / beat error / amplitude /
beat rate) plus a signal-strength meter, a six-position picker (DU/DD/CU/CD/CL/CR) with
per-position rate comparison, and a time-axis beat-trace chart (tic/toc dots, rate-trend
line, adjustable ±2/±5/±10/±25 ms wrap) alongside a matching amplitude chart. A watch
library groups recordings under a named movement, with a session-history log, one-click
HTML report export, and a light/dark theme toggle. Controls cover input device, lift
angle, averaging window, BPH mode, and per-device timebase correction (persisted).
Sessions can be recorded to WAV + JSON sidecar and replayed bit-faithfully.
`--headless-seconds N` prints the metrics line and exits (used by CI).

Before any release, run the complete manual test protocol in
[docs/manual-testing.md](docs/manual-testing.md) — live-mic behavior can only be
verified by a human with a real watch.

## CLI

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

Metrics are tiered by signal quality (spec §3.1): Tier 1 = rate only, Tier 2 adds beat
error, Tier 3 adds amplitude — weak signals show "—" with the reason instead of
fabricated numbers.

## Development

Workspace crates:

| Crate | Role |
|---|---|
| `chrona-dsp` | Analyzer core: envelope, period estimation, fold, matched filter, metrics, tiers, calibration, synthesizer |
| `chrona-audio` | cpal capture, device enumeration, callback-safe ring feed |
| `chrona-session` | WAV + JSON-sidecar recording, replay, config store |
| `chrona-cli` | `synth` / `analyze` / `verify` / `calibrate` |
| `chrona-app` | egui/eframe live app: engine thread, presenters, instrument UI |

```sh
cargo test --workspace                          # fast suite (hardware-free)
cargo test -p chrona-dsp --release -- --ignored # stress matrix + perf bars
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```

CI runs the fast suite on Linux/macOS/Windows plus the release-mode stress job on Linux.
See [AGENTS.md](AGENTS.md) for the project's working conventions (binding spec, frozen
constants, test discipline).

## License

MIT OR Apache-2.0. The DSP approach follows the published literature (see the spec's
references); no GPL code is used.
