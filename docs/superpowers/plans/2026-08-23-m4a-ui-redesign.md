# M4a UI Redesign Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rebuild the Chrona app UI to the approved Claude Design mockup — theme system, metric cards, time-axis beat trace + amplitude charts, watch library, session history, position comparison, export report — with all M3 honesty semantics intact.

**Architecture:** DSP crate untouched; engine changes limited to recording metadata (watch), finalize-time sidecar summary, and typed banner severity. All new visualization is UI-side: pure presenter math (accumulators, transforms) tested headlessly, thin egui painters over it. Session history is derived by scanning recording sidecars — no database.

**Tech Stack:** Rust edition 2024 stable (pinned), eframe/egui 0.36, serde/toml/serde_json, rfd, embedded TTF fonts (OFL).

**Spec:** docs/superpowers/specs/2026-08-23-ui-redesign-design.md (decisions; binding) + docs/superpowers/specs/assets/2026-08-23-chrona-redesign.dc.html (visual authority — its inline `drawBeat`/`drawAmp` JS is the normative chart-geometry reference). The 2026-08-20 core spec still governs everything non-UI.

## Global Constraints

- Gates for EVERY task: `cargo test --workspace` (0 failures), `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`. The app must compile AND run after every task (incremental wiring — never leave a new module dead/unwired, clippy treats dead code as an error).
- Headless output is FROZEN: `cargo run -p chrona-app -- --simulate --rate 12 --beat-error 0.8 --amplitude 270 --headless-seconds 45` must print exactly `TIER 3 · full  rate +12.0 s/d ⚠ uncal  beat error 0.8 ms  amplitude 270°` and exit 0. Run it in the gate list of every task that touches chrona-app.
- Spec §3.1 honesty: metrics below tier show `—` + reason, never values. No chart interpolation across gaps > 2 s.
- Palette hex values in Task 1 are binding; NO `Color32::from_rgb` outside `theme.rs` (charts/painters take colors from `Palette`).
- Never open a real microphone stream in any test.
- `chrona-dsp` is untouched in this plan. Do not edit it.
- Threading: UI reads `Engine::snapshot()` only; engine owns all DSP calls (M3 architecture).
- Doc comments explain constraints (repo style); tests live in in-file `#[cfg(test)]` modules; rustfmt is strict — run `cargo fmt --all` before the check.

---

### Task 1: Theme foundation — palette, fonts, egui style, persisted theme

**Files:**
- Create: `crates/chrona-app/src/theme.rs`, `crates/chrona-app/assets/fonts/` (4 TTFs + 2 OFL licenses)
- Modify: `crates/chrona-app/src/lib.rs` (add `pub mod theme;`), `crates/chrona-app/src/app.rs` (apply theme at startup + store `Theme` in `ChronaApp`), `crates/chrona-session/src/config.rs` (add `theme` field)

