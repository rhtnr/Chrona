//! Beat-trace presenter (M4a, design spec §10): a UI-owned accumulator that
//! turns a rolling window of `chrona_dsp::analyzer::TapeEvent`s into a
//! beat-offset trace spanning up to `HORIZON_S` seconds, plus the view
//! transforms (pan/zoom/wrap), trend line, amplitude range, and signal
//! meter the chart painters (Tasks 7/8) draw from. Pure functions/state
//! over `chrona_dsp` types only — no `egui` dependency (see the
//! `presenter` module's own doc comment).

use std::collections::VecDeque;

use chrona_dsp::analyzer::{MetricsSnapshot, TapeEvent};
use chrona_dsp::events::Parity;
use chrona_dsp::tier::Tier;

/// Beat-trace visible span bounds and default, in seconds (design spec
/// §10: "span 15–300 s (default 180)").
pub const SPAN_MIN_S: f64 = 15.0;
pub const SPAN_MAX_S: f64 = 300.0;
pub const SPAN_DEFAULT_S: f64 = 180.0;

/// Y-axis wrap-band presets, in ms (design spec §10: "±2 / ±5 / ±10 /
/// ±25 ms"). Each entry is the FULL wrap width — twice the ± radius named
/// in the mockup copy.
pub const WRAP_PRESETS_MS: [f64; 4] = [4.0, 10.0, 20.0, 50.0];

/// How far back (seconds) both accumulators below retain points — the
/// beat trace's and amplitude strip's shared data horizon (design spec
/// §10/§11). Matches `SPAN_MAX_S`: the full visible span stays backed by
/// real data even when panned all the way back.
pub const HORIZON_S: f64 = 300.0;

/// A gap this long (seconds) between consecutive points breaks a chart
/// line rather than interpolating across it (design spec §3/§10/§11:
/// charts never fabricate data that wasn't observed). Consumed by the
/// Task 7/8 painters, not by this module's own accumulators.
pub const GAP_BREAK_S: f64 = 2.0;

/// One plotted beat: absolute stream time, signed offset from the beat
/// grid (ms), and tic/toc parity for coloring.
#[derive(Debug, Clone, Copy)]
pub struct BeatPoint {
    pub t_s: f64,
    pub offset_ms: f64,
    pub is_tic: bool,
}

/// Running beat-grid state, kept OUTSIDE the pruned `points` deque so the
/// `t0`/`k_prev` bookkeeping survives pruning (see `BeatAccum::push_point`
/// — the grid must outlive any individual point being dropped from the
/// front).
#[derive(Clone, Copy)]
struct Grid {
    /// Anchor time: the first kept event's `t_unlock_s`.
    t0: f64,
    /// The most recently kept event's `t_unlock_s` (for computing the next
    /// event's time delta).
    t_prev: f64,
    /// The most recently kept event's running grid position.
    k_prev: i64,
}

/// Accumulates a beat-offset trace across MANY snapshots (up to
/// `HORIZON_S` seconds), independent of `chrona_dsp::Analyzer`'s own
/// ~32 s tape ring.
///
/// **Why an incremental grid, not `TapeEvent::beat_index`:** `beat_index`
/// re-bases every time the analyzer refolds (see `TapeEvent`'s own doc
/// comment in `chrona_dsp::analyzer`) — it's only a stable ordinal WITHIN
/// one snapshot's tape, not a persistent counter across snapshots or over
/// time. Two events from different `extend()` calls can carry the exact
/// same `beat_index` while representing beats seconds apart. So this
/// accumulator can't anchor its beat grid on `beat_index` at all; instead
/// it rebuilds its own running grid position (`Grid::k_prev`) purely from
/// TIME deltas between consecutive KEPT events' `t_unlock_s` — which IS
/// comparable across calls, since it's an absolute nominal-clock
/// timestamp — rounding each delta to the nearest whole number of beats so
/// a detection gap (missed beats) doesn't break grid continuity (see the
/// binding offset math on `extend` below).
///
/// **Why the offset sign is "fast rises":** a fast watch's beats arrive
/// EARLY, so at any kept event the nominal grid's predicted elapsed time
/// (`k · T_beat`) has run ahead of the watch's actual elapsed time
/// (`t_unlock − t0`) — `offset = k·T_beat − actual` is therefore POSITIVE
/// and grows over time for a fast watch, matching the mockup's and
/// Watch-O-Scope's up-is-fast reading. This is the opposite sign from M3's
/// now-removed paper-tape presenter (`dev = actual − nominal`, i.e. down-
/// is-fast there) — a deliberate, binding convention change for M4a's
/// redesigned chart, not an inconsistency to reconcile.
#[derive(Default)]
pub struct BeatAccum {
    points: VecDeque<BeatPoint>,
    bph_nominal: Option<u32>,
    grid: Option<Grid>,
    /// The largest `t_drop_corr_s` ever SEEN (placed on the grid or
    /// skipped for lacking `t_unlock_s`) — the dedupe watermark. Tracked
    /// separately from `grid.t_prev` (which only advances on PLACED
    /// events) so an unplaceable event still keeps overlapping snapshots
    /// from being re-processed.
    max_seen_t_drop: Option<f64>,
}

