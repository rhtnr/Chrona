# Chrona M1 — Headless Pipeline Proof: Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A workspace where `chrona analyze watch.wav` detects a mechanical watch's beat rate and prints its rate in s/day, proven against a synthetic ground-truth generator, with 3-OS CI.

**Architecture:** Two crates for now — `chrona-dsp` (pure DSP: filters → envelope → autocorrelation period estimator → rate; zero I/O deps; includes the synthetic signal generator) and `chrona-cli` (`synth`/`analyze`/`verify` subcommands over WAV files). Streaming push-samples API from day one so the M3 live app reuses the identical pipeline.

**Tech Stack:** Rust (edition 2024, stable toolchain), realfft/rustfft, biquad, hound, clap, serde/serde_json, thiserror/anyhow.

**Spec:** `docs/superpowers/specs/2026-08-20-chrona-timegrapher-design.md` (§2 domain formulas, §5 pipeline stages 1–3, §10 testing strategy, §12 milestone M1). The plan argues from the spec; executors read both.

## Global Constraints

- License: `MIT OR Apache-2.0` (workspace-wide `license` field; LICENSE-MIT and LICENSE-APACHE files at root)
- **Never copy code from `tg` (GPLv2)** — algorithms are reimplemented from the spec's published description only
- `chrona-dsp` depends only on: `realfft`, `biquad`, `thiserror` (dev-deps may add `hound`) — no audio-I/O, no OS deps (spec §4)
- No `unwrap()`/`expect()` in library code paths reachable from user input; errors via `thiserror`. `unwrap` is fine in tests and in code with mathematically guaranteed invariants (document the invariant in a comment)
- All tests deterministic: every random signal is seeded (spec §10)
- `cargo fmt --check` and `cargo clippy --all-targets --workspace -- -D warnings` must pass at every commit
- Beat arithmetic verbatim from spec §2.2: `T_beat = 3600/bph` s, `T_osc = 7200/bph` s; rate `= 86400·(T_nom − T̂)/T_nom`, positive = fast (§2.3)
- Auto-detect BPH set verbatim from spec §2.2: 12000, 14400, 17280, 18000, 19800, 21600, 25200, 28800, 36000, 43200, 72000
- Calibration correction applied as `sr_eff = sr_nominal·(1 + ppm/10⁶)` (spec §3.4); M1 plumbs `ppm_correction` through (default 0.0) but ships no calibration UI/wizard
- M1 accuracy bar (from spec §10, tightened to what stages 1–3 can deliver): recovered rate within **±0.5 s/d at 30 dB SNR** and **±1.0 s/d at 10 dB SNR** on synthetic signals

## Decisions locked by this plan (parameter choices within spec ranges)

