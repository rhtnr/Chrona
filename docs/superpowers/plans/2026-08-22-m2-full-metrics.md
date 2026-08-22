# Chrona M2 — Full Metrics: Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn the M1 rate meter into a real timegrapher: tic/toc separation, per-beat event extraction (learned matched filter + unlocking pulses), **beat error** and **amplitude** with validity gates, the tier/confidence engine, and quartz-reference calibration math — all proven against synthetic ground truth and wired through the CLI.

**Architecture:** Four new `chrona-dsp` modules layered on M1's pipeline — `fold` (stage 4: phase fold → tic/toc anchors + octave guard), `events` (stage 5: raw-signal matched-filter drop detection + envelope-edge unlocking extraction), `metrics` (stage 6: three-parameter regression → rate/beat error; amplitude with gates), `tier` (stage 7) — plus `cal` (quartz ppm math) and a small `ring` utility. `Analyzer::current()` grows from `RateEstimate` to a full `MetricsSnapshot` (breaking API; CLI updated in the same milestone).

**Tech Stack:** unchanged from M1 — Rust stable (pinned via rust-toolchain.toml), realfft, biquad, thiserror in `chrona-dsp`; clap/hound/serde_json/anyhow in `chrona-cli`.

**Spec:** `docs/superpowers/specs/2026-08-20-chrona-timegrapher-design.md` (§2.1 pulse physics, §2.3 formulas, §3.1 tiers, §3.4 calibration, §5 stages 4–7, §10 bars, §12 M2). Also read `docs/superpowers/notes/m1-followups.md` — Task 1 executes its spec amendments and Tasks 1–2 clear its code debt. The spec is the binding authority.

## Global Constraints

- Everything from the M1 plan's Global Constraints still holds: MIT OR Apache-2.0, no tg code copied, `chrona-dsp` deps limited to realfft/biquad/thiserror, no `unwrap()`/`expect()` in library paths reachable from user input (tests exempt; `.expect` allowed only for documented mathematically-guaranteed invariants), deterministic seeded tests, `cargo fmt --all --check` + `cargo clippy --all-targets --workspace -- -D warnings` green at every commit
- Metric definitions verbatim from spec §2.3: beat error `BE = |t1 − t2| / 2` (alternate unlocking-interval asymmetry), displayed 0.0–9.9 ms at 0.1 ms; amplitude `A = L / (2·sin(π·Δt/T_osc))` with gates **135° ≤ A ≤ 360°** and **|A_tic − A_toc| < 60°**, reported as the tic/toc mean; lift angle default **52.0°**, accepted range 10–90
- Tier thresholds verbatim from spec §3.1 (initial values, tunable): **Tier 2** when ≥ 60 % of expected beats yield onsets AND onset jitter σ ≤ 0.5 ms; **Tier 3** when ≥ 50 % of anchored beats yield a gated unlocking pulse; metrics below their tier are `None`, never fabricated
- Stage 5 verbatim from spec §5.5: tick template learned per anchor by phase-aligned trimmed-mean averaging **of the band-passed signal (not just the envelope)**; unlocking pulse found in the `T_beat/8` window *before* the drop with noise floor from the `T_beat/8` window *after* the peak, threshold `max(1 % of global max, 1.4 × noise)` escalating ×1.4 while below 20 % of max
- M2 accuracy bars (spec §10, at 30 dB SNR on synthetic ground truth, 40 s, averaging window 30 s): recovered **beat error within ±0.1 ms**, **amplitude within ±5°**, rate (regression source) within **±0.3 s/d**
- Calibration convention verbatim from spec §3.4: stored correction is ppm on the sample clock, applied as `sr_eff = sr_nominal·(1 + ppm/10⁶)` (time divide by `1 + ppm/10⁶`); quartz reference = 1 Hz stepper tick, ≥ 5 minutes required, result = ppm such that passing it as `--ppm` zeroes a perfect quartz's rate
- Breaking API changes are confined to this milestone and every consumer (analyzer tests, CLI) is updated within the task that breaks it — the workspace is never left red between tasks

## Decisions locked by this plan

- **Amplitude timing is bias-cancelled by construction.** The envelope low-pass delays leading edges by a fixed shape-dependent amount. Both Δt endpoints (unlocking edge AND drop edge) are therefore measured with the *same* leading-edge threshold algorithm on the *same* smoothed envelope, so the delay cancels in the difference. The raw-signal matched filter is the *detector/anchor* (it finds beats at low SNR, yields per-beat SNR and jitter for the tier engine, and centers the edge-search windows); it is not the amplitude clock.
- **Rate has two sources:** Tier 1 uses M1's period-regression rate; Tier ≥ 2 switches to the unlocking-timestamp regression (spec §5.6, unlocking-referenced). `MetricsSnapshot.rate_source` says which. No cross-source consistency machinery in M2 (YAGNI; revisit if real recordings disagree).
- **Beat error comes free from the rate regression:** model `t_unlock[k] = a + T_beat·k + c·s_k` with `s_k = ±1` by parity; alternate intervals are `T_beat ∓ 2c`, so `BE = 2·|c|`. One 3-parameter least squares delivers rate, BE, and jitter (residual σ) together.
- **Octave guard lives in the fold stage** and definitively retires M1's known limitation: if the period estimate is doubled (extreme tic/toc asymmetry defeated the 40 % divisor walk), the fold at T̂ shows four evenly spaced clusters while the fold at T̂/2 shows two — detect and halve. Testable via the new `toc_gain` synth field (gain 0.1 defeats the walk: beat-lag autocorr value ≈ 2g/(1+g²) ≈ 0.198 < 0.40).
- **Fold bins:** `NBINS = 512` fixed; per-bin trimmed mean discards the top quintile of contributing cycles (impulse-noise robustness, matches spec §5.4).
- **Drop template:** 384 samples at 48 kHz (≈ 8 ms, covers the drop burst's 6τ tail), one template per parity, refreshed by trimmed-mean over the last 64 beats of that parity; located by direct time-domain cross-correlation inside a phase-predicted gate of ±T_beat/8, sub-sample via the existing parabolic refinement math. Direct correlation ≈ 1.2 M MAC/beat — no FFT, no planner.
- **Rings:** `chrona-dsp` gains a tiny `ring.rs` (`SampleRing`); the analyzer keeps a 32 s filtered-raw ring (~6.1 MB at 48 kHz) for the matched filter and a 32 s envelope ring shared by fold/events. `PeriodEstimator`'s private ring stays untouched (its 384 KB duplicate is cheaper than a refactor).
- **Clip counter** (tier-engine input, spec §5.7): counted on the raw *input* samples before filtering, `|s| ≥ 0.999`, cumulative `u64`. The AGC score input is **deferred to M4** with the Mic Doctor (recorded in the tier struct as a doc note, no stub field).
- **Quality/tier reporting:** `current()` returning `None` IS Tier 0 (nothing measurable — the caller shows setup guidance); the `Tier` enum inside a `Some` snapshot is `{T1, T2, T3}`.
- **Averaging window:** `AnalyzerConfig.averaging_s: f64` (default 30.0, valid 2–60 per spec §2.3/Weishi) bounds the event window for stage-6 metrics.

## File Structure

```
crates/chrona-dsp/src/
  lib.rs         MODIFY: add pub mod ring/fold/events/metrics/tier/cal; re-export MetricsSnapshot, Tier, Quality
  envelope.rs    MODIFY (T1): parameterized LP cutoff min(1500, 0.45·sr/16), comment fix, reserve
  period.rs      MODIFY (T2): invariant comment, capacity .max(1), Debug, doc comments
  autocorr.rs    MODIFY (T2): Peak derives, 2 new tests
  synth.rs       MODIFY (T3): toc_gain field; synthesize_quartz()
  ring.rs        NEW (T4): SampleRing — bounded ring with absolute sample indexing
  fold.rs        NEW (T4+T5): FoldProfile, two anchors, octave guard
  events.rs      NEW (T6+T7): templates, matched-filter drop detection, envelope edges, BeatEvent
  metrics.rs     NEW (T8+T9): 3-param regression (rate/BE/jitter), amplitude with gates
  tier.rs        NEW (T10): Tier, Quality, tier assignment
  analyzer.rs    MODIFY (T10): rings, clip counter, lift/averaging config, MetricsSnapshot
  cal.rs         NEW (T12): quartz calibration math
crates/chrona-cli/src/
  wav.rs         MODIFY (T3): write_mono_i16
  commands.rs    MODIFY (T3: --pcm16; T11: --lift/--averaging, extended report; T12: calibrate)
  main.rs        MODIFY (T11, T12): new args/subcommand
crates/chrona-cli/tests/
  roundtrip.rs   MODIFY (T11): extended assertions
  calibrate.rs   NEW (T12)
docs/superpowers/specs/…  MODIFY (T1): amendments
fixtures/README.md        MODIFY (T13): recording guide, extended expectation schema
README.md                 MODIFY (T13): M2 status
```

---

### Task 1: Spec amendments + envelope cutoff parameterization

**Files:**
- Modify: `docs/superpowers/specs/2026-08-20-chrona-timegrapher-design.md` (§5 items)
- Modify: `crates/chrona-dsp/src/envelope.rs`

**Interfaces:**
- Consumes: `filter::Butterworth::low_pass` (M1)
- Produces: `EnvelopeExtractor` public API unchanged (`new`, `envelope_rate_hz`, `process`, `DECIMATION`); only the internal cutoff and docs change. Every later task may assume the envelope is alias-safe at any input rate.

- [ ] **Step 1: Amend the spec** (docs change, no test)

In the spec, make exactly these edits and add a changelog line under the Status header (`- **Amended:** 2026-08-22 — §5.2 envelope LP cutoff, §5.3 divisor threshold, Hann-taper note (M1 final review; see docs/superpowers/notes/m1-followups.md)`):

1. §5 item 2 currently reads "**Envelope:** full-wave rectify → low-pass (~2–3 kHz) → decimate." Replace the parenthetical: "full-wave rectify → low-pass at `min(1.5 kHz, 0.45 · sr/16)` (must sit below the post-decimation Nyquist of `sr/32`) → decimate ×16."
2. §5 item 2's "Mean removal + Hann edge taper per analysis window" → append: "(M1 ships mean removal only; the Hann taper was reviewed as unnecessary at current accuracy margins and is deliberately omitted — revisit only if real-corpus σ regresses.)"
3. §5 item 3's "harmonic disambiguation via integer-divisor peaks (≥ 90 % of fundamental)" → "harmonic disambiguation via integer-divisor peaks (≥ 40 % of the original candidate — beat error smears the beat-period peak, so the spec's original 90 % rejected real watches; ratified in M1 review)".

- [ ] **Step 2: Write the failing test for the cutoff**

Append to the `tests` module in `crates/chrona-dsp/src/envelope.rs`:

```rust
    #[test]
    fn cutoff_stays_below_post_decimation_nyquist_at_44100() {
        // At 44.1 kHz the post-decimation Nyquist is 44100/32 = 1378 Hz; the fixed
        // 1.5 kHz cutoff sat ABOVE it (M1 known issue). The cutoff must now be
        // min(1500, 0.45·sr/16) = 1240 Hz there, and a 1360 Hz tone (below old
        // cutoff, above new) must be strongly attenuated before decimation.
        let sr = 44_100.0;
        let mut env = EnvelopeExtractor::new(sr).unwrap();
        assert!((env.envelope_rate_hz() - sr / 16.0).abs() < 1e-9);
        // Rectified DC passes; a tone near the old cutoff must not alias through.
        // Feed |sin| at 1360 Hz (post-rectification fundamental at 2720 Hz is
        // irrelevant — we probe the LP directly with a slow+fast mix).
        let n = (sr * 2.0) as usize;
        let x: Vec<f32> = (0..n)
            .map(|i| {
                let t = i as f64 / sr;
                (0.5 + 0.5 * (2.0 * std::f64::consts::PI * 1_360.0 * t).sin()) as f32
            })
            .collect();
        let mut out = Vec::new();
        env.process(&x, &mut out);
        // Remove the DC part, measure residual ripple at the tone frequency.
        let tail = &out[out.len() / 2..];
        let mean = tail.iter().map(|v| *v as f64).sum::<f64>() / tail.len() as f64;
        let rms =
            (tail.iter().map(|v| (*v as f64 - mean).powi(2)).sum::<f64>() / tail.len() as f64).sqrt();
        // Input ripple RMS is 0.5/√2 ≈ 0.354; require ≥ 20 dB attenuation.
        assert!(rms < 0.035, "ripple rms {rms}");
    }
```

- [ ] **Step 3: Run to verify it fails**

Run: `cargo test -p chrona-dsp envelope`
Expected: the new test FAILS (cutoff is the fixed 1_500.0 → only ~1–2 dB down at 1360 Hz at 44.1 k)

- [ ] **Step 4: Implement**

In `EnvelopeExtractor::new`, replace the fixed cutoff line with:

```rust
        // Alias-safe envelope LP (spec §5.2 as amended): must sit below the
        // post-decimation Nyquist sr/(2·DECIMATION). 0.45 leaves transition-band margin.
        let cutoff_hz = (0.45 * sample_rate_hz / DECIMATION as f64).min(1_500.0);
        Ok(EnvelopeExtractor {
            lp: Butterworth::low_pass(sample_rate_hz, cutoff_hz)?,
            sample_rate_hz,
            phase: 0,
        })
```

Also in `process`, add `out.reserve(samples.len() / DECIMATION + 1);` as the first line (M1 followup), and fix the old comment: the 48 kHz post-decimation *rate* is 3 kHz and its *Nyquist* is 1.5 kHz.

- [ ] **Step 5: Run tests to verify all pass**

Run: `cargo test -p chrona-dsp envelope`
Expected: all envelope tests PASS (the 48 kHz tests still pass: min(1500, 0.45·3000)=1350 Hz — verify `low_pass_1k5_passes_low_blocks_high` in filter.rs is untouched; the envelope's own tests don't assert the exact cutoff)

Note: if `envelope_peaks_once_per_beat` fails after the cutoff change (1500 → 1350 Hz at 48 k slightly slows edges), the tolerance windows there are ±10 ms — a 0.1 ms edge shift cannot fail them; treat any failure as a real bug, not tolerance drift.

- [ ] **Step 6: Full gates and commit**

Run: `cargo fmt --all --check && cargo clippy --all-targets --workspace -- -D warnings && cargo test --workspace`

```bash
git add docs/superpowers/specs crates/chrona-dsp/src/envelope.rs
git commit -m "docs+fix(dsp): amend spec per M1 review; alias-safe envelope cutoff"
```

---

### Task 2: M1 debt sweep (mechanical batch)

**Files:**
- Modify: `crates/chrona-dsp/src/period.rs`
- Modify: `crates/chrona-dsp/src/autocorr.rs`

**Interfaces:**
- Consumes: existing M1 APIs
- Produces: `Peak` now derives `Debug, Clone, Copy` (Tasks 4–7 rely on copying `Peak`); `PeriodEstimator` derives `Debug`; no signature changes.

This is a batched mechanical task — several small, same-shape edits reviewed as one unit.

- [ ] **Step 1: period.rs edits**

1. `PeriodEstimator::new`: `let capacity = ((WINDOWS_S[3] * envelope_rate_hz) as usize).max(1);` (M1 followup: capacity 0 → unbounded ring).
2. Add `#[derive(Debug)]` on `pub struct PeriodEstimator`.
3. Doc comments: on `new` — "`envelope_rate_hz` is the DECIMATED envelope rate (`sample_rate/16`), not the audio rate."; on `estimate` — "Returns `None` both when there is not yet enough data (< 4 s of envelope) and when nothing passes the σ gate — callers must not distinguish the two."
4. Where the per-cycle tolerance is set (the `0.005` in `estimate_window`), add the invariant comment: `// invariant: tolerance · K_max < 0.5 (0.005·64 = 0.32), else the search window reaches the neighboring beat peak at k ≥ 0.5/tolerance and the fit re-acquires the >1 % bias fixed in M1.`

- [ ] **Step 2: autocorr.rs edits + two tests**

1. Add `#[derive(Debug, Clone, Copy)]` on `pub struct Peak`.
2. Append two tests to the `tests` module:

```rust
    #[test]
    fn parabolic_clamp_engages_on_pathological_shape() {
        // A flat-topped two-sample plateau makes the parabola degenerate; the
        // refined lag must stay within ±0.5 of the integer peak (clamp engaged
        // or denominator guard returns 0 offset) — never fly off.
        let mut r = vec![0.0f32; 64];
        r[0] = 1.0;
        r[30] = 0.5;
        r[31] = 0.5; // plateau
        let p = find_peak_in_band(&r, 10, 60).unwrap();
        assert!((p.lag - 30.0).abs() <= 0.5, "lag {}", p.lag);
    }

    #[test]
    fn non_power_of_two_length_matches_naive() {
        let mut rng = crate::synth::Rng::new(11);
        let x: Vec<f32> = (0..1000).map(|_| rng.next_f32()).collect(); // pads 2000→2048
        let fast = autocorrelate(&x);
        let slow = naive_autocorr(&x);
        for (i, (a, b)) in fast.iter().zip(slow.iter()).enumerate() {
            assert!((a - b).abs() < 1e-4, "lag {i}: {a} vs {b}");
        }
    }
```

(`naive_autocorr` already exists in this test module from M1.)

- [ ] **Step 3: period.rs test hardening**

In `period.rs` tests, add to `beat_error_does_not_bias_the_period` (M1 followup): after the existing rel-err assert, `assert!(est.window_s >= 16.0, "window {}", est.window_s);`