impl BeatAccum {
    /// Feeds one snapshot's tape into the accumulator (design spec §10).
    /// `bph_nominal: None` (Free mode, or no snap yet) appends nothing —
    /// there's no defensible beat period to grid against. A CHANGE in
    /// `bph_nominal` since the last call invalidates the existing grid (a
    /// different nominal bph means a different `T_beat`, so every
    /// previously computed offset is meaningless) and resets before this
    /// call's events are considered.
    ///
    /// Dedupe: `events` overlaps across snapshots (`tape_events()`
    /// re-reports its ~32 s ring on every call), so only events strictly
    /// newer than `max_seen_t_drop` are considered at all — tracked
    /// independently of whether an event actually got PLACED on the grid,
    /// so an event without `t_unlock_s` still advances the watermark
    /// without advancing the grid.
    ///
    /// Offset math (binding): the first kept event anchors `t0 =
    /// t_unlock`, `k = 0`, `offset = 0`. Each subsequent kept event
    /// computes `k_i = k_prev + max(1, round((t_unlock_i −
    /// t_unlock_prev)/T_beat))` — the rounding rides through detection
    /// gaps — then `offset_ms_i = (k_i·T_beat − (t_unlock_i − t0)) ·
    /// 1000.0`.
    pub fn extend(&mut self, events: &[TapeEvent], bph_nominal: Option<u32>) {
        if bph_nominal != self.bph_nominal {
            self.reset();
            self.bph_nominal = bph_nominal;
        }
        let Some(bph) = self.bph_nominal else {
            return;
        };
        let t_beat = 3_600.0 / bph as f64;

        for e in events {
            if let Some(seen) = self.max_seen_t_drop
                && e.t_drop_corr_s <= seen
            {
                continue;
            }
            self.max_seen_t_drop = Some(e.t_drop_corr_s);

            let Some(t_unlock) = e.t_unlock_s else {
                continue;
            };
            let is_tic = e.parity == Parity::Tic;

            let offset_ms = match self.grid {
                None => {
                    self.grid = Some(Grid {
                        t0: t_unlock,
                        t_prev: t_unlock,
                        k_prev: 0,
                    });
                    0.0
                }
                Some(g) => {
                    let step = ((t_unlock - g.t_prev) / t_beat).round().max(1.0) as i64;
                    let k = g.k_prev + step;
                    self.grid = Some(Grid {
                        t0: g.t0,
                        t_prev: t_unlock,
                        k_prev: k,
                    });
                    (k as f64 * t_beat - (t_unlock - g.t0)) * 1_000.0
                }
            };

            self.push_point(BeatPoint {
                t_s: t_unlock,
                offset_ms,
                is_tic,
            });
        }
    }

    /// Pushes `p`, then prunes from the front anything older than
    /// `p.t_s − HORIZON_S`. The just-pushed point is always itself within
    /// the horizon of itself, so this never empties the deque.
    fn push_point(&mut self, p: BeatPoint) {
        let newest = p.t_s;
        self.points.push_back(p);
        while let Some(front) = self.points.front() {
            if front.t_s < newest - HORIZON_S {
                self.points.pop_front();
            } else {
                break;
            }
        }
    }

    /// The accumulated points, oldest first, pruned to `HORIZON_S`.
    pub fn points(&self) -> &VecDeque<BeatPoint> {
        &self.points
    }

