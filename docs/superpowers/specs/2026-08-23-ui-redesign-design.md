# Chrona UI Redesign (M4a) — Design

**Status:** draft for user review. Amends the binding spec's §6 (UI) and §8 (sidecar);
everything else in `2026-08-20-chrona-timegrapher-design.md` stands unchanged.

**Visual authority:** the user's Claude Design mockup, committed verbatim at
[assets/2026-08-23-chrona-redesign.dc.html](assets/2026-08-23-chrona-redesign.dc.html).
Where this document is silent on look/spacing/copy, the mockup decides. Where data,
honesty, or feasibility require deviating from the mockup, this document decides and
lists the deviation in §14.

**Scope:** the FULL mockup — restyle plus the new subsystems it implies (watch library,
session history, position comparison, export report, chart pan/zoom). This pulls the
"positions summary/export" slice of M4 forward; Mic Doctor, calibration wizard UI, and
scope view remain future M4 work.

## 1. Visual system

- **Palette** — the mockup's OKLCH tokens converted to exact sRGB (computed via CSS
  Color 4 math; these hex values are binding):

  | token | dark | light | | token | dark | light |
  |---|---|---|---|---|---|---|
  | bg | `#090e11` | `#f2f4f5` | | accent | `#4ab8e8` | `#007cb8` |
  | panel | `#12171a` | `#ffffff` | | accent_ink | `#080c0f` | `#fcfcfc` |
  | panel2 | `#1b2024` | `#eceff1` | | tick | `#e7b643` | `#ce871b` |
  | border | `#252a2d` | `#d5d8da` | | good | `#53be70` | `#25984d` |
  | border2 | `#33393d` | `#babec1` | | warnbg | `#3f2903` | `#f7e6c3` |
  | text | `#e5e8eb` | `#1b2024` | | warnfg | `#f2c86c` | `#8a5600` |
  | muted | `#81878c` | `#5e6468` | | rec | `#c92f33` | `#c92f33` |
  | faint | `#646a6e` | `#757b80` | | grid | `#1a1d20` | `#e2e5e7` |
  | chartbg | `#070a0c` | `#fbfcfd` | | grid0 | `#363b3f` | `#babec1` |

  One `Palette` struct with `dark()`/`light()` constructors; every widget draws from it
  (no ad-hoc `Color32::from_rgb` outside the palette module).
- **Fonts** — embedded via `include_bytes!`: Archivo Regular/SemiBold/Bold + Fragment
  Mono Regular (`crates/chrona-app/assets/fonts/`, ~440 KB total, each with its OFL
  license file; source: Google Fonts static TTFs). egui families: `Proportional` →
  Archivo Regular (SemiBold/Bold as named families for headings/emphasis), `Monospace`
  → Fragment Mono. All numerals, codes, and timestamps render monospace per the mockup.
- **Theme** — dark is default; toolbar toggle (☾ Dark / ☀ Light) switches instantly and
  persists as `theme = "dark" | "light"` in `config.toml` (invalid value → dark).
- Panels: 12 px rounding, 1 px `border` stroke; pills, segmented controls, modals per
  the mockup.

## 2. Layout (top → bottom)

Toolbar · banner strip (see §4) · position/session strip · metric-card band · beat
trace · amplitude strip · history + position-comparison grid · modals (help, add
watch). The mockup's toolbar buttons map: theme toggle, "Open recording…" (existing
rfd flow), "Export report" (§9), Record/Stop (existing engine flow, new styling with
pulsing dot while recording).

## 3. Honesty mapping (binding — spec §3.1 survives restyle)

