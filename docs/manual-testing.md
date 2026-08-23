# Chrona manual test protocol (M3 — live app)

Run before tagging any release. Needs: a mechanical watch, a quartz watch, ~15 min.

## A. Simulate mode (no hardware)
1. `cargo run -p chrona-app -- --simulate --rate 12 --beat-error 0.8 --amplitude 270`
2. EXPECT within ~10 s: TIER 3 badge, rate ≈ +12.0 s/d ⚠ uncal (⚠ uncal assumes a fresh config —
   a previously saved ppm for your last device suppresses it), BE ≈ 0.8 ms, amp ≈ 270°,
   two-color tape: two parallel bands sloping gently. Change wrap to ±2 ms: bands separate visibly.
3. Lift preset 38: amplitude drops to ≈ 197°, rate unchanged. Set back to 52.
4. Record 10 s (position DU) → stop. EXPECT `● REC` while on, file named chrona-<ts>.wav
   in the recordings dir with a .json sidecar (open it: position "DU", lift 52).
5. Open recording… select that file. EXPECT the blue REPLAY line in the session panel, same metrics, '· done' when finished; Back to live works. (Back to live switches to the *microphone*, not back to simulate — on macOS the mic-permission prompt may appear here; restart with `--simulate` to return to the demo. On a machine with no working input device this step ends with a red source-failure banner instead — expected.) (Health banners like clipping may still appear during replay — by design.) The replay announces "replay: using recorded settings (lift X°, ppm Y)" when a sidecar is present.

## B. Live microphone
1. `cargo run -p chrona-app` (default device). First run on macOS: the mic-permission
   prompt must appear (unless you already granted it during A.5) (it attributes to your terminal app); grant it.
2. Silence on the desk: EXPECT "listening…" placeholder, then the amber no-signal
   banner after ~3 s if the room is truly quiet at the mic. No crash, no numerals.
3. Place a running mechanical watch case-back on the mic (laptop: near the keyboard
   top edge). EXPECT within ~30 s: at least TIER 1 with a plausible BPH label; with a
   quiet room / contact coupling, TIER 2-3.
4. Tap the mic hard: clipping banner appears (red), clears ~5 s after you stop.
5. Unplug/replug a USB mic (if available): error banner → auto-reconnect (or pick it
   again in the combo after refresh).
6. Record 60 s of the watch, replay it: tiers/metrics comparable to live.
7. Set the ppm you measured with `chrona calibrate` (CLI) for this device: rate badge
   drops the ⚠ uncal marker; per-device ppm persists across app restarts.

## C. Cross-checks
- `cargo run -p chrona-app -- --simulate --headless-seconds 45` prints TIER 3 metrics and exits 0.
- CLI vs app: `chrona analyze` on the B.6 recording matches the app's replay numbers.

Record results (pass/fail + notes) in the PR/commit message that tags the milestone.