    /// The newest kept point's `t_s`, or `None` before any point is kept.
    pub fn newest_t(&self) -> Option<f64> {
        self.points.back().map(|p| p.t_s)
    }

    /// Clears all accumulated state (points, grid, dedupe watermark) —
    /// called internally on a `bph_nominal` change, and available for a
    /// caller-driven manual reset.
    pub fn reset(&mut self) {
        self.points.clear();
        self.grid = None;
        self.max_seen_t_drop = None;
    }
}

/// Accumulates one `(t_s, amplitude_deg)` point per second (design spec
/// §11), independent of `chrona_dsp::Analyzer`'s own averaging window.
#[derive(Default)]
pub struct AmpAccum {
    points: VecDeque<(f64, f64)>,
    last_kept_t: Option<f64>,
}

impl AmpAccum {
    /// Pushes at most one point per second: only when `amp.is_some()` AND
    /// `t_now` (the beat accumulator's `newest_t()`) has advanced ≥ 1 s
    /// past the last kept point. Pruned to `HORIZON_S`, same as
    /// `BeatAccum`.
    pub fn extend(&mut self, t_now: Option<f64>, amp: Option<f64>) {
        let (Some(t), Some(a)) = (t_now, amp) else {
            return;
        };
        if let Some(last) = self.last_kept_t
            && t - last < 1.0
        {
            return;
        }
        self.points.push_back((t, a));
        self.last_kept_t = Some(t);
        while let Some(&(front_t, _)) = self.points.front() {
            if front_t < t - HORIZON_S {
                self.points.pop_front();
            } else {
                break;
            }
        }
    }

    /// The accumulated `(t_s, amplitude_deg)` points, oldest first.
    pub fn points(&self) -> &VecDeque<(f64, f64)> {
        &self.points
    }
}

/// The beat-trace chart's pan/zoom/follow state (design spec §10).
#[derive(Debug, Clone, Copy)]
pub struct TraceView {
    pub span_s: f64,
    pub follow: bool,
    pub end_rel_s: f64,
}

impl Default for TraceView {
    fn default() -> Self {
        TraceView {
            span_s: SPAN_DEFAULT_S,
            follow: true,
            end_rel_s: 0.0,
        }
    }
}

impl TraceView {
    /// The visible window's right edge, in absolute stream time:
    /// `newest_t − end_rel_s` (`end_rel_s == 0` when following live).
    pub fn end_t(&self, newest_t: f64) -> f64 {
        newest_t - self.end_rel_s
    }

    /// Multiplies `span_s` by `factor`, clamped to `[SPAN_MIN_S,
    /// SPAN_MAX_S]`.
    pub fn zoom(&mut self, factor: f64) {
        self.span_s = (self.span_s * factor).clamp(SPAN_MIN_S, SPAN_MAX_S);
    }

    /// Pans the view by `dt_s` seconds: positive shifts the visible
    /// window backward in time (away from live), negative shifts it
    /// forward (toward live) — see `end_rel_s`'s own meaning via
    /// `end_t`. Reaching (or crossing) the live edge (`end_rel_s <= 0`)
    /// snaps back into `follow` mode. Clamped so the window can't be
    /// panned behind the accumulators' own retention horizon
    /// (`HORIZON_S`) relative to `newest_t`, where there is no data left
    /// to show at all.
    pub fn pan(&mut self, dt_s: f64, newest_t: f64) {
        self.end_rel_s += dt_s;
        if self.end_t(newest_t) < newest_t - HORIZON_S {
            self.end_rel_s = HORIZON_S;
        }
        if self.end_rel_s <= 0.0 {
            self.end_rel_s = 0.0;
            self.follow = true;
        } else {
            self.follow = false;
        }
    }

    /// Resets to the default span, following live.
    pub fn fit(&mut self) {
        self.span_s = SPAN_DEFAULT_S;
        self.follow = true;
        self.end_rel_s = 0.0;
    }
}

/// Wraps `v_ms` into `(-wrap_ms/2, wrap_ms/2]` — a physical paper tape's
/// ink line re-enters from the opposite edge rather than running off it
/// (same convention as M3's `presenter::tape` module; reimplemented here
/// rather than shared across the module boundary, since that module's
/// version is private).
pub fn wrap_signed(v_ms: f64, wrap_ms: f64) -> f64 {
    let half = wrap_ms / 2.0;
    half - (half - v_ms).rem_euclid(wrap_ms)
}

