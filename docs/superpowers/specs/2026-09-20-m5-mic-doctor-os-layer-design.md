# Chrona M5 — Mic Doctor OS Layer (Design)

**Status:** draft for user review. Implements the binding spec's §3.2 steps 4–5 and
§3.3 (OS checks, gain control, volume listeners, Bluetooth refusal, first-launch flow)
**macOS-first**: full runtime implementation for macOS (buildable and manually
verifiable on the user's machine); Windows/Linux ship an honest `Unsupported` stub
behind the same trait plus per-OS manual instructions in the advice tables. Their
runtime implementations land in a later milestone when a real machine can verify them
— programmatic gain-writing must never ship unexercised (user decision, 2026-09-20).

**Authority:** the 2026-08-20 binding spec; M4's Doctor core/panel conventions carry
over. Deviations recorded in §9.

## 1. Platform abstraction (`chrona-audio`, new module `os_control`)

```rust
pub enum Transport { BuiltIn, Usb, Bluetooth, Other(String), Unknown }
pub struct InputControl { /* per-OS handle */ }
impl InputControl {
    /// Resolves the OS control surface for a capture device by the same
    /// name/id the existing enumeration uses. None on unsupported OSes
    /// (Windows/Linux for now) or when the device has vanished.
    pub fn open(device: &DeviceInfo) -> Option<InputControl>;
    pub fn volume(&self) -> Option<f32>;              // 0..=1; None = no control
    pub fn set_volume(&self, v: f32) -> Result<(), OsAudioError>;
    pub fn transport(&self) -> Transport;
    /// Registers an OS property listener; the returned guard unregisters on
    /// Drop. The callback only flips an atomic (no allocation, no UI work).
    pub fn watch_volume(&self, flag: Arc<AtomicBool>) -> Option<VolumeWatch>;
}
```

- **macOS impl:** CoreAudio — `kAudioDevicePropertyVolumeScalar` (input scope,
  read/write), `kAudioDevicePropertyTransportType` (Bluetooth =
  `kAudioDeviceTransportTypeBluetooth`/LE), `AudioObjectAddPropertyListener` for the
  watch. Dependency: `coreaudio-sys`, target-gated `[target.'cfg(target_os =
  "macos")'.dependencies]` — cpal already pulls it transitively on macOS, so the tree
  gains nothing new. ALL unsafe FFI lives in this one module with SAFETY comments;
  the rest of the workspace stays safe Rust (AGENTS.md gets the line).
- **Windows/Linux stub:** `open` returns `None`; a doc comment names the deferred
  APIs (WASAPI `IAudioEndpointVolume` + effects enumeration; ALSA mixer) and the
  user-decision rationale. Transport falls back to name heuristics ONLY for the
  Bluetooth-refusal warning copy (a name containing "bluetooth"/"airpods"/"hands-free"
  ⇒ warn-level hint, never a hard refusal — detection isn't authoritative there).

## 2. Engine integration

- **Peak/headroom tracking:** `pull_tick` tracks the running max-abs sample per
  publish interval; `HealthView` gains `peak_dbfs: Option<f64>` (None when the
  interval delivered no samples). dBFS = `20·log10(peak)`, clamped at −120. Testable
  headless: synth normalizes to 0.9 peak ⇒ Simulate publishes ≈ −0.92 dBFS.
- **Volume watch:** the engine opens `InputControl` at mic-source build (alongside
  the stream), registers the watch with an `Arc<AtomicBool>`, and checks it per tick:
  flag set ⇒ Warn banner "input volume changed outside Chrona (now N%)" (N re-read
  from `volume()`), then re-arm. Watch and control drop on source teardown. On
  non-mac / no-control the whole feature is silently absent (no banner, no lies).
- **Gain control:** `ControlMsg::SetInputVolume(f32)` — engine-owned handle applies
  `set_volume(clamped 0..=1)`; failure ⇒ Warn banner with the OS error. Never sent
  automatically — only from the Doctor's slider (§3). Self-inflicted changes must not
  trigger the volume-watch banner: the engine swallows the next watch flag after its
  own set (a one-shot suppression counter, documented).
- **Bluetooth refusal (binding §3.2 "refused with an explanation"):** on macOS, a
  `SwitchSource(Mic)` targeting a device whose `transport()` is Bluetooth is REFUSED
  before the stream is built: Error banner "Bluetooth input refused: Bluetooth voice
  codecs (SCO/HFP) destroy tick timing — use a wired or USB microphone", engine stays
  on the previous source. The device combo marks such devices "(Bluetooth)". On
  stub OSes: name-heuristic hint only (see §1), selection allowed.

## 3. Doctor panel additions

- **Levels section** (shown whenever the source is Mic): a horizontal headroom meter
  of `peak_dbfs` with the binding spec's target zone marked (peaks −12…−6 dBFS; good
  color inside, warn outside, rec at clipping); a gain slider bound to
  `InputControl::volume()` sending `SetInputVolume` (debounced like the ppm
  DragValue), shown only when a control exists; otherwise the per-OS instruction line
  from the advice table (§4). Transport shown as a faint tag (Built-in / USB /
  Bluetooth).
- **Score integration:** two new OS-fed inputs to `doctor_score` (new Option params,
  same honesty rules): `volume_moved_during_test: Option<bool>` (−10, advice: "another
  app is adjusting your input gain — close it or disable auto-gain") and
  `peak_dbfs: Option<f64>` (−10 when outside −12…−6 by more than 3 dB, advice: "set
  input gain so ticks peak between −12 and −6 dBFS"). Un-run/unavailable stays out of
  scoring, as ever. The M4 advice strings stay byte-frozen; these two are new pins.
- **First-launch auto-open:** `ConfigStore` gains `doctor_prompted: bool`
  (`#[serde(default)]`); on the first `ChronaApp::new` whose initial source is Mic
  and `!doctor_prompted`, the Doctor panel OPENS (intro visible, nothing auto-runs —
  steps stay user-triggered, honesty unchanged) and the flag persists immediately.
  `--simulate`/headless never trigger it; the frozen headless line is untouched.

## 4. Per-OS advice tables

Static, pinned strings (Doctor Levels + the effects advice slot): macOS "System
Settings → Sound → Input — drag the input level"; Windows "Settings → System → Sound →
your microphone → Audio enhancements: Off (effects detection lands with the Windows
build of Chrona)"; Linux "use pavucontrol or alsamixer to set the capture level".
These fill the M4 advice slots that were left empty, honestly labeled as manual
instructions where live detection doesn't exist yet.

## 5. Testing

- Trait-level: the engine consumes `InputControl` through a thin internal seam so the
  volume-watch/suppression/banner logic is testable headless — extend the existing
  `test-fault-injection` feature pattern (chrona-audio exposes, under that feature, a
  way to construct a fake `InputControl` with settable volume/transport and a
  trippable watch flag; the engine test drives: flag trip ⇒ Warn banner once ⇒
  re-arm; self-set suppression ⇒ no banner; Bluetooth transport ⇒ SwitchSource
  refusal with the exact Error text).
- Peak tracking: Simulate turbo test pins `peak_dbfs ≈ −0.92 ± 0.2` and None-on-Idle.
- Score: table tests for the two new deductions/advice (boundary at the ±3 dB band).
- Meter/slider geometry + advice-table completeness: pure-fn tests per M4a/M4
  convention. macOS FFI itself: compile + manual protocol (§6) — no fake-quartz-style
  ground truth exists for OS calls; the module stays thin enough to review.

## 6. Protocol & docs

`docs/manual-testing.md`: **B.10** — Levels: with the real mic, the meter moves;
adjust the slider, watch OS input level track it; move the input level in System
Settings while Chrona runs ⇒ the "changed outside Chrona" Warn banner appears once.
**B.11** — with a Bluetooth mic paired (if available): the combo tags it, selecting it
is refused with the explanation banner, previous source keeps running. **B.12** —
fresh config (`rm` the config file): first mic launch auto-opens the Doctor intro;
relaunching doesn't. README + AGENTS updated (new pins, unsafe-confinement line,
followups discharge annotations for the M4/M5 items this closes).

## 7. New dependencies

`coreaudio-sys` (macOS-only target gate; already in the tree transitively via cpal).
Nothing else. The `windows`/`alsa` crates explicitly NOT added this milestone.

## 8. Out of scope (recorded)

Windows/Linux runtime implementations (the trait seam is their landing pad); WASAPI
raw-mode path; ML-denoiser/mic-mode detection beyond M4's signal-level AGC/gate
detectors (the binding spec's "treats their presence as a fault" is served by those
detectors — a processed signal shows up as AGC/gate signatures; noted as the deliberate
detection strategy); auto-gain (the spec's gain adjustment is always user-initiated).

## 9. Risks & honesty notes

- OS API calls can fail or lie (device unplugged mid-call): every failure path
  degrades to the feature being absent plus, where user-initiated, a Warn banner —
  never a fabricated level or a silent wrong write.
- `set_volume` is clamped and only ever applies the slider's explicit value — no
  stepping/animation, no automation.
- The volume-watch's self-set suppression is a heuristic (one-shot counter); a
  genuinely simultaneous external change could be swallowed once — documented,
  cosmetic, re-detected on the next change.
