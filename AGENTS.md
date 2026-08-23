# Chrona — instructions for coding agents

## Authority chain

- The **binding design document** is
  `docs/superpowers/specs/2026-08-20-chrona-timegrapher-design.md`. It has been amended
  during M1–M3 (see its changelog). When code and spec disagree, the spec wins; when a
  plan and the spec disagree, the spec wins. Amend the spec (with a changelog line)
  rather than silently diverging.
- Implementation plans live in `docs/superpowers/plans/`; deferred debt in
  `docs/superpowers/notes/m{1,2,3}-followups.md`. **M4 planning starts from
  `m3-followups.md`** (which also carries the still-open M2 items). First ticket in
  line: `parabolic3` unification — deferred four times already.
- `docs/manual-testing.md` is the **human-only acceptance protocol** (live mic, macOS
  permission flow, hot-unplug). Never weaken it to make automation pass; agents cannot
  hear a watch.

## Gates — every change passes all of these

```sh
cargo test --workspace                            # 152+ tests, 0 failures expected
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p chrona-dsp --release -- --ignored   # when touching chrona-dsp: stress matrix + perf bar
cargo run -p chrona-app -- --simulate --rate 12 --beat-error 0.8 --amplitude 270 --headless-seconds 45
# must print: TIER 3 · full  rate +12.0 s/d ⚠ uncal  beat error 0.8 ms  amplitude 270°  (exit 0)
```

Toolchain is **stable, pinned by `rust-toolchain.toml`** (edition 2024). Never install
nightly as a fix and never change the global rustup default.

## Frozen decisions — do not re-litigate

These were settled by proofs or stress tests; changing them requires new evidence of the
same strength, recorded in the spec:

- Period-estimator tolerances **0.03 (seed) / 0.005 (walk)** — do not widen, ever, and
  especially not with "distance" arguments.
- Octave-guard probe constants (1.8× floor, ±NBINS/16 window, 0.5 cluster factor) —
  frozen-and-proven by the 64-case stress matrix (the `--ignored` release tests). The
  octave backstop is a **plain `>=`** (a 1.15× margin vetoed real period halvings).
- Envelope low-pass cutoff `min(1500, 0.45 · sr/16)`.
- Event edge timing runs on the **native-rate** envelope at half-height. The decimated
  envelope is a resolution ceiling — do not "simplify" back to it; the ±5° amplitude bar
  is unreachable there.
- Tape normalization is fixed-denominator, anchored-at-newest (pin-tested).
- Engine ingress clamps: lift 10–90°, ppm finite ±500. TOML has literal `inf`/`nan` and
  `f64::clamp` **propagates NaN** — always gate `is_finite()` first on hand-editable
  ingress (config file, CLI); JSON sidecars cannot carry NaN.

## Honesty rule (spec §3.1)

Metrics below the current signal tier are **never fabricated**: show `—` plus the gate
reason. `rate_source` is `Some` iff a rate exists. Do not add fallbacks that guess a
number when the pipeline declined to produce one — a wrong-but-confident display is the
project's defining anti-goal.

## Threading rules (spec §4)

- The cpal audio callback does **no allocation, no locks, no I/O** — pre-reserved
  scratch, ring writes, atomics only.
- Only the engine (DSP) thread calls `Analyzer::current_metrics` — never the UI thread
  (a refold costs ~90 ms). The UI reads `Engine::snapshot()` (triple buffer) only.
- Refolds and analyzer rebuilds happen on the engine thread; UI sends `ControlMsg`s.

## Test discipline

- The suite is **hardware-free**: never open a real microphone stream in `cargo test`
  (it fires the macOS permission prompt mid-suite). Mic behavior is covered by the
  manual protocol; if you need it testable, build an injectable device-enumeration seam
  first.
- Presenter output strings are byte-exact pinned (e.g. `"+12.3 s/d"`, `"⚠ uncal"`,
  `"TIER 3 · full"`, and the signal-meter labels `"No signal"`, `"Weak · rate only"`,
  `"Fair · rate + beat error"`, `"Strong · full regression"`). Change the format ⇒
  change the pins deliberately.
- Tests live in in-file `#[cfg(test)]` modules. Sweep tests collect failures then
  assert, rather than asserting inside the loop (an inline assert once masked a real
  330° amplitude failure).
- When reviewing: **re-run the gates yourself**; do not trust pasted output. When
  reporting: paste raw command output; if you didn't capture it, say so — never
  reconstruct it.

## Licensing

MIT OR Apache-2.0. Never copy from GPL timegrapher projects (e.g. `tg`); algorithms are
reimplemented from the published literature cited in the spec.

## Style

Match the codebase: thorough doc comments that explain *constraints* (why a bound holds,
what an invariant protects), not narration of the next line. Small focused commits with
imperative messages (`feat(dsp): …`, `fix(app): …`, `docs: …`).