- [ ] **Step 4: Run and verify**

Run: `cargo test -p chrona-dsp`
Expected: all tests PASS (the new clamp test exercises `parabolic_offset` through the public API; if the plateau picks index 31 instead of 30, both satisfy the assertion by design)

- [ ] **Step 5: Full gates and commit**

Run: `cargo fmt --all --check && cargo clippy --all-targets --workspace -- -D warnings && cargo test --workspace`

```bash
git add crates/chrona-dsp
git commit -m "chore(dsp): M1 review debt — derives, capacity guard, invariant docs, coverage"
```

---

### Task 3: Synth extensions — tic/toc asymmetry, quartz reference, 16-bit PCM output

**Files:**
- Modify: `crates/chrona-dsp/src/synth.rs`
- Modify: `crates/chrona-cli/src/wav.rs`, `crates/chrona-cli/src/commands.rs`
- Modify: `crates/chrona-cli/tests/roundtrip.rs`

**Interfaces:**
- Consumes: existing `SynthConfig`/`synthesize`/`SynthError`; `wav::read_mono`
- Produces (used by Tasks 5, 9, 10, 12):
  - `SynthConfig` gains `pub toc_gain: f64` (default 1.0) — scales ALL three burst amplitudes on odd-index (toc) beats
  - `pub fn synthesize_quartz(duration_s: f64, sample_rate_hz: f64, ppm_offset: f64, snr_db: f64, seed: u64) -> Result<Vec<f32>, SynthError>` — 1 Hz stepper tick whose true period is `1.0·(1 + ppm_offset/10⁶)` nominal-clock seconds
  - `pub fn write_mono_i16(path: &Path, samples: &[f32], sample_rate_hz: f64) -> anyhow::Result<()>` in `wav.rs`
  - `chrona synth --pcm16` flag

- [ ] **Step 1: Write the failing dsp tests**

Append to `synth.rs` tests:

```rust
    #[test]
    fn toc_gain_scales_alternate_beats() {
        let strong = SynthConfig { duration_s: 4.0, snr_db: 50.0, ..SynthConfig::default() };
        let weak = SynthConfig { toc_gain: 0.1, ..strong };
        let (xs, xw) = (synthesize(&strong).unwrap(), synthesize(&weak).unwrap());
        let sr = strong.sample_rate_hz;
        let t_beat = crate::bph::t_beat_s(strong.bph);
        // Peak amplitude around beat 4 (k=3, toc: k%2==1 with drops at (k+1)·t_beat
        // ⇒ beat index k=3 → drop at 4·t_beat) must shrink ~10x; beat 5 (tic) must not.
        let peak_near = |x: &[f32], t: f64| {
            let (lo, hi) = (((t - 0.01) * sr) as usize, ((t + 0.01) * sr) as usize);
            x[lo..hi].iter().fold(0.0f32, |m, s| m.max(s.abs()))
        };
        let toc_t = 4.0 * t_beat;
        let tic_t = 5.0 * t_beat;
        assert!(peak_near(&xw, toc_t) < 0.25 * peak_near(&xs, toc_t), "toc not attenuated");
        assert!(peak_near(&xw, tic_t) > 0.8 * peak_near(&xs, tic_t), "tic wrongly attenuated");
    }

    #[test]
    fn toc_gain_must_be_finite_and_positive() {
        let bad = SynthConfig { toc_gain: f64::NAN, ..SynthConfig::default() };
        assert!(synthesize(&bad).is_err());
        let bad2 = SynthConfig { toc_gain: 0.0, ..SynthConfig::default() };
        assert!(synthesize(&bad2).is_err());
    }

    #[test]
    fn quartz_ticks_land_on_the_ppm_stretched_grid() {
        let x = synthesize_quartz(20.0, 48_000.0, 100.0, 40.0, 3).unwrap();
        let sr = 48_000.0;
        let period = 1.0 * (1.0 + 100.0 / 1e6);
        for k in 1..=18u32 {
            let t = k as f64 * period;
            let (lo, hi) = (((t - 0.02) * sr) as usize, ((t + 0.02) * sr) as usize);
            let (idx, v) = x[lo..hi]
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
                .unwrap();
            assert!(v.abs() > 0.05, "tick {k} missing");
            let t_found = (lo + idx) as f64 / sr;
            assert!((t_found - t).abs() < 5e-3, "tick {k} at {t_found} want {t}");
        }
        assert!(synthesize_quartz(f64::NAN, 48_000.0, 0.0, 30.0, 1).is_err());
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p chrona-dsp synth`
Expected: FAIL — `toc_gain` field / `synthesize_quartz` unknown

- [ ] **Step 3: Implement in synth.rs**

1. Add `pub toc_gain: f64,` to `SynthConfig` (doc: "Gain applied to every burst of odd-index (toc) beats; 1.0 = symmetric. Values ≪ 1 model extreme tic/toc asymmetry (octave-guard testing).") and `toc_gain: 1.0,` to `Default`.
2. In the finiteness validation array add `("toc_gain", cfg.toc_gain)`; in the range checks add: `if cfg.toc_gain <= 0.0 { return Err(SynthError::InvalidConfig { reason: format!("toc_gain {} must be > 0", cfg.toc_gain) }); }`.
3. In the beat loop, before the three `add_burst` calls: `let g = if k % 2 == 0 { 1.0 } else { cfg.toc_gain };` and multiply each burst's amplitude by `g` (`tick_amp * 0.35 * g`, `tick_amp * 0.25 * g`, `tick_amp * g`).
4. Add:

```rust
/// 1 Hz quartz stepper reference for timebase calibration (spec §3.4).
/// The tick grid's true period is `1.0 · (1 + ppm_offset/1e6)` in nominal-clock
/// seconds — i.e. what a perfect quartz looks like through an ADC that is
/// `ppm_offset` ppm fast.
pub fn synthesize_quartz(
    duration_s: f64,
    sample_rate_hz: f64,
    ppm_offset: f64,
    snr_db: f64,
    seed: u64,
) -> Result<Vec<f32>, SynthError> {
    for (name, v) in [
        ("duration_s", duration_s),
        ("sample_rate_hz", sample_rate_hz),
        ("ppm_offset", ppm_offset),
        ("snr_db", snr_db),
    ] {
        if !v.is_finite() {
            return Err(SynthError::InvalidConfig { reason: format!("{name} is not finite") });
        }
    }
    if sample_rate_hz <= 0.0 || duration_s < 0.0 {
        return Err(SynthError::InvalidConfig {
            reason: "sample_rate_hz must be > 0 and duration_s >= 0".into(),
        });
    }
    let sr = sample_rate_hz;
    let n = (duration_s * sr) as usize;
    let mut x = vec![0.0f32; n];
    let mut rng = Rng::new(seed);
    let noise_rms = 0.02f64;
    let tick_amp = noise_rms * 10f64.powf(snr_db / 20.0);
    let period = 1.0 * (1.0 + ppm_offset / 1e6);
    let mut k = 1u64;
    loop {
        let t = k as f64 * period;
        if t + 0.02 >= duration_s {
            break;
        }
        add_burst(&mut x, sr, t, tick_amp, 4_000.0, 0.0015);
        k += 1;
    }
    let noise_gain = noise_rms * 3f64.sqrt();
    for s in x.iter_mut() {
        *s += (noise_gain * rng.next_f32() as f64) as f32;
    }
    let peak = x.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    if peak > 0.9 {
        let g = 0.9 / peak;
        for s in x.iter_mut() {
            *s *= g;
        }
    }
    Ok(x)
}
```

- [ ] **Step 4: Run dsp tests to verify they pass**

Run: `cargo test -p chrona-dsp synth` — all PASS (including the pre-existing ones; the added struct field compiles everywhere because all construction sites use `..SynthConfig::default()` or full `Default`)

- [ ] **Step 5: CLI — 16-bit PCM output + int-read coverage**

In `wav.rs` add:

```rust
pub fn write_mono_i16(path: &Path, samples: &[f32], sample_rate_hz: f64) -> anyhow::Result<()> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: sample_rate_hz as u32,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w =
        hound::WavWriter::create(path, spec).with_context(|| format!("create {}", path.display()))?;
    for &s in samples {
        w.write_sample((s.clamp(-1.0, 1.0) * 32_767.0) as i16)?;
    }
    w.finalize()?;
    Ok(())
}
```

In `commands.rs` `SynthArgs` add: `/// Write 16-bit PCM instead of 32-bit float` + `#[arg(long)] pub pcm16: bool,` and in `run_synth` route: `if a.pcm16 { crate::wav::write_mono_i16(&a.out, &samples, cfg.sample_rate_hz) } else { crate::wav::write_mono_f32(&a.out, &samples, cfg.sample_rate_hz) }`.

Append to `tests/roundtrip.rs` (this closes the M1 "int-PCM read path untested" gap):

```rust
#[test]
fn pcm16_synth_roundtrips_through_the_int_read_path() {
    let dir = tempfile::tempdir().unwrap();
    let wav = dir.path().join("t16.wav");
    chrona()
        .args(["synth", wav.to_str().unwrap(), "--rate", "10.0", "--pcm16"])
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
    let rate = v["rate_s_per_day"].as_f64().unwrap();
    assert!((rate - 10.0).abs() < 0.5, "rate {rate}");
}
```

- [ ] **Step 6: Full gates and commit**

Run: `cargo fmt --all --check && cargo clippy --all-targets --workspace -- -D warnings && cargo test --workspace`

```bash
git add crates
git commit -m "feat(dsp+cli): toc-gain asymmetry, quartz reference synth, 16-bit PCM output"
```

---

### Task 4: SampleRing + phase fold with tic/toc anchors (`ring.rs`, `fold.rs`)

**Files:**
- Create: `crates/chrona-dsp/src/ring.rs`, `crates/chrona-dsp/src/fold.rs`
- Modify: `crates/chrona-dsp/src/lib.rs` (add `pub mod ring;` `pub mod fold;`)

**Interfaces:**
- Consumes: `synth`/`envelope` (tests only)
- Produces (used by Tasks 5–10):
  - `ring::SampleRing`: `pub fn new(capacity: usize) -> Self` (capacity clamped ≥ 1), `pub fn push_slice(&mut self, s: &[f32])`, `pub fn len(&self) -> usize`, `pub fn is_empty(&self) -> bool`, `pub fn total_pushed(&self) -> u64`, `pub fn start_index(&self) -> u64` (absolute index of the oldest retained sample), `pub fn copy_last(&self, n: usize, out: &mut Vec<f32>)` (clears `out`, copies the newest `min(n, len)` samples oldest-first), `pub fn copy_range_abs(&self, start_abs: u64, len: usize, out: &mut Vec<f32>) -> bool` (false — and `out` cleared — if any requested sample is evicted or not yet pushed)
  - `fold::NBINS: usize = 512`
  - `fold::FoldProfile { pub bins: Vec<f32>, pub t_osc_env: f64, pub anchor_a_phase: f64, pub anchor_b_phase: f64, pub contrast: f32 }` (phases in bin units, [0, NBINS); `contrast` = anchor-A bin value over profile median, quality signal)
  - `fold::fold_envelope(env: &[f32], t_osc_env: f64) -> Option<FoldProfile>` — `None` when fewer than 8 full cycles fit or `t_osc_env` is not finite/positive
  - `impl FoldProfile { pub fn anchor_env_offsets(&self) -> (f64, f64) }` — anchor phases converted to envelope-sample offsets within one cycle (`phase/NBINS · t_osc_env`)

- [ ] **Step 1: Write the failing ring tests**

`crates/chrona-dsp/src/ring.rs`:

```rust
//! Bounded sample ring with absolute (since-start) indexing, shared by the
//! fold and event stages (spec §5.4–§5.5).

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eviction_and_absolute_indexing() {
        let mut r = SampleRing::new(4);
        r.push_slice(&[1.0, 2.0, 3.0]);
        assert_eq!((r.len(), r.total_pushed(), r.start_index()), (3, 3, 0));
        r.push_slice(&[4.0, 5.0]); // evicts 1.0
        assert_eq!((r.len(), r.total_pushed(), r.start_index()), (4, 5, 1));
        let mut out = Vec::new();
        r.copy_last(2, &mut out);
        assert_eq!(out, vec![4.0, 5.0]);
        r.copy_last(10, &mut out); // clamped to len
        assert_eq!(out, vec![2.0, 3.0, 4.0, 5.0]);
    }

    #[test]
    fn copy_range_abs_detects_eviction_and_future() {
        let mut r = SampleRing::new(4);
        r.push_slice(&[0.0, 1.0, 2.0, 3.0, 4.0, 5.0]); // retains abs 2..=5
        let mut out = Vec::new();
        assert!(r.copy_range_abs(2, 3, &mut out));
        assert_eq!(out, vec![2.0, 3.0, 4.0]);
        assert!(!r.copy_range_abs(1, 3, &mut out), "evicted start must fail");
        assert!(!r.copy_range_abs(4, 3, &mut out), "reaching past newest must fail");
        assert!(out.is_empty());
    }

    #[test]
    fn zero_capacity_is_clamped() {
        let mut r = SampleRing::new(0);
        r.push_slice(&[7.0]);
        assert_eq!(r.len(), 1);
    }
}
```

- [ ] **Step 2: Implement SampleRing**

Above the tests in `ring.rs`:

```rust
use std::collections::VecDeque;

pub struct SampleRing {
    buf: VecDeque<f32>,
    capacity: usize,
    total: u64,
}

impl SampleRing {
    pub fn new(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        SampleRing { buf: VecDeque::with_capacity(capacity), capacity, total: 0 }
    }

    pub fn push_slice(&mut self, s: &[f32]) {
        for &v in s {
            if self.buf.len() == self.capacity {
                self.buf.pop_front();
            }
            self.buf.push_back(v);
        }
        self.total += s.len() as u64;
    }

    pub fn len(&self) -> usize {
        self.buf.len()
    }
    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }
    /// Samples ever pushed; the newest retained sample has absolute index `total_pushed() - 1`.
    pub fn total_pushed(&self) -> u64 {
        self.total
    }
    /// Absolute index of the oldest retained sample.
    pub fn start_index(&self) -> u64 {
        self.total - self.buf.len() as u64
    }

    /// Copy the newest `n` samples (clamped to `len`) into `out`, oldest-first.
    pub fn copy_last(&self, n: usize, out: &mut Vec<f32>) {
        out.clear();
        let n = n.min(self.buf.len());
        out.extend(self.buf.iter().skip(self.buf.len() - n));
    }

    /// Copy `len` samples starting at absolute index `start_abs`. Returns false
    /// (with `out` cleared) if the range touches evicted or not-yet-pushed samples.
    pub fn copy_range_abs(&self, start_abs: u64, len: usize, out: &mut Vec<f32>) -> bool {
        out.clear();
        let end_abs = start_abs + len as u64;
        if start_abs < self.start_index() || end_abs > self.total {
            return false;
        }
        let offset = (start_abs - self.start_index()) as usize;
        out.extend(self.buf.iter().skip(offset).take(len));
        true
    }
}
```

Run: `cargo test -p chrona-dsp ring` — 3 tests PASS (write tests first, watch them fail to compile, then implement — the RED state is the missing type).

- [ ] **Step 3: Write the failing fold tests**

`crates/chrona-dsp/src/fold.rs`:

```rust
//! Stage 4 (spec §5.4): fold the envelope at the oscillation period with
//! trimmed-mean stacking; the two folded maxima are the tic and toc
//! drop-pulse anchors.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::EnvelopeExtractor;
    use crate::synth::{synthesize, SynthConfig};

    fn envelope_of(cfg: &SynthConfig) -> (Vec<f32>, f64) {
        let x = synthesize(cfg).expect("valid synth config");
        let mut env = EnvelopeExtractor::new(cfg.sample_rate_hz).unwrap();
        let mut out = Vec::new();
        env.process(&x, &mut out);
        (out, env.envelope_rate_hz())
    }

    fn circular_dist(a: f64, b: f64, n: f64) -> f64 {
        let d = (a - b).rem_euclid(n);
        d.min(n - d)
    }

    #[test]
    fn anchors_sit_half_a_cycle_apart() {
        let cfg = SynthConfig { snr_db: 30.0, ..SynthConfig::default() };
        let (env, env_rate) = envelope_of(&cfg);
        let t_osc_env = crate::bph::t_osc_s(cfg.bph) * env_rate;
        let p = fold_envelope(&env, t_osc_env).expect("enough cycles");
        let sep = circular_dist(p.anchor_a_phase, p.anchor_b_phase, NBINS as f64);
        assert!((sep - NBINS as f64 / 2.0).abs() < 8.0, "separation {sep}");
        assert!(p.contrast > 3.0, "contrast {}", p.contrast);
    }

    #[test]
    fn beat_error_shifts_but_does_not_lose_anchors() {
        let cfg = SynthConfig { beat_error_ms: 2.0, snr_db: 30.0, ..SynthConfig::default() };
        let (env, env_rate) = envelope_of(&cfg);
        let t_osc_env = crate::bph::t_osc_s(cfg.bph) * env_rate;
        let p = fold_envelope(&env, t_osc_env).expect("enough cycles");
        // ±BE/2 shifts move the anchors ±(be/2)/T_osc·NBINS ≈ ±2.0 bins for 2 ms.
        let sep = circular_dist(p.anchor_a_phase, p.anchor_b_phase, NBINS as f64);
        let expected_shift = 2.0e-3 / crate::bph::t_osc_s(cfg.bph) * NBINS as f64;
        assert!(
            (sep - NBINS as f64 / 2.0).abs() < expected_shift + 8.0,
            "separation {sep} (allowed shift {expected_shift})"
        );
    }

    #[test]
    fn noise_yields_low_contrast() {
        let mut rng = crate::synth::Rng::new(9);
        let env: Vec<f32> = (0..(3_000.0 * 12.0) as usize).map(|_| 0.02 * rng.next_f32().abs()).collect();
        let p = fold_envelope(&env, 750.0).expect("cycles fit");
        assert!(p.contrast < 2.0, "contrast {}", p.contrast);
    }

    #[test]
    fn too_few_cycles_or_bad_period_is_none() {
        let env = vec![0.0f32; 1000];
        assert!(fold_envelope(&env, 750.0).is_none()); // 1.3 cycles < 8
        assert!(fold_envelope(&env, f64::NAN).is_none());
        assert!(fold_envelope(&env, 0.0).is_none());
    }
}
```