/// A wrapped offset as a `-0.5..=0.5` fraction of the frame height, for a
/// chart Y coordinate.
pub fn y_frac(offset_ms: f64, wrap_ms: f64) -> f64 {
    wrap_signed(offset_ms, wrap_ms) / wrap_ms
}

/// Y-axis gridline spacing (ms) for a given wrap band, from the mockup's
/// JS step table (binding): keyed off the HALF-wrap (the frame's radius,
/// since the frame spans `±half`), not the full `wrap_ms`.
pub fn y_step_ms(wrap_ms: f64) -> f64 {
    let half = wrap_ms / 2.0;
    if half <= 2.0 {
        0.5
    } else if half <= 5.0 {
        1.0
    } else if half <= 10.0 {
        2.0
    } else {
        5.0
    }
}

/// X-axis gridline spacing (s) for a given visible span, from the
/// mockup's JS step table (binding).
pub fn x_step_s(span_s: f64) -> f64 {
    if span_s <= 40.0 {
        5.0
    } else if span_s <= 90.0 {
        10.0
    } else if span_s <= 200.0 {
        30.0
    } else {
        60.0
    }
}

/// One rate trend line: draw as `anchor_off_ms + slope_ms_per_s * (t −
/// anchor_t)`.
#[derive(Debug, Clone, Copy)]
pub struct Trend {
    pub anchor_t: f64,
    pub anchor_off_ms: f64,
    pub slope_ms_per_s: f64,
}

/// The rate trend line (design spec §10): `None` when there's no rate to
/// draw, or the accumulator has no points to anchor to. Slope is
/// `rate_s_per_day` converted from s/day to ms/s, sign-matched to
/// `BeatAccum`'s fast-rises convention (a positive rate is a fast watch,
/// same sign as a positive offset). The anchor is the MEDIAN offset of
/// the newest 10 s of points (not just the single newest point) — a
/// robust, de-jittered start for the line.
pub fn trend(points: &VecDeque<BeatPoint>, rate_s_per_day: Option<f64>) -> Option<Trend> {
    let rate = rate_s_per_day?;
    let newest_t = points.back()?.t_s;
    let window_start = newest_t - 10.0;
    let mut recent: Vec<f64> = points
        .iter()
        .filter(|p| p.t_s >= window_start)
        .map(|p| p.offset_ms)
        .collect();
    if recent.is_empty() {
        return None;
    }
    recent.sort_by(f64::total_cmp);
    let anchor_off_ms = recent[recent.len() / 2];
    Some(Trend {
        anchor_t: newest_t,
        anchor_off_ms,
        slope_ms_per_s: rate * 1_000.0 / 86_400.0,
    })
}

/// Amplitude strip Y-range (design spec §11): the smallest `[30·k, 30·m]`
/// degree band (k, m integers) containing the data, widened to at least
/// 60° of span — extending the UPPER bound first, then the lower. Empty
/// data (nothing at Tier 3 yet) falls back to the mockup's static
/// `(240, 300)`.
pub fn amp_range(points: &VecDeque<(f64, f64)>) -> (f64, f64) {
    if points.is_empty() {
        return (240.0, 300.0);
    }
    let mut min = f64::INFINITY;
    let mut max = f64::NEG_INFINITY;
    for &(_, deg) in points {
        min = min.min(deg);
        max = max.max(deg);
    }
    let mut lo = (min / 30.0).floor() * 30.0;
    let mut hi = (max / 30.0).ceil() * 30.0;
    if hi - lo < 60.0 {
        hi += 30.0;
        if hi - lo < 60.0 {
            lo -= 30.0;
        }
    }
    (lo, hi)
}

/// Signal-meter bar coloring (design spec §3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignalClass {
    Faint,
    Warn,
    Good,
}