**Interfaces (produced, used by Tasks 4–10):**
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Theme { Dark, Light }
pub struct Palette { /* one egui::Color32 field per token */
    pub bg: Color32, pub panel: Color32, pub panel2: Color32,
    pub border: Color32, pub border2: Color32, pub text: Color32,
    pub muted: Color32, pub faint: Color32, pub grid: Color32,
    pub grid0: Color32, pub chartbg: Color32, pub accent: Color32,
    pub accent_ink: Color32, pub tick: Color32, pub good: Color32,
    pub warnbg: Color32, pub warnfg: Color32, pub rec: Color32,
    pub overlay: Color32,
}
impl Palette { pub fn of(theme: Theme) -> &'static Palette; }
pub fn install_fonts(ctx: &egui::Context);          // once at startup
pub fn apply_style(ctx: &egui::Context, theme: Theme); // idempotent, on switch too
pub fn theme_from_config(s: Option<&str>) -> Theme;  // "light" => Light, else Dark
pub fn theme_config_value(t: Theme) -> &'static str; // "dark" | "light"
// Font family constants:
pub const FAMILY_SEMIBOLD: &str = "archivo-semibold";
pub const FAMILY_BOLD: &str = "archivo-bold";
```

- [ ] **Step 1: Commit the font assets.** The controller pre-downloaded verified TTFs; copy them (do NOT re-download unless missing):
```bash
mkdir -p crates/chrona-app/assets/fonts
S=/private/tmp/claude-501/-Users-rohit-source-Chrona/261e8759-d099-4acd-828f-bce39baf627e/scratchpad
cp $S/Archivo-Regular.ttf $S/Archivo-SemiBold.ttf $S/Archivo-Bold.ttf $S/FragmentMono-Regular.ttf crates/chrona-app/assets/fonts/
file crates/chrona-app/assets/fonts/*.ttf   # each must say "TrueType Font data"
curl -so crates/chrona-app/assets/fonts/OFL-Archivo.txt https://raw.githubusercontent.com/google/fonts/main/ofl/archivo/OFL.txt
curl -so crates/chrona-app/assets/fonts/OFL-FragmentMono.txt https://raw.githubusercontent.com/google/fonts/main/ofl/fragmentmono/OFL.txt
head -3 crates/chrona-app/assets/fonts/OFL-*.txt  # both must contain "SIL OPEN FONT LICENSE" or copyright line
```
If the scratchpad copies are missing, fetch the TTFs from the gstatic URLs in the controller dispatch note. If any download fails, report BLOCKED.

- [ ] **Step 2: Failing tests** in `theme.rs`:
```rust
#[test] fn palette_tokens_exact() {
    let d = Palette::of(Theme::Dark);
    assert_eq!(d.bg, egui::Color32::from_rgb(0x09, 0x0e, 0x11));
    assert_eq!(d.accent, egui::Color32::from_rgb(0x4a, 0xb8, 0xe8));
    assert_eq!(d.tick, egui::Color32::from_rgb(0xe7, 0xb6, 0x43));
    let l = Palette::of(Theme::Light);
    assert_eq!(l.bg, egui::Color32::from_rgb(0xf2, 0xf4, 0xf5));
    assert_eq!(l.accent, egui::Color32::from_rgb(0x00, 0x7c, 0xb8));
    assert_ne!(d.text, l.text);
}
#[test] fn theme_config_roundtrip() {
    assert_eq!(theme_from_config(Some("light")), Theme::Light);
    assert_eq!(theme_from_config(Some("dark")), Theme::Dark);
    assert_eq!(theme_from_config(Some("mauve")), Theme::Dark); // invalid → dark
    assert_eq!(theme_from_config(None), Theme::Dark);
    assert_eq!(theme_config_value(Theme::Light), "light");
}
```

- [ ] **Step 3: Implement.** Full palettes (BOTH themes, every token — these hex values are binding, from the spec §1 table):
dark: bg #090e11, panel #12171a, panel2 #1b2024, border #252a2d, border2 #33393d, text #e5e8eb, muted #81878c, faint #646a6e, grid #1a1d20, grid0 #363b3f, chartbg #070a0c, accent #4ab8e8, accent_ink #080c0f, tick #e7b643, good #53be70, warnbg #3f2903, warnfg #f2c86c, rec #c92f33, overlay #020405.
light: bg #f2f4f5, panel #ffffff, panel2 #eceff1, border #d5d8da, border2 #babec1, text #1b2024, muted #5e6468, faint #757b80, grid #e2e5e7, grid0 #babec1, chartbg #fbfcfd, accent #007cb8, accent_ink #fcfcfc, tick #ce871b, good #25984d, warnbg #f7e6c3, warnfg #8a5600, rec #c92f33, overlay #020405.
`install_fonts`: `include_bytes!` the 4 TTFs; FontDefinitions: `Proportional` → Archivo-Regular first; `Monospace` → FragmentMono-Regular first; insert named families `archivo-semibold`, `archivo-bold`. `apply_style`: build `egui::Visuals` from the palette (dark_mode flag by theme; `panel_fill`/`window_fill` = bg; `widgets.*` bg = panel2, stroke = border2, fg = text; `selection` = accent; corner radius 8 for widgets; window_corner_radius 12; hyperlink = accent) and set `ctx.set_visuals`. Config: `ConfigStore` gains `#[serde(default)] pub theme: Option<String>` (serialize as written). `app.rs`: in `ChronaApp::new` call `install_fonts` + `apply_style(theme_from_config(config.theme.as_deref()))`, store `theme: Theme` field. No visible layout change yet beyond fonts/colors.

- [ ] **Step 4: Gates** (incl. headless pin — must be byte-identical; the headless path never touches egui so this proves it).
- [ ] **Step 5: Commit** `feat(app): theme foundation — palette, embedded fonts, persisted theme`

---

### Task 2: Session crate — sidecar v2 (watch + summary), finalize rewrite, config watch list

**Files:**
- Modify: `crates/chrona-session/src/sidecar.rs`, `crates/chrona-session/src/writer.rs`, `crates/chrona-session/src/config.rs`, `crates/chrona-session/src/lib.rs` (re-exports)

**Interfaces (produced; consumed by Tasks 3/5/6/9):**
```rust
// sidecar.rs
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SessionSummary {
    pub tier: String,                 // "none" | "T1" | "T2" | "T3"
    pub rate_s_per_day: Option<f64>,
    pub beat_error_ms: Option<f64>,
    pub amplitude_deg: Option<f64>,
    pub bph_detected: Option<f64>,
    pub duration_s: f64,
}
// SessionMeta gains: #[serde(default)] pub watch: Option<String>,
//                    #[serde(default)] pub summary: Option<SessionSummary>
// schema_version written as 2. Reader stays best-effort (unchanged fn).
pub fn read_meta(wav_path: &Path) -> Option<SessionMeta>; // pub wrapper over read_sidecar
// writer.rs
impl SessionWriter {
    pub fn finalize_with(self, summary: Option<SessionSummary>) -> Result<PathBuf>;
    // finalize(self) := finalize_with(None)  (kept, delegates)
    // finalize_with computes duration_s = samples_written as f64 / sample_rate,
    // sets meta.summary = summary.map(|mut s| { s.duration_s = duration; s }),
    // REWRITES the sidecar (write_sidecar) after closing the WAV.
}
// config.rs — ConfigStore gains (all #[serde(default)]):
pub watches: Vec<String>, pub last_watch: Option<String>, pub theme: Option<String>,
// plus normalization applied inside load_from/load_default:
pub fn normalize(&mut self); // trims watches, drops empties, dedupes preserving
                             // first occurrence; clears last_watch if not in watches
```

- [ ] **Step 1: Failing tests.** sidecar: `v1_sidecar_loads_with_none_new_fields` (write a JSON literal with ONLY the v1 fields — schema_version 1 — parse via `read_meta`, assert `watch == None && summary == None`); `v2_roundtrip_preserves_summary` (full SessionMeta with summary through write/read, assert equality and `schema_version == 2` in the written JSON text); writer: `finalize_with_rewrites_sidecar_with_duration` (create writer w/ 48 kHz meta, push 48_000 samples, `finalize_with(Some(summary))`, re-read sidecar: summary present, `duration_s` within 1e-9 of 1.0); `finalize_none_keeps_summary_absent`. config: `normalize_trims_dedupes_and_validates_last_watch` (watches `[" a ", "a", "", "b"]`, last_watch `Some("zz")` → watches `["a","b"]`, last_watch None); `watch_fields_roundtrip_through_toml` (save_to/load_from temp path).
- [ ] **Step 2: Run — they fail** (missing fields/APIs).
- [ ] **Step 3: Implement** exactly the interfaces above. `SessionMeta` construction sites in tests/engine still compile: add the two new fields with `None` at every existing literal (this task fixes chrona-session's own; Task 3 handles engine's). NOTE: to keep the workspace compiling THIS task, add `..` -style construction is not possible on plain struct literals — update the engine's two `SessionMeta { … }` literals in this task with `watch: None, summary: None` (mechanical, no behavior change; Task 3 wires real values).
- [ ] **Step 4: Gates.**
- [ ] **Step 5: Commit** `feat(session): sidecar v2 (watch + stop-time summary), finalize rewrite, watch-list config`

---

### Task 3: Engine — watch metadata, summary at finalize, typed banner severity

**Files:**
- Modify: `crates/chrona-app/src/engine.rs`, `crates/chrona-app/src/ui/controls.rs` (pick_banner typing), `crates/chrona-app/src/app.rs` (banner call site), `crates/chrona-app/tests/engine.rs`

**Interfaces (produced; consumed by Tasks 6/7/9/10):**
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BannerSeverity { Info, Warn, Error }
#[derive(Debug, Clone, PartialEq)]
pub struct Banner { pub severity: BannerSeverity, pub text: String }
// EngineSnapshot: `error_banner: Option<String>` → `banner: Option<Banner>`
// ControlMsg::StartRecording gains `watch: Option<String>`
```

Severity mapping (spec §4, binding): replay "using recorded settings…" info = Info; "recording stopped (source switched)"-class notices = Info; silence / clipping / overruns = Warn (these come from `pick_banner`'s health side); every `failed to *` / `invalid *` / fault string = Error; "saved input device unavailable — using default input" = Warn.

- [ ] **Step 1: Failing tests.** Update `tests/engine.rs` expectations: banner assertions now match `Banner { severity, text }` (e.g. the forced StartRecording-Err test asserts `severity == BannerSeverity::Error` and text contains "failed to start recording"); new test `stop_recording_writes_summary_sidecar`: turbo simulate engine, wait for Tier 3 snapshot, `StartRecording` into a temp dir with `watch: Some("Test Watch".into())`, wait until `snapshot().recording.is_some()`, sleep ≥ 1 s of audio, `StopRecording`, wait until recording is None, then `read_meta` on the WAV: `watch == Some("Test Watch")`, `summary.is_some()`, summary tier `"T3"`, `rate_s_per_day` within 1.0 of 12.0, `duration_s > 0.0`.
- [ ] **Step 2: Run — fails** (field/type mismatches).
- [ ] **Step 3: Implement.** Engine keeps `last_metrics: Option<MetricsSnapshot>` (updated every publish tick). `summary_from(m: &MetricsSnapshot) -> SessionSummary` maps tier via `Tier::T1.. => "T1".."T3"` / no-metrics caller side → tier "none" with all-None (when `last_metrics` is None at stop, summary = Some(SessionSummary{tier:"none", …None, duration filled by writer)). All finalize paths (StopRecording, SwitchSource auto-stop, Shutdown/Drop) call `finalize_with(summary)`. `StartRecording` stores `watch` into the meta. Banner: replace the `error_banner: Option<String>` local + snapshot field with `Option<Banner>`; classify at every set site per the mapping; `pick_banner` (controls.rs) signature becomes `pick_banner(engine_banner: Option<&Banner>, health: &HealthView, kind: SourceKind) -> Option<Banner>` — health-derived banners get Warn; precedence order unchanged from M3 (its 7-case test updates to assert severity too). `run_headless` untouched.
- [ ] **Step 4: Gates + headless pin.**
- [ ] **Step 5: Commit** `feat(app): watch metadata + stop-time summary through engine; typed banner severity`

---

### Task 4: Trace presenter — accumulators, transforms, trend, signal meter, bph format (pure logic)

**Files:**
- Create: `crates/chrona-app/src/presenter/trace.rs`
- Modify: `crates/chrona-app/src/presenter.rs` or `presenter/mod.rs` (add `pub mod trace;`), `crates/chrona-app/src/presenter/format.rs` (add two fns)

**Interfaces (produced; consumed by Tasks 7/8/10):**
```rust
// trace.rs — NO egui imports (presenter rule)
pub const SPAN_MIN_S: f64 = 15.0;  pub const SPAN_MAX_S: f64 = 300.0;
pub const SPAN_DEFAULT_S: f64 = 180.0;
pub const WRAP_PRESETS_MS: [f64; 4] = [4.0, 10.0, 20.0, 50.0]; // ±2/±5/±10/±25
pub const HORIZON_S: f64 = 300.0;  pub const GAP_BREAK_S: f64 = 2.0;

#[derive(Debug, Clone, Copy)]
pub struct BeatPoint { pub t_s: f64, pub offset_ms: f64, pub is_tic: bool }
#[derive(Default)]
pub struct BeatAccum { /* points: VecDeque<BeatPoint>, grid state */ }
impl BeatAccum {
    /// Feed one snapshot's tape. `bph_nominal: None` (Free/no-snap) appends
    /// nothing. A CHANGE of bph_nominal resets the accumulator (grid invalid).
    pub fn extend(&mut self, events: &[chrona_dsp::analyzer::TapeEvent], bph_nominal: Option<u32>);
    pub fn points(&self) -> &VecDeque<BeatPoint>;
    pub fn newest_t(&self) -> Option<f64>;
    pub fn reset(&mut self);
}
#[derive(Default)]
pub struct AmpAccum { /* (t_s, deg) points, 1/s sampling */ }
impl AmpAccum {
    /// Push at most one point per second: only when `amp.is_some()` AND
    /// `t_now` (the beat accum's newest_t) advanced ≥ 1 s past the last kept.
    pub fn extend(&mut self, t_now: Option<f64>, amp: Option<f64>);
    pub fn points(&self) -> &VecDeque<(f64, f64)>;
}
#[derive(Debug, Clone, Copy)]
pub struct TraceView { pub span_s: f64, pub follow: bool, pub end_rel_s: f64 }
impl Default for TraceView { /* span 180, follow true, end_rel 0 */ }
impl TraceView {
    pub fn end_t(&self, newest_t: f64) -> f64;   // newest_t - end_rel_s (0 when follow)
    pub fn zoom(&mut self, factor: f64);          // span *= factor, clamped
    pub fn pan(&mut self, dt_s: f64, newest_t: f64); // adjusts end_rel; end_rel<=0 → follow=true, end_rel=0
    pub fn fit(&mut self);                        // span=180, follow=true, end_rel=0
}
/// Wrap into (-wrap_ms/2, wrap_ms/2]; same convention as M3's tape.
pub fn wrap_signed(v_ms: f64, wrap_ms: f64) -> f64;
/// y fraction in -0.5..=0.5 for a wrapped offset.
pub fn y_frac(offset_ms: f64, wrap_ms: f64) -> f64;
/// Gridline step tables (from the mockup JS, binding):
pub fn y_step_ms(wrap_ms: f64) -> f64;   // half-wrap w=wrap/2: w<=2 → 0.5, w<=5 → 1, w<=10 → 2, else 5
pub fn x_step_s(span_s: f64) -> f64;     // span<=40 → 5, <=90 → 10, <=200 → 30, else 60
/// Trend line: slope +rate·1000/86400 ms/s (fast ⇒ rising, mockup convention),
/// anchored at (anchor_t = newest_t, anchor_off = median offset of points with
/// t >= newest_t − 10 s). None when rate is None or no points in that window.
pub struct Trend { pub anchor_t: f64, pub anchor_off_ms: f64, pub slope_ms_per_s: f64 }
pub fn trend(points: &VecDeque<BeatPoint>, rate_s_per_day: Option<f64>) -> Option<Trend>;
/// Amplitude y-range: smallest [30k, 30m] containing the data, span ≥ 60°
/// (extend upper bound first, then lower). Empty data → (240.0, 300.0).
pub fn amp_range(points: &VecDeque<(f64, f64)>) -> (f64, f64);
/// Signal meter (spec §3): (bars 0..=4, label, class)
pub enum SignalClass { Faint, Warn, Good }
pub fn signal_meter(m: Option<&MetricsSnapshot>) -> (u8, &'static str, SignalClass);
// format.rs additions:
pub fn format_bph_grouped(bph: f64) -> String; // round → int, thousands with ' ' → "28 800"
pub fn day_label(entry_unix: u64, now_unix: u64) -> String; // "Today"/"Yesterday"/"YYYY-MM-DD" (UTC-day math OK, doc it)
```

**Offset math (binding).** The accumulator maintains its own beat grid, independent of the snapshot-relative `beat_index` (which re-bases at refolds — see `TapeEvent` docs):
- First kept event (needs `t_unlock_s`): `t0 = t_unlock`, `k = 0`, `offset = 0`.
- Each subsequent kept event: `k_i = k_prev + max(1, round((t_unlock_i − t_unlock_prev)/T_beat))` where `T_beat = 3600/bph as f64` seconds — the rounding rides through detection gaps; `offset_ms_i = (k_i·T_beat − (t_unlock_i − t0)) · 1000.0` (note the SIGN: a fast watch's beats arrive early ⇒ offset grows positive ⇒ the trace RISES, matching the mockup and Watch-O-Scope's up-is-fast).
- Dedupe: append only events with `t_drop_corr_s` strictly greater than the last appended event's (snapshots overlap; `tape_events()` is time-ascending). `t_s` stored = `t_unlock_s`.
- Prune: drop points older than `newest_t − HORIZON_S`. `t0`/`k` bookkeeping survives pruning (keep running `(t_prev, k_prev, t0)` state separate from the deque).

- [ ] **Step 1: Failing tests** (this is the load-bearing task — test thoroughly):
```rust
// helpers: mk_event(k, t_unlock, tic) like presenter/tape.rs's ev()
#[test] fn perfect_watch_offsets_are_zero_and_fast_watch_rises() { /* 100 perfect beats → all |offset|<1e-9; then fresh accum, beats arriving 0.125 ms early each → offsets strictly increasing, offset[80]-offset[0] ≈ 80*0.125 ms within 1e-6 */ }
#[test] fn dedupe_across_overlapping_snapshots() { /* extend with events 0..50, then 30..80 → point count 80, no duplicates (t strictly increasing) */ }
#[test] fn detection_gap_keeps_grid_continuity() { /* beats 0..20, skip 21..29, beats 30..50 (same perfect grid) → offsets all ~0 after the gap (k jumped by 10) */ }
#[test] fn prune_and_newest() { /* 400 s of beats at 0.125 s → oldest kept ≥ newest−300 */ }
#[test] fn bph_change_resets_and_none_bph_appends_nothing() {}
#[test] fn wrap_and_steps_match_mockup_tables() { /* wrap_signed(5.1, 10.0) ≈ −4.9; y_step_ms(4.0)=0.5? NO: wrap_ms=4 ⇒ half=2 ⇒ 0.5. y_step_ms(10)=1, y_step_ms(20)=2, y_step_ms(50)=5. x_step_s(40)=5, (90)=10, (200)=30, (300)=60 */ }
#[test] fn trend_slope_sign_and_anchor() { /* rising fast-watch points, rate +12 → slope ≈ +0.13889 ms/s; anchor_off ≈ median of last-10s offsets; rate None → None */ }
#[test] fn amp_accum_one_per_second_and_range() { /* extend with t 0.0,0.5,1.0,2.2 amp Some(270) → kept at 0.0,1.0,2.2; amp_range of 250..=285 data → (240,300); empty → (240,300); constant 305 → (300,360) */ }
#[test] fn signal_meter_mapping() { /* None→(0,"No signal",Faint); T1→(1,"Weak · rate only",Warn); T2→(2,"Fair · rate + beat error",Warn); T3 det .7→(3,"Strong · full regression",Good); T3 det .85→(4, same, Good) */ }
#[test] fn view_zoom_pan_fit_clamps() { /* zoom below 15 clamps; pan back sets follow=false; pan to end_rel<=0 restores follow */ }
#[test] fn bph_grouping_and_day_label() { /* 28800→"28 800", 9000→"9 000", 108000→"108 000"; day_label(now,now)=="Today" */ }
```
- [ ] **Step 2: Run — fail.** - [ ] **Step 3: Implement per the binding math above.**
- [ ] **Step 4: Wire minimally:** `ChronaApp` gains `beat_accum/amp_accum/trace_view` fields fed in `update()` from each snapshot (`bph_nominal` from `metrics.bph_nominal`), even though rendering still uses the old tape until Task 8 — private unused FIELDS would be dead code, so the feed wiring lands now. (The new `pub` presenter functions are library API and cannot trip dead-code lints; Tasks 7/8 consume them.)
- [ ] **Step 5: Gates + headless pin.** - [ ] **Step 6: Commit** `feat(app): trace presenter — beat/amp accumulators, view transforms, trend, signal meter`

---

### Task 5: History index, position comparison, export report (pure logic)

**Files:**
- Create: `crates/chrona-app/src/history.rs`, `crates/chrona-app/src/export.rs`
- Modify: `crates/chrona-app/src/lib.rs` (`pub mod history; pub mod export;`)

**Interfaces (produced; consumed by Tasks 6/9):**
```rust
// history.rs
#[derive(Debug, Clone, PartialEq)]
pub struct SessionEntry {
    pub wav_path: PathBuf, pub watch: Option<String>, pub position: Option<String>,
    pub started_unix_s: u64, pub summary: Option<chrona_session::SessionSummary>,
}
pub const POSITIONS: [(&str, &str); 6] = [("DU","Dial up"),("DD","Dial down"),
    ("CU","Crown up"),("CD","Crown down"),("CL","Crown left"),("CR","Crown right")];
pub struct HistoryIndex { /* entries sorted started_unix_s DESC */ }
impl HistoryIndex {
    pub fn scan(dir: &Path) -> HistoryIndex;      // read_dir *.json → read_meta on the wav stem;
        // started ordering: meta.started_unix_s; if 0/absent fall back to file mtime (as unix)
    pub fn upsert(&mut self, wav_path: &Path);    // re-read one sidecar, replace-or-insert, re-sort
    pub fn entries(&self) -> &[SessionEntry];
    pub fn for_watch<'a>(&'a self, watch: &str) -> impl Iterator<Item = &'a SessionEntry>;
}
/// Latest summary rate per canonical position for `watch`.
pub struct PositionRates { pub by_code: [Option<f64>; 6], pub spread: Option<f64> }
pub fn position_rates(idx: &HistoryIndex, watch: &str) -> PositionRates;
/// Bar geometry (mockup): half-width fraction of |rate|/15 clamped to 0.5.
pub fn bar_frac(rate: f64) -> f64;
// export.rs
pub struct ReportInput<'a> { pub watch: &'a str, pub app_version: &'a str,
    pub lift_deg: f64, pub bph_mode: &'a str, pub averaging_s: f64, pub ppm: f64,
    pub rates: &'a PositionRates, pub sessions: &'a [&'a SessionEntry],
    pub exported_unix_s: u64 }
