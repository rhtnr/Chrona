# M4a follow-ups (from the M4a UI-redesign final review, 2026-08-23)

Everything here was reviewed and deliberately deferred — none of it blocked the M4a merge.
The M3 backlog lives in m3-followups.md and REMAINS the M4-planning start point for
non-UI work; M4a discharged only its typed-banner-severity, blank-tape-caption, and
info-banners-render-red items (annotated there). This file adds the redesign's own debt.

## Still first in line for M4 proper (unchanged from m3-followups)

- `parabolic3` unification (FIFTH milestone without it — M4a was UI-scoped, so it rode
  again; it must lead the next DSP-touching milestone).
- Engine survives *total* startup failure (keep the loop alive with a degraded source).
- Recording tee swallows `writer.push` errors; overrun banner latches; the
  `chrona_session::replay` vs engine-path semantic divergence; `assert_no_alloc`.

## New accepted characteristics from the redesign (deliberate, documented)

- Toolbar does not wrap on narrow windows (mockup's CSS wraps; egui right-aligned
  cluster fights `horizontal_wrapped`) — revisit only if real-window use bites.
- Metric cards: 4-up with a 2×2 fallback below 800 px total width (mockup auto-fits
  continuously).
- App-side banner clears on ANY pointer click (broader than the spec's "next user
  action"; strictly safer — never stale).
- Theme toggle repaints the toolbar's own row one frame late (palette captured at
  row start; immediate-mode convention, self-correcting).
- High-BPH edge: at 72,000 bph the beat half-period (25 ms) equals the ±25 ms wrap
  radius, so k-rounding vs wrap display can alias at that extreme (documented in
  `presenter/trace.rs`; spec-accepted).
- Beat-grid k-rounding rides through detection gaps by time-delta rounding; a gap
  containing a genuine rate step larger than half a beat period would re-anchor with a
  one-beat offset (cosmetic, self-corrects at the next refold-independent grid reset).
- Session-history table is a deliberate global log (newest 50 across all watches);
  only the position-comparison card is watch-scoped.
- `day_label`/`session_time_label` use UTC day bucketing (GUI label approximation).

## Process notes

- Report-integrity incidents this milestone: Task 5's report had wrong test totals and
  a TDD narrative contradicted by its own transcript (sonnet-tier; caught by review
  ground-truthing; "TDD claims must match your transcript" dispatch language added
  mid-milestone and honored by every later task).
- The wheel-zoom sign inversion (T8) is the milestone's marquee catch: egui's
  smooth_scroll_delta sign is opposite DOM deltaY for the same gesture — now documented
  in charts.rs; remember it for any future scroll-wheel feature.
- Visual acceptance remains human-only: no sandbox agent could composite a window all
  milestone. The manual protocol (A.1–A.6) is the visual gate.

## From the final whole-branch review (ride-to-next-milestone, priority order)

1. **Local-time display for history rows and the day label.** `session_time_label`
   derives HH:MM from `started_unix_s % 86400` (UTC) and `day_label` buckets by UTC
   day — every non-UTC user sees clock times shifted by their full TZ offset (a 14:32
   IST session lists as 09:02). Ordering, durations, and honesty are unaffected. The
   fix needs a timezone source (std has none): decide `time` crate vs libc at the next
   milestone's dependency review. (Final-review I-3; the milestone's top rider.)
2. **Beat-trace x-gridlines anchor to absolute multiples of the step**, so live
   fractional `end` yields labels like "-17s" and the "now" label almost never renders.
   Anchor gridlines to `end` instead: fixed on-screen positions, round labels, "now" at
   the right edge. (M-1.)
3. **Spec text sync**: §7's ordering-fallback wording (filename-civil vs the
   implemented started_unix_s → sidecar-mtime), §5's "No watch" vs the implemented
   "Select watch" combo text — amend both + §14 entries. (M-3, M-4.)
4. **Export "BPH mode" line echoes the raw Other-field buffer**, which can hold
   unsent/unparseable text; echo the engine-effective mode instead. (M-5.)
5. **Free-mode amplitude strip shows "listening…" forever** while the amplitude CARD
   legitimately shows a number (amplitude derives from t_osc, not the snap table; the
   strip needs the beat accumulator's time anchor, which Free never provides). Honest
   but mildly mispromising — caption should say "no beat grid" in Free mode. (M-6.)
6. **Toolbar wrap on narrow windows** (existing accepted characteristic, mockup wraps).