/// Signal meter (design spec §3, binding pinned strings): bar count,
/// label, and color class from the current tier. `None` (no metrics yet)
/// is the zero-bar "No signal" state; T3 lights a 4th bar once
/// `detection_ratio >= 0.8`.
pub fn signal_meter(m: Option<&MetricsSnapshot>) -> (u8, &'static str, SignalClass) {
    let Some(m) = m else {
        return (0, "No signal", SignalClass::Faint);
    };
    match m.tier {
        Tier::T1 => (1, "Weak · rate only", SignalClass::Warn),
        Tier::T2 => (2, "Fair · rate + beat error", SignalClass::Warn),
        Tier::T3 => {
            let bars = if m.quality.detection_ratio >= 0.8 {
                4
            } else {
                3
            };
            (bars, "Strong · full regression", SignalClass::Good)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrona_dsp::analyzer::Quality;
    use chrona_dsp::period::PeriodEstimate;

    /// Like `presenter/tape.rs`'s `ev()` helper.
    fn mk_event(k: i64, t_unlock: f64, tic: bool) -> TapeEvent {
        TapeEvent {
            beat_index: k,
            parity: if tic { Parity::Tic } else { Parity::Toc },
            t_unlock_s: Some(t_unlock),
            t_drop_corr_s: t_unlock + 0.007,
        }
    }

    fn mk_metrics(tier: Tier, detection_ratio: f64) -> MetricsSnapshot {
        MetricsSnapshot {
            tier,
            bph_detected: 28_800.0,
            bph_nominal: Some(28_800),
            rate_s_per_day: None,
            rate_source: None,
            beat_error_ms: None,
            amplitude_deg: None,
            period: PeriodEstimate {
                t_osc_s: 0.25,
                sigma_s: 0.0,
                window_s: 4.0,
            },
            quality: Quality {
                detection_ratio,
                onset_jitter_ms: None,
                mean_beat_snr_db: None,
                unlocking_ratio: 0.0,
                clipped_samples: 0,
                amplitude_gate: None,
            },
            calibrated: false,
        }
    }

    #[test]
    fn perfect_watch_offsets_are_zero_and_fast_watch_rises() {
        let t_beat = 3_600.0 / 28_800.0;
        let perfect: Vec<TapeEvent> = (0..100)
            .map(|k| mk_event(k, 5.0 + k as f64 * t_beat, k % 2 == 0))
            .collect();
        let mut accum = BeatAccum::default();
        accum.extend(&perfect, Some(28_800));
        assert_eq!(accum.points().len(), 100);
        for p in accum.points() {
            assert!(p.offset_ms.abs() < 1e-9, "offset {}", p.offset_ms);
        }

        let mut fast = BeatAccum::default();
        let shrink = 0.125e-3;
        let events: Vec<TapeEvent> = (0..81)
            .map(|k| mk_event(k, 5.0 + k as f64 * (t_beat - shrink), k % 2 == 0))
            .collect();
        fast.extend(&events, Some(28_800));
        let pts: Vec<BeatPoint> = fast.points().iter().copied().collect();
        assert_eq!(pts.len(), 81);
        for w in pts.windows(2) {
            assert!(
                w[1].offset_ms > w[0].offset_ms,
                "must strictly increase: {} then {}",
                w[0].offset_ms,
                w[1].offset_ms
            );
        }
        let delta = pts[80].offset_ms - pts[0].offset_ms;
        assert!((delta - 80.0 * 0.125).abs() < 1e-6, "delta {delta}");
    }

    #[test]
    fn dedupe_across_overlapping_snapshots() {
        let t_beat = 3_600.0 / 28_800.0;
        let all: Vec<TapeEvent> = (0..80)
            .map(|k| mk_event(k, 5.0 + k as f64 * t_beat, k % 2 == 0))
            .collect();
        let mut accum = BeatAccum::default();
        accum.extend(&all[0..50], Some(28_800));
        assert_eq!(accum.points().len(), 50);
        accum.extend(&all[30..80], Some(28_800));
        assert_eq!(accum.points().len(), 80);
        let ts: Vec<f64> = accum.points().iter().map(|p| p.t_s).collect();
        for w in ts.windows(2) {
            assert!(
                w[1] > w[0],
                "t must strictly increase: {} then {}",
                w[0],
                w[1]
            );
        }
    }

    #[test]
    fn detection_gap_keeps_grid_continuity() {
        let t_beat = 3_600.0 / 28_800.0;
        let mut events: Vec<TapeEvent> = (0..20)
            .map(|k| mk_event(k, 5.0 + k as f64 * t_beat, k % 2 == 0))
            .collect();
        events.extend((30..=50).map(|k| mk_event(k, 5.0 + k as f64 * t_beat, k % 2 == 0)));
        let mut accum = BeatAccum::default();
        accum.extend(&events, Some(28_800));
        assert_eq!(accum.points().len(), 20 + 21);
        for p in accum.points() {
            assert!(p.offset_ms.abs() < 1e-6, "offset {}", p.offset_ms);
        }
    }

    #[test]
    fn prune_and_newest() {
        let t_beat = 0.125;
        let n = (400.0 / t_beat) as i64;
        let events: Vec<TapeEvent> = (0..n)
            .map(|k| mk_event(k, k as f64 * t_beat, k % 2 == 0))
            .collect();
        let mut accum = BeatAccum::default();
        accum.extend(&events, Some(28_800));
        let newest = accum.newest_t().expect("has points");
        let oldest = accum.points().front().expect("has points").t_s;
        assert!(
            oldest >= newest - HORIZON_S,
            "oldest {oldest} newest {newest}"
        );
        assert!(newest - oldest <= HORIZON_S + 1e-6);
    }

    #[test]
    fn bph_change_resets_and_none_bph_appends_nothing() {
        let t_beat_a = 3_600.0 / 28_800.0;
        let events_a: Vec<TapeEvent> = (0..10)
            .map(|k| mk_event(k, 5.0 + k as f64 * t_beat_a, k % 2 == 0))
            .collect();
        let mut accum = BeatAccum::default();
        accum.extend(&events_a, Some(28_800));
        assert_eq!(accum.points().len(), 10);

        let events_b = [mk_event(0, 50.0, true)];
        accum.extend(&events_b, Some(21_600));
        assert_eq!(
            accum.points().len(),
            1,
            "bph change must reset the accumulator"
        );

        let events_c = [mk_event(1, 50.125, false)];
        accum.extend(&events_c, None);
        assert!(
            accum.points().is_empty(),
            "None bph appends nothing (and resets on change)"
        );

        accum.extend(&events_c, None);
        assert!(accum.points().is_empty(), "repeat None is a stable no-op");
    }

    #[test]
    fn reset_clears_everything() {
        let t_beat = 3_600.0 / 28_800.0;
        let events: Vec<TapeEvent> = (0..10)
            .map(|k| mk_event(k, 5.0 + k as f64 * t_beat, k % 2 == 0))
            .collect();
        let mut accum = BeatAccum::default();
        accum.extend(&events, Some(28_800));
        assert_eq!(accum.points().len(), 10);
        accum.reset();
        assert!(accum.points().is_empty());
        assert_eq!(accum.newest_t(), None);
    }

    #[test]
    fn wrap_and_steps_match_mockup_tables() {
        assert!((wrap_signed(5.1, 10.0) - (-4.9)).abs() < 1e-9);
        assert_eq!(y_step_ms(4.0), 0.5);
        assert_eq!(y_step_ms(10.0), 1.0);
        assert_eq!(y_step_ms(20.0), 2.0);
        assert_eq!(y_step_ms(50.0), 5.0);
        assert_eq!(x_step_s(40.0), 5.0);
        assert_eq!(x_step_s(90.0), 10.0);
        assert_eq!(x_step_s(200.0), 30.0);
        assert_eq!(x_step_s(300.0), 60.0);
    }

    #[test]
    fn trend_slope_sign_and_anchor() {
        let t_beat = 3_600.0 / 28_800.0;
        let shrink = 0.125e-3;
        let events: Vec<TapeEvent> = (0..100)
            .map(|k| mk_event(k, 5.0 + k as f64 * (t_beat - shrink), k % 2 == 0))
            .collect();
        let mut accum = BeatAccum::default();
        accum.extend(&events, Some(28_800));
        let newest = accum.newest_t().expect("has points");

        let t = trend(accum.points(), Some(12.0)).expect("trend");
        assert!((t.slope_ms_per_s - 0.138_888_888_9).abs() < 1e-6);
        assert_eq!(t.anchor_t, newest);

        let window_start = newest - 10.0;
        let mut offs: Vec<f64> = accum
            .points()
            .iter()
            .filter(|p| p.t_s >= window_start)
            .map(|p| p.offset_ms)
            .collect();
        assert!(
            offs.len() < accum.points().len(),
            "window must be a real subset"
        );
        offs.sort_by(f64::total_cmp);
        let expected_median = offs[offs.len() / 2];
        assert!((t.anchor_off_ms - expected_median).abs() < 1e-9);

        assert!(trend(accum.points(), None).is_none());
    }

    #[test]
    fn amp_accum_one_per_second_and_range() {
        let mut amp = AmpAccum::default();
        amp.extend(Some(0.0), Some(270.0));
        amp.extend(Some(0.5), Some(270.0));
        amp.extend(Some(1.0), Some(270.0));
        amp.extend(Some(2.2), Some(270.0));
        let ts: Vec<f64> = amp.points().iter().map(|&(t, _)| t).collect();
        assert_eq!(ts, vec![0.0, 1.0, 2.2]);

        let mut d = VecDeque::new();
        d.push_back((0.0, 250.0));
        d.push_back((1.0, 285.0));
        assert_eq!(amp_range(&d), (240.0, 300.0));

        let empty: VecDeque<(f64, f64)> = VecDeque::new();
        assert_eq!(amp_range(&empty), (240.0, 300.0));

        let mut c = VecDeque::new();
        c.push_back((0.0, 305.0));
        c.push_back((1.0, 305.0));
        assert_eq!(amp_range(&c), (300.0, 360.0));
    }

    #[test]
    fn amp_accum_none_or_missing_amp_appends_nothing() {
        let mut amp = AmpAccum::default();
        amp.extend(None, Some(270.0));
        amp.extend(Some(1.0), None);
        assert!(amp.points().is_empty());
    }

    #[test]
    fn amp_accum_prunes_to_horizon() {
        let mut amp = AmpAccum::default();
        let n = (HORIZON_S as i64) + 100;
        for i in 0..n {
            amp.extend(Some(i as f64), Some(270.0));
        }
        let newest = amp.points().back().unwrap().0;
        let oldest = amp.points().front().unwrap().0;
        assert!(
            oldest >= newest - HORIZON_S,
            "oldest {oldest} newest {newest}"
        );
    }

    #[test]
    fn signal_meter_mapping() {
        assert_eq!(signal_meter(None), (0, "No signal", SignalClass::Faint));
        let t1 = mk_metrics(Tier::T1, 0.0);
        assert_eq!(
            signal_meter(Some(&t1)),
            (1, "Weak · rate only", SignalClass::Warn)
        );
        let t2 = mk_metrics(Tier::T2, 0.0);
        assert_eq!(
            signal_meter(Some(&t2)),
            (2, "Fair · rate + beat error", SignalClass::Warn)
        );
        let t3_low = mk_metrics(Tier::T3, 0.7);
        assert_eq!(
            signal_meter(Some(&t3_low)),
            (3, "Strong · full regression", SignalClass::Good)
        );
        let t3_high = mk_metrics(Tier::T3, 0.85);
        assert_eq!(
            signal_meter(Some(&t3_high)),
            (4, "Strong · full regression", SignalClass::Good)
        );
    }

    #[test]
    fn view_zoom_pan_fit_clamps() {
        let mut v = TraceView::default();
        assert_eq!(v.span_s, SPAN_DEFAULT_S);
        assert!(v.follow);
        assert_eq!(v.end_rel_s, 0.0);

        for _ in 0..40 {
            v.zoom(0.5);
        }
        assert_eq!(v.span_s, SPAN_MIN_S, "zoom below SPAN_MIN_S clamps");

        for _ in 0..40 {
            v.zoom(2.0);
        }
        assert_eq!(v.span_s, SPAN_MAX_S, "zoom above SPAN_MAX_S clamps");

        v.pan(30.0, 100.0);
        assert!(!v.follow, "pan back sets follow=false");
        assert_eq!(v.end_rel_s, 30.0);
        assert_eq!(v.end_t(100.0), 70.0);

        v.pan(-40.0, 100.0);
        assert!(v.follow, "pan to end_rel<=0 restores follow");
        assert_eq!(v.end_rel_s, 0.0);
        assert_eq!(v.end_t(100.0), 100.0);

        v.fit();
        assert_eq!(v.span_s, SPAN_DEFAULT_S);
        assert!(v.follow);
        assert_eq!(v.end_rel_s, 0.0);
    }

    #[test]
    fn pan_clamps_to_retention_horizon() {
        let mut v = TraceView::default();
        v.pan(1_000.0, 100.0);
        assert_eq!(v.end_rel_s, HORIZON_S);
        assert!(!v.follow);
    }
}