pub fn render_report(input: &ReportInput) -> String; // ONE self-contained HTML doc
pub fn watch_slug(watch: &str) -> String; // lowercase, alnum runs kept, others → '-', trimmed
pub fn default_report_name(watch: &str, unix_s: u64) -> String; // chrona-report-<slug>-<YYYY-MM-DD>.html
```
Report content requirements (assert in tests): contains the watch name, `chrona` + app_version, every session row's position code and formatted rate (or `—`), the string `uncalibrated` iff `ppm == 0.0`, a `Δ spread` line when spread is Some, NO occurrence of `http` (self-contained), `<style>` present, starts with `<!DOCTYPE html>`.

- [ ] **Step 1: Failing tests:** build temp dirs with 3 sidecars (two watches, one v1-style without watch/summary) → `scan` ordering DESC by started; `upsert` after adding a 4th; `position_rates` picks the NEWEST rate per position, skips summary-less and rate-less sessions, spread over ≥2 rated positions else None; `bar_frac(7.5)==0.25`, `bar_frac(30.0)==0.5`; `watch_slug("Seiko SKX007 · 7S26") == "seiko-skx007-7s26"`; `render_report` content assertions above (one calibrated, one uncalibrated case); `default_report_name` date math (fixed unix).
- [ ] **Step 2: Run — fail.** - [ ] **Step 3: Implement.** (Date formatting: reuse/extend `chrona_session::civil` — it already turns unix into Y-M-D parts; add a `pub fn ymd(unix_secs: u64) -> (i64,u32,u32)` there if not exposed.)
- [ ] **Step 4: Wire minimally:** `ChronaApp` gains `history: HistoryIndex` scanned at startup from the recordings dir (the same dir the session panel records into) — field read by Task 9's UI; until then reference it in the session panel's `rec` finalize confirmation path (upsert on recording→None transition), which is real behavior needed anyway.
- [ ] **Step 5: Gates + headless pin.** - [ ] **Step 6: Commit** `feat(app): session history index, position comparison math, report export`

---

### Task 6: Toolbar UI (replaces controls row) + add-watch modal

**Files:**
- Create: `crates/chrona-app/src/ui/toolbar.rs`, `crates/chrona-app/src/ui/modals.rs`
- Modify: `crates/chrona-app/src/ui.rs`/`mod` (exports), `crates/chrona-app/src/app.rs` (call toolbar instead of controls_row), `crates/chrona-app/src/ui/controls.rs` (keep state/helpers; delete only the old `controls_row` render fn and its render-only helpers — `ControlsState`, `to_bph_mode`, sanitizers, `pick_banner`, ClipTracker, device polling all SURVIVE and are consumed here)

**Layout (mockup, binding):** logo badge (accent rounded 7px square, bold "C" in accent_ink) + "Chrona" semibold; separator; Watch combo (max width ~180, lists `config.watches` + `+ Add new watch…` which opens the modal); separator; Input device combo (existing device list/refresh logic); Lift numeric `DragValue` (monospace, suffix °, clamp 10–90, sends SetLift); BPH combo Auto/Free/18000/21600/25200/28800/36000/Other… (Other reveals the M3 validated text field inline; sends via `to_bph_mode`); Averaging combo 10 s/30 s/60 s; `cal {ppm:+.1} ppm` monospace faint text; right-aligned: theme toggle button (`☾ Dark`/`☀ Light` — switches `apply_style`, saves config.theme), `Open recording…` (existing rfd flow moved here from session panel), `Export report` (enabled iff a watch is selected AND `history.for_watch(...)` non-empty; disabled shows tooltip "Select a watch with recorded sessions"; click handled fully in Task 9 — this task wires a no-op callback flag), Record/Stop button (accent when idle, rec red while recording, leading dot square while recording; pulse = paint dot with alpha from `ui.input(|i| i.time)` sine — egui needs `ctx.request_repaint_after(Duration::from_millis(100))` only while recording).

**Behavior contracts:** watch selection persists (`config.last_watch`, saved); add-watch modal (modals.rs): text field autofocused, Enter=save, Esc=cancel, blank rejected, duplicate selects existing (case-sensitive exact); saving appends + selects + saves config. All ControlMsg sends identical to M3 semantics (SetLift/SetAveraging/SetBphMode/SetPpm/SwitchSource...). Device→ppm behavior unchanged.

- [ ] **Step 1: Failing tests** (pure helpers in toolbar.rs/modals.rs): `add_watch_outcome` pure fn `(input: &str, watches: &[String]) -> AddWatchOutcome{Added(String)|Selected(usize)|Rejected}` — blank/whitespace → Rejected, existing → Selected, new → Added(trimmed); `averaging_options() == [10.0, 30.0, 60.0]`; `bph_combo_items()` first two are Auto/Free then the 5 values then Other; `export_enabled(selected_watch, has_sessions)` truth table.
- [ ] **Step 2: Run — fail.** **Step 3: Implement + wire into app.rs** (old controls_row deleted same commit; banner strip render moves here temporarily below the toolbar using Task 3's severity → color mapping: Info=accent, Warn=warnfg on warnbg, Error=rec).
- [ ] **Step 4: Gates + headless pin + launch check** `cargo run -p chrona-app -- --simulate --rate 12 --beat-error 0.8 --amplitude 270` builds (do not leave it running).
- [ ] **Step 5: Commit** `feat(app): redesigned toolbar with watch library and theme toggle`

---

### Task 7: Position/session strip, metric cards, help modals (replaces numerals strip + session panel surface)

**Files:**
- Create: `crates/chrona-app/src/ui/strip.rs`, `crates/chrona-app/src/ui/cards.rs`
- Modify: `crates/chrona-app/src/ui/modals.rs` (help modal), `app.rs` (wire strip+cards between toolbar and tape; remove numerals_strip call), `crates/chrona-app/src/ui/instrument.rs` (delete numerals code; tape_panel remains until Task 8), `crates/chrona-app/src/ui/session_panel.rs` (its rec-elapsed + replay-line move into strip.rs WITH their unit tests; file deleted when empty — the open-recording flow already moved in Task 6)

**Strip (mockup):** "POSITION" label; segmented control DU/DD/CU/CD/CL/CR (two-line buttons: mono code + small word; selected = accent bg + accent_ink); selection stored in `ChronaApp` and passed as `meta_position` + highlight source (consumed by Task 9's comparison); right side: "SIGNAL" label + 4 bars (heights 8/12/16/20 px, lit per `signal_meter` — Good=good, Warn=warnfg, unlit=border2) + label text colored by class; REPLAY mode line (M3 semantics/copy) beside it when replaying; elapsed `● MM:SS` monospace right-aligned (existing `rec_elapsed_label` logic).

**Cards (mockup):** 4 equal cards in a responsive row (panel bg, border, 12 rounding, 14/16 padding): RATE (uncal pill right — warnbg/warnfg pill, tooltip per mockup, shown iff rate shown && !calibrated), BEAT ERROR, AMPLITUDE, BEAT RATE (right tag: auto/fixed/free faint). Value line: 30 pt monospace value + 15 pt muted unit (s/d, ms, °, bph with `format_bph_grouped`). Below-tier: value `—` (30 pt) + the M3 gate-reason caption (11 pt muted) — reuse the existing per-metric reason source functions from instrument.rs (move them into cards.rs with their tests). Each label row has a 16 px circular "?" button opening the help modal.

**Help modal (modals.rs):** overlay (palette overlay at 55% alpha), centered panel ≤ 480 px: title semibold 16, what-paragraph, "WHY IT MATTERS" boxed section (panel2, rounded 10), "Good range:" line (good color lead). Copy VERBATIM from the mockup's `helpTopics` (4 topics — rate/beatError/amplitude/beatRate; the exact strings are in the committed mockup asset, transcribe them into a `const HELP_TOPICS: [HelpTopic; 4]`). ✕ button, click-outside, and Esc all close.

- [ ] **Step 1: Failing tests:** `help_topics_complete` (4 topics, each title/what/why/good non-empty, `rate` topic contains "regulator", `beatRate` contains "28,800"); card caption honesty — move/keep the existing instrument.rs cell tests (rate/be/amp/bph cell fns return (`value`,`caption`) pairs) and re-point them at cards.rs, PLUS `uncal_pill_shown_iff_rate_and_uncalibrated`; `position codes render two-line data` pure fn test (`POSITIONS` reused from history.rs).
- [ ] **Step 2: Run — fail.** **Step 3: Implement + wire.** (session_panel's banner/open-recording already gone in T6; move remaining elapsed/replay helpers + tests; delete the file when empty.)
- [ ] **Step 4: Gates + headless pin.** **Step 5: Commit** `feat(app): position strip, signal meter, metric cards, help modals`

---

### Task 8: Charts — beat trace + amplitude painters, pan/zoom, wrap selector; retire the old tape

**Files:**
- Create: `crates/chrona-app/src/ui/charts.rs`
- Modify: `app.rs` (replace tape_panel call with charts section), Delete: `crates/chrona-app/src/presenter/tape.rs` + its module export + `ui/instrument.rs` (now empty — WRAP_PRESETS/wrap_label move to charts.rs re-exporting from `presenter::trace::WRAP_PRESETS_MS`)

**Chart header row (mockup):** "Beat trace" semibold; legend (tick dot=tick color "Tick", accent dot "Tock", good line "Rate trend"); spacer; hint "scroll to zoom · drag to pan" faint; `+`/`−`/`Fit` buttons (28 px, panel2/border2) → `TraceView::zoom(1/1.5)`, `zoom(1.5)`, `fit()`; wrap ComboBox (±2/±5/±10/±25 ms over `WRAP_PRESETS_MS`, default ±5, labels via a `wrap_label` producing "±1"-style — reuse/adapt the T-M3 formatter: `wrap_label(10.0) == "±5 ms"`); a `Live ⏵` chip appears when `!view.follow` — click = `fit()`.

**Beat trace painter (port of the mockup's `drawBeat`, binding geometry):** panel: chartbg fill, border stroke, 12 rounding, min height 260, fills remaining vertical space; pads L46 R12 T12 B26; `X(t) = padL + pw·(t−t0)/span`, `Y(off) = padT + ph·(1 − (wrap_signed(off,wrap)+w)/(2w))` with `w = wrap_ms/2` (equivalently `0.5 − y_frac`); horizontal gridlines every `y_step_ms` (zero line grid0, others grid; labels right-aligned in the left pad, `+n`/`−n`, monospace 11); vertical gridlines every `x_step_s` with labels `-Ns`/`now` under the plot; rotated left axis title `offset (ms)` (use `egui::Painter::text` with `Rot2`? egui has no rotated text on Painter — use `painter.add(TextShape::with_angle)`: build a `epaint::TextShape` with `angle = -FRAC_PI_2`); dots: for each `BeatPoint` in window, 1.8 px radius circle (1.3 when span > 200), tick color / accent by parity, y from wrapped offset; trend: when `trend()` is Some, polyline sampled every `span/300` s of `wrap_signed(anchor + slope·(t − anchor_t), wrap)`, breaking segments when consecutive samples jump > `wrap/2` (the mockup's `moveTo` rule), good color, 1.5 px. Empty states: no nominal → centered faint caption "no beat grid — select or detect a beat rate"; no events yet → "listening…".

**Interactions:** wheel over the plot rect → `zoom(1.15^signum)` (use `ui.input scroll_delta.y`); drag → `pan(drag_dx / pw · span, newest_t)` (sign: dragging right pans back in time = mockup's `end − dt`); while `follow` the window tracks `newest_t` each frame.

**Amplitude strip:** header "Amplitude" + faint "same time scale"; 110 px tall panel; pads L46 R12 T8 B20; y-range from `amp_range`; 3 gridlines (lo/mid/hi) labeled `N°`; accent 1.5 px polyline of points within the window, with line breaks at gaps > `GAP_BREAK_S`; below-T3 → centered faint `amplitude requires Tier 3 · <gate reason>` (reason from the amplitude card's caption source).

- [ ] **Step 1: Failing tests** (geometry helpers in charts.rs, pure): `x_of/y_of` transform round-trips at the pad constants; `dot_radius(span)` table; `trend_segment_breaks_at_wrap_jump` (sampled polyline from a known Trend crosses the wrap edge → ≥2 segments, no segment jumps > wrap/2); `wrap_label` pins (`"±2 ms"`, `"±5 ms"`, `"±10 ms"`, `"±25 ms"`); amp gap-break segmentation pure fn (`segments(points, window, GAP_BREAK_S)` → counts). Keep M3's protocol pin: `WRAP_PRESETS_MS contains 4.0` (±2, protocol A.2).
- [ ] **Step 2: Run — fail.** **Step 3: Implement + wire; delete presenter/tape.rs + instrument.rs** (their still-relevant tests were already migrated in T7/T4; the constant-velocity pins die with the feature per spec §10).
- [ ] **Step 4: Gates + headless pin + launch check.** **Step 5: Commit** `feat(app): time-axis beat trace and amplitude charts with pan/zoom`

---

### Task 9: History + comparison cards, row-click replay, export flow

**Files:**
- Create: `crates/chrona-app/src/ui/history_ui.rs`
- Modify: `app.rs` (bottom grid section; export button flow; upsert-on-finalize wiring from T5 confirmed), `ui/toolbar.rs` (Export click → this flow)

**History card (mockup):** panel card "Session history" + right faint day label (`day_label` of newest entry); column header row (WATCH/POS/TIME/RATE/BEAT ERR/AMPL, 11 px faint uppercase, grid 1fr/44/90/64/64/54); up to 50 rows: watch (ellipsized), pos code mono, time (`HH:MM · N min` from started_unix_s local + summary.duration_s rounded to min, mono-muted), rate/err/amp right-aligned mono (`+12.0`, `0.8`, `270°`; `—` when absent). Row hover = panel2 bg; CLICK = send `SwitchSource(ReplayFile{path})` (same as Open recording…). Empty state: faint "no sessions yet — record one".

**Comparison card (mockup):** "Position comparison" + right `Δ spread N.N s/d` (or `—`); 6 rows (all canonical positions): code mono (accent when == current strip selection, else muted), track (8 px, panel2, rounded) with center line (border2) and deviation bar (`bar_frac`, accent when current position else border2, left half for negative), right rate mono (`+N.N` / `—`). Footer faint caption per mockup. Empty state when selected watch has no rated sessions: faint "record sessions in different positions to compare".

**Export flow:** Export button (T6) → `rfd::FileDialog::save_file()` with `default_report_name`; on path: `std::fs::write(render_report(...))`; error → Error banner (app-side banner slot — note: app currently displays only engine banners; add a transient app-side banner field (severity, text, shown until next action) rendered by the same strip); success → Info banner "report saved to <name>".

- [ ] **Step 1: Failing tests:** row time formatting pure fn (`session_time_label(started, Some(126 s)) == "HH:MM · 2 min"`, no-summary → duration omitted); comparison row model builder from `PositionRates` (accent flag by selected position, `—` handling); app-banner precedence pure fn (app banner shows only when engine banner is None — engine truth outranks UI notices).
- [ ] **Step 2: Run — fail.** **Step 3: Implement + wire (this completes the mockup's full layout).**
- [ ] **Step 4: Gates + headless pin + launch check.** **Step 5: Commit** `feat(app): session history and position comparison cards; report export flow`

---

### Task 10: Assembly polish — layout order, scroll behavior, theme completeness sweep, dead-code cleanup

**Files:**
- Modify: `app.rs`, any straggler in `ui/`, `crates/chrona-app/tests/flags.rs` (only if assertions reference removed UI — headless strings must be untouched)

- [ ] **Step 1:** Final layout pass to the mockup's order/spacing inside a `CentralPanel` + outer `ScrollArea::vertical` (charts section keeps `min 260 px` and grows; page scrolls when short). Verify: every `Color32` literal lives in theme.rs (grep `from_rgb` — only theme.rs); theme toggle restyles EVERYTHING live (both directions) including chart colors next frame; no `ui/instrument.rs`/`session_panel.rs`/`presenter/tape.rs` references remain; `cargo tree`-level: no new deps were added beyond the plan.
- [ ] **Step 2:** Run the full manual-A smoke yourself headlessly: gates + headless pin + `--simulate` launch compiles.
- [ ] **Step 3:** `grep -rn "from_rgb" crates/chrona-app/src | grep -v theme.rs` → empty; `grep -rn "error_banner" crates/chrona-app` → empty.
- [ ] **Step 4: Commit** `refactor(app): redesign assembly polish and cleanup`

---

### Task 11: Docs — manual protocol, README, AGENTS, followups discharge

**Files:**
- Modify: `docs/manual-testing.md`, `README.md`, `AGENTS.md`, `docs/superpowers/notes/m3-followups.md`

- [ ] **Step 1:** manual-testing.md: A.2 append "(wrap selector now on the chart header; ±2 ms preset)"; A.3 "Lift preset 38" → "Set lift to 38 in the toolbar"; new **A.6**: "Add a watch (toolbar → + Add new watch…, name 'Test SKX'), select position DD, record ~10 s, stop. EXPECT: a Session history row (watch, DD, rate/err/amp matching the cards), a DD bar in Position comparison, and Export report saves an HTML file that opens in a browser showing the same numbers. Click the history row: it replays."; B.1 note "(position picker in the strip replaces the old free-text position)". Also A.2's expected look: "four metric cards + signal meter 'Strong · full regression'" replacing the numerals-strip wording, and "tape" → "beat trace (rising slope for +12 s/d)". Sweep the whole file for now-wrong UI wording (e.g. banner colors: info banners are no longer red).
- [ ] **Step 2:** README App section: mention watch library, session history, position comparison, export, theme toggle, new charts (one short paragraph); screenshot placeholder NOT added. AGENTS.md pinned-strings note: add the four signal-meter labels and "headless line frozen" reminder (already there — extend the example list). m3-followups.md: mark DISCHARGED in place (one-line annotations): typed banner severity (#3 first-week), blank-tape caption (charts have explicit empty states), "-0.0 s/d" if fixed by format changes — check `format_rate` behavior; if unchanged leave it listed.
- [ ] **Step 3:** Gates (docs don't break code, but run anyway) + commit `docs: manual protocol + README + AGENTS for the redesigned UI`

---

## Self-review notes (writing-plans checklist)

- Spec coverage: §1→T1, §2→T6/7/8/9/10, §3→T4/T7, §4→T3/T6, §5→T2/T6, §6→T2/T3, §7→T5/T9, §8→T5/T9, §9→T5/T9, §10→T4/T8, §11→T4/T8, §12→T6/T7, §13 tests distributed per task, §14 deviations encoded in T4 (wrap presets), T6 (averaging/BPH combo), T8 (dynamic amp range), §15→T11. No gaps found.
- Type consistency: `Banner`/`BannerSeverity` (T3) consumed by T6/T9 by those names; `SessionSummary` (T2) in T3/T5; `BeatAccum/AmpAccum/TraceView/Trend` (T4) in T8; `HistoryIndex/PositionRates/SessionEntry/POSITIONS` (T5) in T7 (POSITIONS)/T9; `Palette/Theme` (T1) everywhere.
- Placeholders: none; all constants, formulas, strings, and test cases are stated or point at the committed mockup asset for verbatim copy.