- [ ] **Step 4: Run to verify they fail, then implement**

Run: `cargo test -p chrona-dsp fold` — FAIL (module missing). Then implement above the tests:

```rust
/// Number of phase bins in a fold profile.
pub const NBINS: usize = 512;

#[derive(Debug, Clone)]
pub struct FoldProfile {
    /// Trimmed-mean envelope value per phase bin.
    pub bins: Vec<f32>,
    /// The folding period, in envelope samples.
    pub t_osc_env: f64,
    /// Anchor phases in bin units, [0, NBINS). A is the global max; B the
    /// opposite-half max (the other beat's drop pulse).
    pub anchor_a_phase: f64,
    pub anchor_b_phase: f64,
    /// Anchor-A bin value over the profile median — a fold-quality signal.
    pub contrast: f32,
}

impl FoldProfile {
    /// Anchor phases as envelope-sample offsets within one cycle.
    pub fn anchor_env_offsets(&self) -> (f64, f64) {
        let scale = self.t_osc_env / NBINS as f64;
        (self.anchor_a_phase * scale, self.anchor_b_phase * scale)
    }
}

/// Parabolic refinement on a circular bin array around index `i`.
fn circular_parabolic(bins: &[f32], i: usize) -> f64 {
    let n = bins.len();
    let (a, b, c) =
        (bins[(i + n - 1) % n] as f64, bins[i] as f64, bins[(i + 1) % n] as f64);
    let denom = a - 2.0 * b + c;
    let off = if denom.abs() < 1e-20 { 0.0 } else { (0.5 * (a - c) / denom).clamp(-0.5, 0.5) };
    (i as f64 + off).rem_euclid(n as f64)
}

/// Fold `env` at `t_osc_env` with per-bin trimmed-mean stacking (spec §5.4:
/// drop the top quintile of contributing cycles per bin).
pub fn fold_envelope(env: &[f32], t_osc_env: f64) -> Option<FoldProfile> {
    if !(t_osc_env.is_finite() && t_osc_env > 1.0) {
        return None;
    }
    let cycles = (env.len() as f64 / t_osc_env).floor() as usize;
    if cycles < 8 {
        return None;
    }
    let usable = (cycles as f64 * t_osc_env) as usize;
    // Bucket every sample of the freshest `usable` window by phase.
    let start = env.len() - usable;
    let mut buckets: Vec<Vec<f32>> = vec![Vec::with_capacity(cycles + 1); NBINS];
    for (j, &v) in env[start..].iter().enumerate() {
        let phase = (j as f64).rem_euclid(t_osc_env) / t_osc_env;
        let bin = ((phase * NBINS as f64) as usize).min(NBINS - 1);
        buckets[bin].push(v);
    }
    let mut bins = vec![0.0f32; NBINS];
    for (bin, bucket) in bins.iter_mut().zip(buckets.iter_mut()) {
        if bucket.is_empty() {
            continue;
        }
        bucket.sort_by(f32::total_cmp);
        // Trimmed mean: drop the top quintile (impulse-noise robustness).
        let keep = (bucket.len() * 4).div_ceil(5).max(1);
        *bin = bucket[..keep].iter().sum::<f32>() / keep as f32;
    }

    // Anchor A: global max, parabolic-refined on the circle.
    let a_idx = bins
        .iter()
        .enumerate()
        .max_by(|x, y| x.1.total_cmp(y.1))
        .map(|(i, _)| i)?;
    let anchor_a_phase = circular_parabolic(&bins, a_idx);
    // Anchor B: max within the circular window centered half a cycle away,
    // width NBINS/4 (tolerates beat-error shifts up to ±T_osc/8).
    let center = (a_idx + NBINS / 2) % NBINS;
    let half_w = NBINS / 8;
    let b_idx = (0..=2 * half_w)
        .map(|k| (center + NBINS - half_w + k) % NBINS)
        .max_by(|&i, &j| bins[i].total_cmp(&bins[j]))?;
    let anchor_b_phase = circular_parabolic(&bins, b_idx);

    let mut sorted = bins.clone();
    sorted.sort_by(f32::total_cmp);
    let median = sorted[NBINS / 2].max(1e-12);
    let contrast = bins[a_idx] / median;

    Some(FoldProfile { bins, t_osc_env, anchor_a_phase, anchor_b_phase, contrast })
}
```

Add `pub mod ring;` and `pub mod fold;` to `lib.rs`.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p chrona-dsp` — all PASS. If `noise_yields_low_contrast` is marginal (contrast near 2.0), the cause is usually rectified-noise median near zero — verify the median floor `1e-12` and that the noise test uses `.abs()` (envelope-like nonnegative input) as written.

- [ ] **Step 6: Full gates and commit**

```bash
git add crates/chrona-dsp
git commit -m "feat(dsp): sample ring and phase-fold stage with tic/toc anchors"
```

---

### Task 5: Octave guard (`fold.rs` extension)

**Files:**
- Modify: `crates/chrona-dsp/src/fold.rs`

**Interfaces:**
- Consumes: `fold_envelope`, `FoldProfile` (Task 4); `SynthConfig.toc_gain` (Task 3)
- Produces (used by Task 10):
  - `pub fn fold_with_octave_guard(env: &[f32], t_osc_env: f64) -> Option<(FoldProfile, bool)>` — folds at `t_osc_env`; if the profile shows the four-evenly-spaced-cluster signature of a DOUBLED period and folding at `t_osc_env/2` yields an equal-or-better two-anchor profile, returns the half-period fold with `true` (period was halved). Otherwise the plain fold with `false`. This retires M1's known octave-error limitation (spec §5.3's phase-fold resolution).

- [ ] **Step 1: Write the failing tests**

Append to `fold.rs` tests:

```rust
    #[test]
    fn octave_guard_halves_a_doubled_period() {
        // toc_gain 0.1 defeats the 40% divisor walk upstream (beat-lag autocorr
        // value ≈ 2g/(1+g²) ≈ 0.198 < 0.40), so the period stage would deliver
        // 2·T_osc. The guard must recognize the 4-cluster fold and halve it.
        let cfg = SynthConfig { toc_gain: 0.1, snr_db: 35.0, ..SynthConfig::default() };
        let (env, env_rate) = envelope_of(&cfg);
        let t_osc_true = crate::bph::t_osc_s(cfg.bph) * env_rate;
        let (p, halved) = fold_with_octave_guard(&env, 2.0 * t_osc_true).expect("fold");
        assert!(halved, "guard must detect the doubled period");
        assert!((p.t_osc_env - t_osc_true).abs() / t_osc_true < 0.01, "t_osc {}", p.t_osc_env);
    }

    #[test]
    fn octave_guard_leaves_a_correct_period_alone() {
        for cfg in [
            SynthConfig { snr_db: 30.0, ..SynthConfig::default() },
            SynthConfig { beat_error_ms: 2.0, snr_db: 30.0, ..SynthConfig::default() },
            SynthConfig { toc_gain: 0.5, snr_db: 30.0, ..SynthConfig::default() },
        ] {
            let (env, env_rate) = envelope_of(&cfg);
            let t_osc_env = crate::bph::t_osc_s(cfg.bph) * env_rate;
            let (p, halved) = fold_with_octave_guard(&env, t_osc_env).expect("fold");
            assert!(!halved, "false halving (be={} toc={})", cfg.beat_error_ms, cfg.toc_gain);
            assert!((p.t_osc_env - t_osc_env).abs() < 1e-9);
        }
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p chrona-dsp fold` — FAIL (`fold_with_octave_guard` unknown)

- [ ] **Step 3: Implement**

Append to `fold.rs` (above tests):

```rust
/// Count clusters of bins above `median + 0.5·(max − median)`, treating the
/// bin array as circular; returns cluster count and the circular gaps (in bins)
/// between consecutive cluster centroids.
fn significant_clusters(bins: &[f32]) -> (usize, Vec<f64>) {
    let n = bins.len();
    let mut sorted = bins.to_vec();
    sorted.sort_by(f32::total_cmp);
    let median = sorted[n / 2];
    let max = sorted[n - 1];
    let thr = median + 0.5 * (max - median);
    let above: Vec<bool> = bins.iter().map(|&v| v > thr).collect();
    if above.iter().all(|&b| b) || above.iter().all(|&b| !b) {
        return (0, Vec::new());
    }
    // Walk the circle once, starting just after a below-threshold bin.
    let start = (0..n).find(|&i| !above[i]).unwrap_or(0);
    let mut centroids = Vec::new();
    let mut run: Vec<usize> = Vec::new();
    for k in 1..=n {
        let i = (start + k) % n;
        if above[i] {
            run.push(k); // unwrapped position to keep centroids monotonic
        } else if !run.is_empty() {
            let c = run.iter().sum::<usize>() as f64 / run.len() as f64;
            centroids.push(c);
            run.clear();
        }
    }
    let count = centroids.len();
    let mut gaps = Vec::new();
    if count >= 2 {
        for w in centroids.windows(2) {
            gaps.push(w[1] - w[0]);
        }
        gaps.push(n as f64 - (centroids[count - 1] - centroids[0])); // wrap gap
    }
    (count, gaps)
}

/// Fold with period-doubling detection (retires M1's octave-error limitation).
pub fn fold_with_octave_guard(env: &[f32], t_osc_env: f64) -> Option<(FoldProfile, bool)> {
    let full = fold_envelope(env, t_osc_env)?;
    let (count, gaps) = significant_clusters(&full.bins);
    let quarter = NBINS as f64 / 4.0;
    let four_even = count == 4
        && gaps
            .iter()
            .all(|g| (g - quarter).abs() < quarter / 4.0);
    if four_even {
        if let Some(half) = fold_envelope(env, t_osc_env / 2.0) {
            if half.contrast >= full.contrast {
                return Some((half, true));
            }
        }
    }
    Some((full, false))
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p chrona-dsp fold`
Expected: all PASS. Debug guidance if `octave_guard_halves_a_doubled_period` fails: dump `significant_clusters(&full.bins)` — at toc_gain 0.1 the two weak (toc) clusters may fall below the 50 %-of-range threshold; if count == 2 instead of 4, lower the cluster threshold factor from 0.5 to 0.35 — but then re-run the leave-alone test (be = 2.0) to confirm no false positives; both tests green is the contract, the threshold factor is tunable.

- [ ] **Step 5: Full gates and commit**

```bash
git add crates/chrona-dsp
git commit -m "feat(dsp): octave guard — fold-based period-doubling detection and correction"
```

---

### Task 6: Matched-filter drop detection (`events.rs`, part 1)

**Files:**
- Create: `crates/chrona-dsp/src/events.rs`
- Modify: `crates/chrona-dsp/src/lib.rs` (add `pub mod events;`)

**Interfaces:**
- Consumes: `fold::FoldProfile` (Task 4), `synth`/`envelope` (tests)
- Produces (used by Tasks 7–10):
  - `pub enum Parity { Tic, Toc }` (derives `Debug, Clone, Copy, PartialEq, Eq`) — Tic = fold anchor A, Toc = anchor B
  - `pub struct BeatEvent { pub beat_index: i64, pub parity: Parity, pub t_drop_corr_s: f64, pub snr_db: f32, pub t_drop_edge_s: Option<f64>, pub t_unlock_s: Option<f64> }` (derives `Debug, Clone, Copy`) — all times are absolute nominal-clock seconds since stream start; Task 6 fills the first four fields and leaves both edges `None`; Task 7 fills the edges
  - `pub const DETECT_SNR_DB: f32 = 6.0` — correlation-peak SNR below which a beat is not emitted
  - `pub fn extract_events(raw: &[f32], raw_start_abs: u64, env: &[f32], env_start_abs: u64, sample_rate_hz: f64, profile: &FoldProfile) -> Vec<BeatEvent>` — stateless; templates are rebuilt from the given window on every call (spec §5.5 "refreshed continuously"). **Alignment invariant (document in the fn docs):** `raw` and `env` come from rings fed in lockstep from stream start, so envelope absolute index `j` corresponds to raw absolute index `j · envelope::DECIMATION`; callers pass `env_start_abs` in envelope-domain units and `raw_start_abs` in raw units. **Fold-window invariant:** the fold in Task 4 starts at `env.len() − (cycles·t_osc_env) as usize` computed from the SAME env slice length; `extract_events` recomputes that offset with the identical two lines (kept in sync by this comment on both sites).

**Algorithm (spec §5.5, drop half):**
1. Recompute `cycles` and `fold_start` exactly as `fold_envelope` did. Predicted drop position (env domain, relative to slice start) for cycle `c`, parity P: `fold_start + c·t_osc_env + off_P` where `(off_a, off_b) = profile.anchor_env_offsets()`.
2. **Template per parity** (raw domain, `TEMPLATE_LEN = 384`, peak anchored at index 96): for each cycle, convert the predicted env position to a raw index, search ±`gate` (`gate = t_osc_env/16 · DECIMATION` raw samples, i.e. T_beat/8) for the max-|x| sample, take the 384-sample window starting 96 before it. Drop windows whose peak |x| is in the top or bottom quintile of all windows (outlier and missed-beat rejection), per-position mean the rest, then normalize the template to unit energy (`Σ template² = 1`). Require ≥ 6 surviving windows per parity, else that parity emits no events this call.
3. **Correlation:** for each cycle/parity, slide the template across `[predicted − gate, predicted + gate]` (direct time-domain dot products), find the peak, parabolic-refine (reuse the same three-point formula as autocorr — reimplement locally, 6 lines), convert to absolute seconds: `t = (raw_start_abs + peak_index + 96) / sample_rate_hz` (`+96` re-centers on the burst peak the template anchors at).
4. **SNR:** `20·log10(peak / rms_off_peak)` where `rms_off_peak` is the correlation RMS excluding ±`2 ms · sample_rate_hz` around the peak. Emit the event only when `snr_db ≥ DETECT_SNR_DB`.
5. `beat_index`: `2c` for the parity whose anchor offset is smaller, `2c + 1` for the other (events sorted by time ascending in the returned Vec).

- [ ] **Step 1: Write the failing tests**

`crates/chrona-dsp/src/events.rs` (tests module; the shared `pipeline_to_events` helper is reused by Task 7's tests):

```rust
//! Stage 5 (spec §5.5): learned matched-filter drop detection on the
//! band-passed signal, plus envelope-edge extraction (Task 7).

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::{EnvelopeExtractor, DECIMATION};
    use crate::filter::{Butterworth, DcBlocker};
    use crate::fold::fold_envelope;
    use crate::synth::{synthesize, SynthConfig};

    /// synth → precondition → envelope → fold → extract_events, one shot.
    fn pipeline_to_events(cfg: &SynthConfig) -> (Vec<BeatEvent>, f64 /*t_beat true*/) {
        let x = synthesize(cfg).expect("valid synth config");
        let sr = cfg.sample_rate_hz;
        let mut dc = DcBlocker::new();
        let mut hp = Butterworth::high_pass(sr, 3_000.0).unwrap();
        let raw: Vec<f32> = x.iter().map(|&s| hp.process(dc.process(s))).collect();
        let mut envx = EnvelopeExtractor::new(sr).unwrap();
        let mut env = Vec::new();
        envx.process(&raw, &mut env);
        let t_osc_env = crate::bph::t_osc_s(cfg.bph) * (1.0 - cfg.rate_s_per_day / 86_400.0)
            * envx.envelope_rate_hz();
        let profile = fold_envelope(&env, t_osc_env).expect("fold");
        let events = extract_events(&raw, 0, &env, 0, sr, &profile);
        let t_beat = crate::bph::t_beat_s(cfg.bph) * (1.0 - cfg.rate_s_per_day / 86_400.0);
        (events, t_beat)
    }

    #[test]
    fn clean_signal_detects_nearly_every_beat_with_low_jitter() {
        let cfg = SynthConfig { snr_db: 30.0, ..SynthConfig::default() };
        let (events, t_beat) = pipeline_to_events(&cfg);
        let expected = (cfg.duration_s / t_beat) as usize;
        assert!(
            events.len() as f64 >= 0.85 * expected as f64,
            "{} of ~{expected} beats",
            events.len()
        );
        // Ground truth: drops at (k+1)·t_beat. Detection has a constant bias
        // (template anchor vs burst onset) — assert small bias and tiny jitter.
        let errs: Vec<f64> = events
            .iter()
            .map(|e| {
                let k = (e.t_drop_corr_s / t_beat).round();
                e.t_drop_corr_s - k * t_beat
            })
            .collect();
        let mean = errs.iter().sum::<f64>() / errs.len() as f64;
        let jitter =
            (errs.iter().map(|e| (e - mean).powi(2)).sum::<f64>() / errs.len() as f64).sqrt();
        assert!(mean.abs() < 2e-3, "bias {mean}");
        assert!(jitter < 2e-4, "jitter {jitter}");
        // Parities alternate along the sorted event stream.
        for w in events.windows(2) {
            if w[1].beat_index == w[0].beat_index + 1 {
                assert_ne!(w[0].parity, w[1].parity, "adjacent beats share parity");
            }
        }
        assert!(events.iter().all(|e| e.snr_db >= DETECT_SNR_DB));
        assert!(events.iter().all(|e| e.t_drop_edge_s.is_none() && e.t_unlock_s.is_none()));
    }

    #[test]
    fn low_snr_still_detects_a_useful_fraction() {
        let cfg = SynthConfig { snr_db: 10.0, ..SynthConfig::default() };
        let (events, t_beat) = pipeline_to_events(&cfg);
        let expected = (cfg.duration_s / t_beat) as usize;
        assert!(
            events.len() as f64 >= 0.5 * expected as f64,
            "{} of ~{expected} beats at 10 dB",
            events.len()
        );
    }

    #[test]
    fn beat_error_preserves_parity_split() {
        let cfg = SynthConfig { beat_error_ms: 0.8, snr_db: 30.0, ..SynthConfig::default() };
        let (events, _) = pipeline_to_events(&cfg);
        let tics = events.iter().filter(|e| e.parity == Parity::Tic).count();
        let tocs = events.len() - tics;
        assert!(tics >= 100 && tocs >= 100, "tics {tics} tocs {tocs}");
    }
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p chrona-dsp events` — FAIL (module/types missing)

- [ ] **Step 3: Implement**

Above the tests in `events.rs`:

```rust
use crate::envelope::DECIMATION;
use crate::fold::FoldProfile;

/// Correlation-peak SNR (dB) below which a beat is not emitted.
pub const DETECT_SNR_DB: f32 = 6.0;
const TEMPLATE_LEN: usize = 384;
const TEMPLATE_PEAK_AT: usize = 96;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Parity {
    Tic,
    Toc,
}

#[derive(Debug, Clone, Copy)]
pub struct BeatEvent {
    pub beat_index: i64,
    pub parity: Parity,
    /// Matched-filter drop time, absolute nominal-clock seconds since stream start.
    pub t_drop_corr_s: f64,
    pub snr_db: f32,
    /// Envelope leading-edge times (Task 7); None until extracted or when not found.
    pub t_drop_edge_s: Option<f64>,
    pub t_unlock_s: Option<f64>,
}

fn parabolic3(a: f64, b: f64, c: f64) -> f64 {
    let denom = a - 2.0 * b + c;
    if denom.abs() < 1e-20 { 0.0 } else { (0.5 * (a - c) / denom).clamp(-0.5, 0.5) }
}

/// Build one parity's unit-energy drop template from max-|x|-anchored windows.
/// Returns None when fewer than 6 windows survive outlier trimming.
fn build_template(raw: &[f32], predictions: &[f64], gate: usize) -> Option<Vec<f32>> {
    let mut windows: Vec<(f32, Vec<f32>)> = Vec::new();
    for &pred in predictions {
        let center = pred.round() as i64;
        let lo = center - gate as i64;
        let hi = center + gate as i64;
        if lo < 0 || (hi as usize) + TEMPLATE_LEN >= raw.len() {
            continue;
        }
        let (lo, hi) = (lo as usize, hi as usize);
        let peak_idx = (lo..hi)
            .max_by(|&i, &j| raw[i].abs().total_cmp(&raw[j].abs()))
            .unwrap_or(lo);
        if peak_idx < TEMPLATE_PEAK_AT || peak_idx + (TEMPLATE_LEN - TEMPLATE_PEAK_AT) >= raw.len() {
            continue;
        }
        let start = peak_idx - TEMPLATE_PEAK_AT;
        let w = raw[start..start + TEMPLATE_LEN].to_vec();
        windows.push((raw[peak_idx].abs(), w));
    }
    if windows.len() < 6 {
        return None;
    }
    // Drop top and bottom quintile by peak amplitude (outliers / missed beats).
    windows.sort_by(|a, b| a.0.total_cmp(&b.0));
    let q = windows.len() / 5;
    let kept = &windows[q..windows.len() - q];
    if kept.len() < 6 {
        return None;
    }
    let mut template = vec![0.0f32; TEMPLATE_LEN];
    for (_, w) in kept {
        for (t, v) in template.iter_mut().zip(w.iter()) {
            *t += v;
        }
    }
    let energy: f64 = template.iter().map(|v| (*v as f64).powi(2)).sum();
    if energy <= 0.0 {
        return None;
    }
    let norm = (energy.sqrt()) as f32;
    for t in template.iter_mut() {
        *t /= norm;
    }
    Some(template)
}

/// Correlate `template` across `[center − gate, center + gate]`; return
/// (fractional peak index into `raw`, peak value, off-peak rms).
fn correlate_in_gate(raw: &[f32], template: &[f32], center: i64, gate: usize, sr: f64) -> Option<(f64, f64, f64)> {
    let lo = center - gate as i64;
    if lo < 0 {
        return None;
    }
    let lo = lo as usize;
    let hi = (center as usize + gate).min(raw.len().saturating_sub(TEMPLATE_LEN));
    if hi <= lo + 2 {
        return None;
    }
    let mut corr: Vec<f64> = Vec::with_capacity(hi - lo);
    for start in lo..hi {
        let mut acc = 0.0f64;
        for (t, v) in template.iter().zip(&raw[start..start + TEMPLATE_LEN]) {
            acc += *t as f64 * *v as f64;
        }
        corr.push(acc);
    }
    let (pi, _) = corr
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))?;
    let peak = corr[pi].abs();
    let excl = (2.0e-3 * sr) as usize;
    let mut acc = 0.0f64;
    let mut n = 0usize;
    for (i, c) in corr.iter().enumerate() {
        if i.abs_diff(pi) > excl {
            acc += c * c;
            n += 1;
        }
    }
    if n < 8 {
        return None;
    }
    let rms = (acc / n as f64).sqrt().max(1e-30);
    let off = if pi == 0 || pi + 1 == corr.len() {
        0.0
    } else {
        parabolic3(corr[pi - 1].abs(), corr[pi].abs(), corr[pi + 1].abs())
    };
    Some((lo as f64 + pi as f64 + off, peak, rms))
}

