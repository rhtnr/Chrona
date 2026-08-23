//! Paper-tape geometry: normalized coordinates for the tic/toc "paper
//! tape" readout, replicating a mechanical timegrapher's ink strip. Pure
//! functions over `chrona_dsp::analyzer::TapeEvent` — no `egui` dependency
//! (see the `presenter` module docs).

use chrona_dsp::analyzer::TapeEvent;
use chrona_dsp::events::Parity;

/// Tape display parameters (T8 supplies these from the live config).
#[derive(Debug, Clone, Copy)]
pub struct TapeParams {
    /// Nominal beats-per-hour used to compute the per-beat detrend
    /// (`T_beat_nom = 3600 / bph_nominal`, spec §2.2).
    pub bph_nominal: u32,
    /// Full display height, in milliseconds (e.g. `10.0` ⇒ the frame spans
    /// ±5 ms); deviations wrap into this band rather than running off the
    /// edge.
    pub wrap_ms: f64,
    /// Rolling window width, in beats, kept on screen.
    pub max_beats: usize,
}

/// One plotted point on the tape.
#[derive(Debug, Clone, Copy)]
pub struct TapeDot {
    /// Horizontal position, `0..=1` across the displayed window.
    pub beat_x: f64,
    /// Vertical deviation from nominal, wrapped to `-0.5..=0.5` (the frame
    /// edges).
    pub dev_y: f64,
    /// `true` for a tic, `false` for a toc.
    pub is_tic: bool,
}

/// Wrap `value` into `(-period/2, period/2]` — a physical paper tape's ink
/// line re-enters from the opposite edge rather than running off it.
fn wrap_signed(value: f64, period: f64) -> f64 {
    let half = period / 2.0;
    half - (half - value).rem_euclid(period)
}

