# Chrona manual test protocol (M4 — live app)

Run before tagging any release. Needs: a mechanical watch, a quartz watch, ~15 min.

## A. Simulate mode (no hardware)
1. `cargo run -p chrona-app -- --simulate --rate 12 --beat-error 0.8 --amplitude 270`
2. EXPECT within ~10 s: the four metric cards — RATE ≈ +12.0 s/d with an "uncal" pill
   (uncal assumes a fresh config — a previously saved ppm for your last device
   suppresses it), BEAT ERROR ≈ 0.8 ms, AMPLITUDE ≈ 270° — plus a "Strong · full
   regression" signal meter (4 bars lit). The beat trace shows a rising slope for
   +12 s/d (fast = up), tick/tock dots colored per the header legend, with a green
   rate-trend line through them. Change wrap to ±2 ms (the wrap selector now lives in
   the chart header, presets ±2/±5/±10/±25 ms, default ±5): the band narrows and the
   trace wraps more often.
3. Set lift to 38 in the toolbar (a numeric drag/type field now — the old preset menu
   is gone): amplitude drops to ≈ 197°, rate unchanged. Set back to 52.
4. Select position DU in the strip's position picker (DU/DD/CU/CD/CL/CR buttons —
   replaces the old free-text position field), then Record → wait 10 s → Stop. EXPECT
   the toolbar button turn into a red pulsing "Stop", a "● mm:ss" elapsed clock in the
   strip while recording, and afterward a file named chrona-<ts>.wav in the recordings
   dir with a .json sidecar (open it: position "DU", lift 52).
5. Open recording… select that file. EXPECT the blue REPLAY line in the position strip,
   same metrics, '· done' when finished; Back to live (the button beside the REPLAY
   line) works. (Back to live switches to the *microphone*, not back to simulate — on
   macOS the mic-permission prompt may appear here; restart with `--simulate` to return
   to the demo. On a machine with no working input device this step ends with a red
   source-failure banner instead — expected.) (Health banners like clipping may still
   appear during replay — by design.) The replay announces "replay: using recorded
   settings (lift X°, ppm Y)" when a sidecar is present.
6. Add a watch: toolbar watch combo → "+ Add new watch…" → type "Test SKX" → Save
   watch. Select position DD in the strip's position picker. Record ~10 s → Stop (the
   Session history row only appears once Stop finalizes the sidecar, a moment later).
   EXPECT: a new Session history row (watch "Test SKX", pos DD, rate/beat error/
   amplitude matching what the metric cards showed), a DD bar in Position comparison,
   and the toolbar's Export report button now enabled. The row's HH:MM is your
   machine's LOCAL wall-clock time now (it used to be UTC) — sanity-check it against
   your own clock. Click Export report, save; EXPECT a self-contained .html file that
   opens in a browser and shows the same watch/position/rate numbers. Click the new
   history row: EXPECT it to replay (same REPLAY-line behavior as step 5).
7. Open the "Scope" section (collapsed by default, above the Session history/Position
   comparison cards — click the "▸ Scope" header). EXPECT it flips to "▾ Scope" and a
   stacked-waveform panel appears below: the newest detected beat on top, older beats
   below it, each trace tinted per the header's own Tick/Tock legend dots. Every row
   shows a "drop" marker (thin vertical line); the newest (top) row ALSO gets an
   "unlock" marker when the unlocking pulse was resolved, and is the only row with
   "drop"/"unlock" TEXT labels — older rows show the markers unlabeled. Each row has
   its own faint "snr N dB" readout, top-right. Collapse it again: EXPECT it flips back
   to "▸ Scope" and the panel disappears.
8. Click the "?" button in the beat-trace header, after the Tick/Tock/Rate-trend legend
   items. EXPECT a "Classic tape patterns" modal listing six rows, each a small drawn
   schematic plus a one-line meaning: Level line (healthy/on-rate), Sloped line (fast/
   slow), Two parallel bands (beat error), Wavy line (periodic gear/pivot fault),
   Scattered dots no line (weak/dirty signal — check Mic Doctor), Sudden vertical jumps
   (rebanking/knock). Esc, the ✕ button, or a click outside the card all close it.