- Internal sample format `f32` mono; synth default 48 000 Hz; pipeline parameterized by actual rate
- High-pass: 2nd-order Butterworth @ 3 kHz (spec §5.1). Envelope: full-wave rectify → 2nd-order Butterworth LP @ 1.5 kHz → decimate ×16 → envelope rate = `sr/16` (3 kHz at 48 k) (spec §5.2 "LP → decimate")
- Period search band: `T_osc ∈ [0.08 s, 0.72 s]` (covers spec's 12000–72000 bph with margin)
- Stepped windows: 4, 8, 16, 32 s; keep the longest whose σ gate passes; **σ gate: σ_T < T_osc/10⁴** (spec §5.3)
- Per-cycle refinement: for candidate `T̂`, locate autocorrelation peaks near `k·T̂` (±2 %), parabolic-refine each, fit `lag_k = T·k` through the origin by least squares; `σ_T` = standard error of that slope
- BPH snap tolerance: relative deviation < 1.5 % from a table entry (inside tg's ±2 % refinement bound, spec §5.3)
- Free mode reports detected BPH but **no rate** (rate requires a nominal reference — spec §3.1 honesty rule)
- Synthetic tick = 3 damped sine bursts (unlocking 0.35×, 5.2 kHz, τ 0.8 ms; impulse 0.25× ± jitter, 6.5 kHz, τ 0.6 ms, position jittered; drop 1.0×, 7.8 kHz, τ 1.2 ms) with unlocking→drop spacing `Δt = (T_osc/π)·asin(L/(2A))` — the spec §2.3 amplitude formula inverted. M1 noise models: white + optional mains hum (pink/babble/AGC-sim arrive with M2 per spec §10)

## File Structure

```
Cargo.toml                          workspace: members = crates/*, workspace deps/lints
.gitignore                          /target, *.wav in fixtures kept explicitly
LICENSE-MIT, LICENSE-APACHE
.github/workflows/ci.yml            3-OS matrix: fmt, clippy, test
crates/chrona-dsp/
  Cargo.toml
  src/lib.rs                        public API + re-exports + crate docs
  src/bph.rs                        beat arithmetic, AUTO_BPH table, snapping
  src/synth.rs                      deterministic synthetic ground-truth generator (+ Rng)
  src/filter.rs                     DC blocker, Butterworth HP/LP (biquad wrappers)
  src/envelope.rs                   rectify → LP → decimate
  src/autocorr.rs                   FFT autocorrelation, band-limited peak search, parabolic refine
  src/period.rs                     stepped-window estimator, per-cycle refinement, σ gate
  src/analyzer.rs                   Analyzer: streaming push_samples → RateEstimate
crates/chrona-cli/
  Cargo.toml
  src/main.rs                       thin arg-parse + dispatch
  src/wav.rs                        WAV read (mono-ize) / write helpers (hound)
  src/commands.rs                   synth / analyze / verify implementations
  tests/roundtrip.rs                synth→analyze integration tests
fixtures/
  README.md                         corpus conventions + expected-JSON schema (real WAVs land in M2+)
```

---

### Task 1: Workspace scaffold, licenses, CI

**Files:**
- Create: `Cargo.toml`, `.gitignore`, `LICENSE-MIT`, `LICENSE-APACHE`, `crates/chrona-dsp/Cargo.toml`, `crates/chrona-dsp/src/lib.rs`, `.github/workflows/ci.yml`

**Interfaces:**
- Consumes: nothing (first task)
- Produces: compiling workspace; `chrona_dsp` lib crate other tasks add modules to; CI that later tasks must keep green

- [ ] **Step 1: Write the workspace and crate manifests**

`Cargo.toml` (root):

```toml
[workspace]
resolver = "3"
members = ["crates/chrona-dsp"]

[workspace.package]
edition = "2024"
license = "MIT OR Apache-2.0"
repository = "https://github.com/rohit/chrona"
version = "0.1.0"

[workspace.dependencies]
realfft = "3.5"
biquad = "0.6"
thiserror = "2"
hound = "3.5"
clap = { version = "4", features = ["derive"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
anyhow = "1"

[workspace.lints.clippy]
unwrap_used = "warn"
```

`crates/chrona-dsp/Cargo.toml`:

```toml
[package]
name = "chrona-dsp"
description = "Pure DSP core for the Chrona timegrapher: filters, envelope, beat-period estimation, metrics"
edition.workspace = true
license.workspace = true
version.workspace = true

[dependencies]
realfft = { workspace = true }
biquad = { workspace = true }
thiserror = { workspace = true }

[lints]
workspace = true
```

`crates/chrona-dsp/src/lib.rs`:

```rust
//! Chrona's pure DSP core. No audio I/O, no OS dependencies (spec §4):
//! push `f32` mono samples in, read typed estimates out.

#[cfg(test)]
mod smoke {
    #[test]
    fn workspace_builds_and_tests_run() {
        assert_eq!(2 + 2, 4);
    }
}
```

`.gitignore`:

```
/target
```

- [ ] **Step 2: Add license texts**

`LICENSE-MIT` — the standard MIT text with the line `Copyright (c) 2026 Rohit`:

```
MIT License

Copyright (c) 2026 Rohit

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

`LICENSE-APACHE` — fetch the canonical text:

```bash
curl -fsSL https://www.apache.org/licenses/LICENSE-2.0.txt -o LICENSE-APACHE
```

- [ ] **Step 3: Write CI**

`.github/workflows/ci.yml`:

```yaml
name: CI
on:
  push: { branches: [main] }
  pull_request:
jobs:
  test:
    strategy:
      fail-fast: false
      matrix:
        os: [ubuntu-latest, macos-latest, windows-latest]
    runs-on: ${{ matrix.os }}
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with: { components: "rustfmt, clippy" }
      - uses: Swatinem/rust-cache@v2
      - run: cargo fmt --all --check
      - run: cargo clippy --all-targets --workspace -- -D warnings
      - run: cargo test --workspace
```

- [ ] **Step 4: Verify locally**

Run: `cargo fmt --all --check && cargo clippy --all-targets --workspace -- -D warnings && cargo test --workspace`
Expected: builds; 1 test passes (`workspace_builds_and_tests_run`)

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "chore: workspace scaffold, dual license, 3-OS CI"
```

---

### Task 2: Beat arithmetic and BPH tables (`bph.rs`)

**Files:**
- Create: `crates/chrona-dsp/src/bph.rs`
- Modify: `crates/chrona-dsp/src/lib.rs` (add `pub mod bph;`)

**Interfaces:**
- Consumes: nothing
- Produces (used by Tasks 8–10):
  - `pub const AUTO_BPH: [u32; 11]` — spec §2.2 auto-detect set
  - `pub fn t_beat_s(bph: u32) -> f64` — `3600/bph`
  - `pub fn t_osc_s(bph: u32) -> f64` — `7200/bph`
  - `pub fn bph_from_t_osc(t_osc_s: f64) -> f64` — `7200/t_osc`
  - `pub fn snap_to_table(bph_detected: f64) -> Option<u32>` — nearest `AUTO_BPH` entry iff relative deviation < 1.5 %
  - `pub fn rate_s_per_day(t_osc_measured_s: f64, bph_nominal: u32) -> f64` — spec §2.3 formula (positive = fast)

- [ ] **Step 1: Write the failing tests**

In `crates/chrona-dsp/src/bph.rs`:

```rust
//! Beat arithmetic and standard beat-rate tables (spec §2.2–§2.3).

/// Auto-detect BPH set (spec §2.2 — Weishi + tg union).
pub const AUTO_BPH: [u32; 11] = [
    12_000, 14_400, 17_280, 18_000, 19_800, 21_600, 25_200, 28_800, 36_000, 43_200, 72_000,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn beat_arithmetic_28800() {
        assert!((t_beat_s(28_800) - 0.125).abs() < 1e-12);
        assert!((t_osc_s(28_800) - 0.250).abs() < 1e-12);
        assert!((bph_from_t_osc(0.250) - 28_800.0).abs() < 1e-9);
    }

    #[test]
    fn snap_within_tolerance() {
        assert_eq!(snap_to_table(28_795.0), Some(28_800)); // 0.017 % off
        assert_eq!(snap_to_table(28_800.0 * 1.014), Some(28_800)); // 1.4 % off — inside
        assert_eq!(snap_to_table(28_800.0 * 1.016), None); // 1.6 % off — outside
        assert_eq!(snap_to_table(21_650.0), Some(21_600));
    }

    #[test]
    fn rate_sign_convention_positive_is_fast() {
        // Watch beating exactly nominal: 0 s/d.
        assert!((rate_s_per_day(0.250, 28_800)).abs() < 1e-9);
        // Faster watch → shorter measured period → positive rate.
        // T̂ = T_nom·(1 − 10/86400) is a watch gaining 10 s/d.
        let t_fast = 0.250 * (1.0 - 10.0 / 86_400.0);
        assert!((rate_s_per_day(t_fast, 28_800) - 10.0).abs() < 1e-6);
        let t_slow = 0.250 * (1.0 + 30.0 / 86_400.0);
        assert!((rate_s_per_day(t_slow, 28_800) + 30.0).abs() < 1e-6);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Add `pub mod bph;` to `lib.rs`. Run: `cargo test -p chrona-dsp bph`
Expected: FAIL — `t_beat_s` etc. not found (compile error is the failing state here)

- [ ] **Step 3: Implement**

Append to `crates/chrona-dsp/src/bph.rs` (above the tests module):

```rust
/// Beat period in seconds (spec §2.2): one tic or one toc.
pub fn t_beat_s(bph: u32) -> f64 {
    3600.0 / bph as f64
}

/// Full oscillation period in seconds (spec §2.2): tic + toc.
pub fn t_osc_s(bph: u32) -> f64 {
    7200.0 / bph as f64
}

/// Detected beats-per-hour from a measured full-oscillation period.
pub fn bph_from_t_osc(t_osc_s: f64) -> f64 {
    7200.0 / t_osc_s
}

/// Snap a detected BPH to the auto-detect table iff within 1.5 % relative deviation.
pub fn snap_to_table(bph_detected: f64) -> Option<u32> {
    AUTO_BPH
        .iter()
        .copied()
        .map(|b| (b, ((bph_detected - b as f64) / b as f64).abs()))
        .filter(|(_, dev)| *dev < 0.015)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(b, _)| b)
}

/// Rate in s/day from measured full-oscillation period vs nominal (spec §2.3).
/// Positive = fast.
pub fn rate_s_per_day(t_osc_measured_s: f64, bph_nominal: u32) -> f64 {
    let t_nom = t_osc_s(bph_nominal);
    86_400.0 * (t_nom - t_osc_measured_s) / t_nom
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p chrona-dsp bph`
Expected: 3 tests PASS

- [ ] **Step 5: Commit**

```bash
git add crates/chrona-dsp
git commit -m "feat(dsp): beat arithmetic, BPH auto-detect table, rate formula"
```

---

### Task 3: Synthetic ground-truth generator (`synth.rs`)

**Files:**
- Create: `crates/chrona-dsp/src/synth.rs`
- Modify: `crates/chrona-dsp/src/lib.rs` (add `pub mod synth;`)

**Interfaces:**
- Consumes: `bph::{t_beat_s, t_osc_s}` (Task 2)
- Produces (used by Tasks 5–10 tests and the CLI `synth` command):
  - `pub struct Rng(u64)` with `pub fn new(seed: u64) -> Rng`, `pub fn next_f32(&mut self) -> f32` (uniform in [-1, 1))
  - `pub struct SynthConfig { pub sample_rate_hz: f64, pub duration_s: f64, pub bph: u32, pub rate_s_per_day: f64, pub beat_error_ms: f64, pub amplitude_deg: f64, pub lift_angle_deg: f64, pub snr_db: f64, pub hum_hz: Option<f64>, pub seed: u64 }` with `impl Default` (48 kHz, 40 s, 28 800 bph, 0.0 s/d, 0.0 ms, 270°, 52°, 30 dB, None, 1)
  - `pub fn synthesize(cfg: &SynthConfig) -> Vec<f32>` — deterministic for a given config
  - `pub fn unlock_to_drop_dt_s(amplitude_deg: f64, lift_angle_deg: f64, t_osc_s: f64) -> f64` — spec §2.3 amplitude formula inverted: `Δt = (T_osc/π)·asin(L/(2A))`

- [ ] **Step 1: Write the failing tests**

In `crates/chrona-dsp/src/synth.rs`:

```rust
//! Deterministic synthetic watch-signal generator (spec §10).
//! Ticks are 3 damped sine bursts (unlocking / impulse / drop, spec §2.1);
//! unlocking→drop spacing comes from inverting the amplitude formula (spec §2.3).

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_for_seed() {
        let cfg = SynthConfig { duration_s: 2.0, ..SynthConfig::default() };
        assert_eq!(synthesize(&cfg), synthesize(&cfg));
        let other = SynthConfig { seed: 2, ..cfg };
        assert_ne!(synthesize(&cfg), synthesize(&other));
    }

    #[test]
    fn dt_formula_matches_spec_worked_example() {
        // Spec §2.3: 28,800 bph, L = 52°, A ≈ 296° → Δt ≈ 7 ms.
        let dt = unlock_to_drop_dt_s(296.0, 52.0, 0.250);
        assert!((dt - 0.007).abs() < 0.0001, "dt = {dt}");
    }

    #[test]
    fn drop_pulses_land_on_the_configured_beat_grid() {
        // Loud drop pulses must appear once per beat, at the rate-adjusted period.
        let cfg = SynthConfig {
            duration_s: 6.0,
            rate_s_per_day: 30.0, // fast watch
            snr_db: 60.0,         // near-clean so peaks dominate
            ..SynthConfig::default()
        };
        let x = synthesize(&cfg);
        let sr = cfg.sample_rate_hz;
        let t_beat = crate::bph::t_beat_s(cfg.bph) * (1.0 - cfg.rate_s_per_day / 86_400.0);
        // Find the absolute peak within each expected beat window; successive peak
        // spacing must equal t_beat within 1 ms (drop bursts are the loudest, spec §2.1).
        let n_beats = 20;
        let mut peaks = Vec::new();
        for k in 0..n_beats {
            let lo = ((k as f64 + 0.2) * t_beat * sr) as usize;
            let hi = ((k as f64 + 1.2) * t_beat * sr) as usize;
            let (idx, _) = x[lo..hi]
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
                .unwrap();
            peaks.push((lo + idx) as f64 / sr);
        }
        for w in peaks.windows(2) {
            assert!(((w[1] - w[0]) - t_beat).abs() < 1e-3, "spacing {}", w[1] - w[0]);
        }
    }

    #[test]
    fn output_is_normalized_and_finite() {
        let cfg = SynthConfig { duration_s: 2.0, snr_db: 0.0, ..SynthConfig::default() };
        let x = synthesize(&cfg);
        assert!(x.iter().all(|s| s.is_finite()));
        let peak = x.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(peak <= 0.9 + 1e-6, "peak {peak}");
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Add `pub mod synth;` to `lib.rs`. Run: `cargo test -p chrona-dsp synth`
Expected: FAIL (types/functions not defined)

- [ ] **Step 3: Implement**

Above the tests module in `synth.rs`:

```rust
/// xorshift64* — tiny deterministic PRNG; no external dependency (spec §10 determinism).
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed.max(1)) // xorshift state must be nonzero
    }
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    /// Uniform in [-1, 1).
    pub fn next_f32(&mut self) -> f32 {
        ((self.next_u64() >> 40) as f32 / (1u64 << 24) as f32) * 2.0 - 1.0
    }
}

pub struct SynthConfig {
    pub sample_rate_hz: f64,
    pub duration_s: f64,
    pub bph: u32,
    pub rate_s_per_day: f64,
    pub beat_error_ms: f64,
    pub amplitude_deg: f64,
    pub lift_angle_deg: f64,
    /// Drop-pulse peak amplitude vs noise RMS, in dB.
    pub snr_db: f64,
    pub hum_hz: Option<f64>,
    pub seed: u64,
}

impl Default for SynthConfig {
    fn default() -> Self {
        SynthConfig {
            sample_rate_hz: 48_000.0,
            duration_s: 40.0,
            bph: 28_800,
            rate_s_per_day: 0.0,
            beat_error_ms: 0.0,
            amplitude_deg: 270.0,
            lift_angle_deg: 52.0,
            snr_db: 30.0,
            hum_hz: None,
            seed: 1,
        }
    }
}

/// Spec §2.3 amplitude formula solved for Δt: `Δt = (T_osc/π)·asin(L/(2A))`.
pub fn unlock_to_drop_dt_s(amplitude_deg: f64, lift_angle_deg: f64, t_osc_s: f64) -> f64 {
    (t_osc_s / std::f64::consts::PI) * (lift_angle_deg / (2.0 * amplitude_deg)).asin()
}

/// Add one damped sine burst at time `t0` (seconds).
fn add_burst(x: &mut [f32], sr: f64, t0: f64, amp: f64, f_hz: f64, tau_s: f64) {
    let start = (t0 * sr) as usize;
    let len = (tau_s * 6.0 * sr) as usize; // ~6τ ≈ −52 dB tail
    for i in 0..len {
        let Some(s) = x.get_mut(start + i) else { break };
        let t = i as f64 / sr;
        *s += (amp * (-t / tau_s).exp() * (2.0 * std::f64::consts::PI * f_hz * t).sin()) as f32;
    }
}

pub fn synthesize(cfg: &SynthConfig) -> Vec<f32> {
    let sr = cfg.sample_rate_hz;
    let n = (cfg.duration_s * sr) as usize;
    let mut x = vec![0.0f32; n];
    let mut rng = Rng::new(cfg.seed);

    // Rate-adjusted beat period (spec §2.3: T̂ = T_nom·(1 − rate/86400)).
    let t_beat = crate::bph::t_beat_s(cfg.bph) * (1.0 - cfg.rate_s_per_day / 86_400.0);
    let t_osc = 2.0 * t_beat;
    let dt = unlock_to_drop_dt_s(cfg.amplitude_deg, cfg.lift_angle_deg, t_osc);
    let be = cfg.beat_error_ms / 1000.0;

    let noise_rms = 0.02f64;
    let tick_amp = noise_rms * 10f64.powf(cfg.snr_db / 20.0);

    // Tick clusters (spec §2.1): the *drop* lands on the beat grid (it is the
    // detection anchor); unlocking precedes it by dt. Beat error shifts
    // alternate beats by ±be/2 so tic→toc and toc→tick intervals differ by 2·(be/2).
    let mut k = 0usize;
    loop {
        let shift = if k % 2 == 0 { be / 2.0 } else { -be / 2.0 };
        let t_drop = (k as f64 + 1.0) * t_beat + shift;
        if t_drop + 0.02 >= cfg.duration_s {
            break;
        }
        let t_unlock = t_drop - dt;
        let impulse_jitter = 0.15 * dt * rng.next_f32() as f64;
        let t_impulse = t_unlock + 0.4 * dt + impulse_jitter;
        add_burst(&mut x, sr, t_unlock, tick_amp * 0.35, 5_200.0, 0.0008);
        add_burst(&mut x, sr, t_impulse, tick_amp * 0.25, 6_500.0, 0.0006);
        add_burst(&mut x, sr, t_drop, tick_amp, 7_800.0, 0.0012);
        k += 1;
    }

    // White noise at the configured floor. Uniform [−1,1) has RMS 1/√3, so scale
    // by √3 to make `noise_rms` the actual RMS.
    let noise_gain = noise_rms * 3f64.sqrt();
    for s in x.iter_mut() {
        *s += (noise_gain * rng.next_f32() as f64) as f32;
    }

    // Optional mains hum + 2 harmonics (spec §3.2 hum detection needs this in fixtures).
    if let Some(f) = cfg.hum_hz {
        for (i, s) in x.iter_mut().enumerate() {
            let t = i as f64 / sr;
            let hum = 0.5 * noise_rms
                * ((2.0 * std::f64::consts::PI * f * t).sin()
                    + 0.6 * (2.0 * std::f64::consts::PI * 2.0 * f * t).sin()
                    + 0.4 * (2.0 * std::f64::consts::PI * 3.0 * f * t).sin());
            *s += hum as f32;
        }
    }

    // Normalize to ≤ 0.9 peak, preserving ratios (never clip the synthetic ADC).
    let peak = x.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    if peak > 0.9 {
        let g = 0.9 / peak;
        for s in x.iter_mut() {
            *s *= g;
        }
    }
    x
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p chrona-dsp synth`
Expected: 4 tests PASS

- [ ] **Step 5: Commit**

```bash
git add crates/chrona-dsp
git commit -m "feat(dsp): deterministic synthetic watch-signal generator"
```

---

### Task 4: Filters (`filter.rs`)

**Files:**
- Create: `crates/chrona-dsp/src/filter.rs`
- Modify: `crates/chrona-dsp/src/lib.rs` (add `pub mod filter;`)

**Interfaces:**
- Consumes: `biquad` crate
- Produces (used by Tasks 5, 8, 9):
  - `pub struct DcBlocker` with `pub fn new() -> Self`, `pub fn process(&mut self, s: f32) -> f32`
  - `pub struct Butterworth` with `pub fn high_pass(sample_rate_hz: f64, cutoff_hz: f64) -> Result<Self, FilterError>`, `pub fn low_pass(sample_rate_hz: f64, cutoff_hz: f64) -> Result<Self, FilterError>`, `pub fn process(&mut self, s: f32) -> f32`
  - `pub enum FilterError` (thiserror) — invalid cutoff/rate combinations

- [ ] **Step 1: Write the failing tests**

In `crates/chrona-dsp/src/filter.rs`:

```rust
//! Streaming filters for the precondition stage (spec §5.1).

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(sr: f64, f: f64, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| (2.0 * std::f64::consts::PI * f * i as f64 / sr).sin() as f32)
            .collect()
    }

    /// Steady-state RMS gain of a filter for a pure tone (skip the transient).
    fn tone_gain(mut filt: impl FnMut(f32) -> f32, sr: f64, f: f64) -> f64 {
        let x = sine(sr, f, (sr * 0.5) as usize);
        let y: Vec<f32> = x.iter().map(|&s| filt(s)).collect();
        let tail = &y[y.len() / 2..];
        let rms = |v: &[f32]| (v.iter().map(|s| (*s as f64).powi(2)).sum::<f64>() / v.len() as f64).sqrt();
        rms(tail) / rms(&x[x.len() / 2..])
    }

    #[test]
    fn dc_blocker_removes_offset() {
        let mut dc = DcBlocker::new();
        let y: Vec<f32> = (0..48_000).map(|_| dc.process(0.5)).collect();
        assert!(y[y.len() - 1].abs() < 1e-3);
    }

    #[test]
    fn high_pass_3k_attenuates_hum_passes_ticks() {
        let sr = 48_000.0;
        let mut hp = Butterworth::high_pass(sr, 3_000.0).unwrap();
        let g_low = tone_gain(|s| hp.process(s), sr, 100.0);
        assert!(g_low < 0.01, "100 Hz gain {g_low}"); // ≥ −40 dB (2nd order, ~59 dB expected)
        let mut hp2 = Butterworth::high_pass(sr, 3_000.0).unwrap();
        let g_band = tone_gain(|s| hp2.process(s), sr, 7_800.0);
        assert!(g_band > 0.9, "7.8 kHz gain {g_band}");
    }

    #[test]
    fn low_pass_1k5_passes_low_blocks_high() {
        let sr = 48_000.0;
        let mut lp = Butterworth::low_pass(sr, 1_500.0).unwrap();
        let g_pass = tone_gain(|s| lp.process(s), sr, 100.0);
        assert!(g_pass > 0.95, "100 Hz gain {g_pass}");
        let mut lp2 = Butterworth::low_pass(sr, 1_500.0).unwrap();
        let g_stop = tone_gain(|s| lp2.process(s), sr, 12_000.0);
        assert!(g_stop < 0.01, "12 kHz gain {g_stop}");
    }

    #[test]
    fn invalid_cutoff_is_an_error() {
        assert!(Butterworth::low_pass(48_000.0, 0.0).is_err());
        assert!(Butterworth::low_pass(48_000.0, 30_000.0).is_err()); // above Nyquist
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Add `pub mod filter;` to `lib.rs`. Run: `cargo test -p chrona-dsp filter`
Expected: FAIL (types not defined)

- [ ] **Step 3: Implement**

Above the tests module in `filter.rs`:

```rust
use biquad::{Biquad, Coefficients, DirectForm2Transposed, ToHertz, Type, Q_BUTTERWORTH_F32};

#[derive(Debug, thiserror::Error)]
pub enum FilterError {
    #[error("invalid filter parameters: cutoff {cutoff_hz} Hz at sample rate {sample_rate_hz} Hz")]
    InvalidParams { sample_rate_hz: f64, cutoff_hz: f64 },
}

/// One-pole DC blocker: y[n] = x[n] − x[n−1] + R·y[n−1], R = 0.995.
pub struct DcBlocker {
    x1: f32,
    y1: f32,
}

impl DcBlocker {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        DcBlocker { x1: 0.0, y1: 0.0 }
    }
    pub fn process(&mut self, s: f32) -> f32 {
        let y = s - self.x1 + 0.995 * self.y1;
        self.x1 = s;
        self.y1 = y;
        y
    }
}

/// 2nd-order Butterworth section (spec §5.1: HP @ 3 kHz; §5.2: LP @ 1.5 kHz).
pub struct Butterworth {
    inner: DirectForm2Transposed<f32>,
}

impl Butterworth {
    pub fn high_pass(sample_rate_hz: f64, cutoff_hz: f64) -> Result<Self, FilterError> {
        Self::build(Type::HighPass, sample_rate_hz, cutoff_hz)
    }
    pub fn low_pass(sample_rate_hz: f64, cutoff_hz: f64) -> Result<Self, FilterError> {
        Self::build(Type::LowPass, sample_rate_hz, cutoff_hz)
    }
    fn build(ty: Type<f32>, sample_rate_hz: f64, cutoff_hz: f64) -> Result<Self, FilterError> {
        if !(cutoff_hz > 0.0 && cutoff_hz < sample_rate_hz / 2.0) {
            return Err(FilterError::InvalidParams { sample_rate_hz, cutoff_hz });
        }
        let coeffs = Coefficients::<f32>::from_params(
            ty,
            (sample_rate_hz as f32).hz(),
            (cutoff_hz as f32).hz(),
            Q_BUTTERWORTH_F32,
        )
        .map_err(|_| FilterError::InvalidParams { sample_rate_hz, cutoff_hz })?;
        Ok(Butterworth { inner: DirectForm2Transposed::<f32>::new(coeffs) })
    }
    pub fn process(&mut self, s: f32) -> f32 {
        self.inner.run(s)
    }
}
```

Note: if the `biquad` 0.6 API differs from the above (e.g. `Type` is not generic, or
`from_params` is infallible), adapt the `build` internals to the crate's actual
signatures — the public `Butterworth` interface and the test behavior are the contract,
not the biquad call shape. Check `cargo doc -p biquad --open` or docs.rs/biquad/0.6.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p chrona-dsp filter`
Expected: 4 tests PASS

- [ ] **Step 5: Commit**

```bash
git add crates/chrona-dsp
git commit -m "feat(dsp): DC blocker and Butterworth HP/LP precondition filters"
```

---

### Task 5: Envelope extractor (`envelope.rs`)

**Files:**
- Create: `crates/chrona-dsp/src/envelope.rs`
- Modify: `crates/chrona-dsp/src/lib.rs` (add `pub mod envelope;`)

**Interfaces:**
- Consumes: `filter::Butterworth` (Task 4), `synth` (Task 3, tests only)
- Produces (used by Tasks 8, 9):
  - `pub const DECIMATION: usize = 16`
  - `pub struct EnvelopeExtractor` with `pub fn new(sample_rate_hz: f64) -> Result<Self, crate::filter::FilterError>`, `pub fn envelope_rate_hz(&self) -> f64` (= `sample_rate_hz / 16`), `pub fn process(&mut self, samples: &[f32], out: &mut Vec<f32>)` — appends decimated envelope samples to `out`; carries phase across calls (streaming)

- [ ] **Step 1: Write the failing tests**

In `crates/chrona-dsp/src/envelope.rs`:

```rust
//! Envelope stage (spec §5.2): full-wave rectify → Butterworth LP @ 1.5 kHz → ÷16 decimate.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synth::{synthesize, SynthConfig};

    #[test]
    fn envelope_is_nonnegative_and_decimated() {
        let cfg = SynthConfig { duration_s: 2.0, ..SynthConfig::default() };
        let x = synthesize(&cfg);
        let mut env = EnvelopeExtractor::new(cfg.sample_rate_hz).unwrap();
        let mut out = Vec::new();
        env.process(&x, &mut out);
        assert_eq!(out.len(), x.len() / DECIMATION);
        // LP ringing can undershoot slightly; envelope must be essentially nonnegative.
        assert!(out.iter().all(|&e| e > -0.05));
        assert!((env.envelope_rate_hz() - 3_000.0).abs() < 1e-9);
    }

    #[test]
    fn envelope_peaks_once_per_beat() {
        // At high SNR the decimated envelope must peak near each drop pulse.
        let cfg = SynthConfig { duration_s: 4.0, snr_db: 40.0, ..SynthConfig::default() };
        let x = synthesize(&cfg);
        let mut env = EnvelopeExtractor::new(cfg.sample_rate_hz).unwrap();
        let mut out = Vec::new();
        env.process(&x, &mut out);
        let env_rate = env.envelope_rate_hz();
        let t_beat = crate::bph::t_beat_s(cfg.bph);
        // For beats 2..=20: max envelope sample within ±10 ms of the expected drop time
        // must exceed 3× the window's median (the drop stands out of the floor).
        for k in 2..=20 {
            let t_drop = k as f64 * t_beat;
            let lo = ((t_drop - 0.010) * env_rate) as usize;
            let hi = ((t_drop + 0.010) * env_rate) as usize;
            let peak = out[lo..hi].iter().cloned().fold(f32::MIN, f32::max);
            let mut w: Vec<f32> = out[lo.saturating_sub(100)..hi + 100].to_vec();
            w.sort_by(f32::total_cmp);
            let median = w[w.len() / 2];
            assert!(peak > 3.0 * median.max(1e-6), "beat {k}: peak {peak}, median {median}");
        }
    }

    #[test]
    fn streaming_chunks_equal_one_shot() {
        let cfg = SynthConfig { duration_s: 1.0, ..SynthConfig::default() };
        let x = synthesize(&cfg);
        let mut a = EnvelopeExtractor::new(cfg.sample_rate_hz).unwrap();
        let mut one = Vec::new();
        a.process(&x, &mut one);
        let mut b = EnvelopeExtractor::new(cfg.sample_rate_hz).unwrap();
        let mut chunked = Vec::new();
        for chunk in x.chunks(479) {
            // deliberately not a multiple of DECIMATION
            b.process(chunk, &mut chunked);
        }
        assert_eq!(one, chunked);
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Add `pub mod envelope;` to `lib.rs`. Run: `cargo test -p chrona-dsp envelope`
Expected: FAIL (types not defined)

- [ ] **Step 3: Implement**

Above the tests module in `envelope.rs`:

```rust
use crate::filter::{Butterworth, FilterError};

/// Decimation factor from audio rate to envelope rate (48 kHz → 3 kHz).
pub const DECIMATION: usize = 16;

pub struct EnvelopeExtractor {
    lp: Butterworth,
    sample_rate_hz: f64,
    /// Phase within the decimation cycle, carried across `process` calls.
    phase: usize,
}

impl EnvelopeExtractor {
    pub fn new(sample_rate_hz: f64) -> Result<Self, FilterError> {
        Ok(EnvelopeExtractor {
            // 1.5 kHz keeps the envelope below the 3 kHz post-decimation Nyquist (spec §5.2).
            lp: Butterworth::low_pass(sample_rate_hz, 1_500.0)?,
            sample_rate_hz,
            phase: 0,
        })
    }

    pub fn envelope_rate_hz(&self) -> f64 {
        self.sample_rate_hz / DECIMATION as f64
    }

    /// Rectify → low-pass → keep every 16th sample. Appends to `out`.
    pub fn process(&mut self, samples: &[f32], out: &mut Vec<f32>) {
        for &s in samples {
            let e = self.lp.process(s.abs());
            if self.phase == 0 {
                out.push(e);
            }
            self.phase = (self.phase + 1) % DECIMATION;
        }
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p chrona-dsp envelope`
Expected: 3 tests PASS

- [ ] **Step 5: Commit**

```bash
git add crates/chrona-dsp
git commit -m "feat(dsp): streaming envelope extractor (rectify, LP, decimate)"
```

---

### Task 6: FFT autocorrelation + band-limited peak search (`autocorr.rs`)

**Files:**
- Create: `crates/chrona-dsp/src/autocorr.rs`
- Modify: `crates/chrona-dsp/src/lib.rs` (add `pub mod autocorr;`)

**Interfaces:**
- Consumes: `realfft`
- Produces (used by Tasks 7, 8):
  - `pub fn autocorrelate(x: &[f32]) -> Vec<f32>` — linear (zero-padded) autocorrelation, lags `0..x.len()`, normalized so lag 0 = 1.0 (all-zero input → all zeros)
  - `pub struct Peak { pub lag: f64, pub value: f32 }` (lag in fractional samples after parabolic refinement)
  - `pub fn find_peak_in_band(r: &[f32], min_lag: usize, max_lag: usize) -> Option<Peak>` — highest local maximum in `[min_lag, max_lag]`, parabolic-refined; `None` if no interior local max exists
  - `pub fn refine_peak_near(r: &[f32], expected_lag: f64, tolerance: f64) -> Option<Peak>` — peak search restricted to `expected_lag·(1 ± tolerance)` (used by per-cycle refinement, Task 8)

- [ ] **Step 1: Write the failing tests**

In `crates/chrona-dsp/src/autocorr.rs`:

```rust
//! FFT-based autocorrelation and peak location (spec §5.3).

#[cfg(test)]
mod tests {
    use super::*;

    /// O(n²) reference: linear autocorrelation r[k] = Σ x[n]·x[n+k].
    fn naive_autocorr(x: &[f32]) -> Vec<f32> {
        let n = x.len();
        let mut r = vec![0.0f32; n];
        for k in 0..n {
            let mut acc = 0.0f64;
            for i in 0..n - k {
                acc += x[i] as f64 * x[i + k] as f64;
            }
            r[k] = acc as f32;
        }
        let r0 = r[0].max(f32::MIN_POSITIVE);
        r.iter().map(|v| v / r0).collect()
    }

    #[test]
    fn matches_naive_reference() {
        let mut rng = crate::synth::Rng::new(7);
        let x: Vec<f32> = (0..1024).map(|_| rng.next_f32()).collect();
        let fast = autocorrelate(&x);
        let slow = naive_autocorr(&x);
        assert_eq!(fast.len(), 1024);
        for (i, (a, b)) in fast.iter().zip(slow.iter()).enumerate() {
            assert!((a - b).abs() < 1e-4, "lag {i}: {a} vs {b}");
        }
    }

    #[test]
    fn impulse_train_peaks_at_its_period() {
        // Period-97 impulse train → autocorr local max at lag 97.
        let mut x = vec![0.0f32; 4096];
        for i in (0..4096).step_by(97) {
            x[i] = 1.0;
        }
        let r = autocorrelate(&x);
        let p = find_peak_in_band(&r, 50, 150).unwrap();
        assert!((p.lag - 97.0).abs() < 0.5, "lag {}", p.lag);
    }

    #[test]
    fn parabolic_refinement_finds_fractional_lag() {
        // A slightly-detuned sinusoid has a fractional-lag autocorr peak.
        let period = 100.25f64;
        let x: Vec<f32> = (0..8192)
            .map(|i| (2.0 * std::f64::consts::PI * i as f64 / period).cos() as f32)
            .collect();
        let r = autocorrelate(&x);
        let p = find_peak_in_band(&r, 80, 120).unwrap();
        assert!((p.lag - period).abs() < 0.1, "lag {}", p.lag);
    }

    #[test]
    fn refine_near_expected() {
        let mut x = vec![0.0f32; 4096];
        for i in (0..4096).step_by(97) {
            x[i] = 1.0;
        }
        let r = autocorrelate(&x);
        let p = refine_peak_near(&r, 95.0, 0.05).unwrap(); // 95·(1±5 %) covers 97
        assert!((p.lag - 97.0).abs() < 0.5);
        assert!(refine_peak_near(&r, 60.0, 0.02).is_none()); // no peak near 60
    }

    #[test]
    fn zero_input_is_flat_not_nan() {
        let r = autocorrelate(&[0.0; 512]);
        assert!(r.iter().all(|v| v.is_finite()));
        assert!(find_peak_in_band(&r, 10, 100).is_none());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Add `pub mod autocorr;` to `lib.rs`. Run: `cargo test -p chrona-dsp autocorr`
Expected: FAIL (functions not defined)

- [ ] **Step 3: Implement**

Above the tests module in `autocorr.rs`:

```rust
use realfft::RealFftPlanner;

pub struct Peak {
    /// Fractional lag in samples (parabolically refined).
    pub lag: f64,
    pub value: f32,
}

/// Linear autocorrelation via FFT: zero-pad to ≥ 2n (next power of two) to avoid
/// circular wrap-around, forward FFT, |X|², inverse FFT, normalize by lag 0.
pub fn autocorrelate(x: &[f32]) -> Vec<f32> {
    let n = x.len();
    if n == 0 {
        return Vec::new();
    }
    let padded = (2 * n).next_power_of_two();
    let mut planner = RealFftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(padded);
    let ifft = planner.plan_fft_inverse(padded);

    let mut buf = vec![0.0f32; padded];
    buf[..n].copy_from_slice(x);
    let mut spectrum = fft.make_output_vec();
    // Invariant: buffer lengths come from the planner itself, so process cannot fail.
    fft.process(&mut buf, &mut spectrum).expect("planner-sized buffers");
    for c in spectrum.iter_mut() {
        *c = *c * c.conj(); // |X|² (imaginary parts become 0)
    }
    let mut r = vec![0.0f32; padded];
    ifft.process(&mut spectrum, &mut r).expect("planner-sized buffers");

    let r0 = r[0];
    if r0 <= f32::MIN_POSITIVE {
        return vec![0.0; n]; // silence in → flat autocorrelation, not NaN
    }
    r.truncate(n);
    r.iter().map(|v| v / r0).collect()
}

/// Parabolic interpolation around integer peak `i` → fractional lag offset in (−0.5, 0.5).
fn parabolic_offset(r: &[f32], i: usize) -> f64 {
    let (a, b, c) = (r[i - 1] as f64, r[i] as f64, r[i + 1] as f64);
    let denom = a - 2.0 * b + c;
    if denom.abs() < 1e-20 {
        0.0
    } else {
        (0.5 * (a - c) / denom).clamp(-0.5, 0.5)
    }
}

/// Highest interior local maximum in `[min_lag, max_lag]`, parabolic-refined.
pub fn find_peak_in_band(r: &[f32], min_lag: usize, max_lag: usize) -> Option<Peak> {
    let lo = min_lag.max(1);
    let hi = max_lag.min(r.len().saturating_sub(2));
    let mut best: Option<usize> = None;
    for i in lo..=hi {
        if r[i] > r[i - 1] && r[i] >= r[i + 1] && r[i] > 0.0 {
            if best.is_none_or(|b| r[i] > r[b]) {
                best = Some(i);
            }
        }
    }
    best.map(|i| Peak { lag: i as f64 + parabolic_offset(r, i), value: r[i] })
}

/// Peak restricted to `expected_lag·(1 ± tolerance)` (per-cycle refinement, spec §5.3).
pub fn refine_peak_near(r: &[f32], expected_lag: f64, tolerance: f64) -> Option<Peak> {
    let lo = (expected_lag * (1.0 - tolerance)).floor() as usize;
    let hi = (expected_lag * (1.0 + tolerance)).ceil() as usize;
    find_peak_in_band(r, lo, hi)
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p chrona-dsp autocorr`
Expected: 5 tests PASS

- [ ] **Step 5: Commit**

```bash
git add crates/chrona-dsp
git commit -m "feat(dsp): FFT autocorrelation with band-limited parabolic peak search"
```

---

### Task 7: Harmonic disambiguation (`autocorr.rs` extension)

**Files:**
- Modify: `crates/chrona-dsp/src/autocorr.rs`

**Interfaces:**
- Consumes: `autocorrelate`, `find_peak_in_band`, `refine_peak_near` (Task 6)
- Produces (used by Task 8):
  - `pub fn fundamental_beat_lag(r: &[f32], min_lag: usize, max_lag: usize) -> Option<Peak>` — the **beat-period** lag: strongest peak in band, then walked down integer divisors (÷2, ÷3, ÷4) while a local peak with ≥ 40 % of the candidate's value exists there (spec §5.3 "harmonic disambiguation via integer-divisor peaks"). The caller derives `T_osc = 2 × beat lag`.

**Why 40 % (design note for the implementer):** with beat error, alternate beats shift ±BE/2, which smears the beat-period peak but leaves full-oscillation multiples exact. The smeared beat peak still retains well over 40 % of the oscillation peak for realistic beat errors (≤ 2 ms vs ≥ 100 ms beat periods), while pure noise leaves no coherent 40 % peak at an exact divisor. Extreme tic/toc loudness asymmetry could in principle defeat this; that case is definitively resolved by M2's phase-fold stage — note it as a known M1 limitation in the module docs.

- [ ] **Step 1: Write the failing tests**

Append to the `tests` module in `autocorr.rs`:

```rust
    #[test]
    fn asymmetric_tic_toc_resolves_to_beat_period() {
        // Alternating 1.0 / 0.7 impulses every 300 samples: the strongest autocorr
        // peak is the full oscillation (600), but the fundamental beat lag is 300.
        let mut x = vec![0.0f32; 8192];
        for (j, i) in (0..8192).step_by(300).enumerate() {
            x[i] = if j % 2 == 0 { 1.0 } else { 0.7 };
        }
        let r = autocorrelate(&x);
        let p = fundamental_beat_lag(&r, 120, 1080).unwrap();
        assert!((p.lag - 300.0).abs() < 1.0, "lag {}", p.lag);
    }

    #[test]
    fn uniform_train_is_its_own_fundamental() {
        let mut x = vec![0.0f32; 8192];
        for i in (0..8192).step_by(250) {
            x[i] = 1.0;
        }
        let r = autocorrelate(&x);
        let p = fundamental_beat_lag(&r, 120, 1080).unwrap();
        assert!((p.lag - 250.0).abs() < 1.0, "lag {}", p.lag);
    }

    #[test]
    fn triple_harmonic_walks_down() {
        // Impulses every 200; strongest band peak may be 600 (3×) if band excludes 200?
        // Band includes 200 — but force the walk by searching only [500, 1080] first:
        // fundamental_beat_lag must still return ~200 via the divisor walk.
        let mut x = vec![0.0f32; 8192];
        for i in (0..8192).step_by(200) {
            x[i] = 1.0;
        }
        let r = autocorrelate(&x);
        let p = fundamental_beat_lag(&r, 500, 1080).unwrap();
        assert!((p.lag - 200.0).abs() < 1.0, "lag {}", p.lag);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p chrona-dsp autocorr`
Expected: 3 new tests FAIL (`fundamental_beat_lag` not defined)

- [ ] **Step 3: Implement**

Append to `autocorr.rs` (above tests):

```rust
/// Fundamental beat-period lag (spec §5.3): strongest peak in `[min_lag, max_lag]`,
/// then repeatedly step down to any integer divisor (÷2, ÷3, ÷4) that holds a local
/// peak with ≥ 40 % of the original candidate's value. Divisor peaks may lie below
/// `min_lag` — the walk is exempt from the band (the band constrains the *search*,
/// not the fundamental).
pub fn fundamental_beat_lag(r: &[f32], min_lag: usize, max_lag: usize) -> Option<Peak> {
    let start = find_peak_in_band(r, min_lag, max_lag)?;
    let floor_value = 0.4 * start.value;
    let mut current = start;
    loop {
        let mut stepped = false;
        for d in 2..=4u32 {
            let cand_lag = current.lag / d as f64;
            if cand_lag < 8.0 {
                continue; // too close to lag 0 to be a real beat period
            }
            if let Some(p) = refine_peak_near(r, cand_lag, 0.03) {
                if p.value >= floor_value {
                    current = p;
                    stepped = true;
                    break; // restart divisor walk from the new, smaller lag
                }
            }
        }
        if !stepped {
            return Some(current);
        }
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p chrona-dsp autocorr`
Expected: all autocorr tests PASS (8 total)

- [ ] **Step 5: Commit**

```bash
git add crates/chrona-dsp
git commit -m "feat(dsp): integer-divisor harmonic disambiguation to the beat period"
```

---

### Task 8: Stepped-window period estimator with σ gate (`period.rs`)

**Files:**
- Create: `crates/chrona-dsp/src/period.rs`
- Modify: `crates/chrona-dsp/src/lib.rs` (add `pub mod period;`)
- Modify: `Cargo.toml` (root — add test-profile optimization, DSP tests get slow in debug)

**Interfaces:**
- Consumes: `autocorr::{autocorrelate, fundamental_beat_lag, refine_peak_near, Peak}` (Tasks 6–7), `envelope`/`synth` (tests)
- Produces (used by Task 9):
  - `pub struct PeriodEstimate { pub t_osc_s: f64, pub sigma_s: f64, pub window_s: f64 }`
  - `pub struct PeriodEstimator` with `pub fn new(envelope_rate_hz: f64) -> Self`, `pub fn push_envelope(&mut self, env: &[f32])`, `pub fn estimate(&self) -> Option<PeriodEstimate>`
  - Constants: `pub const WINDOWS_S: [f64; 4] = [4.0, 8.0, 16.0, 32.0]`, `pub const SIGMA_GATE: f64 = 1e-4` (σ_T < T·SIGMA_GATE, spec §5.3), search band `T_osc ∈ [0.08 s, 0.72 s]` → beat band `[0.04 s, 0.36 s]`

**Algorithm (spec §5.3, fully concrete):** keep a ring of the most recent 32 s of envelope. `estimate()` tries windows largest-first over the freshest data: autocorrelate the window; `fundamental_beat_lag` over the beat band in envelope samples; candidate `T̂_osc = 2 × beat lag`. **Per-cycle refinement:** for `k = 1..=K` (`K = min(⌊(window·rate·0.9)/T̂⌋, 64)`), `refine_peak_near(r, k·T̂, 0.02)`; require ≥ 8 hits; least-squares through the origin `T = Σ(k·lag_k)/Σk²`; `σ_T = sqrt(Σ residual² / (n−1)) / sqrt(Σk²)` (standard error of the slope). Beat error does not bias this: full-oscillation multiples are immune to alternate-beat shifts. Return the first (longest) window with `σ_T < T·1e-4` and detected bph inside `[12000·0.98, 72000·1.02]`.

- [ ] **Step 1: Speed up DSP tests**

Append to root `Cargo.toml`:

```toml
[profile.test]
opt-level = 2

[profile.test.package."*"]
opt-level = 2
```

- [ ] **Step 2: Write the failing tests**

In `crates/chrona-dsp/src/period.rs`:

```rust
//! Stepped-window beat-period estimator with σ gate (spec §5.3).

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::EnvelopeExtractor;
    use crate::synth::{synthesize, SynthConfig};

    fn estimate_for(cfg: &SynthConfig) -> Option<PeriodEstimate> {
        let x = synthesize(cfg);
        let mut env = EnvelopeExtractor::new(cfg.sample_rate_hz).unwrap();
        let mut out = Vec::new();
        env.process(&x, &mut out);
        let mut pe = PeriodEstimator::new(env.envelope_rate_hz());
        pe.push_envelope(&out);
        pe.estimate()
    }

    #[test]
    fn clean_28800_within_a_ppm_scale_error() {
        let cfg = SynthConfig { snr_db: 30.0, ..SynthConfig::default() };
        let est = estimate_for(&cfg).expect("clean signal must estimate");
        let rel = (est.t_osc_s - 0.250).abs() / 0.250;
        assert!(rel < 6e-6, "rel err {rel}"); // ±0.5 s/d bar = 5.8 ppm (Global Constraints)
        assert!(est.sigma_s < est.t_osc_s * SIGMA_GATE);
        assert!(est.window_s >= 16.0, "window {}", est.window_s);
    }

    #[test]
    fn beat_error_does_not_bias_the_period() {
        let cfg = SynthConfig { beat_error_ms: 0.8, snr_db: 30.0, ..SynthConfig::default() };
        let est = estimate_for(&cfg).expect("beat error must not break estimation");
        let rel = (est.t_osc_s - 0.250).abs() / 0.250;
        assert!(rel < 6e-6, "rel err {rel}");
    }

    #[test]
    fn other_beat_rates_estimate_correctly() {
        for bph in [18_000u32, 21_600, 36_000] {
            let cfg = SynthConfig { bph, snr_db: 30.0, ..SynthConfig::default() };
            let est = estimate_for(&cfg).unwrap_or_else(|| panic!("bph {bph}"));
            let expected = crate::bph::t_osc_s(bph);
            let rel = (est.t_osc_s - expected).abs() / expected;
            assert!(rel < 6e-6, "bph {bph}: rel err {rel}");
        }
    }

    #[test]
    fn low_snr_still_estimates_within_relaxed_bar() {
        let cfg = SynthConfig { snr_db: 10.0, ..SynthConfig::default() };
        let est = estimate_for(&cfg).expect("10 dB SNR must still estimate");
        let rel = (est.t_osc_s - 0.250).abs() / 0.250;
        assert!(rel < 1.2e-5, "rel err {rel}"); // ±1.0 s/d bar (Global Constraints)
    }

    #[test]
    fn pure_noise_returns_none() {
        let mut rng = crate::synth::Rng::new(42);
        let noise: Vec<f32> = (0..(3_000.0 * 34.0) as usize).map(|_| 0.02 * rng.next_f32()).collect();
        let mut pe = PeriodEstimator::new(3_000.0);
        pe.push_envelope(&noise);
        assert!(pe.estimate().is_none(), "σ gate must reject noise");
    }

    #[test]
    fn insufficient_data_returns_none() {
        let cfg = SynthConfig { duration_s: 2.0, ..SynthConfig::default() };
        assert!(estimate_for(&cfg).is_none()); // < smallest 4 s window
    }
}
```

- [ ] **Step 3: Run tests to verify they fail**

Add `pub mod period;` to `lib.rs`. Run: `cargo test -p chrona-dsp period`
Expected: FAIL (types not defined)

- [ ] **Step 4: Implement**

Above the tests module in `period.rs`:

```rust
use crate::autocorr::{autocorrelate, fundamental_beat_lag, refine_peak_near};
use std::collections::VecDeque;

/// Stepped analysis windows in seconds, tried longest-first (spec §5.3).
pub const WINDOWS_S: [f64; 4] = [4.0, 8.0, 16.0, 32.0];
/// σ gate: accept when σ_T < T_osc · SIGMA_GATE (spec §5.3: σ < period/10⁴).
pub const SIGMA_GATE: f64 = 1e-4;
/// Full-oscillation search band in seconds (12000–72000 bph with margin).
const T_OSC_BAND_S: (f64, f64) = (0.08, 0.72);

#[derive(Debug, Clone, Copy)]
pub struct PeriodEstimate {
    /// Full oscillation period (tic + toc) in seconds, at the nominal sample clock.
    pub t_osc_s: f64,
    /// Standard error of the period fit, seconds.
    pub sigma_s: f64,
    /// Analysis window that produced the estimate, seconds.
    pub window_s: f64,
}

pub struct PeriodEstimator {
    env_rate_hz: f64,
    ring: VecDeque<f32>,
    capacity: usize,
}

impl PeriodEstimator {
    pub fn new(envelope_rate_hz: f64) -> Self {
        let capacity = (WINDOWS_S[3] * envelope_rate_hz) as usize;
        PeriodEstimator { env_rate_hz: envelope_rate_hz, ring: VecDeque::with_capacity(capacity), capacity }
    }

    pub fn push_envelope(&mut self, env: &[f32]) {
        for &e in env {
            if self.ring.len() == self.capacity {
                self.ring.pop_front();
            }
            self.ring.push_back(e);
        }
    }

    pub fn estimate(&self) -> Option<PeriodEstimate> {
        for &window_s in WINDOWS_S.iter().rev() {
            let n = (window_s * self.env_rate_hz) as usize;
            if self.ring.len() < n {
                continue;
            }
            // Freshest `n` envelope samples, mean-removed (spec §5.2).
            let start = self.ring.len() - n;
            let mut x: Vec<f32> = self.ring.iter().skip(start).copied().collect();
            let mean = x.iter().map(|v| *v as f64).sum::<f64>() / n as f64;
            for v in x.iter_mut() {
                *v -= mean as f32;
            }
            if let Some(est) = self.estimate_window(&x, window_s) {
                return Some(est);
            }
        }
        None
    }

    fn estimate_window(&self, x: &[f32], window_s: f64) -> Option<PeriodEstimate> {
        let r = autocorrelate(x);
        let beat_lo = (T_OSC_BAND_S.0 / 2.0 * self.env_rate_hz) as usize;
        let beat_hi = (T_OSC_BAND_S.1 / 2.0 * self.env_rate_hz) as usize;
        let beat = fundamental_beat_lag(&r, beat_lo, beat_hi)?;
        let t_hat = 2.0 * beat.lag; // full oscillation, in envelope samples

        // Per-cycle refinement over full-oscillation multiples (beat-error immune).
        let max_k = ((x.len() as f64 * 0.9) / t_hat).floor() as usize;
        let max_k = max_k.min(64);
        let mut ks: Vec<f64> = Vec::new();
        let mut lags: Vec<f64> = Vec::new();
        for k in 1..=max_k {
            if let Some(p) = refine_peak_near(&r, k as f64 * t_hat, 0.02) {
                ks.push(k as f64);
                lags.push(p.lag);
            }
        }
        if ks.len() < 8 {
            return None;
        }
        // Least squares through the origin: T = Σ(k·lag) / Σk².
        let sum_k2: f64 = ks.iter().map(|k| k * k).sum();
        let sum_klag: f64 = ks.iter().zip(&lags).map(|(k, l)| k * l).sum();
        let t_fit = sum_klag / sum_k2;
        let ss_res: f64 = ks.iter().zip(&lags).map(|(k, l)| (l - t_fit * k).powi(2)).sum();
        let sigma_t = (ss_res / (ks.len() as f64 - 1.0)).sqrt() / sum_k2.sqrt();

        let t_osc_s = t_fit / self.env_rate_hz;
        let sigma_s = sigma_t / self.env_rate_hz;
        let bph = crate::bph::bph_from_t_osc(t_osc_s);
        let in_band = (12_000.0 * 0.98..=72_000.0 * 1.02).contains(&bph);
        (sigma_s < t_osc_s * SIGMA_GATE && in_band).then_some(PeriodEstimate {
            t_osc_s,
            sigma_s,
            window_s,
        })
    }
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p chrona-dsp period`
Expected: 6 tests PASS. If `clean_28800_within_a_ppm_scale_error` fails on the 6-ppm bound, debug before loosening: check that refinement hits ≥ 32 multiples (print `ks.len()`), that the parabolic offsets aren't saturating at ±0.5 (peak too flat → try longer window first), and that the synth beat grid matches the assertion's expectation. The bound is the product requirement (±0.5 s/d); it must be earned, not relaxed.

- [ ] **Step 6: Commit**

```bash
git add crates/chrona-dsp Cargo.toml
git commit -m "feat(dsp): stepped-window period estimator with per-cycle refinement and sigma gate"
```

---

### Task 9: Streaming Analyzer with rate, modes, and ppm correction (`analyzer.rs`)

**Files:**
- Create: `crates/chrona-dsp/src/analyzer.rs`
- Modify: `crates/chrona-dsp/src/lib.rs` (add `pub mod analyzer;` and the re-exports listed below)

**Interfaces:**
- Consumes: `filter::{DcBlocker, Butterworth, FilterError}`, `envelope::EnvelopeExtractor`, `period::{PeriodEstimator, PeriodEstimate}`, `bph::*`
- Produces (the crate's public entry point; used by Task 10 CLI and by M3's live app):
  - `pub enum BphMode { Auto, Fixed(u32), Free }`
  - `pub struct AnalyzerConfig { pub sample_rate_hz: f64, pub bph_mode: BphMode, pub ppm_correction: f64 }` + `impl Default` (48 kHz, `Auto`, 0.0)
  - `pub struct RateEstimate { pub period: PeriodEstimate, pub bph_detected: f64, pub bph_nominal: Option<u32>, pub seconds_per_day: Option<f64>, pub calibrated: bool }`
  - `pub struct Analyzer` with `pub fn new(config: AnalyzerConfig) -> Result<Self, crate::filter::FilterError>`, `pub fn push_samples(&mut self, samples: &[f32])`, `pub fn current(&self) -> Option<RateEstimate>`
  - `lib.rs` re-exports: `pub use analyzer::{Analyzer, AnalyzerConfig, BphMode, RateEstimate};` `pub use period::PeriodEstimate;`

**Honesty rules implemented here (spec §3.1):** `seconds_per_day` is `None` when there is no defensible nominal reference — `Free` mode always; `Auto` mode when the detected BPH snaps to no table entry; `Fixed(n)` when the detected BPH deviates more than 3 % from `n` (the user pinned the wrong rate — showing a rate against it would be garbage). Clock correction (spec §3.4): `t_osc_true = t_osc_nominal / (1 + ppm/10⁶)`; `calibrated = ppm_correction != 0.0` for M1.

- [ ] **Step 1: Write the failing tests**

In `crates/chrona-dsp/src/analyzer.rs`:

```rust
//! Streaming analyzer: precondition → envelope → period → rate (spec §5, stages 1–3 + 6).

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synth::{synthesize, SynthConfig};

    fn analyze(cfg: &SynthConfig, mode: BphMode, ppm: f64) -> Option<RateEstimate> {
        let x = synthesize(cfg);
        let mut a = Analyzer::new(AnalyzerConfig {
            sample_rate_hz: cfg.sample_rate_hz,
            bph_mode: mode,
            ppm_correction: ppm,
        })
        .unwrap();
        a.push_samples(&x);
        a.current()
    }

    #[test]
    fn property_grid_rate_recovery_at_30db() {
        // Global Constraints bar: ±0.5 s/d at 30 dB SNR.
        for bph in [18_000u32, 21_600, 28_800, 36_000] {
            for rate in [-30.0f64, -5.0, 0.0, 12.0, 45.0] {
                let cfg = SynthConfig {
                    bph,
                    rate_s_per_day: rate,
                    beat_error_ms: 0.6,
                    snr_db: 30.0,
                    ..SynthConfig::default()
                };
                let est = analyze(&cfg, BphMode::Auto, 0.0)
                    .unwrap_or_else(|| panic!("no estimate: bph {bph} rate {rate}"));
                assert_eq!(est.bph_nominal, Some(bph), "bph {bph} rate {rate}");
                let got = est.seconds_per_day.expect("auto+snap must produce a rate");
                assert!((got - rate).abs() < 0.5, "bph {bph}: want {rate}, got {got}");
            }
        }
    }

    #[test]
    fn rate_recovery_at_10db() {
        // Global Constraints bar: ±1.0 s/d at 10 dB SNR.
        for rate in [-30.0f64, 45.0] {
            let cfg = SynthConfig { rate_s_per_day: rate, snr_db: 10.0, ..SynthConfig::default() };
            let est = analyze(&cfg, BphMode::Auto, 0.0).expect("10 dB must estimate");
            let got = est.seconds_per_day.expect("rate");
            assert!((got - rate).abs() < 1.0, "want {rate}, got {got}");
        }
    }

    #[test]
    fn ppm_correction_removes_clock_error() {
        // An ADC running 50 ppm fast makes a perfect watch look −4.32 s/d slow.
        let apparent = -86_400.0 * 50e-6;
        let cfg = SynthConfig { rate_s_per_day: apparent, ..SynthConfig::default() };
        let uncal = analyze(&cfg, BphMode::Auto, 0.0).unwrap();
        assert!((uncal.seconds_per_day.unwrap() - apparent).abs() < 0.5);
        assert!(!uncal.calibrated);
        let cal = analyze(&cfg, BphMode::Auto, 50.0).unwrap();
        assert!(cal.seconds_per_day.unwrap().abs() < 0.5, "corrected {:?}", cal.seconds_per_day);
        assert!(cal.calibrated);
    }

    #[test]
    fn free_mode_reports_detection_but_no_rate() {
        let cfg = SynthConfig::default();
        let est = analyze(&cfg, BphMode::Free, 0.0).unwrap();
        assert!(est.seconds_per_day.is_none());
        assert!(est.bph_nominal.is_none());
        assert!((est.bph_detected - 28_800.0).abs() < 30.0);
    }

    #[test]
    fn fixed_mode_rejects_gross_mismatch() {
        let cfg = SynthConfig::default(); // 28,800 bph signal
        let ok = analyze(&cfg, BphMode::Fixed(28_800), 0.0).unwrap();
        assert!(ok.seconds_per_day.is_some());
        let wrong = analyze(&cfg, BphMode::Fixed(18_000), 0.0).unwrap();
        assert!(wrong.seconds_per_day.is_none(), "3 % mismatch guard");
        assert_eq!(wrong.bph_nominal, Some(18_000)); // the pin is still reported
    }

    #[test]
    fn chunked_push_equals_one_shot() {
        let cfg = SynthConfig { duration_s: 20.0, ..SynthConfig::default() };
        let x = synthesize(&cfg);
        let mk = || {
            Analyzer::new(AnalyzerConfig {
                sample_rate_hz: cfg.sample_rate_hz,
                bph_mode: BphMode::Auto,
                ppm_correction: 0.0,
            })
            .unwrap()
        };
        let mut a = mk();
        a.push_samples(&x);
        let mut b = mk();
        for chunk in x.chunks(480) {
            b.push_samples(chunk);
        }
        let (ra, rb) = (a.current().unwrap(), b.current().unwrap());
        assert!((ra.period.t_osc_s - rb.period.t_osc_s).abs() < 1e-12);
    }

    #[test]
    fn silence_yields_none() {
        let mut a = Analyzer::new(AnalyzerConfig::default()).unwrap();
        a.push_samples(&vec![0.0f32; 48_000 * 10]);
        assert!(a.current().is_none());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Add `pub mod analyzer;` plus the re-exports to `lib.rs`. Run: `cargo test -p chrona-dsp analyzer`
Expected: FAIL (types not defined)

- [ ] **Step 3: Implement**

Above the tests module in `analyzer.rs`:

```rust
use crate::envelope::EnvelopeExtractor;
use crate::filter::{Butterworth, DcBlocker, FilterError};
use crate::period::{PeriodEstimate, PeriodEstimator};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BphMode {
    /// Detect and snap to the standard table (spec §2.2).
    Auto,
    /// User-pinned nominal BPH.
    Fixed(u32),
    /// Report detection only; no rate (no nominal reference — spec §3.1).
    Free,
}

#[derive(Debug, Clone, Copy)]
pub struct AnalyzerConfig {
    pub sample_rate_hz: f64,
    pub bph_mode: BphMode,
    /// Audio-clock correction in ppm (spec §3.4). 0.0 = uncalibrated.
    pub ppm_correction: f64,
}

impl Default for AnalyzerConfig {
    fn default() -> Self {
        AnalyzerConfig { sample_rate_hz: 48_000.0, bph_mode: BphMode::Auto, ppm_correction: 0.0 }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct RateEstimate {
    pub period: PeriodEstimate,
    /// Clock-corrected detected beats per hour.
    pub bph_detected: f64,
    /// The nominal reference in force (snapped, or the Fixed pin), if any.
    pub bph_nominal: Option<u32>,
    /// Rate vs nominal, s/day, positive = fast. None when no defensible nominal (spec §3.1).
    pub seconds_per_day: Option<f64>,
    pub calibrated: bool,
}

pub struct Analyzer {
    config: AnalyzerConfig,
    dc: DcBlocker,
    hp: Butterworth,
    envelope: EnvelopeExtractor,
    period: PeriodEstimator,
    scratch: Vec<f32>,
}

impl Analyzer {
    pub fn new(config: AnalyzerConfig) -> Result<Self, FilterError> {
        let envelope = EnvelopeExtractor::new(config.sample_rate_hz)?;
        let period = PeriodEstimator::new(envelope.envelope_rate_hz());
        Ok(Analyzer {
            config,
            dc: DcBlocker::new(),
            hp: Butterworth::high_pass(config.sample_rate_hz, 3_000.0)?, // spec §5.1
            envelope,
            period,
            scratch: Vec::new(),
        })
    }

    pub fn push_samples(&mut self, samples: &[f32]) {
        self.scratch.clear();
        self.scratch.reserve(samples.len());
        for &s in samples {
            self.scratch.push(self.hp.process(self.dc.process(s)));
        }
        let filtered = std::mem::take(&mut self.scratch);
        let mut env_out = Vec::with_capacity(filtered.len() / crate::envelope::DECIMATION + 1);
        self.envelope.process(&filtered, &mut env_out);
        self.period.push_envelope(&env_out);
        self.scratch = filtered; // reuse the allocation next call
    }

    pub fn current(&self) -> Option<RateEstimate> {
        let raw = self.period.estimate()?;
        // Spec §3.4: sr_eff = sr_nom·(1 + ppm/1e6) ⇒ true seconds = nominal seconds / (1 + ppm/1e6).
        let clock = 1.0 + self.config.ppm_correction / 1e6;
        let period = PeriodEstimate {
            t_osc_s: raw.t_osc_s / clock,
            sigma_s: raw.sigma_s / clock,
            window_s: raw.window_s,
        };
        let bph_detected = crate::bph::bph_from_t_osc(period.t_osc_s);

        let (bph_nominal, seconds_per_day) = match self.config.bph_mode {
            BphMode::Free => (None, None),
            BphMode::Auto => match crate::bph::snap_to_table(bph_detected) {
                Some(nom) => (Some(nom), Some(crate::bph::rate_s_per_day(period.t_osc_s, nom))),
                None => (None, None),
            },
            BphMode::Fixed(nom) => {
                let dev = (bph_detected - nom as f64).abs() / nom as f64;
                let rate = (dev <= 0.03).then(|| crate::bph::rate_s_per_day(period.t_osc_s, nom));
                (Some(nom), rate)
            }
        };

        Some(RateEstimate {
            period,
            bph_detected,
            bph_nominal,
            seconds_per_day,
            calibrated: self.config.ppm_correction != 0.0,
        })
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p chrona-dsp` (the whole crate — all prior modules must stay green)
Expected: all chrona-dsp tests PASS. The property grid is the M1 acceptance bar; treat a failure exactly as in Task 8 Step 5 (debug, don't loosen).

- [ ] **Step 5: Run lint gates and commit**

Run: `cargo fmt --all && cargo clippy --all-targets --workspace -- -D warnings`

```bash
git add crates/chrona-dsp Cargo.toml
git commit -m "feat(dsp): streaming Analyzer with BPH modes, rate, and ppm clock correction"
```

---

### Task 10: CLI — `chrona synth` and `chrona analyze`

**Files:**
- Create: `crates/chrona-cli/Cargo.toml`, `crates/chrona-cli/src/main.rs`, `crates/chrona-cli/src/wav.rs`, `crates/chrona-cli/src/commands.rs`, `crates/chrona-cli/tests/roundtrip.rs`
- Modify: `Cargo.toml` (root — add `"crates/chrona-cli"` to `members`)

**Interfaces:**
- Consumes: `chrona_dsp::{Analyzer, AnalyzerConfig, BphMode, RateEstimate}`, `chrona_dsp::synth::{synthesize, SynthConfig}`
- Produces (used by Task 11 and by humans/CI):
  - Binary `chrona` with subcommands `synth` and `analyze`
  - `pub fn read_mono(path: &Path) -> anyhow::Result<(Vec<f32>, f64)>` and `pub fn write_mono_f32(path: &Path, samples: &[f32], sample_rate_hz: f64) -> anyhow::Result<()>` in `wav.rs`
  - `pub fn run_analyze(args: &AnalyzeArgs) -> anyhow::Result<AnalyzeReport>` and `pub struct AnalyzeReport` (serde-Serialize) in `commands.rs`
  - Exit codes: 0 = ok, 2 = no beat found, 1 = error

- [ ] **Step 1: Write the failing integration test**

`crates/chrona-cli/Cargo.toml`:

```toml
[package]
name = "chrona-cli"
description = "Chrona timegrapher CLI: synthesize test signals, analyze WAV recordings"
edition.workspace = true
license.workspace = true
version.workspace = true

[[bin]]
name = "chrona"
path = "src/main.rs"

[dependencies]
chrona-dsp = { path = "../chrona-dsp" }
clap = { workspace = true }
hound = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
anyhow = { workspace = true }

[dev-dependencies]
tempfile = "3"
assert_cmd = "2"

[lints]
workspace = true
```

`crates/chrona-cli/tests/roundtrip.rs`:

```rust
//! synth → WAV → analyze roundtrips (spec §10: the CLI is the dev harness).

use assert_cmd::Command;

fn chrona() -> Command {
    Command::cargo_bin("chrona").expect("binary builds")
}

#[test]
fn synth_then_analyze_recovers_the_configured_rate() {
    let dir = tempfile::tempdir().unwrap();
    let wav = dir.path().join("t.wav");
    chrona()
        .args(["synth", wav.to_str().unwrap(), "--bph", "21600", "--rate", "12.5", "--snr", "30"])
        .assert()
        .success();
    let out = chrona()
        .args(["analyze", wav.to_str().unwrap(), "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(v["status"], "ok");
    assert_eq!(v["bph_nominal"], 21_600);
    let rate = v["rate_s_per_day"].as_f64().unwrap();
    assert!((rate - 12.5).abs() < 0.5, "rate {rate}");
    assert_eq!(v["calibrated"], false);
}

#[test]
fn analyze_honors_ppm_and_fixed_bph() {
    let dir = tempfile::tempdir().unwrap();
    let wav = dir.path().join("t.wav");
    // Apparent −4.32 s/d caused by a 50 ppm-fast clock (see analyzer unit tests).
    chrona()
        .args(["synth", wav.to_str().unwrap(), "--rate", "-4.32"])
        .assert()
        .success();
    let out = chrona()
        .args(["analyze", wav.to_str().unwrap(), "--bph", "28800", "--ppm", "50", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
    let rate = v["rate_s_per_day"].as_f64().unwrap();
    assert!(rate.abs() < 0.5, "corrected rate {rate}");
    assert_eq!(v["calibrated"], true);
}

#[test]
fn analyze_noise_exits_2_with_no_beat() {
    let dir = tempfile::tempdir().unwrap();
    let wav = dir.path().join("noise.wav");
    chrona()
        .args(["synth", wav.to_str().unwrap(), "--snr", "-40", "--duration", "10"])
        .assert()
        .success();
    let out = chrona()
        .args(["analyze", wav.to_str().unwrap(), "--json"])
        .assert()
        .code(2)
        .get_output()
        .stdout
        .clone();
    let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(v["status"], "no_beat");
}

#[test]
fn missing_file_is_an_error() {
    chrona().args(["analyze", "does-not-exist.wav"]).assert().code(1);
}
```

- [ ] **Step 2: Run to verify it fails**

Add `"crates/chrona-cli"` to workspace members. Run: `cargo test -p chrona-cli`
Expected: FAIL (binary/sources don't exist yet)

- [ ] **Step 3: Implement WAV I/O**

`crates/chrona-cli/src/wav.rs`:

```rust
use anyhow::Context;
use std::path::Path;

/// Read any PCM/float WAV, average channels to mono, return (samples, sample_rate).
pub fn read_mono(path: &Path) -> anyhow::Result<(Vec<f32>, f64)> {
    let mut reader = hound::WavReader::open(path).with_context(|| format!("open {}", path.display()))?;
    let spec = reader.spec();
    let ch = spec.channels as usize;
    let interleaved: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader.samples::<f32>().collect::<Result<_, _>>()?,
        hound::SampleFormat::Int => {
            let scale = (1i64 << (spec.bits_per_sample - 1)) as f32;
            reader.samples::<i32>().map(|s| s.map(|v| v as f32 / scale)).collect::<Result<_, _>>()?
        }
    };
    let mono: Vec<f32> = interleaved
        .chunks_exact(ch)
        .map(|frame| frame.iter().sum::<f32>() / ch as f32)
        .collect();
    Ok((mono, spec.sample_rate as f64))
}

pub fn write_mono_f32(path: &Path, samples: &[f32], sample_rate_hz: f64) -> anyhow::Result<()> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: sample_rate_hz as u32,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut w = hound::WavWriter::create(path, spec).with_context(|| format!("create {}", path.display()))?;
    for &s in samples {
        w.write_sample(s)?;
    }
    w.finalize()?;
    Ok(())
}
```

- [ ] **Step 4: Implement commands and main**

`crates/chrona-cli/src/commands.rs`:

```rust
use anyhow::{bail, Context};
use chrona_dsp::synth::{synthesize, SynthConfig};
use chrona_dsp::{Analyzer, AnalyzerConfig, BphMode};
use clap::Args;
use serde::Serialize;
use std::path::PathBuf;

#[derive(Args)]
pub struct SynthArgs {
    /// Output WAV path
    pub out: PathBuf,
    #[arg(long, default_value_t = 28_800)]
    pub bph: u32,
    /// Applied rate error in s/day (positive = fast)
    #[arg(long, default_value_t = 0.0)]
    pub rate: f64,
    #[arg(long, default_value_t = 0.0)]
    pub beat_error: f64,
    #[arg(long, default_value_t = 270.0)]
    pub amplitude: f64,
    #[arg(long, default_value_t = 52.0)]
    pub lift: f64,
    /// Drop-pulse peak vs noise RMS, dB
    #[arg(long, default_value_t = 30.0)]
    pub snr: f64,
    #[arg(long, default_value_t = 40.0)]
    pub duration: f64,
    #[arg(long, default_value_t = 1)]
    pub seed: u64,
    /// Add mains hum at this frequency (e.g. 50 or 60)
    #[arg(long)]
    pub hum: Option<f64>,
}

pub fn run_synth(a: &SynthArgs) -> anyhow::Result<()> {
    let cfg = SynthConfig {
        bph: a.bph,
        rate_s_per_day: a.rate,
        beat_error_ms: a.beat_error,
        amplitude_deg: a.amplitude,
        lift_angle_deg: a.lift,
        snr_db: a.snr,
        duration_s: a.duration,
        seed: a.seed,
        hum_hz: a.hum,
        ..SynthConfig::default()
    };
    crate::wav::write_mono_f32(&a.out, &synthesize(&cfg), cfg.sample_rate_hz)
}

#[derive(Args)]
pub struct AnalyzeArgs {
    /// Input WAV file
    pub file: PathBuf,
    /// "auto", "free", or a numeric BPH like 28800
    #[arg(long, default_value = "auto")]
    pub bph: String,
    /// Audio-clock correction in ppm (spec §3.4)
    #[arg(long, default_value_t = 0.0)]
    pub ppm: f64,
    #[arg(long)]
    pub json: bool,
}

#[derive(Serialize)]
pub struct AnalyzeReport {
    pub status: &'static str, // "ok" | "no_beat"
    pub file: String,
    pub sample_rate_hz: f64,
    pub duration_s: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bph_detected: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bph_nominal: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rate_s_per_day: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub period_sigma_s: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window_s: Option<f64>,
    pub calibrated: bool,
}

pub fn parse_bph_mode(s: &str) -> anyhow::Result<BphMode> {
    match s {
        "auto" => Ok(BphMode::Auto),
        "free" => Ok(BphMode::Free),
        n => Ok(BphMode::Fixed(n.parse::<u32>().with_context(|| format!("invalid --bph '{n}'"))?)),
    }
}

pub fn run_analyze(a: &AnalyzeArgs) -> anyhow::Result<AnalyzeReport> {
    let (samples, sr) = crate::wav::read_mono(&a.file)?;
    if samples.is_empty() {
        bail!("empty WAV: {}", a.file.display());
    }
    let mut analyzer = Analyzer::new(AnalyzerConfig {
        sample_rate_hz: sr,
        bph_mode: parse_bph_mode(&a.bph)?,
        ppm_correction: a.ppm,
    })?;
    analyzer.push_samples(&samples);
    let duration_s = samples.len() as f64 / sr;
    let base = |status| AnalyzeReport {
        status,
        file: a.file.display().to_string(),
        sample_rate_hz: sr,
        duration_s,
        bph_detected: None,
        bph_nominal: None,
        rate_s_per_day: None,
        period_sigma_s: None,
        window_s: None,
        calibrated: a.ppm != 0.0,
    };
    Ok(match analyzer.current() {
        None => base("no_beat"),
        Some(est) => AnalyzeReport {
            bph_detected: Some(est.bph_detected),
            bph_nominal: est.bph_nominal,
            rate_s_per_day: est.seconds_per_day,
            period_sigma_s: Some(est.period.sigma_s),
            window_s: Some(est.period.window_s),
            calibrated: est.calibrated,
            ..base("ok")
        },
    })
}

pub fn print_human(r: &AnalyzeReport) {
    println!("File: {} ({} Hz, {:.1} s)", r.file, r.sample_rate_hz, r.duration_s);
    match r.status {
        "ok" => {
            match (r.bph_nominal, r.bph_detected) {
                (Some(nom), Some(det)) => println!("Beat rate: {nom} bph (detected {det:.1})"),
                (None, Some(det)) => println!("Beat rate: {det:.1} bph detected (no nominal reference)"),
                _ => {}
            }
            match r.rate_s_per_day {
                Some(rate) => {
                    let badge = if r.calibrated { "" } else { "  [uncalibrated timebase]" };
                    println!("Rate: {rate:+.1} s/d{badge}");
                }
                // Spec §3.1: no defensible nominal → no rate, with the reason.
                None => println!("Rate: — (no nominal beat rate to compare against)"),
            }
            if let (Some(sig), Some(w)) = (r.period_sigma_s, r.window_s) {
                println!("Period σ: {:.1} µs over {w:.0} s window", sig * 1e6);
            }
        }
        _ => println!("No beat found — is a watch near the microphone? (try `chrona` Mic Doctor in a later milestone)"),
    }
}
```

`crates/chrona-cli/src/main.rs`:

```rust
mod commands;
mod wav;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "chrona", version, about = "Chrona timegrapher CLI")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Generate a synthetic watch recording (deterministic ground truth)
    Synth(commands::SynthArgs),
    /// Analyze a WAV recording of a mechanical watch
    Analyze(commands::AnalyzeArgs),
}

fn main() {
    let cli = Cli::parse();
    let code = match run(cli) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e:#}");
            1
        }
    };
    std::process::exit(code);
}

fn run(cli: Cli) -> anyhow::Result<i32> {
    match cli.cmd {
        Cmd::Synth(a) => {
            commands::run_synth(&a)?;
            Ok(0)
        }
        Cmd::Analyze(a) => {
            let report = commands::run_analyze(&a)?;
            if a.json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                commands::print_human(&report);
            }
            Ok(if report.status == "ok" { 0 } else { 2 })
        }
    }
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p chrona-cli`
Expected: 4 integration tests PASS (note: `--snr -40` in the no-beat test buries ticks 40 dB *below* the noise floor — nothing detectable)

- [ ] **Step 6: Lint and commit**

Run: `cargo fmt --all && cargo clippy --all-targets --workspace -- -D warnings && cargo test --workspace`

```bash
git add -A
git commit -m "feat(cli): chrona binary with synth and analyze subcommands"
```

---

### Task 11: `chrona verify`, fixtures corpus conventions, project README

**Files:**
- Create: `fixtures/README.md`, `crates/chrona-cli/tests/verify.rs`, `README.md`
- Modify: `crates/chrona-cli/src/commands.rs`, `crates/chrona-cli/src/main.rs`

**Interfaces:**
- Consumes: `run_analyze`, `AnalyzeArgs`, `parse_bph_mode` (Task 10)
- Produces: `chrona verify <dir>` — runs every `*.json` expectation in a directory against its WAV; exit 0 all-pass (or empty), 1 any-fail. Expectation schema (one JSON object per file):

```json
{
  "file": "eta2824_dial_up_contact_mic.wav",
  "bph": "auto",
  "ppm": 0.0,
  "expect_rate_s_per_day": 7.2,
  "tol_rate": 1.0
}
```

`"bph"` takes the same strings as `--bph`. `"file"` is relative to the JSON's directory. Real recordings land in M2+ (spec §10); committed WAVs should be ≤ 60 s, 16-bit PCM mono to keep the repo lean.

- [ ] **Step 1: Write the failing test**

`crates/chrona-cli/tests/verify.rs`:

```rust
use assert_cmd::Command;

fn chrona() -> Command {
    Command::cargo_bin("chrona").expect("binary builds")
}

#[test]
fn verify_passes_a_good_expectation_and_fails_a_bad_one() {
    let dir = tempfile::tempdir().unwrap();
    let wav = dir.path().join("w.wav");
    chrona()
        .args(["synth", wav.to_str().unwrap(), "--rate", "10.0"])
        .assert()
        .success();
    let good = r#"{"file":"w.wav","bph":"auto","expect_rate_s_per_day":10.0,"tol_rate":1.0}"#;
    std::fs::write(dir.path().join("good.json"), good).unwrap();
    chrona().args(["verify", dir.path().to_str().unwrap()]).assert().success();

    let bad = r#"{"file":"w.wav","bph":"auto","expect_rate_s_per_day":-25.0,"tol_rate":1.0}"#;
    std::fs::write(dir.path().join("bad.json"), bad).unwrap();
    chrona().args(["verify", dir.path().to_str().unwrap()]).assert().code(1);
}

#[test]
fn verify_empty_dir_succeeds_with_notice() {
    let dir = tempfile::tempdir().unwrap();
    chrona().args(["verify", dir.path().to_str().unwrap()]).assert().success();
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p chrona-cli verify`
Expected: FAIL (`verify` subcommand unknown)

- [ ] **Step 3: Implement `verify`**

Append to `commands.rs`:

```rust
#[derive(Args)]
pub struct VerifyArgs {
    /// Directory containing *.json expectations next to their WAV files
    pub dir: PathBuf,
}

#[derive(serde::Deserialize)]
struct Expectation {
    file: String,
    #[serde(default = "default_bph_mode")]
    bph: String,
    #[serde(default)]
    ppm: f64,
    expect_rate_s_per_day: f64,
    tol_rate: f64,
}

fn default_bph_mode() -> String {
    "auto".into()
}

/// Returns the number of failures.
pub fn run_verify(a: &VerifyArgs) -> anyhow::Result<usize> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(&a.dir)
        .with_context(|| format!("read dir {}", a.dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    entries.sort();
    if entries.is_empty() {
        println!("verify: no expectations in {} — nothing to do", a.dir.display());
        return Ok(0);
    }
    let mut failures = 0usize;
    for path in &entries {
        let text = std::fs::read_to_string(path)?;
        let exp: Expectation =
            serde_json::from_str(&text).with_context(|| format!("parse {}", path.display()))?;
        let report = run_analyze(&AnalyzeArgs {
            file: a.dir.join(&exp.file),
            bph: exp.bph.clone(),
            ppm: exp.ppm,
            json: false,
        })?;
        let verdict = match report.rate_s_per_day {
            Some(rate) if (rate - exp.expect_rate_s_per_day).abs() <= exp.tol_rate => {
                format!("PASS  rate {rate:+.2} s/d (want {:+.2} ± {})", exp.expect_rate_s_per_day, exp.tol_rate)
            }
            Some(rate) => {
                failures += 1;
                format!("FAIL  rate {rate:+.2} s/d (want {:+.2} ± {})", exp.expect_rate_s_per_day, exp.tol_rate)
            }
            None => {
                failures += 1;
                format!("FAIL  no rate (status {})", report.status)
            }
        };
        println!("{}: {verdict}", exp.file);
    }
    Ok(failures)
}
```

In `main.rs`, add the variant and arm:

```rust
    /// Run expectation files against their recordings (regression corpus)
    Verify(commands::VerifyArgs),
```

```rust
        Cmd::Verify(a) => {
            let failures = commands::run_verify(&a)?;
            Ok(if failures == 0 { 0 } else { 1 })
        }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p chrona-cli`
Expected: all CLI tests PASS

- [ ] **Step 5: Write `fixtures/README.md` and project `README.md`**

`fixtures/README.md`:

```markdown
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
```

`README.md` (root):

```markdown
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
```

- [ ] **Step 6: Full gate and commit**

Run: `cargo fmt --all --check && cargo clippy --all-targets --workspace -- -D warnings && cargo test --workspace`
Expected: everything green

```bash
git add -A
git commit -m "feat(cli): verify subcommand, fixture corpus conventions, project README"
```

---

## Self-review notes (already applied)

1. **Spec coverage (M1 = spec §12.1):** workspace scaffold → Task 1; synthetic generator → Task 3; stage 1 filters → Task 4; stage 2 envelope → Task 5; stage 3 period estimation → Tasks 6–8; rate → Tasks 2, 9; `chrona-cli analyze` on fixtures → Tasks 10–11; CI → Task 1. Stages 4–7 (fold, matched filter, unlocking pulse, beat error, amplitude, tier engine) are M2 by design and deliberately absent here.
2. **Known M1 limitation, documented in Task 7:** extreme tic/toc loudness asymmetry could defeat the 40 % divisor rule; M2's phase-fold stage resolves it definitively.
3. **Type consistency check:** `EnvelopeExtractor::envelope_rate_hz` (Tasks 5→8→9), `PeriodEstimate { t_osc_s, sigma_s, window_s }` (Tasks 8→9→10), `BphMode`/`AnalyzerConfig`/`RateEstimate` (Tasks 9→10), `run_analyze`/`AnalyzeArgs`/`parse_bph_mode` (Tasks 10→11) — names and signatures match across tasks.
4. **API-drift guards:** Task 4 (biquad) and Task 6 (realfft) call third-party APIs from memory; both tasks pin the *public contract + tests* as the requirement and instruct checking docs.rs if signatures drifted. This is the only sanctioned deviation.
5. **Noise scaling fix:** synth noise samples are uniform in [−1, 1), so achieving the configured RMS requires scaling by √3 (RMS of uniform = 1/√3), not √2. Task 3's implementation must use `noise_rms * 3f64.sqrt() * rng.next_f32() as f64` — the code block above has been corrected accordingly; if you spot `SQRT_2` there, that's the bug this note kills.