/// Stage 5: per-beat drop events via a learned matched filter (spec §5.5).
/// See the plan's alignment and fold-window invariants; times are absolute
/// nominal-clock seconds. Edges stay None (filled by the edge pass).
pub fn extract_events(
    raw: &[f32],
    raw_start_abs: u64,
    env: &[f32],
    env_start_abs: u64,
    sample_rate_hz: f64,
    profile: &FoldProfile,
) -> Vec<BeatEvent> {
    let _ = env_start_abs; // env used by the Task-7 edge pass; alignment documented above
    let t_osc_env = profile.t_osc_env;
    if !(t_osc_env.is_finite() && t_osc_env > 1.0) || env.is_empty() {
        return Vec::new();
    }
    // Must match fold_envelope's window arithmetic (see invariant note).
    let cycles = (env.len() as f64 / t_osc_env).floor() as usize;
    if cycles < 8 {
        return Vec::new();
    }
    let usable = (cycles as f64 * t_osc_env) as usize;
    let fold_start = env.len() - usable;

    let (off_a, off_b) = profile.anchor_env_offsets();
    let gate = ((t_osc_env / 16.0) * DECIMATION as f64) as usize; // T_beat/8 in raw samples
    let mut out = Vec::new();

    // (parity, env offset, grid slot within the cycle)
    let (first, second) = if off_a <= off_b {
        ((Parity::Tic, off_a, 0i64), (Parity::Toc, off_b, 1i64))
    } else {
        ((Parity::Toc, off_b, 0i64), (Parity::Tic, off_a, 1i64))
    };

    for (parity, off, slot) in [first, second] {
        let predictions: Vec<f64> = (0..cycles)
            .map(|c| (fold_start as f64 + c as f64 * t_osc_env + off) * DECIMATION as f64)
            .collect();
        let Some(template) = build_template(raw, &predictions, gate) else { continue };
        for (c, &pred) in predictions.iter().enumerate() {
            let Some((idx, peak, rms)) =
                correlate_in_gate(raw, &template, pred.round() as i64, gate, sample_rate_hz)
            else {
                continue;
            };
            let snr_db = (20.0 * (peak / rms).log10()) as f32;
            if snr_db < DETECT_SNR_DB {
                continue;
            }
            out.push(BeatEvent {
                beat_index: 2 * c as i64 + slot,
                parity,
                t_drop_corr_s: (raw_start_abs as f64 + idx + TEMPLATE_PEAK_AT as f64)
                    / sample_rate_hz,
                snr_db,
                t_drop_edge_s: None,
                t_unlock_s: None,
            });
        }
    }
    out.sort_by(|a, b| a.t_drop_corr_s.total_cmp(&b.t_drop_corr_s));
    out
}
```

Add `pub mod events;` to `lib.rs`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p chrona-dsp events`
Expected: 3 tests PASS. Debug guidance: if detection count is low on the clean test, print the per-parity template survival (`build_template` returning None means the prediction→raw conversion is off — check the `× DECIMATION` factor and the fold_start recomputation); if jitter fails, check the parabolic sign and that correlation uses |corr| consistently for peak-finding.

- [ ] **Step 5: Full gates and commit**

```bash
git add crates/chrona-dsp
git commit -m "feat(dsp): learned matched-filter drop detection (stage 5, part 1)"
```

---

### Task 7: Envelope-edge extraction — drop edge + unlocking pulse (`events.rs`, part 2)

**Files:**
- Modify: `crates/chrona-dsp/src/events.rs`