## B. Live microphone
1. `cargo run -p chrona-app` (default device). First run on macOS: the mic-permission
   prompt must appear (unless you already granted it during A.5) (it attributes to your terminal app); grant it.
2. Silence on the desk: EXPECT "listening…" placeholder, then the amber no-signal
   banner after ~3 s if the room is truly quiet at the mic. No crash, no numerals.
3. Place a running mechanical watch case-back on the mic (laptop: near the keyboard
   top edge). EXPECT within ~30 s: at least the "Weak · rate only" signal meter (1 bar)
   with a plausible BEAT RATE card reading; with a quiet room / contact coupling,
   "Fair · rate + beat error" or "Strong · full regression" (2-4 bars).
4. Tap the mic hard: clipping banner appears (amber warning, not red — only the Error
   severity is red now), clears ~5 s after you stop.
5. Unplug/replug a USB mic (if available): error banner → auto-reconnect (or pick it
   again in the combo after refresh).
6. Record 60 s of the watch, replay it: tiers/metrics comparable to live.
7. Click the toolbar's "cal N.N ppm" text to open the calibration popup and manually
   enter a ppm (e.g. one you measured with `chrona calibrate` — CLI, now the documented
   alternative to the in-app wizard in step 9 below): the RATE card's "⚠ uncal" pill
   disappears; per-device ppm persists across app restarts. Let the mic run
   uninterrupted for ~2 minutes: EXPECT the same popup to grow a faint
   "system-clock cross-check: +N.N ppm over N min" line (sign flips negative as
   needed) with a "use as correction" button below the ppm field — click it and EXPECT
   the ppm field to jump to the shown value (this is a rough NTP-style sanity check
   against your system clock, not a substitute for the wizard).
8. Click the toolbar's "Mic Doctor" button (its own top-level button, beside "Open
   recording…"/"Export report" — NOT inside the cal-ppm popup). Run each of the three
   steps (Silence 5s, Tick 10s, AGC 10s) against the real mic in turn — only one can run
   at a time, and the other two Run buttons disable while it does. EXPECT each step to
   show a Pass/Warn/Fail verdict plus detail text once it finishes, "not run" before you
   run it. In a genuinely quiet room, the Silence step should NOT report hum unless your
   room actually has 50/60 Hz mains interference nearby — a false hum report in a quiet
   space is a bug worth filing. With a running watch against the mic, the Tick step
   should reach at least Tier 1. Once ≥ 1 step has run, EXPECT a "Setup score: N/100"
   line plus ranked advice bullets (or "No issues found…" if nothing tripped).
   Switching input device/source clears all three results back to "not run" the next
   time you open the panel. Close via Close, Esc, or a click outside the card.
9. Open the calibration wizard: toolbar's "cal N.N ppm" popup → "Calibrate…". EXPECT an
   intro screen ("clamp any quartz watch…"), then "Start capture" begins a live elapsed
   clock plus a signal reading. The Analyze button stays disabled, with an "Available in
   MM:SS" countdown, until 5 minutes (300 s) have elapsed — 10–15 minutes is
   recommended for a tighter fit. Clamp a REAL QUARTZ watch to the pickup for the full
   5+ minutes, then Analyze: EXPECT a result screen showing "+N.N ppm ± N.NN ppm"
   (signed measured ppm, then the fit residual) plus event count and capture duration,
   in the same ballpark as what `chrona calibrate` reports for the same setup. Click
   "Save for <device>": EXPECT the toolbar's ppm to update and the RATE card's
   "⚠ uncal" pill to disappear. Now re-run the wizard against a MECHANICAL watch
   instead: EXPECT it to refuse — "Calibration failed" plus advice that this looks like
   a mechanical watch, not quartz — rather than silently accepting a plausible-looking
   but bogus ppm.

## C. Cross-checks
- `cargo run -p chrona-app -- --simulate --headless-seconds 45` prints TIER 3 metrics and exits 0.
- CLI vs app: `chrona analyze` on the B.6 recording matches the app's replay numbers.

Record results (pass/fail + notes) in the PR/commit message that tags the milestone.