- **Metric cards**: value renders ONLY when the tier grants it; otherwise the numeral
  area shows `—` and the caption line shows the existing gate reason (restyled, same
  text source as M3's presenters). The `uncal` pill appears on the Rate card iff a rate
  is shown and `calibrated == false` (tooltip: mockup's wording). The Beat-rate card's
  corner tag is `auto` / `fixed` / `free` from the BPH mode; the rate-source caption
  (period-slope vs regression) is kept from M3.
- **Signal meter** (position strip): bars lit by tier — none = 0, T1 = 1, T2 = 2,
  T3 = 3, plus the 4th bar iff T3 with `detection_ratio ≥ 0.8`. Labels (new pinned
  strings): `No signal` (faint), `Weak · rate only` (warn color), `Fair · rate + beat
  error` (warn color), `Strong · full regression` (good color, 3 or 4 bars).
- **Charts** never fabricate: dots/lines only where events/metrics existed; gaps > 2 s
  in a line series break the line rather than interpolating. Amplitude strip below T3
  shows the empty state with the gate reason.
- **Headless output is frozen**: `--headless-seconds` still prints the M3 line
  byte-identically (`TIER 3 · full  rate +12.0 s/d ⚠ uncal  …`). The redesign changes
  GUI presenters only; `format.rs`'s headless/tier functions and their pin tests stay.
  GUI-only format changes: bph numerals grouped with a thin space (`28 800`).

## 4. Banners and replay (M3 semantics survive)

A banner strip sits directly under the toolbar. `EngineSnapshot.error_banner:
Option<String>` becomes `banner: Option<(BannerSeverity, String)>` with `enum
BannerSeverity { Info, Warn, Error }` — this discharges the m3-followups "typed
severity" ticket. Mapping: replay-restored-settings = Info (accent tint), silence /
clipping / overrun notices = Warn (warn colors), stream/config/fault errors = Error
(rec red). Replay mode keeps its blue REPLAY line + `· done`, restyled into the
position/session strip's right side beside the elapsed timer.

## 5. Watch library

- `ConfigStore` gains `watches: Vec<String>` (ordered, unique, trimmed, non-empty) and
  `last_watch: Option<String>`. Toolbar combo lists watches + `+ Add new watch…`;
  choosing it opens the add-watch modal (mockup copy; Enter saves, Esc cancels, blank
  rejected, duplicate selects the existing entry). Selected watch persists and stamps
  new recordings. No watch selected (fresh install) → combo shows `No watch` and
  recordings simply carry no watch (never blocks recording).

## 6. Sidecar schema v2 (amends spec §8)

`SessionMeta.schema_version: 2`, adding:

```rust
watch: Option<String>,
recorded_at: Option<String>,      // RFC3339 with local offset, set at finalize
summary: Option<SessionSummary>,  // last MetricsSnapshot at StopRecording
// SessionSummary { tier: String ("T0".."T3"), rate_s_per_day: Option<f64>,
//   beat_error_ms: Option<f64>, amplitude_deg: Option<f64>,
//   bph_detected: Option<f64>, duration_s: f64 }
```

The summary is *what the instrument showed when the recording stopped* — display-only
provenance for the history table; replay always recomputes real metrics from audio.
Reader: all new fields `#[serde(default)]`, so v1 sidecars load with `None`s (test
required); unknown future fields already tolerated. Writer always writes v2. The engine
supplies the summary from its last published snapshot at `StopRecording` finalize.

## 7. Session history

- Index source of truth = the recordings directory: scan `*.json` sidecars at startup
  (background-friendly: it's a directory listing + small JSON reads at app start) and
  upsert after each finalized recording. Entry order: `recorded_at` desc, falling back
  to the filename's civil timestamp, then file mtime.
- Table per the mockup (Watch / Pos / Time / Rate / Beat err / Ampl); missing values
  render `—`; the header's right label shows the newest entry's day (Today / Yesterday
  / date). Display cap 50 rows. Clicking a row starts replay of that session (same path
  as "Open recording…").

## 8. Position comparison

For the selected watch: the most recent summary rate per canonical position (DU DD CU
CD CL CR). Bars per the mockup: deviation from 0 s/d, fixed ±15 s/d scale, clamped at
full bar; center line = 0; current position highlighted accent. `Δ spread` = max−min
over positions that have a rate; with fewer than 2 rated positions it shows `—`.
Positions without data: dimmed code, empty bar, `—`. Sessions whose summary has no rate
are excluded (never guessed).

## 9. Export report

Toolbar button → `rfd` save dialog, default name
`chrona-report-<watch-slug>-<YYYY-MM-DD>.html`. Output: ONE self-contained HTML file
(inline CSS only, no JS, no external requests, printable) containing: watch name,
export date, app version, current config (lift, BPH mode, averaging, cal ppm with an
explicit *uncalibrated* warning when ppm = 0), the position-comparison table with Δ
spread, the full session-history table for that watch, and a footnote explaining tiers
and that metrics were recorded at each session's end. Button disabled with a tooltip
when no watch is selected or the watch has no indexed sessions. Write errors surface as
an Error banner.

## 10. Beat trace (replaces the M3 paper tape)

- **X axis is time** (seconds), labels `-Ns` … `now`; span 15–300 s (default 180);
  live mode keeps `end = now` unless the user pans back (then a `Live ⏵` chip appears —
  clicking or panning fully right re-follows). Wheel = zoom span (×1.15 steps), drag =
  pan, `+`/`−` buttons = ×1.5 span steps, `Fit` = span 180 & re-follow. Hint text per
  mockup.
- **Y axis**: beat-grid offset in ms, wrapped to ±(wrap/2); gridline every step (0.5/1/
  2/5 ms by wrap), zero line brighter (`grid0`), labels left, rotated `offset (ms)`
  axis title. Wrap presets `±2 / ±5 / ±10 / ±25 ms` (`wrap_ms` 4/10/20/50), default ±5
  — the mockup's set; M3's ±1 is dropped (§14).
- **Data**: a UI-side accumulator (in `ChronaApp`, fed from each snapshot's
  `tape` — the engine/analyzer are untouched) keyed by `t_drop_corr_s`, appending only
  events newer than the last kept, pruned to 300 s. Tick dots = `tick` token, tock =
  `accent` (replacing M3's hardcoded amber/blue — same semantic split). Dot radius 1.8
  px (1.3 px when span > 200 s).
- **Rate trend**: green wrapped line with slope `rate_s_per_day × 1000 / 86400` ms/s,
  anchored to the median offset of the last 10 s of events; drawn only when a rate
  exists. Legend per mockup.
- The M3 constant-velocity index-x presenter (`tape_dots`) and its pin tests are
  retired with this feature (the time-x transform gets its own pinned tests, §13).

## 11. Amplitude strip

Same time axis/window as the beat trace ("same time scale" per mockup). Data: one
`(t, amplitude_deg)` point per second sampled from snapshots when amplitude is `Some`;
pruned with the same 300 s horizon; gaps > 2 s break the line. Y range: dynamic —
smallest `[30°·k, 30°·m]` band containing the data with span ≥ 60°; three labeled
gridlines (lo/mid/hi). Below T3 the strip shows `amplitude requires Tier 3 · <reason>`.

## 12. Help popups, positions, misc

- Help modal per metric card ("?" buttons): title/what/why/good-range copy **verbatim**
  from the mockup's `helpTopics`. Click-outside and ✕ close (Esc too).
- Position segmented control (DU/DD/CU/CD/CL/CR, two-line buttons per mockup) replaces
  M3's free-text position; the selection stamps `meta_position` on recordings and
  highlights the comparison row. (Spec §6's six canonical positions.)
- Toolbar: lift angle becomes a monospace numeric field (drag/type), sanitized
  10–90 (M3's preset menu is retired, §14); BPH combo **Auto / Free / 18000 / 21600 /
  25200 / 28800 / 36000 / Other…** — "Other…" reveals M3's validated free-text Fixed
  field (M3 exposed Auto/Free/Fixed-text; the mockup's fixed-value list alone would
  regress that, §14); averaging combo **10 s / 30 s / 60 s** (§14); `cal {ppm:+.1} ppm`
  monospace text.
- Elapsed timer `● MM:SS` in the strip (engine-confirmed recording, M3 semantics).

## 13. Architecture & testing

- **Threading unchanged**: UI reads snapshots; engine owns DSP; the DSP crate is
  untouched. Engine changes limited to: `StartRecording` meta gains `watch`;
  finalize writes the v2 summary from the last snapshot; typed `BannerSeverity`.
- New/changed modules in `chrona-app`: `theme.rs` (palette + fonts + egui style),
  `ui/charts.rs` (beat trace + amplitude painters), `ui/cards.rs`, `ui/toolbar.rs`,
  `ui/strip.rs`, `ui/history.rs`, `ui/modals.rs`, `presenter/trace.rs` (time-x
  transform, wrap math, trend line), `history.rs` (index scan/upsert),
  `export.rs` (report HTML); `chrona-session`: sidecar v2, config fields.
- Tests (all headless; GUI paint fns stay thin over tested pure helpers): palette
  completeness (every token both themes); signal-meter mapping (all tiers + threshold);
  accumulator append/dedupe/prune/gap-break; time-x + wrap transforms (pinned
  round-trips); trend-line slope math; watch add/trim/dedupe/persist round-trip;
  sidecar v2 round-trip + v1-tolerance + summary honesty (None stays None); history
  index ordering incl. fallbacks + missing-metrics rows; comparison latest-per-position,
  spread, clamp, absent-position; export HTML (contains watch/rows/uncal warning, no
  `http` substring, valid utf-8); banner severity mapping; wrap presets pin (±2
  present); headless line byte-identical pin (unchanged).

## 14. Recorded deviations from the mockup

1. Averaging option **120 s is dropped** (analyzer's valid range is 2–60 s; >32 s is
   ring-truncated anyway). Options: 10/30/60 s.
2. Wrap preset **±1 ms (M3) is dropped**; mockup's ±2/±5/±10/±25 adopted. Manual
   protocol A.2 still works (±2 exists).
3. Mockup's static "Strong · full regression" strip is dynamic per §3 (tier-driven).
4. Mockup's Input combo lists sample devices; the real device enumeration + System
   default (M3) is used.
4b. Mockup's BPH combo lists only Auto + four fixed values; the app keeps M3's Free
   mode and validated free-text Fixed entry behind an "Other…" item (§12) — dropping
   them would regress working diagnostics.
5. Mockup's history "today" header label generalizes to Today/Yesterday/date.
6. Amplitude strip y-range is dynamic (mockup hardcodes 240–300°).
7. Position-comparison sample data (mockup) replaced by real per-watch index data;
   with no data the card shows an explanatory empty state.
8. Help copy correction: none needed (mockup's horology copy verified: 28,800 bph =
   8 beats/s = 4 Hz oscillation; COSC −4/+6; both correct).
9. The mockup's `support.js` React-like runtime is preview scaffolding only — nothing
   from it is ported.

## 15. Protocol & docs updates (in this milestone)

`docs/manual-testing.md`: A.2 unchanged in spirit (wrap ±2), A.3 becomes "set lift to
38", new A.6 (add watch → record 10 s → row appears in history → comparison bar for its
position → export report opens/contains it), B note for the position picker. README
screenshot-worthy feature list line. AGENTS.md: pinned-strings note extended to the new
signal labels.

## 16. Out of scope (unchanged M4 backlog)

Mic Doctor meters, calibration wizard UI, scope view, classic-pattern legend overlay,
localization, packaging/icons.
