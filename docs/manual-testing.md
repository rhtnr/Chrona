# Chrona manual test protocol (M4a — live app)

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
   and the toolbar's Export report button now enabled. Click Export report, save;
   EXPECT a self-contained .html file that opens in a browser and shows the same
   watch/position/rate numbers. Click the new history row: EXPECT it to replay (same
   REPLAY-line behavior as step 5).

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
7. Set the ppm you measured with `chrona calibrate` (CLI) for this device via the
   toolbar's "cal N.N ppm" popup (click it to open a small calibration field): the RATE
   card's "uncal" pill disappears; per-device ppm persists across app restarts.

## C. Cross-checks
- `cargo run -p chrona-app -- --simulate --headless-seconds 45` prints TIER 3 metrics and exits 0.
- CLI vs app: `chrona analyze` on the B.6 recording matches the app's replay numbers.

Record results (pass/fail + notes) in the PR/commit message that tags the milestone.