**Interfaces:**
- Consumes: Task 6's internals; `envelope::DECIMATION`
- Produces (used by Tasks 8–10): `extract_events` now fills `t_drop_edge_s` and `t_unlock_s` per the spec §5.5 unlocking algorithm. **Timing decision (locked):** pulses are *found* with the spec's escalating absolute threshold, but *timed* at the half-height crossing of each pulse's own local maximum — amplitude-invariant, so the envelope LP delay cancels in `Δt = drop_edge − unlock` (the plan's bias-cancellation decision).

**Algorithm per accepted beat:**
1. Work on a 3-sample box-smoothed copy of `env` (≈1 ms at 3 kHz, spec §5.5), computed once per `extract_events` call. `global_max` = max of the smoothed window.
2. Drop peak: env index of the max smoothed value within ±3 ms of `t_drop_corr_s`. Drop edge = half-height crossing: scan backward from the peak to the last index where `smoothed < 0.5·peak_value`, then linear-interpolate the crossing between that index and the next; `t_drop_edge_s = (env_start_abs + fractional_index) · DECIMATION / sample_rate_hz`.
3. Unlocking (spec §5.5): search window = the `T_beat/8` env samples ending 1 ms before the drop edge; noise floor = mean smoothed env over the `T_beat/8` window *after* the drop peak; base threshold `thr = max(0.01·global_max, 1.4·noise)`. Find all upward crossings of `thr` in the window; while there are more than 3 crossings and `thr < 0.2·global_max`, escalate `thr ×= 1.4` and recount. Zero crossings → `t_unlock_s = None`. Otherwise the FIRST crossing marks the unlocking pulse: take the local max following that crossing (within the window), and time the pulse at its own half-height crossing (same method as the drop).
4. Sanity gate: require `1 ms ≤ (t_drop_edge − t_unlock) ≤ T_beat/8` else `t_unlock_s = None`.

- [ ] **Step 1: Write the failing tests**

Append to the `events.rs` tests module:

```rust
    #[test]
    fn edges_recover_the_unlock_to_drop_interval() {
        // Ground truth: synth places unlocking exactly dt before each drop,
        // dt = unlock_to_drop_dt_s(amplitude, lift, T_osc).
        let cfg = SynthConfig { snr_db: 30.0, ..SynthConfig::default() };
        let (events, _) = pipeline_to_events(&cfg);
        let t_osc = crate::bph::t_osc_s(cfg.bph);
        let dt_true =
            crate::synth::unlock_to_drop_dt_s(cfg.amplitude_deg, cfg.lift_angle_deg, t_osc);
        let dts: Vec<f64> = events
            .iter()
            .filter_map(|e| Some(e.t_drop_edge_s? - e.t_unlock_s?))
            .collect();
        assert!(
            dts.len() as f64 >= 0.5 * events.len() as f64,
            "unlocking found on {} of {}",
            dts.len(),
            events.len()
        );
        let mut sorted = dts.clone();
        sorted.sort_by(f64::total_cmp);
        let median = sorted[sorted.len() / 2];
        assert!(
            (median - dt_true).abs() < 3e-4,
            "median dt {median} want {dt_true}"
        );
    }

    #[test]
    fn weak_signal_degrades_unlocking_not_drops() {
        let cfg = SynthConfig { snr_db: 12.0, ..SynthConfig::default() };
        let (events, _) = pipeline_to_events(&cfg);
        assert!(!events.is_empty());
        let unlocked = events.iter().filter(|e| e.t_unlock_s.is_some()).count();
        // At low SNR the quiet unlocking pulse is lost more often than the loud
        // drop — the point of the tiered design. No hard floor here; just prove
        // the code degrades to None instead of fabricating.
        for e in &events {
            if let (Some(u), Some(d)) = (e.t_unlock_s, e.t_drop_edge_s) {
                let dt = d - u;
                assert!(dt >= 1e-3 && dt <= crate::bph::t_beat_s(cfg.bph) / 8.0, "dt {dt}");
            }
        }
        assert!(unlocked <= events.len());
    }

    #[test]
    fn amplitude_sweep_tracks_dt_monotonically() {
        // Bigger amplitude ⇒ smaller dt. The measured medians must order correctly.
        let mut medians = Vec::new();
        for amp in [220.0f64, 270.0, 320.0] {
            let cfg = SynthConfig { amplitude_deg: amp, snr_db: 30.0, ..SynthConfig::default() };
            let (events, _) = pipeline_to_events(&cfg);
            let mut dts: Vec<f64> = events
                .iter()
                .filter_map(|e| Some(e.t_drop_edge_s? - e.t_unlock_s?))
                .collect();
            assert!(dts.len() > 50, "amp {amp}: {} intervals", dts.len());
            dts.sort_by(f64::total_cmp);
            medians.push(dts[dts.len() / 2]);
        }
        assert!(medians[0] > medians[1] && medians[1] > medians[2], "medians {medians:?}");
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p chrona-dsp events` — the 3 new tests FAIL (edges are still `None` → `dts` empty)

- [ ] **Step 3: Implement**

Add to `events.rs` (above tests) and call it at the end of `extract_events` (after the sort, before `out` is returned — change the tail of `extract_events` to `fill_edges(&mut out, env, env_start_abs, sample_rate_hz, t_osc_env); out.sort_by(...); out` with the sort staying last):

```rust
/// 3-sample box smooth (≈1 ms at 3 kHz envelope rate), spec §5.5.
fn smooth3(env: &[f32]) -> Vec<f32> {
    let n = env.len();
    let mut out = vec![0.0f32; n];
    for i in 0..n {
        let a = env[i.saturating_sub(1)];
        let b = env[i];
        let c = env[(i + 1).min(n - 1)];
        out[i] = (a + b + c) / 3.0;
    }
    out
}

/// Half-height leading-edge time (fractional env index) of the pulse whose
/// local maximum is at `peak_idx`. Scans backward for the 0.5·peak crossing.
fn half_height_edge(sm: &[f32], peak_idx: usize) -> Option<f64> {
    let half = sm[peak_idx] * 0.5;
    let mut i = peak_idx;
    while i > 0 && sm[i - 1] >= half {
        i -= 1;
    }
    if i == 0 {
        return None;
    }
    // sm[i-1] < half ≤ sm[i]: interpolate between i-1 and i.
    let (lo, hi) = (sm[i - 1] as f64, sm[i] as f64);
    let frac = if hi > lo { (half as f64 - lo) / (hi - lo) } else { 0.5 };
    Some((i - 1) as f64 + frac)
}

/// Upward crossings of `thr` inside `sm[lo..hi]` (indices into `sm`).
fn upward_crossings(sm: &[f32], lo: usize, hi: usize, thr: f32) -> Vec<usize> {
    (lo.max(1)..hi)
        .filter(|&i| sm[i - 1] < thr && sm[i] >= thr)
        .collect()
}

fn fill_edges(
    events: &mut [BeatEvent],
    env: &[f32],
    env_start_abs: u64,
    sample_rate_hz: f64,
    t_osc_env: f64,
) {
    if env.is_empty() {
        return;
    }
    let sm = smooth3(env);
    let global_max = sm.iter().fold(0.0f32, |m, &v| m.max(v));
    if global_max <= 0.0 {
        return;
    }
    let env_rate = sample_rate_hz / DECIMATION as f64;
    let to_env_idx = |t_s: f64| t_s * env_rate - env_start_abs as f64;
    let to_seconds = |idx: f64| (env_start_abs as f64 + idx) / env_rate;
    let tbeat8 = (t_osc_env / 16.0) as usize; // T_beat/8 in env samples
    let ms3 = (3.0e-3 * env_rate) as usize;

    for e in events.iter_mut() {
        let center = to_env_idx(e.t_drop_corr_s);
        if !center.is_finite() || center < 0.0 {
            continue;
        }
        let center = center as usize;
        let lo = center.saturating_sub(ms3);
        let hi = (center + ms3).min(sm.len());
        if hi <= lo + 2 {
            continue;
        }
        let peak_idx = (lo..hi)
            .max_by(|&i, &j| sm[i].total_cmp(&sm[j]))
            .unwrap_or(center);
        let Some(drop_edge_idx) = half_height_edge(&sm, peak_idx) else { continue };
        e.t_drop_edge_s = Some(to_seconds(drop_edge_idx));

        // Unlocking search window: T_beat/8 ending 1 ms before the drop edge.
        let one_ms = (1.0e-3 * env_rate).ceil() as usize;
        let w_end = (drop_edge_idx as usize).saturating_sub(one_ms);
        let w_start = w_end.saturating_sub(tbeat8);
        if w_end <= w_start + 2 {
            continue;
        }
        // Noise floor: mean over the T_beat/8 window AFTER the drop peak (spec §5.5).
        let n_start = (peak_idx + 1).min(sm.len());
        let n_end = (peak_idx + 1 + tbeat8).min(sm.len());
        if n_end <= n_start {
            continue;
        }
        let noise =
            sm[n_start..n_end].iter().map(|&v| v as f64).sum::<f64>() / (n_end - n_start) as f64;
        let mut thr = (0.01 * global_max as f64).max(1.4 * noise) as f32;
        let mut crossings = upward_crossings(&sm, w_start, w_end, thr);
        while crossings.len() > 3 && (thr as f64) < 0.2 * global_max as f64 {
            thr *= 1.4;
            crossings = upward_crossings(&sm, w_start, w_end, thr);
        }
        let Some(&first) = crossings.first() else { continue };
        // The unlocking pulse's own local max after the crossing, inside the window.
        let u_peak = (first..w_end)
            .max_by(|&i, &j| sm[i].total_cmp(&sm[j]))
            .unwrap_or(first);
        let Some(u_edge_idx) = half_height_edge(&sm, u_peak) else { continue };
        let t_unlock = to_seconds(u_edge_idx);
        let dt = e.t_drop_edge_s.unwrap_or(f64::NAN) - t_unlock;
        let t_beat_s = t_osc_env / 2.0 / env_rate;
        if dt >= 1.0e-3 && dt <= t_beat_s / 8.0 {
            e.t_unlock_s = Some(t_unlock);
        }
    }
}
```

Note: `e.t_drop_edge_s.unwrap_or(f64::NAN)` is reachable only after the `Some` assignment above in the same iteration — keep it anyway (defensive, and NaN fails the `dt >=` gate safely). The `unwrap_or` keeps the no-unwrap constraint intact.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p chrona-dsp events`
Expected: all 6 events tests PASS. Debug guidance: if the median Δt is biased high by ~0.3–0.5 ms, the unlocking search is catching the IMPULSE pulse (middle of the cluster) — check that the window ends 1 ms before the drop *edge* (not the drop *peak*) and that the first-crossing rule is taking the earliest pulse; if Δt is biased low, the drop edge is being timed at the unlocking pulse's rise — check the ±3 ms drop-peak window is centered on `t_drop_corr_s`.

- [ ] **Step 5: Full gates and commit**

```bash
git add crates/chrona-dsp
git commit -m "feat(dsp): envelope-edge extraction — drop edge and spec unlocking algorithm"
```

---

### Task 8: Rate + beat-error regression (`metrics.rs`, part 1)

**Files:**
- Create: `crates/chrona-dsp/src/metrics.rs`
- Modify: `crates/chrona-dsp/src/lib.rs` (add `pub mod metrics;`)

**Interfaces:**
- Consumes: `events::{BeatEvent, Parity}` (Tasks 6–7)
- Produces (used by Tasks 9–11):
  - `pub struct RegressionResult { pub t_beat_s: f64, pub beat_error_ms: f64, pub jitter_ms: f64, pub events_used: usize }` (derives `Debug, Clone, Copy`)
  - `pub fn regress_unlocking(events: &[BeatEvent]) -> Option<RegressionResult>` — three-parameter least squares `t_unlock[k] = a + T_beat·k + c·s_k` (`s = +1` Tic, `−1` Toc, `k = beat_index`) over events with `t_unlock_s` present; `BE = 2·|c|` in ms (plan Decisions: alternate intervals are `T_beat ∓ 2c`); `jitter_ms` = residual σ. `None` when fewer than 12 usable events, either parity has fewer than 4, or the normal-equation determinant is degenerate. All quantities in nominal-clock units — the caller applies the ppm correction.

- [ ] **Step 1: Write the failing tests**

`crates/chrona-dsp/src/metrics.rs`:

```rust
//! Stage 6 (spec §5.6, §2.3): unlocking-referenced rate + beat error via
//! least squares (Watch-O-Scope's regression, not naive averaging), and
//! amplitude with validity gates.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::EnvelopeExtractor;
    use crate::events::{extract_events, BeatEvent, Parity};
    use crate::filter::{Butterworth, DcBlocker};
    use crate::fold::fold_envelope;
    use crate::synth::{synthesize, SynthConfig};

    /// synth → precondition → envelope → fold → events (duplicated from
    /// events.rs tests by design — each module's tests stand alone).
    fn pipeline_to_events(cfg: &SynthConfig) -> Vec<BeatEvent> {
        let x = synthesize(cfg).expect("valid synth config");
        let sr = cfg.sample_rate_hz;
        let mut dc = DcBlocker::new();
        let mut hp = Butterworth::high_pass(sr, 3_000.0).unwrap();
        let raw: Vec<f32> = x.iter().map(|&s| hp.process(dc.process(s))).collect();
        let mut envx = EnvelopeExtractor::new(sr).unwrap();
        let mut env = Vec::new();
        envx.process(&raw, &mut env);
        let t_osc_env = crate::bph::t_osc_s(cfg.bph) * (1.0 - cfg.rate_s_per_day / 86_400.0)
            * envx.envelope_rate_hz();
        let profile = fold_envelope(&env, t_osc_env).expect("fold");
        extract_events(&raw, 0, &env, 0, sr, &profile)
    }

    #[test]
    fn beat_error_sweep_meets_the_bar() {
        // M2 bar (Global Constraints): BE within ±0.1 ms at 30 dB.
        for be_true in [0.0f64, 0.3, 0.8, 2.0] {
            let cfg = SynthConfig {
                beat_error_ms: be_true,
                rate_s_per_day: 12.0,
                snr_db: 30.0,
                ..SynthConfig::default()
            };
            let events = pipeline_to_events(&cfg);
            let r = regress_unlocking(&events).expect("regression");
            assert!(
                (r.beat_error_ms - be_true).abs() <= 0.1,
                "be {be_true}: got {}",
                r.beat_error_ms
            );
            // Rate from the regressed beat period: ±0.3 s/d bar.
            let rate = 86_400.0 * (crate::bph::t_beat_s(cfg.bph) - r.t_beat_s)
                / crate::bph::t_beat_s(cfg.bph);
            assert!((rate - 12.0).abs() <= 0.3, "be {be_true}: rate {rate}");
            assert!(r.jitter_ms < 0.5, "jitter {}", r.jitter_ms);
            assert!(r.events_used >= 12);
        }
    }

    #[test]
    fn regression_refuses_thin_or_one_sided_data() {
        let mk = |i: i64, p: Parity, t: f64| BeatEvent {
            beat_index: i,
            parity: p,
            t_drop_corr_s: t,
            snr_db: 20.0,
            t_drop_edge_s: Some(t),
            t_unlock_s: Some(t - 0.007),
        };
        // 11 events → too few.
        let few: Vec<BeatEvent> = (0..11)
            .map(|i| mk(i, if i % 2 == 0 { Parity::Tic } else { Parity::Toc }, 0.125 * i as f64))
            .collect();
        assert!(regress_unlocking(&few).is_none());
        // 20 events but only 2 tocs → one-sided.
        let sided: Vec<BeatEvent> = (0..20)
            .map(|i| mk(i, if i < 18 { Parity::Tic } else { Parity::Toc }, 0.125 * i as f64))
            .collect();
        assert!(regress_unlocking(&sided).is_none());
        // Events without unlocking timestamps don't count.
        let no_unlock: Vec<BeatEvent> = (0..30)
            .map(|i| BeatEvent {
                t_unlock_s: None,
                ..mk(i, if i % 2 == 0 { Parity::Tic } else { Parity::Toc }, 0.125 * i as f64)
            })
            .collect();
        assert!(regress_unlocking(&no_unlock).is_none());
    }

    #[test]
    fn synthetic_exact_model_recovers_parameters() {
        // Noise-free fabricated grid: t = 5.0 + 0.125·k + 0.0004·s_k
        // (BE = 2·0.0004 s = 0.8 ms). Regression must recover it near-exactly.
        let events: Vec<BeatEvent> = (0..100)
            .map(|k| {
                let parity = if k % 2 == 0 { Parity::Tic } else { Parity::Toc };
                let s = if k % 2 == 0 { 1.0 } else { -1.0 };
                let t = 5.0 + 0.125 * k as f64 + 0.0004 * s;
                BeatEvent {
                    beat_index: k,
                    parity,
                    t_drop_corr_s: t + 0.007,
                    snr_db: 20.0,
                    t_drop_edge_s: Some(t + 0.007),
                    t_unlock_s: Some(t),
                }
            })
            .collect();
        let r = regress_unlocking(&events).expect("regression");
        assert!((r.t_beat_s - 0.125).abs() < 1e-12, "T {}", r.t_beat_s);
        assert!((r.beat_error_ms - 0.8).abs() < 1e-9, "BE {}", r.beat_error_ms);
        assert!(r.jitter_ms < 1e-9);
    }
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p chrona-dsp metrics` — FAIL (module missing)

- [ ] **Step 3: Implement**

Above the tests in `metrics.rs`:

```rust
use crate::events::{BeatEvent, Parity};

#[derive(Debug, Clone, Copy)]
pub struct RegressionResult {
    /// Regressed beat period, nominal-clock seconds.
    pub t_beat_s: f64,
    /// BE = 2·|c| (alternate unlocking intervals are T ∓ 2c), milliseconds.
    pub beat_error_ms: f64,
    /// Residual σ of the fit, milliseconds.
    pub jitter_ms: f64,
    pub events_used: usize,
}

/// Three-parameter LS over unlocking timestamps: t = a + T·k + c·s (spec §5.6).
pub fn regress_unlocking(events: &[BeatEvent]) -> Option<RegressionResult> {
    let usable: Vec<(f64, f64, f64)> = events
        .iter()
        .filter_map(|e| {
            let t = e.t_unlock_s?;
            let s = match e.parity {
                Parity::Tic => 1.0,
                Parity::Toc => -1.0,
            };
            Some((e.beat_index as f64, s, t))
        })
        .collect();
    let n = usable.len();
    let tics = usable.iter().filter(|(_, s, _)| *s > 0.0).count();
    if n < 12 || tics < 4 || n - tics < 4 {
        return None;
    }

    // Normal equations for [a, T, c]:
    // [ n   Σk   Σs  ] [a]   [ Σt  ]
    // [ Σk  Σk²  Σks ] [T] = [ Σkt ]
    // [ Σs  Σks  Σs² ] [c]   [ Σst ]
    let (mut sk, mut ss, mut st, mut sk2, mut sks, mut ss2, mut skt, mut sst) =
        (0.0f64, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
    for &(k, s, t) in &usable {
        sk += k;
        ss += s;
        st += t;
        sk2 += k * k;
        sks += k * s;
        ss2 += s * s;
        skt += k * t;
        sst += s * t;
    }
    let nf = n as f64;
    let det3 = |m: [[f64; 3]; 3]| {
        m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
            - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
            + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
    };
    let m = [[nf, sk, ss], [sk, sk2, sks], [ss, sks, ss2]];
    let d = det3(m);
    if d.abs() < 1e-9 {
        return None;
    }
    let rhs = [st, skt, sst];
    let col = |j: usize| {
        let mut mm = m;
        for i in 0..3 {
            mm[i][j] = rhs[i];
        }
        det3(mm) / d
    };
    let (a, t_beat, c) = (col(0), col(1), col(2));

    let ssr: f64 = usable
        .iter()
        .map(|&(k, s, t)| (t - (a + t_beat * k + c * s)).powi(2))
        .sum();
    let jitter_s = (ssr / (nf - 3.0)).sqrt();

    Some(RegressionResult {
        t_beat_s: t_beat,
        beat_error_ms: 2.0 * c.abs() * 1_000.0,
        jitter_ms: jitter_s * 1_000.0,
        events_used: n,
    })
}
```

Add `pub mod metrics;` to `lib.rs`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p chrona-dsp metrics`
Expected: 3 tests PASS. Debug guidance if the BE sweep misses the bar: dump per-parity mean unlocking offsets — a BE error of exactly 2× or 0.5× means the `2·|c|` convention slipped (re-derive: shifts ±c on the model side vs synth's ±be/2 ⇒ c ≈ be/2); a rate miss with good BE means `beat_index` numbering has gaps treated as consecutive — indices come from events and already carry true grid positions, so check the filter didn't re-index.

- [ ] **Step 5: Full gates and commit**

```bash
git add crates/chrona-dsp
git commit -m "feat(dsp): unlocking-regression rate and beat error (stage 6, part 1)"
```

---

### Task 9: Amplitude with validity gates (`metrics.rs`, part 2)

**Files:**
- Modify: `crates/chrona-dsp/src/metrics.rs`

**Interfaces:**
- Consumes: `events::BeatEvent`, `synth::unlock_to_drop_dt_s` (tests)
- Produces (used by Tasks 10–11):
  - `pub enum AmplitudeGateFail { InsufficientEvents, OutOfRange, TicTocDisagree }` (derives `Debug, Clone, Copy, PartialEq, Eq`)
  - `pub struct AmplitudeResult { pub degrees: f64, pub tic_degrees: f64, pub toc_degrees: f64 }` (derives `Debug, Clone, Copy`)
  - `pub fn amplitude_from_events(events: &[BeatEvent], t_osc_s: f64, lift_angle_deg: f64) -> Result<AmplitudeResult, AmplitudeGateFail>` — per-parity **median** Δt (`t_drop_edge − t_unlock`, both present), ≥ 8 intervals per parity, `A = L/(2·sin(π·Δt/T_osc))` per spec §2.3, gates verbatim: each of A_tic/A_toc in [135°, 360°] else `OutOfRange`; `|A_tic − A_toc| < 60°` else `TicTocDisagree`; success returns the mean. Amplitude is clock-invariant (Δt and T_osc scale identically under ppm), so the caller passes nominal-clock values.

- [ ] **Step 1: Write the failing tests**

Append to the `metrics.rs` tests module:

```rust
    #[test]
    fn amplitude_sweep_meets_the_bar() {
        // M2 bar: ±5° at 30 dB, averaged output.
        for amp_true in [200.0f64, 240.0, 270.0, 300.0, 330.0] {
            let cfg = SynthConfig { amplitude_deg: amp_true, snr_db: 30.0, ..SynthConfig::default() };
            let events = pipeline_to_events(&cfg);
            let t_osc = crate::bph::t_osc_s(cfg.bph);
            let a = amplitude_from_events(&events, t_osc, cfg.lift_angle_deg)
                .unwrap_or_else(|g| panic!("amp {amp_true}: gate {g:?}"));
            assert!((a.degrees - amp_true).abs() <= 5.0, "amp {amp_true}: got {}", a.degrees);
            assert!((a.tic_degrees - a.toc_degrees).abs() < 60.0);
        }
    }

    fn fabricated(dt_tic_s: f64, dt_toc_s: f64, n_per_parity: usize) -> Vec<BeatEvent> {
        (0..2 * n_per_parity)
            .map(|k| {
                let (parity, dt) = if k % 2 == 0 {
                    (Parity::Tic, dt_tic_s)
                } else {
                    (Parity::Toc, dt_toc_s)
                };
                let t = 0.125 * k as f64;
                BeatEvent {
                    beat_index: k as i64,
                    parity,
                    t_drop_corr_s: t,
                    snr_db: 20.0,
                    t_drop_edge_s: Some(t),
                    t_unlock_s: Some(t - dt),
                }
            })
            .collect()
    }

    #[test]
    fn amplitude_gates_fire_correctly() {
        let t_osc = 0.25;
        let lift = 52.0;
        let dt_for = |a: f64| crate::synth::unlock_to_drop_dt_s(a, lift, t_osc);
        // Too few events per parity.
        assert_eq!(
            amplitude_from_events(&fabricated(dt_for(270.0), dt_for(270.0), 5), t_osc, lift),
            Err(AmplitudeGateFail::InsufficientEvents)
        );
        // A = 100° is physically valid input but below the 135° display gate.
        assert_eq!(
            amplitude_from_events(&fabricated(dt_for(100.0), dt_for(100.0), 20), t_osc, lift),
            Err(AmplitudeGateFail::OutOfRange)
        );
        // Tic 270° vs toc 200° → 70° disagreement > 60° gate.
        assert_eq!(
            amplitude_from_events(&fabricated(dt_for(270.0), dt_for(200.0), 20), t_osc, lift),
            Err(AmplitudeGateFail::TicTocDisagree)
        );
        // Healthy case passes.
        let ok = amplitude_from_events(&fabricated(dt_for(280.0), dt_for(275.0), 20), t_osc, lift)
            .expect("healthy");
        assert!((ok.degrees - 277.5).abs() < 1.0, "got {}", ok.degrees);
    }
```

Note for the reviewer: `amplitude_gates_fire_correctly` uses `assert_eq!` on `Result<AmplitudeResult, _>` — `AmplitudeResult` therefore also needs `PartialEq`. Add `PartialEq` to both derives.

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p chrona-dsp metrics` — FAIL (types missing)

- [ ] **Step 3: Implement**

Append to `metrics.rs` (above tests):

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AmplitudeGateFail {
    /// Fewer than 8 unlock→drop intervals on either parity.
    InsufficientEvents,
    /// A_tic or A_toc outside the 135–360° validity band (spec §2.3).
    OutOfRange,
    /// |A_tic − A_toc| ≥ 60° (spec §2.3 agreement gate).
    TicTocDisagree,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AmplitudeResult {
    pub degrees: f64,
    pub tic_degrees: f64,
    pub toc_degrees: f64,
}

fn median(v: &mut Vec<f64>) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(f64::total_cmp);
    Some(v[v.len() / 2])
}

/// Spec §2.3: A = L / (2·sin(π·Δt/T_osc)), gated 135–360° with 60° tic/toc agreement.
pub fn amplitude_from_events(
    events: &[BeatEvent],
    t_osc_s: f64,
    lift_angle_deg: f64,
) -> Result<AmplitudeResult, AmplitudeGateFail> {
    let mut dt_tic = Vec::new();
    let mut dt_toc = Vec::new();
    for e in events {
        if let (Some(u), Some(d)) = (e.t_unlock_s, e.t_drop_edge_s) {
            let dt = d - u;
            if dt > 0.0 {
                match e.parity {
                    Parity::Tic => dt_tic.push(dt),
                    Parity::Toc => dt_toc.push(dt),
                }
            }
        }
    }
    if dt_tic.len() < 8 || dt_toc.len() < 8 {
        return Err(AmplitudeGateFail::InsufficientEvents);
    }
    let amp_of = |dt: f64| {
        let s = (std::f64::consts::PI * dt / t_osc_s).sin();
        if s <= 0.0 { f64::INFINITY } else { lift_angle_deg / (2.0 * s) }
    };
    // Invariant: the vectors were just checked non-empty (len ≥ 8).
    let tic = amp_of(median(&mut dt_tic).expect("non-empty by guard"));
    let toc = amp_of(median(&mut dt_toc).expect("non-empty by guard"));
    let in_range = |a: f64| (135.0..=360.0).contains(&a);
    if !in_range(tic) || !in_range(toc) {
        return Err(AmplitudeGateFail::OutOfRange);
    }
    if (tic - toc).abs() >= 60.0 {
        return Err(AmplitudeGateFail::TicTocDisagree);
    }
    Ok(AmplitudeResult { degrees: (tic + toc) / 2.0, tic_degrees: tic, toc_degrees: toc })
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p chrona-dsp metrics`
Expected: all metrics tests PASS. Pipeline note: very low true amplitudes (≲ 130°) produce Δt > T_beat/8, which the events sanity gate rejects — through the real pipeline they surface as `InsufficientEvents`, not `OutOfRange`; the `OutOfRange` path is exercised by the fabricated-events test by design.

- [ ] **Step 5: Full gates and commit**

```bash
git add crates/chrona-dsp
git commit -m "feat(dsp): amplitude from unlock-to-drop intervals with spec validity gates"
```

---

### Task 10: Tier engine + Analyzer rework → `MetricsSnapshot` (breaking)

**Files:**
- Create: `crates/chrona-dsp/src/tier.rs`
- Modify: `crates/chrona-dsp/src/analyzer.rs` (major rework), `crates/chrona-dsp/src/lib.rs` (add `pub mod tier;`, update re-exports)
- Modify: `crates/chrona-cli/src/commands.rs` (minimal field-rename patch ONLY — the full CLI upgrade is Task 11; the workspace must stay green within this task)

**Interfaces:**
- Consumes: everything from Tasks 4–9
- Produces (used by Tasks 11–12; this is the crate's public face from here on):
  - `tier::Tier { T1, T2, T3 }` (derives `Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord`); `tier::assign_tier(detection_ratio: f64, onset_jitter_ms: Option<f64>, unlocking_ratio: f64, amplitude_ok: bool) -> Tier` — T2 iff `detection_ratio ≥ 0.6` and jitter present `≤ 0.5`; T3 iff T2 conditions and `unlocking_ratio ≥ 0.5` and `amplitude_ok` (spec §3.1 verbatim)
  - `analyzer::AnalyzerError` (thiserror): `#[error(transparent)] Filter(#[from] crate::filter::FilterError)` and `#[error("invalid analyzer config: {reason}")] InvalidConfig { reason: String }`
  - `AnalyzerConfig` gains `pub lift_angle_deg: f64` (default 52.0, valid 10–90) and `pub averaging_s: f64` (default 30.0, valid 2–60); `Analyzer::new -> Result<Self, AnalyzerError>` validates both (finite + range)
  - `pub enum RateSource { PeriodSlope, UnlockingRegression }` (derives `Debug, Clone, Copy, PartialEq, Eq`)
  - `pub struct Quality { pub detection_ratio: f64, pub onset_jitter_ms: Option<f64>, pub mean_beat_snr_db: Option<f64>, pub unlocking_ratio: f64, pub clipped_samples: u64, pub amplitude_gate: Option<crate::metrics::AmplitudeGateFail> }` (derives `Debug, Clone, Copy`)
  - `pub struct MetricsSnapshot { pub tier: crate::tier::Tier, pub bph_detected: f64, pub bph_nominal: Option<u32>, pub rate_s_per_day: Option<f64>, pub rate_source: RateSource, pub beat_error_ms: Option<f64>, pub amplitude_deg: Option<f64>, pub period: PeriodEstimate, pub quality: Quality, pub calibrated: bool }` (derives `Debug, Clone, Copy`)
  - `Analyzer::current() -> Option<MetricsSnapshot>` (replaces `RateEstimate`, which is DELETED); `None` still means Tier 0 (nothing measurable)
  - lib.rs re-exports become: `pub use analyzer::{Analyzer, AnalyzerConfig, AnalyzerError, BphMode, MetricsSnapshot, Quality, RateSource}; pub use period::PeriodEstimate; pub use tier::Tier;`

**current() pipeline (all pieces exist; this task is wiring):**
1. `raw_period = self.period.estimate()?` (nominal clock).
2. Copy the envelope window (`env_ring.copy_last(len)`, `env_start_abs = env_ring.start_index()`); fold with `fold_with_octave_guard(&env_window, raw_period.t_osc_s · env_rate)`. On `None` → build a Tier-1 snapshot (period metrics only, quality zeros). On `halved` → divide `t_osc_s` and `sigma_s` by 2 before everything downstream.
3. Copy the raw window aligned to the envelope: `raw_ring.copy_range_abs(env_start_abs · DECIMATION as u64, env_window.len() · DECIMATION, &mut raw_window)`; on failure (possible only in the first seconds) → Tier-1 snapshot. **Ring-capacity invariant:** the raw ring is constructed with capacity `32·sr + DECIMATION` — the extra `DECIMATION` samples guarantee `env_start·16` is never below the raw ring's retained start (decimation-phase remainder ≤ 15).
4. `events = extract_events(&raw_window, env_start_abs·16, &env_window, env_start_abs, sr, &profile)`; keep `recent` = events with `t_drop_corr_s ≥ newest_time − averaging_s` (`newest_time = raw_ring.total_pushed() as f64 / sr`).
5. `detection_ratio = recent.len() as f64 / expected` clamped to 1.0, `expected = (span / t_beat_nominal_clock).round().max(1.0)` with `span = min(averaging_s, env window seconds)`; `unlocking_ratio` = fraction of `recent` with `t_unlock_s`; `mean_beat_snr_db` = mean of `recent` SNRs (None when empty).
6. `regression = metrics::regress_unlocking(&recent)`; clock-correct `t_beat_s`, `beat_error_ms`, `jitter_ms` by dividing by `1 + ppm/1e6`. `amplitude = metrics::amplitude_from_events(&recent, t_osc_nominal_after_halving, lift)` (clock-invariant by the Task 9 note).
7. `tier = assign_tier(...)` with `amplitude_ok = amplitude.is_ok()`.
8. BPH-mode resolution identical to M1 (Free / Auto-snap / Fixed-3 %) on the clock-corrected, halving-corrected `bph_detected`. Rate: when a nominal exists — `tier ≥ T2 && regression.is_some()` → `86_400·(t_beat_nom − t_beat_regressed_corrected)/t_beat_nom` with `rate_source = UnlockingRegression`; else the M1 period-based formula with `rate_source = PeriodSlope`.
9. Reported metrics obey tiers: `beat_error_ms` only at `tier ≥ T2`; `amplitude_deg` only at `T3`; `quality.amplitude_gate = amplitude.err()`.

- [ ] **Step 1: tier.rs with tests (TDD)**

```rust
//! Stage 7 (spec §3.1, §5.7): tier assignment from quality signals.

/// Signal-quality tier. `Analyzer::current()` returning `None` is Tier 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tier {
    /// Rate + BPH only (long-window periodicity).
    T1,
    /// + beat error (per-beat onsets separable).
    T2,
    /// + amplitude (intra-tick pulse structure resolvable).
    T3,
}

/// Spec §3.1 thresholds (initial values, tunable against the fixture corpus).
pub fn assign_tier(
    detection_ratio: f64,
    onset_jitter_ms: Option<f64>,
    unlocking_ratio: f64,
    amplitude_ok: bool,
) -> Tier {
    let t2 = detection_ratio >= 0.6 && onset_jitter_ms.is_some_and(|j| j <= 0.5);
    if t2 && unlocking_ratio >= 0.5 && amplitude_ok {
        Tier::T3
    } else if t2 {
        Tier::T2
    } else {
        Tier::T1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thresholds_match_spec() {
        assert_eq!(assign_tier(0.9, Some(0.1), 0.9, true), Tier::T3);
        assert_eq!(assign_tier(0.9, Some(0.1), 0.9, false), Tier::T2); // amp gated
        assert_eq!(assign_tier(0.9, Some(0.1), 0.4, true), Tier::T2); // unlocking thin
        assert_eq!(assign_tier(0.59, Some(0.1), 0.9, true), Tier::T1); // detection < 60 %
        assert_eq!(assign_tier(0.9, Some(0.6), 0.9, true), Tier::T1); // jitter > 0.5 ms
        assert_eq!(assign_tier(0.9, None, 0.9, true), Tier::T1); // no regression
    }
}
```

Write the test first (RED = module missing), then the implementation, `pub mod tier;` in lib.rs, run `cargo test -p chrona-dsp tier` → PASS.

- [ ] **Step 2: Rework analyzer.rs**

Replace the config/result/struct sections with (keep the module doc, update it to "stages 1–7"):

```rust
use crate::envelope::{DECIMATION, EnvelopeExtractor};
use crate::events::{extract_events, BeatEvent};
use crate::filter::{Butterworth, DcBlocker, FilterError};
use crate::fold::fold_with_octave_guard;
use crate::metrics::{amplitude_from_events, regress_unlocking, AmplitudeGateFail};
use crate::period::{PeriodEstimate, PeriodEstimator};
use crate::ring::SampleRing;
use crate::tier::{assign_tier, Tier};

#[derive(Debug, thiserror::Error)]
pub enum AnalyzerError {
    #[error(transparent)]
    Filter(#[from] FilterError),
    #[error("invalid analyzer config: {reason}")]
    InvalidConfig { reason: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BphMode {
    Auto,
    Fixed(u32),
    Free,
}

#[derive(Debug, Clone, Copy)]
pub struct AnalyzerConfig {
    pub sample_rate_hz: f64,
    pub bph_mode: BphMode,
    /// Audio-clock correction in ppm (spec §3.4). 0.0 = uncalibrated.
    pub ppm_correction: f64,
    /// Lift angle in degrees (spec §2.3; default 52, valid 10–90).
    pub lift_angle_deg: f64,
    /// Stage-6 averaging window in seconds (spec §2.3; default 30, valid 2–60).
    pub averaging_s: f64,
}

impl Default for AnalyzerConfig {
    fn default() -> Self {
        AnalyzerConfig {
            sample_rate_hz: 48_000.0,
            bph_mode: BphMode::Auto,
            ppm_correction: 0.0,
            lift_angle_deg: 52.0,
            averaging_s: 30.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateSource {
    PeriodSlope,
    UnlockingRegression,
}

#[derive(Debug, Clone, Copy)]
pub struct Quality {
    pub detection_ratio: f64,
    pub onset_jitter_ms: Option<f64>,
    pub mean_beat_snr_db: Option<f64>,
    pub unlocking_ratio: f64,
    pub clipped_samples: u64,
    pub amplitude_gate: Option<AmplitudeGateFail>,
}

#[derive(Debug, Clone, Copy)]
pub struct MetricsSnapshot {
    pub tier: Tier,
    pub bph_detected: f64,
    pub bph_nominal: Option<u32>,
    /// s/day, positive = fast; None when no defensible nominal (spec §3.1).
    pub rate_s_per_day: Option<f64>,
    pub rate_source: RateSource,
    /// Only at tier ≥ T2.
    pub beat_error_ms: Option<f64>,
    /// Only at T3.
    pub amplitude_deg: Option<f64>,
    pub period: PeriodEstimate,
    pub quality: Quality,
    pub calibrated: bool,
}

pub struct Analyzer {
    config: AnalyzerConfig,
    dc: DcBlocker,
    hp: Butterworth,
    envelope: EnvelopeExtractor,
    period: PeriodEstimator,
    raw_ring: SampleRing,
    env_ring: SampleRing,
    clipped: u64,
    scratch: Vec<f32>,
}

impl Analyzer {
    pub fn new(config: AnalyzerConfig) -> Result<Self, AnalyzerError> {
        for (name, v, lo, hi) in [
            ("lift_angle_deg", config.lift_angle_deg, 10.0, 90.0),
            ("averaging_s", config.averaging_s, 2.0, 60.0),
        ] {
            if !v.is_finite() || !(lo..=hi).contains(&v) {
                return Err(AnalyzerError::InvalidConfig {
                    reason: format!("{name} = {v} outside [{lo}, {hi}]"),
                });
            }
        }
        let envelope = EnvelopeExtractor::new(config.sample_rate_hz)?;
        let period = PeriodEstimator::new(envelope.envelope_rate_hz());
        // Ring-capacity invariant: +DECIMATION keeps env_start·16 inside the raw ring
        // (decimation-phase remainder ≤ 15) so the aligned copy in current() succeeds.
        let raw_cap = (32.0 * config.sample_rate_hz) as usize + DECIMATION;
        let env_cap = (32.0 * config.sample_rate_hz) as usize / DECIMATION;
        Ok(Analyzer {
            config,
            dc: DcBlocker::new(),
            hp: Butterworth::high_pass(config.sample_rate_hz, 3_000.0)?,
            envelope,
            period,
            raw_ring: SampleRing::new(raw_cap),
            env_ring: SampleRing::new(env_cap),
            clipped: 0,
            scratch: Vec::new(),
        })
    }

    pub fn push_samples(&mut self, samples: &[f32]) {
        self.clipped += samples.iter().filter(|s| s.abs() >= 0.999).count() as u64;
        self.scratch.clear();
        self.scratch.reserve(samples.len());
        for &s in samples {
            self.scratch.push(self.hp.process(self.dc.process(s)));
        }
        let filtered = std::mem::take(&mut self.scratch);
        self.raw_ring.push_slice(&filtered);
        let mut env_out = Vec::with_capacity(filtered.len() / DECIMATION + 1);
        self.envelope.process(&filtered, &mut env_out);
        self.env_ring.push_slice(&env_out);
        self.period.push_envelope(&env_out);
        self.scratch = filtered;
    }

    pub fn current(&self) -> Option<MetricsSnapshot> {
        let raw_est = self.period.estimate()?;
        let clock = 1.0 + self.config.ppm_correction / 1e6;
        let sr = self.config.sample_rate_hz;
        let env_rate = self.envelope.envelope_rate_hz();

        let mut env_window = Vec::new();
        self.env_ring.copy_last(self.env_ring.len(), &mut env_window);
        let env_start_abs = self.env_ring.start_index();

        let folded = fold_with_octave_guard(&env_window, raw_est.t_osc_s * env_rate);
        let (profile, halved) = match folded {
            Some(p) => p,
            None => return Some(self.tier1_snapshot(raw_est, clock, None)),
        };
        let halving = if halved { 2.0 } else { 1.0 };
        let t_osc_nominal = raw_est.t_osc_s / halving;
        let corrected = PeriodEstimate {
            t_osc_s: t_osc_nominal / clock,
            sigma_s: raw_est.sigma_s / halving / clock,
            window_s: raw_est.window_s,
        };

        let mut raw_window = Vec::new();
        let ok = self.raw_ring.copy_range_abs(
            env_start_abs * DECIMATION as u64,
            env_window.len() * DECIMATION,
            &mut raw_window,
        );
        if !ok {
            return Some(self.tier1_snapshot(raw_est, clock, Some(halving)));
        }

        let events = extract_events(
            &raw_window,
            env_start_abs * DECIMATION as u64,
            &env_window,
            env_start_abs,
            sr,
            &profile,
        );
        let newest_time = self.raw_ring.total_pushed() as f64 / sr;
        let recent: Vec<BeatEvent> = events
            .into_iter()
            .filter(|e| e.t_drop_corr_s >= newest_time - self.config.averaging_s)
            .collect();

        let span = self.config.averaging_s.min(env_window.len() as f64 / env_rate);
        let expected = (span / (t_osc_nominal / 2.0)).round().max(1.0);
        let detection_ratio = (recent.len() as f64 / expected).min(1.0);
        let unlocking_ratio = if recent.is_empty() {
            0.0
        } else {
            recent.iter().filter(|e| e.t_unlock_s.is_some()).count() as f64 / recent.len() as f64
        };
        let mean_beat_snr_db = if recent.is_empty() {
            None
        } else {
            Some(recent.iter().map(|e| e.snr_db as f64).sum::<f64>() / recent.len() as f64)
        };

        let regression = regress_unlocking(&recent);
        let amplitude =
            amplitude_from_events(&recent, t_osc_nominal, self.config.lift_angle_deg);
        let onset_jitter_ms = regression.map(|r| r.jitter_ms / clock);
        let tier = assign_tier(detection_ratio, onset_jitter_ms, unlocking_ratio, amplitude.is_ok());

        let bph_detected = crate::bph::bph_from_t_osc(corrected.t_osc_s);
        let (bph_nominal, period_rate) = self.resolve_mode(bph_detected, corrected.t_osc_s);
        let (rate_s_per_day, rate_source) = match (bph_nominal, tier >= Tier::T2, regression) {
            (Some(nom), true, Some(r)) => {
                let t_beat_nom = crate::bph::t_beat_s(nom);
                let rate = 86_400.0 * (t_beat_nom - r.t_beat_s / clock) / t_beat_nom;
                (Some(rate), RateSource::UnlockingRegression)
            }
            _ => (period_rate, RateSource::PeriodSlope),
        };

        Some(MetricsSnapshot {
            tier,
            bph_detected,
            bph_nominal,
            rate_s_per_day,
            rate_source,
            beat_error_ms: (tier >= Tier::T2)
                .then(|| regression.map(|r| r.beat_error_ms / clock))
                .flatten(),
            amplitude_deg: (tier == Tier::T3)
                .then(|| amplitude.as_ref().ok().map(|a| a.degrees))
                .flatten(),
            period: corrected,
            quality: Quality {
                detection_ratio,
                onset_jitter_ms,
                mean_beat_snr_db,
                unlocking_ratio,
                clipped_samples: self.clipped,
                amplitude_gate: amplitude.err(),
            },
            calibrated: self.config.ppm_correction != 0.0,
        })
    }

    /// Fallback when fold/alignment can't run yet: period metrics only.
    fn tier1_snapshot(
        &self,
        raw_est: PeriodEstimate,
        clock: f64,
        halving: Option<f64>,
    ) -> MetricsSnapshot {
        let halving = halving.unwrap_or(1.0);
        let corrected = PeriodEstimate {
            t_osc_s: raw_est.t_osc_s / halving / clock,
            sigma_s: raw_est.sigma_s / halving / clock,
            window_s: raw_est.window_s,
        };
        let bph_detected = crate::bph::bph_from_t_osc(corrected.t_osc_s);
        let (bph_nominal, rate) = self.resolve_mode(bph_detected, corrected.t_osc_s);
        MetricsSnapshot {
            tier: Tier::T1,
            bph_detected,
            bph_nominal,
            rate_s_per_day: rate,
            rate_source: RateSource::PeriodSlope,
            beat_error_ms: None,
            amplitude_deg: None,
            period: corrected,
            quality: Quality {
                detection_ratio: 0.0,
                onset_jitter_ms: None,
                mean_beat_snr_db: None,
                unlocking_ratio: 0.0,
                clipped_samples: self.clipped,
                amplitude_gate: None,
            },
            calibrated: self.config.ppm_correction != 0.0,
        }
    }

    /// M1's BPH-mode honesty rules, unchanged (spec §3.1).
    fn resolve_mode(&self, bph_detected: f64, t_osc_s: f64) -> (Option<u32>, Option<f64>) {
        match self.config.bph_mode {
            BphMode::Free => (None, None),
            BphMode::Auto => match crate::bph::snap_to_table(bph_detected) {
                Some(nom) => (Some(nom), Some(crate::bph::rate_s_per_day(t_osc_s, nom))),
                None => (None, None),
            },
            BphMode::Fixed(nom) => {
                let dev = (bph_detected - nom as f64).abs() / nom as f64;
                let rate = (dev <= 0.03).then(|| crate::bph::rate_s_per_day(t_osc_s, nom));
                (Some(nom), rate)
            }
        }
    }
}
```

Update lib.rs re-exports to the list in Interfaces.

- [ ] **Step 3: Update the M1 analyzer tests mechanically, add the M2 integration tests**

Mechanical renames in the existing `analyzer.rs` tests: helper return type `Option<RateEstimate>` → `Option<MetricsSnapshot>`; every `est.seconds_per_day` → `est.rate_s_per_day`; the two struct-literal configs gain nothing (Default covers the new fields via `..` — the helper builds `AnalyzerConfig` field-by-field, so add `lift_angle_deg: 52.0, averaging_s: 30.0` or switch to `..AnalyzerConfig::default()`). Semantics of all seven M1 tests are unchanged and they must still pass.

Add these tests:

```rust
    #[test]
    fn full_metrics_at_tier3() {
        let cfg = SynthConfig {
            beat_error_ms: 0.8,
            amplitude_deg: 270.0,
            rate_s_per_day: 12.0,
            snr_db: 30.0,
            ..SynthConfig::default()
        };
        let est = analyze(&cfg, BphMode::Auto, 0.0).expect("snapshot");
        assert_eq!(est.tier, crate::tier::Tier::T3);
        assert_eq!(est.rate_source, RateSource::UnlockingRegression);
        let be = est.beat_error_ms.expect("beat error at T3");
        assert!((be - 0.8).abs() <= 0.1, "be {be}");
        let amp = est.amplitude_deg.expect("amplitude at T3");
        assert!((amp - 270.0).abs() <= 5.0, "amp {amp}");
        let rate = est.rate_s_per_day.expect("rate");
        assert!((rate - 12.0).abs() <= 0.3, "rate {rate}");
        assert!(est.quality.detection_ratio > 0.8);
    }

    #[test]
    fn degradation_order_as_snr_falls() {
        let t3 = analyze(
            &SynthConfig { snr_db: 30.0, ..SynthConfig::default() },
            BphMode::Auto,
            0.0,
        )
        .expect("30 dB");
        assert_eq!(t3.tier, crate::tier::Tier::T3);
        let low = analyze(
            &SynthConfig { snr_db: 10.0, ..SynthConfig::default() },
            BphMode::Auto,
            0.0,
        )
        .expect("10 dB");
        assert!(low.tier <= crate::tier::Tier::T2, "tier {:?}", low.tier);
        assert!(low.amplitude_deg.is_none(), "no fabricated amplitude");
        assert!(low.rate_s_per_day.is_some(), "rate survives at low SNR");
    }

    #[test]
    fn octave_error_is_corrected_end_to_end() {
        // toc_gain 0.1 defeats the divisor walk; M1 would have reported 14400.
        let cfg = SynthConfig { toc_gain: 0.1, snr_db: 35.0, ..SynthConfig::default() };
        let est = analyze(&cfg, BphMode::Auto, 0.0).expect("snapshot");
        assert_eq!(est.bph_nominal, Some(28_800), "octave guard failed: {:?}", est.bph_detected);
    }

    #[test]
    fn clip_counter_accumulates() {
        let mut a = Analyzer::new(AnalyzerConfig::default()).unwrap();
        a.push_samples(&vec![1.5f32; 100]);
        a.push_samples(&vec![0.0f32; 48_000]);
        // No beat → None, but the counter lives on the analyzer; push a synth
        // signal and confirm it survives into the snapshot.
        let x = synthesize(&SynthConfig::default()).unwrap();
        a.push_samples(&x);
        let est = a.current().expect("snapshot");
        assert!(est.quality.clipped_samples >= 100);
    }

    #[test]
    fn config_validation_rejects_bad_lift_and_averaging() {
        for cfg in [
            AnalyzerConfig { lift_angle_deg: 5.0, ..AnalyzerConfig::default() },
            AnalyzerConfig { averaging_s: 100.0, ..AnalyzerConfig::default() },
            AnalyzerConfig { lift_angle_deg: f64::NAN, ..AnalyzerConfig::default() },
        ] {
            assert!(matches!(Analyzer::new(cfg), Err(AnalyzerError::InvalidConfig { .. })));
        }
    }
```

- [ ] **Step 4: Minimal CLI compile patch**

In `crates/chrona-cli/src/commands.rs` `run_analyze` only: `est.seconds_per_day` → `est.rate_s_per_day`. (`Analyzer::new(...)?` keeps working — `AnalyzerError` is a std Error, anyhow converts.) Nothing else; Task 11 does the real CLI upgrade.

- [ ] **Step 5: Run the workspace to verify**

Run: `cargo test --workspace`
Expected: everything PASSES — the seven mechanically-updated M1 analyzer tests, the five new ones, dsp modules, and the untouched CLI integration tests (their JSON shape is unchanged by this task). The new integration tests are the slowest in the repo (~5 pipelines); with `[profile.test] opt-level = 2` the full suite stays under a few minutes.

- [ ] **Step 6: Full gates and commit**

```bash
git add crates
git commit -m "feat(dsp): tier engine and MetricsSnapshot analyzer with full-metrics pipeline"
```

---

### Task 11: CLI upgrade — full metrics in `analyze` and `verify`

**Files:**
- Modify: `crates/chrona-cli/src/commands.rs`, `crates/chrona-cli/src/main.rs` (no change needed unless imports), `crates/chrona-cli/tests/roundtrip.rs`, `crates/chrona-cli/tests/verify.rs`

**Interfaces:**
- Consumes: `MetricsSnapshot`/`Quality`/`Tier`/`RateSource` (Task 10)
- Produces: `chrona analyze <wav> [--bph ..] [--ppm ..] [--lift <deg>] [--averaging <s>] [--json]` with the extended report; `chrona verify` honors optional `expect_beat_error_ms`/`tol_beat_error` and `expect_amplitude_deg`/`tol_amplitude` expectation fields

- [ ] **Step 1: Write the failing tests**

Append to `tests/roundtrip.rs`:

```rust
#[test]
fn full_metrics_appear_in_json_at_tier3() {
    let dir = tempfile::tempdir().unwrap();
    let wav = dir.path().join("full.wav");
    chrona()
        .args([
            "synth", wav.to_str().unwrap(),
            "--rate", "12.0", "--beat-error", "0.8", "--amplitude", "270",
        ])
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
    assert_eq!(v["tier"], "T3");
    assert_eq!(v["rate_source"], "unlocking_regression");
    let be = v["beat_error_ms"].as_f64().unwrap();
    assert!((be - 0.8).abs() <= 0.1, "be {be}");
    let amp = v["amplitude_deg"].as_f64().unwrap();
    assert!((amp - 270.0).abs() <= 5.0, "amp {amp}");
    assert!(v["detection_ratio"].as_f64().unwrap() > 0.8);
}

#[test]
fn lift_flag_scales_amplitude_only() {
    let dir = tempfile::tempdir().unwrap();
    let wav = dir.path().join("lift.wav");
    chrona()
        .args(["synth", wav.to_str().unwrap(), "--amplitude", "270"])
        .assert()
        .success();
    let get = |lift: &str| {
        let out = chrona()
            .args(["analyze", wav.to_str().unwrap(), "--lift", lift, "--json"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        serde_json::from_slice::<serde_json::Value>(&out).unwrap()
    };
    let (v52, v26) = (get("52"), get("26"));
    let (a52, a26) = (
        v52["amplitude_deg"].as_f64().unwrap(),
        v26["amplitude_deg"].as_f64().unwrap(),
    );
    // Amplitude scales ~linearly with lift; rate must not move.
    assert!((a26 - a52 / 2.0).abs() < 6.0, "a52 {a52} a26 {a26}");
    assert!(
        (v52["rate_s_per_day"].as_f64().unwrap() - v26["rate_s_per_day"].as_f64().unwrap()).abs()
            < 0.2
    );
}
```

Append to `tests/verify.rs`:

```rust
#[test]
fn verify_checks_beat_error_and_amplitude_when_present() {
    let dir = tempfile::tempdir().unwrap();
    let wav = dir.path().join("m.wav");
    chrona()
        .args([
            "synth", wav.to_str().unwrap(),
            "--rate", "5.0", "--beat-error", "0.8", "--amplitude", "270",
        ])
        .assert()
        .success();
    let good = r#"{"file":"m.wav","bph":"auto","expect_rate_s_per_day":5.0,"tol_rate":1.0,
        "expect_beat_error_ms":0.8,"tol_beat_error":0.15,
        "expect_amplitude_deg":270.0,"tol_amplitude":6.0}"#;
    std::fs::write(dir.path().join("good.json"), good).unwrap();
    chrona().args(["verify", dir.path().to_str().unwrap()]).assert().success();

    let bad = r#"{"file":"m.wav","bph":"auto","expect_rate_s_per_day":5.0,"tol_rate":1.0,
        "expect_amplitude_deg":330.0,"tol_amplitude":5.0}"#;
    std::fs::write(dir.path().join("zbad.json"), bad).unwrap();
    chrona().args(["verify", dir.path().to_str().unwrap()]).assert().code(1);
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p chrona-cli` — new tests FAIL (`tier` etc. absent from JSON; unknown `--lift`; verify fields unknown are silently ignored by serde? — no: `tol_amplitude` absent from the struct is ignored, but the *assertion* on exit code fails because nothing checks amplitude)

- [ ] **Step 3: Implement in commands.rs**

`AnalyzeArgs` gains:

```rust
    /// Lift angle in degrees (amplitude only; spec §2.3). Default 52.
    #[arg(long, default_value_t = 52.0)]
    pub lift: f64,
    /// Metrics averaging window, seconds (2-60).
    #[arg(long, default_value_t = 30.0)]
    pub averaging: f64,
```

`AnalyzeReport` gains (all `#[serde(skip_serializing_if = "Option::is_none")]` except `clipped_samples`):

```rust
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tier: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rate_source: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub beat_error_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub amplitude_deg: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub amplitude_gate: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detection_ratio: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub onset_jitter_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unlocking_ratio: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mean_beat_snr_db: Option<f64>,
    pub clipped_samples: u64,
```

`run_analyze` builds the config with `lift_angle_deg: a.lift, averaging_s: a.averaging` and maps the snapshot:

```rust
        Some(est) => AnalyzeReport {
            bph_detected: Some(est.bph_detected),
            bph_nominal: est.bph_nominal,
            rate_s_per_day: est.rate_s_per_day,
            period_sigma_s: Some(est.period.sigma_s),
            window_s: Some(est.period.window_s),
            calibrated: est.calibrated,
            tier: Some(match est.tier {
                chrona_dsp::Tier::T1 => "T1",
                chrona_dsp::Tier::T2 => "T2",
                chrona_dsp::Tier::T3 => "T3",
            }),
            rate_source: Some(match est.rate_source {
                chrona_dsp::RateSource::PeriodSlope => "period_slope",
                chrona_dsp::RateSource::UnlockingRegression => "unlocking_regression",
            }),
            beat_error_ms: est.beat_error_ms,
            amplitude_deg: est.amplitude_deg,
            amplitude_gate: est.quality.amplitude_gate.map(|g| format!("{g:?}")),
            detection_ratio: Some(est.quality.detection_ratio),
            onset_jitter_ms: est.quality.onset_jitter_ms,
            unlocking_ratio: Some(est.quality.unlocking_ratio),
            mean_beat_snr_db: est.quality.mean_beat_snr_db,
            clipped_samples: est.quality.clipped_samples,
            ..base("ok")
        },
```

(`base` initializes the new Option fields to `None` and `clipped_samples: 0`.) `print_human` appends after the rate line:

```rust
            if let Some(t) = r.tier {
                println!("Signal tier: {t}");
            }
            match r.beat_error_ms {
                Some(be) => println!("Beat error: {be:.1} ms"),
                None => println!("Beat error: — (needs Tier 2: ≥60% of beats detected with ≤0.5 ms jitter)"),
            }
            match r.amplitude_deg {
                Some(a) => println!("Amplitude: {a:.0}°"),
                None => match &r.amplitude_gate {
                    Some(g) => println!("Amplitude: — (gate: {g})"),
                    None => println!("Amplitude: — (needs Tier 3: unlocking pulse resolved — try a contact mic)"),
                },
            }
            if r.clipped_samples > 0 {
                println!("WARNING: {} clipped samples — reduce input gain", r.clipped_samples);
            }
```

`verify`'s `Expectation` gains four optional fields (`expect_beat_error_ms`, `tol_beat_error`, `expect_amplitude_deg`, `tol_amplitude`, all `Option<f64>`, `#[serde(default)]`) and `run_verify` extends the verdict: after the rate check, for each of the two optional pairs where `expect_*` is `Some`, compare against the report's value (`None` measured value counts as a failure with reason "no <metric> (tier {tier})"); the printed line gains ` be=<..>` / ` amp=<..>` segments for whatever was checked. `run_verify` passes `lift: 52.0, averaging: 30.0` defaults into `AnalyzeArgs` (add the two fields to its construction).

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p chrona-cli`
Expected: all CLI tests PASS (old ones untouched — the JSON only gained fields)

- [ ] **Step 5: Full gates and commit**

```bash
git add crates/chrona-cli
git commit -m "feat(cli): tiered full-metrics output and corpus expectations for BE/amplitude"
```

---

### Task 12: Quartz calibration math + `chrona calibrate` (`cal.rs`)

**Files:**
- Create: `crates/chrona-dsp/src/cal.rs`, `crates/chrona-cli/tests/calibrate.rs`
- Modify: `crates/chrona-dsp/src/lib.rs` (`pub mod cal;`), `crates/chrona-cli/src/commands.rs`, `crates/chrona-cli/src/main.rs`

**Interfaces:**
- Consumes: `filter`, `envelope`, `synth::synthesize_quartz` (tests)
- Produces:
  - `pub enum CalError` (thiserror): `TooShort { seconds: f64, required: f64 }`, `NoTicks`, `Unstable { residual_ppm: f64 }`, `BadInput { reason: String }`
  - `pub struct QuartzCalResult { pub ppm: f64, pub residual_ppm: f64, pub events: usize, pub duration_s: f64 }` (derives `Debug, Clone, Copy`)
  - `pub fn calibrate_quartz(samples: &[f32], sample_rate_hz: f64) -> Result<QuartzCalResult, CalError>` — spec §3.4: detect the 1 Hz stepper tick, regress tick timestamps against the integer grid, `ppm = (T̂_seconds − 1.0)·10⁶`; the sign convention makes the result directly usable as `--ppm` (a +N ppm-fast ADC yields +N, and the analyzer divides by `1 + N/10⁶`). Requires ≥ 300 s; `Unstable` when the slope's standard error exceeds 2 ppm
  - CLI: `chrona calibrate <wav> [--json]` — exit 0 on success (prints ppm, residual, and "pass `--ppm <value>` to analyze"), exit 2 on `CalError` (measurement insufficiency), exit 1 on I/O errors

- [ ] **Step 1: Write the failing dsp tests**

`crates/chrona-dsp/src/cal.rs`:

```rust
//! Quartz-reference timebase calibration (spec §3.4): recover the audio
//! clock's ppm error from a recording of any quartz watch's 1 Hz tick.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synth::synthesize_quartz;

    #[test]
    fn recovers_injected_ppm_within_half_ppm() {
        for ppm_true in [-80.0f64, 0.0, 50.0] {
            let x = synthesize_quartz(320.0, 48_000.0, ppm_true, 30.0, 5).unwrap();
            let r = calibrate_quartz(&x, 48_000.0).unwrap_or_else(|e| panic!("{ppm_true}: {e}"));
            assert!((r.ppm - ppm_true).abs() <= 0.5, "ppm {ppm_true}: got {}", r.ppm);
            assert!(r.residual_ppm < 1.0, "residual {}", r.residual_ppm);
            assert!(r.events >= 250, "events {}", r.events);
        }
    }

    #[test]
    fn short_recording_is_rejected() {
        let x = synthesize_quartz(60.0, 48_000.0, 0.0, 30.0, 1).unwrap();
        assert!(matches!(calibrate_quartz(&x, 48_000.0), Err(CalError::TooShort { .. })));
    }

    #[test]
    fn noise_only_yields_no_ticks() {
        let mut rng = crate::synth::Rng::new(4);
        let x: Vec<f32> = (0..(48_000.0 * 320.0) as usize).map(|_| 0.02 * rng.next_f32()).collect();
        assert!(matches!(calibrate_quartz(&x, 48_000.0), Err(CalError::NoTicks)));
    }
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p chrona-dsp cal` — FAIL (module missing)

- [ ] **Step 3: Implement**

Above the tests in `cal.rs`:

```rust
use crate::envelope::EnvelopeExtractor;
use crate::filter::{Butterworth, DcBlocker};

const MIN_SECONDS: f64 = 300.0;
const MAX_RESIDUAL_PPM: f64 = 2.0;

#[derive(Debug, thiserror::Error)]
pub enum CalError {
    #[error("recording too short: {seconds:.0} s (need at least {required:.0} s of quartz tick)")]
    TooShort { seconds: f64, required: f64 },
    #[error("no 1 Hz quartz tick found — clamp a quartz watch to the pickup and re-record")]
    NoTicks,
    #[error("calibration fit unstable ({residual_ppm:.2} ppm residual) — re-record in a quieter setup")]
    Unstable { residual_ppm: f64 },
    #[error("bad input: {reason}")]
    BadInput { reason: String },
}

#[derive(Debug, Clone, Copy)]
pub struct QuartzCalResult {
    /// Audio-clock error in ppm; pass directly as the analyzer's ppm correction.
    pub ppm: f64,
    /// Standard error of the fit, ppm.
    pub residual_ppm: f64,
    pub events: usize,
    pub duration_s: f64,
}

fn parabolic3(a: f64, b: f64, c: f64) -> f64 {
    let denom = a - 2.0 * b + c;
    if denom.abs() < 1e-20 { 0.0 } else { (0.5 * (a - c) / denom).clamp(-0.5, 0.5) }
}

/// Spec §3.4 quartz calibration: envelope → gated 1 Hz tick tracking →
/// least-squares slope of tick times vs the integer-second grid.
pub fn calibrate_quartz(samples: &[f32], sample_rate_hz: f64) -> Result<QuartzCalResult, CalError> {
    if !(sample_rate_hz.is_finite() && sample_rate_hz > 0.0) {
        return Err(CalError::BadInput { reason: format!("sample rate {sample_rate_hz}") });
    }
    let duration_s = samples.len() as f64 / sample_rate_hz;
    if duration_s < MIN_SECONDS {
        return Err(CalError::TooShort { seconds: duration_s, required: MIN_SECONDS });
    }

    // Precondition + envelope (same chain as the analyzer).
    let mut dc = DcBlocker::new();
    let mut hp = Butterworth::high_pass(sample_rate_hz, 3_000.0)
        .map_err(|e| CalError::BadInput { reason: e.to_string() })?;
    let mut envx = EnvelopeExtractor::new(sample_rate_hz)
        .map_err(|e| CalError::BadInput { reason: e.to_string() })?;
    let filtered: Vec<f32> = samples.iter().map(|&s| hp.process(dc.process(s))).collect();
    let mut env = Vec::new();
    envx.process(&filtered, &mut env);
    let env_rate = envx.envelope_rate_hz();

    // Noise floor and detectability.
    let mut sorted = env.clone();
    sorted.sort_by(f32::total_cmp);
    let floor = sorted[sorted.len() / 2].max(1e-9);
    let thr = 5.0 * floor;

    // Seed: strongest sample in the first 2 s must clear the threshold.
    let head = &env[..((2.0 * env_rate) as usize).min(env.len())];
    let (t0_idx, &t0_v) = head
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .ok_or(CalError::NoTicks)?;
    if t0_v < thr {
        return Err(CalError::NoTicks);
    }

    // Gated tracking: predict each next tick at +T̂, search ±50 ms.
    let gate = (0.05 * env_rate) as usize;
    let mut t_hat = env_rate; // one nominal second, in envelope samples
    let mut ticks: Vec<(f64, f64)> = vec![(0.0, t0_idx as f64)];
    let mut k = 1.0f64;
    loop {
        let pred = t0_idx as f64 + k * t_hat;
        let center = pred.round() as i64;
        let lo = (center - gate as i64).max(1) as usize;
        let hi = ((center + gate as i64) as usize).min(env.len().saturating_sub(2));
        if lo + 2 >= hi {
            break;
        }
        let pk = (lo..hi).max_by(|&i, &j| env[i].total_cmp(&env[j])).unwrap_or(lo);
        if env[pk] >= thr {
            let frac =
                pk as f64 + parabolic3(env[pk - 1] as f64, env[pk] as f64, env[pk + 1] as f64);
            ticks.push((k, frac));
            t_hat = (frac - t0_idx as f64) / k; // running period refinement
        }
        k += 1.0;
        if k > duration_s + 2.0 {
            break;
        }
    }

    let expected = (duration_s - 2.0).max(1.0);
    if (ticks.len() as f64) < 0.8 * expected {
        return Err(CalError::NoTicks);
    }

    // LS: idx = a + T·k  (T in envelope samples per true second of the quartz).
    let n = ticks.len() as f64;
    let (mut sk, mut st, mut sk2, mut skt) = (0.0f64, 0.0, 0.0, 0.0);
    for &(k, t) in &ticks {
        sk += k;
        st += t;
        sk2 += k * k;
        skt += k * t;
    }
    let denom = n * sk2 - sk * sk;
    if denom.abs() < 1e-9 {
        return Err(CalError::NoTicks);
    }
    let t_slope = (n * skt - sk * st) / denom;
    let a = (st - t_slope * sk) / n;
    let ssr: f64 = ticks.iter().map(|&(k, t)| (t - (a + t_slope * k)).powi(2)).sum();
    let sigma_slope = (ssr / (n - 2.0)).sqrt() / (sk2 - sk * sk / n).sqrt();

    let ppm = (t_slope / env_rate - 1.0) * 1e6;
    let residual_ppm = sigma_slope / env_rate * 1e6;
    if residual_ppm > MAX_RESIDUAL_PPM {
        return Err(CalError::Unstable { residual_ppm });
    }
    Ok(QuartzCalResult { ppm, residual_ppm, events: ticks.len(), duration_s })
}
```

Add `pub mod cal;` to lib.rs. Run: `cargo test -p chrona-dsp cal` → 3 tests PASS (the sweep takes ~tens of seconds; opt-level 2 applies).

- [ ] **Step 4: CLI subcommand + integration test**

`commands.rs`:

```rust
#[derive(Args)]
pub struct CalibrateArgs {
    /// WAV recording of a quartz watch (≥ 5 minutes)
    pub file: PathBuf,
    #[arg(long)]
    pub json: bool,
}

#[derive(Serialize)]
pub struct CalReport {
    pub ppm: f64,
    pub residual_ppm: f64,
    pub events: usize,
    pub duration_s: f64,
}

pub fn run_calibrate(a: &CalibrateArgs) -> anyhow::Result<CalReport> {
    let (samples, sr) = crate::wav::read_mono(&a.file)?;
    let r = chrona_dsp::cal::calibrate_quartz(&samples, sr)?;
    Ok(CalReport { ppm: r.ppm, residual_ppm: r.residual_ppm, events: r.events, duration_s: r.duration_s })
}

pub fn print_cal_human(r: &CalReport) {
    println!(
        "Timebase: {:+.1} ppm (±{:.2}) from {} ticks over {:.0} s",
        r.ppm, r.residual_ppm, r.events, r.duration_s
    );
    println!("Use it: chrona analyze <watch.wav> --ppm {:.1}", r.ppm);
}
```

`main.rs`: add `/// Measure the audio clock's ppm error from a quartz-watch recording` + `Calibrate(commands::CalibrateArgs),` and the arm:

```rust
        Cmd::Calibrate(a) => match commands::run_calibrate(&a) {
            Ok(report) => {
                if a.json {
                    println!("{}", serde_json::to_string_pretty(&report)?);
                } else {
                    commands::print_cal_human(&report);
                }
                Ok(0)
            }
            Err(e) if e.downcast_ref::<chrona_dsp::cal::CalError>().is_some() => {
                eprintln!("calibration failed: {e:#}");
                Ok(2)
            }
            Err(e) => Err(e),
        },
```

`tests/calibrate.rs` (writes the quartz WAV with `hound` directly — integration tests can use the package's dependencies but not the binary's private modules):

```rust
use assert_cmd::Command;

fn chrona() -> Command {
    Command::cargo_bin("chrona").expect("binary builds")
}

fn write_f32_wav(path: &std::path::Path, samples: &[f32], sr: u32) {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: sr,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut w = hound::WavWriter::create(path, spec).unwrap();
    for &s in samples {
        w.write_sample(s).unwrap();
    }
    w.finalize().unwrap();
}

#[test]
fn calibrate_recovers_ppm_and_reports_usage() {
    let dir = tempfile::tempdir().unwrap();
    let wav = dir.path().join("quartz.wav");
    let x = chrona_dsp::synth::synthesize_quartz(320.0, 48_000.0, 42.0, 30.0, 7).unwrap();
    write_f32_wav(&wav, &x, 48_000);
    let out = chrona()
        .args(["calibrate", wav.to_str().unwrap(), "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
    let ppm = v["ppm"].as_f64().unwrap();
    assert!((ppm - 42.0).abs() <= 0.5, "ppm {ppm}");
}

#[test]
fn calibrate_short_recording_exits_2() {
    let dir = tempfile::tempdir().unwrap();
    let wav = dir.path().join("short.wav");
    let x = chrona_dsp::synth::synthesize_quartz(30.0, 48_000.0, 0.0, 30.0, 1).unwrap();
    write_f32_wav(&wav, &x, 48_000);
    chrona().args(["calibrate", wav.to_str().unwrap()]).assert().code(2);
}
```

- [ ] **Step 5: Run tests, full gates, commit**

Run: `cargo test --workspace` then the fmt/clippy gates.

```bash
git add crates
git commit -m "feat(dsp+cli): quartz-reference timebase calibration"
```

---

### Task 13: Corpus onboarding docs + README refresh

**Files:**
- Modify: `fixtures/README.md`, `README.md`

**Interfaces:** none (docs). Real recordings are **user-supplied** — this task ships the instructions and schema so the corpus can grow; it does not block on recordings existing.

- [ ] **Step 1: Rewrite `fixtures/README.md`**

```markdown
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
  expectation and note machine + lift-angle setting in the JSON (any extra keys
  are ignored by the runner).
- A quartz-watch recording (≥ 5 min) doubles as the calibration reference:
  `chrona calibrate quartz.wav`, then record the mechanical fixtures and put the
  measured `ppm` in their expectations.

## Expectation schema (one JSON per recording; `file` relative to this dir)

```json
{
  "file": "eta2824_dial_up_contact.wav",
  "bph": "auto",
  "ppm": 12.5,
  "expect_rate_s_per_day": 7.2,  "tol_rate": 1.0,
  "expect_beat_error_ms": 0.3,   "tol_beat_error": 0.15,
  "expect_amplitude_deg": 285.0, "tol_amplitude": 10.0
}
```

`bph` takes auto | free | a number. The beat-error and amplitude pairs are
optional — omit them for Tier-1 (weak-mic) fixtures. Synthetic cases are not
committed; `chrona synth` regenerates them (see `crates/chrona-cli/tests/`).
```

- [ ] **Step 2: Update the root `README.md`**

Replace the Status line with `Status: M2 (full metrics — rate, beat error, amplitude, tiers, calibration). Design: docs/superpowers/specs/2026-08-20-chrona-timegrapher-design.md.` and extend the Try-it block with:

```sh
# Full metrics on a synthetic watch with beat error:
cargo run -p chrona-cli -- synth demo.wav --rate 12 --beat-error 0.8 --amplitude 270
cargo run -p chrona-cli -- analyze demo.wav --lift 52

# Calibrate your sound card against any quartz watch (record ≥ 5 minutes):
cargo run -p chrona-cli -- calibrate quartz.wav
```

and after the calibration paragraph add: `Metrics are tiered by signal quality (spec §3.1): Tier 1 = rate only, Tier 2 adds beat error, Tier 3 adds amplitude — weak signals show "—" with the reason instead of fabricated numbers.`

- [ ] **Step 3: Full gates and commit**

Run: `cargo test --workspace` (docs-only change; suite must still be green)

```bash
git add fixtures/README.md README.md
git commit -m "docs: corpus recording guide with extended expectations; M2 README"
```

---

## Self-review notes (already applied)

1. **Spec coverage (M2 = spec §12.2):** stage 4 → Tasks 4–5; stage 5 → Tasks 6–7; stage 6 → Tasks 8–9; stage 7 tier engine → Task 10; calibration math → Task 12; "corpus tolerances green" → the property-test bars (Tasks 8–10) run in CI on synthetic ground truth, and Task 13 ships the real-corpus onboarding — real recordings are user-supplied and explicitly non-blocking. The AGC-score tier input is deferred to M4 with Mic Doctor (spec §3.2), recorded in the Decisions section.
2. **Known duplications (deliberate, documented at both sites):** `extract_events` recomputes `fold_envelope`'s window arithmetic (`cycles`/`fold_start`) — kept in sync by paired comments; `parabolic3` exists in `autocorr.rs` (private), `events.rs`, and `cal.rs` — three private 6-line copies beat a public helper that widens the autocorr API mid-milestone; `pipeline_to_events` is duplicated between events/metrics test modules per this plan's standalone-tests rule.
3. **Type consistency check:** `BeatEvent`/`Parity` (T6→T7→T8→T9→T10), `RegressionResult` fields (T8→T10), `AmplitudeGateFail`/`AmplitudeResult` + the `PartialEq` note (T9→T10→T11), `Tier`/`Quality`/`MetricsSnapshot`/`RateSource`/`AnalyzerError` (T10→T11→T12), `SynthConfig.toc_gain`/`synthesize_quartz` (T3→T5, T12), `SampleRing` methods (T4→T10), `fold_with_octave_guard` returning `Option<(FoldProfile, bool)>` (T5→T10) — names and signatures match across tasks.
4. **Breaking-change containment:** Task 10 deletes `RateEstimate` and changes `Analyzer::new`'s error type; the same task patches the CLI's two affected lines so the workspace stays green; Task 11 does the real CLI upgrade. No task leaves the tree red.
5. **Runtime budget:** the heaviest additions are the calibration sweep (3 × 320 s synthesis) and the analyzer integration tests (~5 pipelines); with the existing `[profile.test] opt-level = 2` the full workspace suite stays in the minutes range. If CI time becomes a problem, shorten the calibration sweep to two ppm values — never loosen its ±0.5 ppm bar.





