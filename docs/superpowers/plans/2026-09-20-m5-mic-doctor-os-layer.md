# M5 Mic Doctor OS Layer Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Land the binding spec's §3.2 steps 4–5 / §3.3 OS layer macOS-first: platform trait + CoreAudio implementation, headroom tracking, volume watch, explicit gain control, Bluetooth refusal, Doctor Levels section, first-launch auto-open — with honest Unsupported stubs for Windows/Linux.

**Architecture:** all OS FFI confined to one chrona-audio module behind the `InputControl` API; the engine consumes it (watch/suppression/refusal logic headless-testable via a fake control under the established `test-fault-injection` feature); the Doctor panel grows a Levels section; chrona-dsp's `doctor_score` gains two Option inputs. M1–M4 surfaces regression-frozen.

**Tech Stack:** unchanged + `coreaudio-sys` (macOS target-gated; expected transitive via cpal already — Task 2 verifies).

**Spec:** docs/superpowers/specs/2026-09-20-m5-mic-doctor-os-layer-design.md over the 2026-08-20 binding spec §3.2–3.3. M4 Doctor conventions carry over.

## Global Constraints

- Gates for EVERY task: `cargo test --workspace` (0 failures), `cargo fmt --all` then `-- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, frozen headless pin `cargo run -p chrona-app -- --simulate --rate 12 --beat-error 0.8 --amplitude 270 --headless-seconds 45` → byte-exact `TIER 3 · full  rate +12.0 s/d ⚠ uncal  beat error 0.8 ms  amplitude 270°` exit 0. Tasks touching `chrona-dsp` add `cargo test -p chrona-dsp --release -- --ignored` (3 pass). App-touching tasks add `cargo build -p chrona-app` + a bounded launch check.
- M4's pinned strings (advice list, signal labels) stay byte-frozen; the synth golden checksum stays frozen; honesty rules (§3.1, "not run", never fabricate) bind every new surface.
- All unsafe code lives in the one macOS FFI module with SAFETY comments — zero unsafe elsewhere (tests' pre-existing env-var blocks excepted).
- No real-mic streams or REAL OS-volume writes in `cargo test` (the fake control covers logic; CoreAudio is compile + manual-protocol verified).
- All UI colors via `theme::Palette`. Timing in tests: POLL for conditions, never assume machine-speed constants (the M4 CI lesson).

---

### Task 1: `os_control` types, stubs, and the fake test control (chrona-audio)

**Files:** Create: `crates/chrona-audio/src/os_control.rs`. Modify: `crates/chrona-audio/src/lib.rs` (mod + re-exports), `crates/chrona-audio/Cargo.toml` (nothing new yet — the feature exists).

**Interfaces (produced; consumed by T2/T4/T6):**
```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Transport { BuiltIn, Usb, Bluetooth, Other(String), Unknown }
#[derive(Debug, thiserror::Error)]
pub enum OsAudioError { #[error("no writable volume control")] NoControl,
    #[error("os call failed: {0}")] Os(String) }
pub struct VolumeWatch { /* guard; Drop unregisters */ }
pub struct InputControl { inner: ControlImpl }
impl InputControl {
    pub fn open(device: &DeviceInfo) -> Option<InputControl>; // None on stub OSes
    pub fn volume(&self) -> Option<f32>;
    pub fn set_volume(&self, v: f32) -> Result<(), OsAudioError>; // clamps 0..=1
    pub fn transport(&self) -> Transport;
    pub fn watch_volume(&self, flag: std::sync::Arc<std::sync::atomic::AtomicBool>)
        -> Option<VolumeWatch>;
}
/// Name-heuristic Bluetooth HINT for stub OSes (never authoritative): true iff
/// the lowercased name contains "bluetooth", "airpods", or "hands-free".
pub fn name_suggests_bluetooth(name: &str) -> bool;
// Under feature "test-fault-injection" (mirror chrona-session's precedent):
pub struct FakeInputControl { /* settable volume/transport; trip_watch() sets
    every registered flag; construction via FakeInputControl::install(...) or an
    InputControl::fake(...) constructor — pick the shape that lets the ENGINE
    hold a plain InputControl either way (an enum ControlImpl { Mac(..),
    Fake(..) } is the intended internal shape; document it) */ }
```
This task: the enum shell, the stub arm (open→None on non-mac when no fake), the fake arm, `name_suggests_bluetooth`, and unit tests: fake volume round-trip + clamp, trip_watch sets the flag exactly once per trip, watch guard Drop unregisters (second trip after drop doesn't set), heuristic table (["AirPods Pro", "BT Hands-Free"]→true, ["MacBook Pro Microphone", "USB Audio"]→false). The macOS arm lands in Task 2 — this task's `open` on macOS ALSO returns None with a `// Task 2` marker so the crate compiles green on every OS now.

- [ ] Step 1: failing tests (module absent — compile RED, say so). Step 2: implement. Step 3: gates. Step 4: Commit `feat(audio): os_control types, stubs, and fake test control`

---

### Task 2: macOS CoreAudio implementation

**Files:** Modify: `crates/chrona-audio/src/os_control.rs` (the Mac arm), `crates/chrona-audio/Cargo.toml` (target-gated dep).

- [ ] **Step 1: verify the dependency claim** — `cargo tree -p chrona-audio -e no-dev | grep -i coreaudio`: if `coreaudio-sys` is already transitive (expected via cpal→coreaudio-rs), add the explicit target-gated dep pinned to the SAME version (report both); if not, add it and report the resolved tree delta.
- [ ] **Step 2: implement** `ControlImpl::Mac`: resolve the `AudioObjectID` from `DeviceInfo` by matching the device UID/name against `kAudioHardwarePropertyDevices` (read how `devices.rs` builds `DeviceInfo.id` FIRST and match on the same identity); `volume`/`set_volume` via `kAudioDevicePropertyVolumeScalar` (input scope, master element; fall back per-channel average/set-both when master is absent — document); `transport` via `kAudioDevicePropertyTransportType` (map Bluetooth + BluetoothLE → Bluetooth, BuiltIn, USB, else Other(name)); `watch_volume` via `AudioObjectAddPropertyListener` with a C callback that ONLY stores into the `Arc<AtomicBool>` (SAFETY comments: callback lifetime = the leaked-box-freed-on-Drop pattern or a registry — document the chosen ownership carefully; the guard's Drop removes the listener AND frees). ALL unsafe here, SAFETY on every block.
- [ ] **Step 3: tests** — what CAN run headless in CI without touching real devices: `open` on a fabricated nonexistent DeviceInfo returns None without panicking (safe on any machine); everything else is compile + manual protocol (STATE THIS in the report; do NOT write tests that depend on this machine's specific devices). Run the full gates + `cargo build` for the crate on this Mac.
- [ ] Step 4: Commit `feat(audio): CoreAudio input volume/transport/watch implementation`

---

### Task 3: Engine peak tracking (`peak_dbfs`)

**Files:** Modify: `crates/chrona-app/src/engine.rs`, `crates/chrona-app/tests/engine.rs`.

**Interfaces:** `HealthView` gains `pub peak_dbfs: Option<f64>` — max-abs over the samples delivered since the LAST publish, as dBFS (`20·log10(peak)`, clamp ≥ −120), None when the interval delivered zero samples (Idle, stalled mic). Reset per publish. Computed in `pull_tick`'s arms from the same chunks it already tees (running `f32::max` — no extra pass allocation).

- [ ] Step 1 (failing tests): turbo Simulate → snapshot's `peak_dbfs` within ±0.2 of `20·log10(0.9)` ≈ −0.915 (the synth's normalization pin — cite synth.rs); Idle engine (total-startup-failure path from M4-T3's test) → None. Step 2: implement. Step 3: gates. Step 4: Commit `feat(app): per-interval peak dBFS in health`

---

### Task 4: Engine volume watch, SetInputVolume, Bluetooth refusal

**Files:** Modify: `crates/chrona-app/src/engine.rs`, `crates/chrona-app/tests/engine.rs`, `crates/chrona-app/Cargo.toml` (dev-dep feature already set for chrona-session; ADD the chrona-audio feature to dev-deps the same way).

**Interfaces:** `ControlMsg::SetInputVolume(f32)`. Engine mic-source state gains the `InputControl` (opened at build; `None` is fine and disables the whole feature), the watch flag, and a one-shot self-set suppression counter. Banner texts (pinned): external change → Warn `"input volume changed outside Chrona (now {pct}%)"` (pct = volume()·100 rounded; when volume() is None after the trip: `"input volume changed outside Chrona"`); set failure → Warn `"could not set input volume: {e}"`; Bluetooth refusal → Error `"Bluetooth input refused: Bluetooth voice codecs (SCO/HFP) destroy tick timing — use a wired or USB microphone"` and the switch DOES NOT happen (previous source keeps running — reuse the SwitchSource Err-arm shape).

**Refusal placement:** in the `SwitchSource(Mic)` arm BEFORE `SourceRuntime::build`: `InputControl::open(target).map(|c| c.transport())` — `Some(Bluetooth)` refuses. Initial-startup Mic build: same check; refusal there falls into the existing idle-survival path with the same Error banner. Stub OSes: `open` is None ⇒ no refusal (the combo hint is Task 6's job).

- [ ] Step 1 (failing tests, all via the FAKE control — wire the feature so app tests can construct it; how the engine gets a fake instead of the real open: a test-feature-gated injection point mirroring `CHRONA_TEST_FAIL_PUSH_AFTER`'s env-var pattern — e.g. `CHRONA_TEST_FAKE_INPUT_CONTROL=volume:0.5,transport:bluetooth` parsed in a feature-gated branch of the engine's open call; document): (a) trip the fake's watch → exactly ONE Warn banner with the pct, then re-armed (a second trip banners again); (b) `SetInputVolume(0.8)` → fake's volume becomes 0.8 AND no self-triggered banner (suppression); (c) SwitchSource to a fake-Bluetooth mic → the exact Error text, `source_kind` unchanged, engine still live (follow with a Simulate switch reaching T3); (d) SetInputVolume with the fake set to fail → the Warn text. Step 2: implement. Step 3: gates (engine file 3×). Step 4: Commit `feat(app): volume watch, explicit gain control, Bluetooth refusal`

---

### Task 5: `doctor_score` OS inputs (chrona-dsp)

**Files:** Modify: `crates/chrona-dsp/src/doctor.rs`.

**Interfaces:** `doctor_score(silence: Option<&SilenceReport>, tick: Option<&TickReport>, agc: Option<&AgcReport>, volume_moved: Option<bool>, peak_dbfs: Option<f64>) -> DoctorScore`. New deductions (constants pinned): `volume_moved == Some(true)` → −10, advice `"another app is adjusting your input gain — close it or disable auto-gain"`; `peak_dbfs == Some(p)` outside −12…−6 by MORE than 3 dB (p > −3.0 || p < −15.0) → −10, advice `"set input gain so ticks peak between −12 and −6 dBFS"`. Both are CAUSE deductions (rank among causes by size, ties stable); the M4 strings/deductions/symmetry (tier-last earbuds) are byte-frozen — every existing doctor test must pass with only the signature-site updates (None, None at old call sites).

- [ ] Step 1 (failing tests): signature RED at call sites; new table tests: volume_moved Some(true) alone → 90 + that advice; peak −16 → 90 + the peak advice; peak −14 (inside the 3 dB grace) → no deduction; peak −2.9 → deduction; Some(false)/None → nothing; combined with gate: gate advice still first, earbuds still last. Step 2: implement (update the M4 tests' call sites with `None, None`). Step 3: gates incl. release net. Step 4: Commit `feat(dsp): doctor score gains volume-moved and headroom inputs`

---

### Task 6: Doctor Levels UI + combo Bluetooth tag

**Files:** Modify: `crates/chrona-app/src/ui/doctor.rs`, `crates/chrona-app/src/ui/controls.rs` (device combo tag), `crates/chrona-app/src/app.rs` (plumb peak/volume state into the panel ctx).

- Levels section (Mic source only): headroom meter — pure fn `headroom_meter_frac(peak_dbfs) -> f64` mapping −60…0 dBFS to 0…1 with the target zone −12…−6 drawn (good inside, warnfg outside, rec above −3; pure-fn tests at the boundaries); gain slider 0..=100% bound to the engine-reported volume (a new snapshot field? NO — the panel reads volume via a `ToolbarCtx`-style passthrough the app resolves per frame from... the engine owns the control; ADD `pub input_volume: Option<f32>` to `HealthView` (engine refreshes it once per publish from the control — cheap property read; document) — slider sends `SetInputVolume` debounced like the ppm DragValue); when `input_volume` is None: the per-OS instruction line (pinned table: macOS/Windows/Linux strings from spec §4, completeness-tested); transport tag faint (from a new `HealthView::transport: Option<Transport>`? Simpler: include transport alongside — `pub input_transport: Option<Transport>` refreshed at source build only). Wire `volume_moved` tracking for the score: the panel records whether the volume-changed banner fired during a tick-test capture window (panel-side bool per run, cleared with results) — document the approximation honestly (it observes the banner, not the OS directly).
- Device combo: names where `name_suggests_bluetooth` OR (engine-known transport == Bluetooth for the active device) get a faint " (Bluetooth)" suffix — display only; refusal itself is engine-side (Task 4).
- doctor_score call site updates to pass the two new inputs.

- [ ] Step 1 (failing tests): meter frac boundaries (−60→0, 0→1, −12/−6 zone edges), zone-color mapping fn table, advice-table completeness (3 OSes non-empty + exact macOS string), combo-suffix pure fn. Step 2: implement + wire. Step 3: gates + launch. Step 4: Commit `feat(app): doctor levels section with headroom meter and gain slider`

---

### Task 7: First-launch Doctor auto-open

**Files:** Modify: `crates/chrona-session/src/config.rs` (`#[serde(default)] pub doctor_prompted: bool`), `crates/chrona-app/src/app.rs`.

- In `ChronaApp::new`: if the INITIAL source is Mic (not simulate/replay/headless) AND `!config.doctor_prompted` → `doctor.open()` + set flag + `persist_config_now`. Never on later frames, never on SwitchSource, never in headless (structurally: run_headless never constructs ChronaApp — cite).
- [ ] Step 1 (failing tests): config round-trip of the flag + default-false on v-old configs (raw TOML literal without the key); the open-condition pure fn `should_prompt_doctor(is_mic_initial, prompted) -> bool` truth table. Step 2: implement. Step 3: gates + launch + headless pin (must be untouched). Step 4: Commit `feat(app): first-launch mic-doctor prompt`

---

### Task 8: Docs — protocol B.10–B.12, README, AGENTS, followups

**Files:** Modify: `docs/manual-testing.md`, `README.md`, `AGENTS.md`, `docs/superpowers/notes/m4-followups.md` (+ create `m5-followups.md` if any deferred items accrue by then).

- [ ] Step 1 (every line code-grounded, citations in the report): B.10 Levels (meter moves; slider tracks System Settings and vice versa; external change → the exact Warn banner once), B.11 Bluetooth refusal (exact Error text; previous source keeps running; skip-if-no-BT-device note), B.12 first-launch (fresh config → Doctor opens; relaunch → doesn't). README trust paragraph extended (one sentence). AGENTS: unsafe-confinement line (os_control.rs is the ONLY unsafe module), the two new pinned advice strings + banner texts, the poll-not-sleep test lesson if not already present. m4-followups: discharge annotations for the M5-pointer items now landed (first-launch auto-run, gain writeback macOS, volume listeners, Bluetooth refusal — Windows effects enumeration stays open with a note that the trait seam is its landing pad). Step 2: gates. Step 3: Commit `docs: M5 protocol, pins, and followups discharge`

---

## Self-review notes (writing-plans checklist)

- Spec coverage: §1→T1/T2, §2→T3/T4, §3→T5/T6/T7, §4→T6, §5 distributed, §6→T8, §7→T2, §8/§9 recorded (no tasks). No gaps.
- Type consistency: `InputControl/Transport/OsAudioError/FakeInputControl/name_suggests_bluetooth` (T1) consumed by T2/T4/T6 by those names; `peak_dbfs`/`input_volume`/`input_transport` on HealthView (T3/T6) consumed by T6; `SetInputVolume` (T4) sent by T6's slider; `doctor_prompted` (T7) read where set.
- Placeholders: none; banner/advice strings, deduction values, and test cases are stated. T2's ownership-pattern choice and T4's fake-injection env format are deliberate implement-time decisions with documentation required, not gaps.
- Ordering: T1→T2 (same module), T1→T4 (fake), T3 independent, T5 before T6 (signature), T6 before T8; T7 independent after T6's panel exists (open() is M4 API — actually independent entirely; keep numbered order).