/// Paper-tape geometry for the newest `p.max_beats` unlocked events.
///
/// Anchors on `t_0`, the unlocking time of the FIRST event in `events`
/// (array order) that has one; `k = beat_index − beat_index_0` is every
/// other unlocked event's beat-index offset from that anchor. Each
/// surviving event's deviation from nominal is
/// `dev = (t_unlock − t_0) − k·T_beat_nom`, wrapped (see `wrap_signed`)
/// into `±wrap_ms/2` and normalized to `dev_y = dev_ms / wrap_ms`, so a
/// perfectly-timed watch draws a flat line at `dev_y = 0` and the frame
/// edges are `±0.5`.
///
/// Events without `t_unlock_s` are dropped — they can't be placed on the
/// tape, and don't count against `max_beats`. Only the newest (up to)
/// `max_beats` of the remaining events are kept.
///
/// `beat_x` is a CONSTANT-VELOCITY strip-chart coordinate, not a rescale
/// to whatever's currently on screen: the newest surviving event is
/// pinned to `beat_x = 1.0` (the right edge) and each beat back from it
/// steps left by a FIXED `1 / (max_beats − 1)`, so one beat's distance
/// from the right edge depends only on how many newer beats have arrived
/// since — never on how many events happen to be in this particular
/// call's window. That distinction matters because
/// `chrona_dsp::Analyzer::tape_events()` is capped to its 32 s ring (a few
/// hundred beats at typical rates), so a `max_beats` sized for the full
/// display (e.g. 800) routinely never saturates; normalizing against the
/// window's own observed span would make the whole trace visibly rescale
/// on every call as events arrive, rather than scroll. An unsaturated
/// window instead just leaves blank tape to the left of the oldest
/// plotted dot — the same way a real strip-chart recorder's paper is
/// blank until the pen has been running long enough to reach it.
///
/// **Precondition:** `events` must already be time-ascending — the
/// contract `chrona_dsp::Analyzer::tape_events()` guarantees for its
/// output. This function does not sort.
pub fn tape_dots(events: &[TapeEvent], p: &TapeParams) -> Vec<TapeDot> {
    let unlocked: Vec<(TapeEvent, f64)> = events
        .iter()
        .filter_map(|e| Some((*e, e.t_unlock_s?)))
        .collect();
    let Some(&(anchor, t_0)) = unlocked.first() else {
        return Vec::new();
    };
    let beat_index_0 = anchor.beat_index;
    let t_beat_nom = chrona_dsp::bph::t_beat_s(p.bph_nominal);

    let start = unlocked.len().saturating_sub(p.max_beats);
    let windowed = &unlocked[start..];
    let k_newest = windowed.last().map_or(0, |(e, _)| e.beat_index);
    let denom = p.max_beats.saturating_sub(1).max(1) as f64;

    windowed
        .iter()
        .map(|&(e, t_unlock)| {
            let k = e.beat_index - beat_index_0;
            let dev_ms = ((t_unlock - t_0) - k as f64 * t_beat_nom) * 1_000.0;
            TapeDot {
                beat_x: 1.0 - (k_newest - e.beat_index) as f64 / denom,
                dev_y: wrap_signed(dev_ms, p.wrap_ms) / p.wrap_ms,
                is_tic: e.parity == Parity::Tic,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(k: i64, t_unlock: f64, tic: bool) -> TapeEvent {
        TapeEvent {
            beat_index: k,
            parity: if tic { Parity::Tic } else { Parity::Toc },
            t_unlock_s: Some(t_unlock),
            t_drop_corr_s: t_unlock + 0.007,
        }
    }

    #[test]
    fn perfect_watch_draws_a_flat_line_at_zero() {
        let t_beat = 3_600.0 / 28_800.0;
        let events: Vec<TapeEvent> = (0..100)
            .map(|k| ev(k, 5.0 + k as f64 * t_beat, k % 2 == 0))
            .collect();
        let dots = tape_dots(
            &events,
            &TapeParams {
                bph_nominal: 28_800,
                wrap_ms: 10.0,
                max_beats: 800,
            },
        );
        assert_eq!(dots.len(), 100);
        for d in &dots {
            assert!(d.dev_y.abs() < 1e-9, "dev {}", d.dev_y);
        }
        assert!(dots[0].beat_x < dots[99].beat_x);
    }

    #[test]
    fn fast_watch_slopes_and_wraps() {
        // +86.4 s/d ⇒ each beat arrives 0.125 ms early ⇒ dev decreases 0.125 ms/beat,
        // crossing the −5 ms wrap edge after 40 beats and reappearing near +5 ms.
        let t_beat = 0.125;
        let shrink = 0.125e-3;
        let events: Vec<TapeEvent> = (0..80)
            .map(|k| ev(k, 5.0 + k as f64 * (t_beat - shrink), k % 2 == 0))
            .collect();
        let p = TapeParams {
            bph_nominal: 28_800,
            wrap_ms: 10.0,
            max_beats: 800,
        };
        let dots = tape_dots(&events, &p);
        let d10 = dots[10].dev_y;
        let d30 = dots[30].dev_y;
        assert!(d30 < d10, "slope must go down: {d10} vs {d30}");
        // beat 39: dev = −4.875 ms (in range). beat 41: −5.125 → wraps to +4.875.
        assert!(dots[39].dev_y < -0.45);
        assert!(dots[41].dev_y > 0.45, "wrapped: {}", dots[41].dev_y);
    }

    #[test]
    fn beat_error_splits_two_bands() {
        // ±0.4 ms alternate shifts ⇒ two flat bands 0.8 ms apart.
        let t_beat = 0.125;
        let events: Vec<TapeEvent> = (0..100)
            .map(|k| {
                let s = if k % 2 == 0 { 0.4e-3 } else { -0.4e-3 };
                ev(k, 5.0 + k as f64 * t_beat + s, k % 2 == 0)
            })
            .collect();
        let dots = tape_dots(
            &events,
            &TapeParams {
                bph_nominal: 28_800,
                wrap_ms: 10.0,
                max_beats: 800,
            },
        );
        let tics: Vec<f64> = dots.iter().filter(|d| d.is_tic).map(|d| d.dev_y).collect();
        let tocs: Vec<f64> = dots.iter().filter(|d| !d.is_tic).map(|d| d.dev_y).collect();
        let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len() as f64;
        let sep_ms = (mean(&tics) - mean(&tocs)).abs() * 10.0; // dev_y → ms via wrap_ms
        assert!((sep_ms - 0.8).abs() < 0.01, "separation {sep_ms} ms");
    }

    #[test]
    fn window_keeps_newest_max_beats() {
        let t_beat = 0.125;
        let events: Vec<TapeEvent> = (0..1_000)
            .map(|k| ev(k, 5.0 + k as f64 * t_beat, k % 2 == 0))
            .collect();
        let dots = tape_dots(
            &events,
            &TapeParams {
                bph_nominal: 28_800,
                wrap_ms: 10.0,
                max_beats: 800,
            },
        );
        assert_eq!(dots.len(), 800);
        assert!(dots.first().unwrap().beat_x >= 0.0 && dots.last().unwrap().beat_x <= 1.0);
    }

    #[test]
    fn beat_x_advances_at_constant_velocity_anchored_right() {
        let t_beat = 0.125;
        let events: Vec<TapeEvent> = (0..100)
            .map(|k| ev(k, 5.0 + k as f64 * t_beat, k % 2 == 0))
            .collect();
        let p = TapeParams {
            bph_nominal: 28_800,
            wrap_ms: 10.0,
            max_beats: 800,
        };
        let dots = tape_dots(&events, &p);
        assert!(
            (dots.last().unwrap().beat_x - 1.0).abs() < 1e-12,
            "newest anchors at 1.0"
        );
        let step = 1.0 / 799.0;
        for w in dots.windows(2) {
            assert!(
                ((w[1].beat_x - w[0].beat_x) - step).abs() < 1e-12,
                "constant velocity"
            );
        }
        assert!((dots.first().unwrap().beat_x - (1.0 - 99.0 * step)).abs() < 1e-12);
    }

    #[test]
    fn events_without_unlocking_are_skipped_and_empty_is_empty() {
        let mut e = ev(0, 5.0, true);
        e.t_unlock_s = None;
        assert!(
            tape_dots(
                &[e],
                &TapeParams {
                    bph_nominal: 28_800,
                    wrap_ms: 10.0,
                    max_beats: 800
                }
            )
            .is_empty()
        );
        assert!(
            tape_dots(
                &[],
                &TapeParams {
                    bph_nominal: 28_800,
                    wrap_ms: 10.0,
                    max_beats: 800
                }
            )
            .is_empty()
        );
    }
}
