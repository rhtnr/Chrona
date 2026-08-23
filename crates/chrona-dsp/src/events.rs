//! Stage 5 (spec §5.5): learned matched-filter drop detection on the
//! band-passed signal, plus envelope-edge extraction (Task 7).

use crate::envelope::DECIMATION;
use crate::filter::Butterworth;
use crate::fold::FoldProfile;

/// Correlation-peak SNR (dB) below which a beat is not emitted.
pub const DETECT_SNR_DB: f32 = 6.0;
const TEMPLATE_LEN: usize = 384;
const TEMPLATE_PEAK_AT: usize = 96;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Parity {
    Tic,
    Toc,
}

#[derive(Debug, Clone, Copy)]
pub struct BeatEvent {
    pub beat_index: i64,
    pub parity: Parity,
    /// Matched-filter drop time, absolute nominal-clock seconds since stream start.
    pub t_drop_corr_s: f64,
    pub snr_db: f32,
    /// Envelope leading-edge times (Task 7); None until extracted or when not found.
    pub t_drop_edge_s: Option<f64>,
    pub t_unlock_s: Option<f64>,
}

fn parabolic3(a: f64, b: f64, c: f64) -> f64 {
    let denom = a - 2.0 * b + c;
    if denom.abs() < 1e-20 {
        0.0
    } else {
        (0.5 * (a - c) / denom).clamp(-0.5, 0.5)
    }
}

/// Build one parity's unit-energy drop template from max-|x|-anchored windows.
/// Returns None when fewer than 6 windows survive outlier trimming.
fn build_template(raw: &[f32], predictions: &[f64], gate: usize) -> Option<Vec<f32>> {
    let mut windows: Vec<(f32, Vec<f32>)> = Vec::new();
    for &pred in predictions {
        let center = pred.round() as i64;
        let lo = center - gate as i64;
        let hi = center + gate as i64;
        if lo < 0 || (hi as usize) + TEMPLATE_LEN >= raw.len() {
            continue;
        }
        let (lo, hi) = (lo as usize, hi as usize);
        let peak_idx = (lo..hi)
            .max_by(|&i, &j| raw[i].abs().total_cmp(&raw[j].abs()))
            .unwrap_or(lo);
        if peak_idx < TEMPLATE_PEAK_AT || peak_idx + (TEMPLATE_LEN - TEMPLATE_PEAK_AT) >= raw.len()
        {
            continue;
        }
        let start = peak_idx - TEMPLATE_PEAK_AT;
        let w = raw[start..start + TEMPLATE_LEN].to_vec();
        windows.push((raw[peak_idx].abs(), w));
    }
    if windows.len() < 6 {
        return None;
    }
    // Drop top and bottom quintile by peak amplitude (outliers / missed beats).
    windows.sort_by(|a, b| a.0.total_cmp(&b.0));
    let q = windows.len() / 5;
    let kept = &windows[q..windows.len() - q];
    if kept.len() < 6 {
        return None;
    }
    let mut template = vec![0.0f32; TEMPLATE_LEN];
    for (_, w) in kept {
        for (t, v) in template.iter_mut().zip(w.iter()) {
            *t += v;
        }
    }
    let energy: f64 = template.iter().map(|v| (*v as f64).powi(2)).sum();
    if energy <= 0.0 {
        return None;
    }
    let norm = (energy.sqrt()) as f32;
    for t in template.iter_mut() {
        *t /= norm;
    }
    Some(template)
}

/// Correlate `template` across `[center − gate, center + gate]`; return
/// (fractional peak index into `raw`, peak value, off-peak rms).
fn correlate_in_gate(
    raw: &[f32],
    template: &[f32],
    center: i64,
    gate: usize,
    sr: f64,
) -> Option<(f64, f64, f64)> {
    let lo = center - gate as i64;
    if lo < 0 {
        return None;
    }
    let lo = lo as usize;
    let hi = (center as usize + gate).min(raw.len().saturating_sub(TEMPLATE_LEN));
    if hi <= lo + 2 {
        return None;
    }
    let mut corr: Vec<f64> = Vec::with_capacity(hi - lo);
    for start in lo..hi {
        let mut acc = 0.0f64;
        for (t, v) in template.iter().zip(&raw[start..start + TEMPLATE_LEN]) {
            acc += *t as f64 * *v as f64;
        }
        corr.push(acc);
    }
    let (pi, _) = corr
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))?;
    let peak = corr[pi].abs();
    let excl = (2.0e-3 * sr) as usize;
    let mut acc = 0.0f64;
    let mut n = 0usize;
    for (i, c) in corr.iter().enumerate() {
        if i.abs_diff(pi) > excl {
            acc += c * c;
            n += 1;
        }
    }
    if n < 8 {
        return None;
    }
    let rms = (acc / n as f64).sqrt().max(1e-30);
    let off = if pi == 0 || pi + 1 == corr.len() {
        0.0
    } else {
        parabolic3(corr[pi - 1].abs(), corr[pi].abs(), corr[pi + 1].abs())
    };
    Some((lo as f64 + pi as f64 + off, peak, rms))
}

/// Stage 5: per-beat drop events via a learned matched filter (spec §5.5).
/// See the plan's alignment and fold-window invariants; times are absolute
/// nominal-clock seconds. Envelope edges (`t_drop_edge_s`, `t_unlock_s`) are
/// filled in by `fill_edges` (Task 7, spec §5.5) before the final sort; a
/// beat's edges stay `None` when the edge pass can't find or validate them.
/// `fill_edges` re-derives its own native-rate envelope from `raw` per
/// event (see its doc comment) rather than using the decimated `env`, so
/// `env` is passed to it only for the coarse, whole-stream `global_max`
/// scale.
///
/// **Alignment invariant:** `raw` and `env` come from rings fed in lockstep
/// from stream start and start at the same absolute instant — envelope
/// index `j` corresponds to raw index `j · envelope::DECIMATION`; callers
/// pass that shared instant as `raw_start_abs`, in raw-sample units.
///
/// **Fold-window invariant:** the fold in `fold_envelope` starts at
/// `env.len() − (cycles·t_osc_env) as usize` computed from the SAME env
/// slice length. `fold_envelope` only phase-buckets that slice (any window
/// within about one cycle of `cycles` periods is fine there), but this
/// function's downstream per-cycle prediction loop needs the window to
/// yield EXACTLY `cycles` back out when re-divided by `t_osc_env` — so it
/// can't just reuse fold_envelope's plain truncating cast; see
/// `cycle_grid_fold_start`'s doc comment for why.
///
/// `extract_events` derives `fold_start` from `env.len()` via
/// `cycle_grid_fold_start` (self-verifying — see its doc comment) and
/// extracts every cycle in the window: `extract_events_anchored(.., fold_start, 0, 0)`.
///
/// `extract_events_anchored` takes the cycle-grid anchor explicitly instead
/// of deriving it from `env.len()`: `fold_start_env` is the LOCAL env offset
/// (into THIS `env` slice) where cycle 0 begins, `beat_index_base` is added
/// to every emitted `beat_index` (so callers using a window that starts
/// partway through a larger absolute grid can report the correct absolute
/// beat number), and `from_cycle_local` skips correlating/emitting cycles
/// before it (LOCAL to this window's own 0-based numbering) — predictions
/// and templates are still built over ALL cycles in the window (templates
/// are cheap and benefit from full data), only correlation is skipped.
///
/// This is the incremental analyzer's hot path (`Analyzer::current_metrics`):
/// between refolds it re-anchors a fresh window's `fold_start_env` to 0 (the
/// window is copied starting exactly at the desired cycle boundary) and
/// tracks how many GLOBAL cycles have been extracted so far itself, passing
/// the right `beat_index_base`/`from_cycle_local` for whatever window it
/// managed to keep inside the ring — see its own doc comment for why a
/// window's start can't just be re-derived from `env.len()` call to call.
pub fn extract_events(
    raw: &[f32],
    raw_start_abs: u64,
    env: &[f32],
    sample_rate_hz: f64,
    profile: &FoldProfile,
) -> Vec<BeatEvent> {
    let t_osc_env = profile.t_osc_env;
    if !(t_osc_env.is_finite() && t_osc_env > 1.0) || env.is_empty() {
        return Vec::new();
    }
    let fold_start = cycle_grid_fold_start(env.len(), t_osc_env);
    extract_events_anchored(
        raw,
        raw_start_abs,
        env,
        sample_rate_hz,
        profile,
        fold_start,
        0,
        0,
    )
}

/// Computes the `fold_start` (into an env slice of length `env_len`) that
/// `extract_events_anchored` should extract from so that its own
/// cycle-count derivation — `floor((env_len − fold_start) / t_osc_env)` —
/// recovers EXACTLY `floor(env_len / t_osc_env)` cycles (call it `C`).
///
/// TASK-11 FIX ROUND (finding 1): the obvious one-shot estimate, `env_len
/// − ceil(C · t_osc_env)`, is NOT guaranteed to round-trip: `C as f64 *
/// t_osc_env` and `(env_len − fold_start) as f64 / t_osc_env` are two
/// different f64 expressions, and multiplying then dividing by the same
/// f64 value isn't guaranteed to be exact. Concretely, for
/// `t_osc_env=100.909, env_len=100910`: `C=1000`, and `C as f64 *
/// t_osc_env` happens to round to exactly `100909.0` (so `.ceil()` is a
/// no-op) — but `100909.0 / 100.909_f64` then rounds DOWN to just under
/// `1000.0`, so `floor(...)` recovers `999`, silently dropping the
/// trailing cycle's worth of events. This isn't a rare corner: a sweep
/// over fractional `(t_osc_env, env_len)` pairs near the multiply/divide
/// rounding boundary found the one-shot estimate off by one in roughly 2
/// cases in 3 (see the regression tests below).
///
/// So instead of trusting the algebra, treat the one-shot estimate as a
/// starting point and replay `extract_events_anchored`'s own division,
/// nudging `fold_start` by one envelope sample at a time until it
/// reproduces `C` exactly. This is guaranteed to terminate without
/// oscillating: since `t_osc_env > 1` (checked by callers), each unit step
/// of `fold_start` changes the recomputed cycle count by at most one, so
/// the walk can never skip over the target `C` — and decrementing
/// `fold_start` all the way to 0 is guaranteed to hit `C` exactly, because
/// that is literally how `C` itself was defined above. In practice it
/// converges in ≤2 steps (0 steps when the one-shot estimate is already
/// exact — the common case — else 1).
fn cycle_grid_fold_start(env_len: usize, t_osc_env: f64) -> usize {
    let cycles = (env_len as f64 / t_osc_env).floor() as usize;
    let window_size = (cycles as f64 * t_osc_env).ceil() as usize;
    let mut fold_start = env_len.saturating_sub(window_size);
    loop {
        let recomputed = (env_len.saturating_sub(fold_start) as f64 / t_osc_env).floor() as usize;
        if recomputed < cycles && fold_start > 0 {
            fold_start -= 1;
        } else if recomputed > cycles {
            fold_start += 1;
        } else {
            break;
        }
    }
    fold_start
}

// The 8-parameter grid-anchor interface (R4-M3) is the explicit fix for the
// incremental analyzer's steady-state extraction bug (see
// Analyzer::current_metrics's doc comment) — each parameter is independently
// meaningful to a caller re-anchoring its own extraction window, so bundling
// them into a struct would just move the same information one level of
// indirection away without reducing it.
#[allow(clippy::too_many_arguments)]
pub fn extract_events_anchored(
    raw: &[f32],
    raw_start_abs: u64,
    env: &[f32],
    sample_rate_hz: f64,
    profile: &FoldProfile,
    fold_start_env: usize,
    beat_index_base: i64,
    from_cycle_local: usize,
) -> Vec<BeatEvent> {
    let t_osc_env = profile.t_osc_env;
    if !(t_osc_env.is_finite() && t_osc_env > 1.0) || env.is_empty() {
        return Vec::new();
    }
    let fold_start = fold_start_env;
    let cycles = (env.len().saturating_sub(fold_start) as f64 / t_osc_env).floor() as usize;
    if cycles < 8 {
        return Vec::new();
    }

    let (off_a, off_b) = profile.anchor_env_offsets();
    let gate = ((t_osc_env / 16.0) * DECIMATION as f64) as usize; // T_beat/8 in raw samples
    let mut out = Vec::new();

    // (parity, env offset, grid slot within the cycle)
    let (first, second) = if off_a <= off_b {
        ((Parity::Tic, off_a, 0i64), (Parity::Toc, off_b, 1i64))
    } else {
        ((Parity::Toc, off_b, 0i64), (Parity::Tic, off_a, 1i64))
    };

    for (parity, off, slot) in [first, second] {
        // ALL predictions in the window, even when only a suffix is
        // extracted below — templates are cheap and benefit from full data.
        let predictions: Vec<f64> = (0..cycles)
            .map(|c| (fold_start as f64 + c as f64 * t_osc_env + off) * DECIMATION as f64)
            .collect();
        let Some(template) = build_template(raw, &predictions, gate) else {
            continue;
        };
        for (c, &pred) in predictions.iter().enumerate().skip(from_cycle_local) {
            let Some((idx, peak, rms)) =
                correlate_in_gate(raw, &template, pred.round() as i64, gate, sample_rate_hz)
            else {
                continue;
            };
            let snr_db = (20.0 * (peak / rms).log10()) as f32;
            if snr_db < DETECT_SNR_DB {
                continue;
            }
            out.push(BeatEvent {
                beat_index: beat_index_base + 2 * c as i64 + slot,
                parity,
                t_drop_corr_s: (raw_start_abs as f64 + idx + TEMPLATE_PEAK_AT as f64)
                    / sample_rate_hz,
                snr_db,
                t_drop_edge_s: None,
                t_unlock_s: None,
            });
        }
    }
    fill_edges(&mut out, raw, raw_start_abs, env, sample_rate_hz, t_osc_env);
    out.sort_by(|a, b| a.t_drop_corr_s.total_cmp(&b.t_drop_corr_s));
    out
}

/// Half-height leading-edge time (fractional index into `sm`) of the pulse
/// whose local maximum is at `peak_idx`. Scans backward for the 0.5·peak
/// crossing.
fn half_height_edge(sm: &[f32], peak_idx: usize) -> Option<f64> {
    let half = sm[peak_idx] * 0.5;
    let mut i = peak_idx;
    while i > 0 && sm[i - 1] >= half {
        i -= 1;
    }
    if i == 0 {
        return None;
    }
    // sm[i-1] < half ≤ sm[i]: interpolate between i-1 and i.
    let (lo, hi) = (sm[i - 1] as f64, sm[i] as f64);
    let frac = if hi > lo {
        (half as f64 - lo) / (hi - lo)
    } else {
        0.5
    };
    Some((i - 1) as f64 + frac)
}

/// Upward crossings of `thr` inside `sm[lo..hi]` (indices into `sm`).
fn upward_crossings(sm: &[f32], lo: usize, hi: usize, thr: f32) -> Vec<usize> {
    (lo.max(1)..hi)
        .filter(|&i| sm[i - 1] < thr && sm[i] >= thr)
        .collect()
}

/// Fills in `t_drop_edge_s`/`t_unlock_s` per event (Task 7, spec §5.5).
///
/// Half-height edge timing needs sub-millisecond precision: the amplitude
/// formula's sensitivity to the unlock-to-drop interval Δt grows sharply as
/// Δt shrinks (high amplitude), so a Δt bias of only ~0.1 ms can blow the
/// amplitude gate's ±5° bar even though it easily clears this module's own
/// ±0.3 ms bar (task-9 fix report: measured median bias 0.05–0.15 ms,
/// growing with amplitude, on the pre-decimated 3 kHz envelope — a
/// resolution ceiling that no amount of local interpolation cleverness
/// removed). So instead of reusing the already-decimated `env`, each event
/// gets its own small window of `raw` rectified and low-pass filtered fresh
/// at native rate (same cutoff as `EnvelopeExtractor`, spec §5.2, just
/// without its ÷16 decimation) immediately before edge-finding. `env` is
/// only used for `global_max`, a coarse whole-stream scale for the
/// threshold floor below — decimated resolution is fine for that.
fn fill_edges(
    events: &mut [BeatEvent],
    raw: &[f32],
    raw_start_abs: u64,
    env: &[f32],
    sample_rate_hz: f64,
    t_osc_env: f64,
) {
    if raw.is_empty() || env.is_empty() {
        return;
    }
    let global_max = env.iter().fold(0.0f32, |m, &v| m.max(v));
    if global_max <= 0.0 {
        return;
    }
    let cutoff_hz = (0.45 * sample_rate_hz / DECIMATION as f64).min(1_500.0);
    let tbeat8 = (t_osc_env / 16.0 * DECIMATION as f64) as usize; // T_beat/8, raw samples
    let one_ms = (1.0e-3 * sample_rate_hz).ceil() as usize;
    let ms3 = (3.0e-3 * sample_rate_hz) as usize;
    // Each event gets a freshly-seeded filter (events aren't in stream order
    // here — they're grouped by parity, sorted only after this pass), so its
    // window needs enough lead-in for the filter's own state to settle
    // before reaching the region actually used below.
    let settle = (0.015 * sample_rate_hz) as usize;
    let margin = tbeat8 + one_ms + ms3;

    for e in events.iter_mut() {
        let center_f = e.t_drop_corr_s * sample_rate_hz - raw_start_abs as f64;
        if !center_f.is_finite() || center_f < 0.0 {
            continue;
        }
        let center = center_f as usize;
        // Require the FULL window (settle pad + both margins); a beat within
        // `margin+settle` of either end of `raw` gets no edges rather than a
        // silently truncated (and potentially badly wrong) one -- same
        // graceful-degrade-to-None philosophy as everywhere else here.
        let need_lo = margin + settle;
        if center < need_lo || center + margin > raw.len() {
            continue;
        }
        let win_lo = center - need_lo;
        let win_hi = center + margin;
        let Ok(mut lp) = Butterworth::low_pass(sample_rate_hz, cutoff_hz) else {
            continue;
        };
        let local: Vec<f32> = raw[win_lo..win_hi]
            .iter()
            .map(|&s| lp.process(s.abs()))
            .collect();
        let to_seconds =
            |local_idx: f64| (raw_start_abs as f64 + win_lo as f64 + local_idx) / sample_rate_hz;
        let center_local = center - win_lo;

        let lo = center_local.saturating_sub(ms3);
        let hi = (center_local + ms3).min(local.len());
        if hi <= lo + 2 {
            continue;
        }
        let peak_idx = (lo..hi)
            .max_by(|&i, &j| local[i].total_cmp(&local[j]))
            .unwrap_or(center_local);
        let Some(drop_edge_idx) = half_height_edge(&local, peak_idx) else {
            continue;
        };
        e.t_drop_edge_s = Some(to_seconds(drop_edge_idx));

        // Unlocking search window: T_beat/8 ending 1 ms before the drop edge.
        let w_end = (drop_edge_idx as usize).saturating_sub(one_ms);
        let w_start = w_end.saturating_sub(tbeat8);
        if w_end <= w_start + 2 {
            continue;
        }
        // Noise floor: mean over the T_beat/8 window AFTER the drop peak (spec §5.5).
        let n_start = (peak_idx + 1).min(local.len());
        let n_end = (peak_idx + 1 + tbeat8).min(local.len());
        if n_end <= n_start {
            continue;
        }
        let noise =
            local[n_start..n_end].iter().map(|&v| v as f64).sum::<f64>() / (n_end - n_start) as f64;
        let mut thr = (0.01 * global_max as f64).max(1.4 * noise) as f32;
        let mut crossings = upward_crossings(&local, w_start, w_end, thr);
        while crossings.len() > 3 && (thr as f64) < 0.2 * global_max as f64 {
            thr *= 1.4;
            crossings = upward_crossings(&local, w_start, w_end, thr);
        }
        let Some(&first) = crossings.first() else {
            continue;
        };
        // The unlocking pulse's own local max after the crossing, inside the window.
        let u_peak = (first..w_end)
            .max_by(|&i, &j| local[i].total_cmp(&local[j]))
            .unwrap_or(first);
        let Some(u_edge_idx) = half_height_edge(&local, u_peak) else {
            continue;
        };
        let t_unlock = to_seconds(u_edge_idx);
        let dt = e.t_drop_edge_s.unwrap_or(f64::NAN) - t_unlock;
        let t_beat_s = t_osc_env / 2.0 / (sample_rate_hz / DECIMATION as f64);
        if dt >= 1.0e-3 && dt <= t_beat_s / 8.0 {
            e.t_unlock_s = Some(t_unlock);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::EnvelopeExtractor;
    use crate::filter::{Butterworth, DcBlocker};
    use crate::fold::{NBINS, fold_envelope};
    use crate::synth::{SynthConfig, synthesize};

    /// synth → precondition → envelope → fold → extract_events, one shot.
    fn pipeline_to_events(cfg: &SynthConfig) -> (Vec<BeatEvent>, f64 /*t_beat true*/) {
        let x = synthesize(cfg).expect("valid synth config");
        let sr = cfg.sample_rate_hz;
        let mut dc = DcBlocker::new();
        let mut hp = Butterworth::high_pass(sr, 3_000.0).unwrap();
        let raw: Vec<f32> = x.iter().map(|&s| hp.process(dc.process(s))).collect();
        let mut envx = EnvelopeExtractor::new(sr).unwrap();
        let mut env = Vec::new();
        envx.process(&raw, &mut env);
        let t_osc_env = crate::bph::t_osc_s(cfg.bph)
            * (1.0 - cfg.rate_s_per_day / 86_400.0)
            * envx.envelope_rate_hz();
        let profile = fold_envelope(&env, t_osc_env).expect("fold");
        let events = extract_events(&raw, 0, &env, sr, &profile);
        let t_beat = crate::bph::t_beat_s(cfg.bph) * (1.0 - cfg.rate_s_per_day / 86_400.0);
        (events, t_beat)
    }

    #[test]
    fn clean_signal_detects_nearly_every_beat_with_low_jitter() {
        let cfg = SynthConfig {
            snr_db: 30.0,
            ..SynthConfig::default()
        };
        let (events, t_beat) = pipeline_to_events(&cfg);
        let expected = (cfg.duration_s / t_beat) as usize;
        assert!(
            events.len() as f64 >= 0.85 * expected as f64,
            "{} of ~{expected} beats",
            events.len()
        );
        // Ground truth: drops at (k+1)·t_beat. Detection has a constant bias
        // (template anchor vs burst onset) — assert small bias and tiny jitter.
        let errs: Vec<f64> = events
            .iter()
            .map(|e| {
                let k = (e.t_drop_corr_s / t_beat).round();
                e.t_drop_corr_s - k * t_beat
            })
            .collect();
        let mean = errs.iter().sum::<f64>() / errs.len() as f64;
        let jitter =
            (errs.iter().map(|e| (e - mean).powi(2)).sum::<f64>() / errs.len() as f64).sqrt();
        assert!(mean.abs() < 2e-3, "bias {mean}");
        assert!(jitter < 2e-4, "jitter {jitter}");
        // Parities alternate along the sorted event stream.
        for w in events.windows(2) {
            if w[1].beat_index == w[0].beat_index + 1 {
                assert_ne!(w[0].parity, w[1].parity, "adjacent beats share parity");
            }
        }
        assert!(events.iter().all(|e| e.snr_db >= DETECT_SNR_DB));
    }

    #[test]
    fn low_snr_still_detects_a_useful_fraction() {
        let cfg = SynthConfig {
            snr_db: 10.0,
            ..SynthConfig::default()
        };
        let (events, t_beat) = pipeline_to_events(&cfg);
        let expected = (cfg.duration_s / t_beat) as usize;
        assert!(
            events.len() as f64 >= 0.5 * expected as f64,
            "{} of ~{expected} beats at 10 dB",
            events.len()
        );
    }

    #[test]
    fn beat_error_preserves_parity_split() {
        let cfg = SynthConfig {
            beat_error_ms: 0.8,
            snr_db: 30.0,
            ..SynthConfig::default()
        };
        let (events, _) = pipeline_to_events(&cfg);
        let tics = events.iter().filter(|e| e.parity == Parity::Tic).count();
        let tocs = events.len() - tics;
        assert!(tics >= 100 && tocs >= 100, "tics {tics} tocs {tocs}");
    }

    #[test]
    fn edges_recover_the_unlock_to_drop_interval() {
        // Ground truth: synth places unlocking exactly dt before each drop,
        // dt = unlock_to_drop_dt_s(amplitude, lift, T_osc).
        let cfg = SynthConfig {
            snr_db: 30.0,
            ..SynthConfig::default()
        };
        let (events, _) = pipeline_to_events(&cfg);
        let t_osc = crate::bph::t_osc_s(cfg.bph);
        let dt_true =
            crate::synth::unlock_to_drop_dt_s(cfg.amplitude_deg, cfg.lift_angle_deg, t_osc);
        let dts: Vec<f64> = events
            .iter()
            .filter_map(|e| Some(e.t_drop_edge_s? - e.t_unlock_s?))
            .collect();
        assert!(
            dts.len() as f64 >= 0.5 * events.len() as f64,
            "unlocking found on {} of {}",
            dts.len(),
            events.len()
        );
        let mut sorted = dts.clone();
        sorted.sort_by(f64::total_cmp);
        let median = sorted[sorted.len() / 2];
        assert!(
            (median - dt_true).abs() < 3e-4,
            "median dt {median} want {dt_true}"
        );
    }

    #[test]
    fn weak_signal_degrades_unlocking_not_drops() {
        let cfg = SynthConfig {
            snr_db: 12.0,
            ..SynthConfig::default()
        };
        let (events, _) = pipeline_to_events(&cfg);
        assert!(!events.is_empty());
        let unlocked = events.iter().filter(|e| e.t_unlock_s.is_some()).count();
        // At low SNR the quiet unlocking pulse is lost more often than the loud
        // drop — the point of the tiered design. No hard floor here; just prove
        // the code degrades to None instead of fabricating.
        for e in &events {
            if let (Some(u), Some(d)) = (e.t_unlock_s, e.t_drop_edge_s) {
                let dt = d - u;
                assert!(
                    dt >= 1e-3 && dt <= crate::bph::t_beat_s(cfg.bph) / 8.0,
                    "dt {dt}"
                );
            }
        }
        assert!(
            unlocked < events.len(),
            "at 12 dB some unlocking pulses must be lost (unlocked {unlocked} of {})",
            events.len()
        );
    }

    #[test]
    fn amplitude_sweep_tracks_dt_monotonically() {
        // Bigger amplitude ⇒ smaller dt. The measured medians must order correctly.
        let mut medians = Vec::new();
        for amp in [220.0f64, 270.0, 320.0] {
            let cfg = SynthConfig {
                amplitude_deg: amp,
                snr_db: 30.0,
                ..SynthConfig::default()
            };
            let (events, _) = pipeline_to_events(&cfg);
            let mut dts: Vec<f64> = events
                .iter()
                .filter_map(|e| Some(e.t_drop_edge_s? - e.t_unlock_s?))
                .collect();
            assert!(dts.len() > 50, "amp {amp}: {} intervals", dts.len());
            dts.sort_by(f64::total_cmp);
            medians.push(dts[dts.len() / 2]);
        }
        assert!(
            medians[0] > medians[1] && medians[1] > medians[2],
            "medians {medians:?}"
        );
    }

    #[test]
    fn extract_events_wrapper_preserves_cycle_count_with_fractional_t_osc_env() {
        // Regression test for the cycle-loss bug: when t_osc_env is fractional,
        // the wrapper's fold_start computation could cause anchored to recompute
        // one fewer cycle (C → C−1) due to floating-point precision loss.
        // This test verifies the wrapper and a full-window anchored call extract
        // the same number of events at the same beat indices.
        let cfg = SynthConfig {
            rate_s_per_day: 12.0,
            snr_db: 30.0,
            ..SynthConfig::default()
        };
        let (events_via_wrapper, _) = pipeline_to_events(&cfg);

        // Now call anchored directly with a full window (fold_start = 0)
        // to get the "ground truth" extraction of ALL cycles.
        let x = synthesize(&cfg).expect("valid synth config");
        let sr = cfg.sample_rate_hz;
        let mut dc = DcBlocker::new();
        let mut hp = Butterworth::high_pass(sr, 3_000.0).unwrap();
        let raw: Vec<f32> = x.iter().map(|&s| hp.process(dc.process(s))).collect();
        let mut envx = EnvelopeExtractor::new(sr).unwrap();
        let mut env = Vec::new();
        envx.process(&raw, &mut env);
        let t_osc_env = crate::bph::t_osc_s(cfg.bph)
            * (1.0 - cfg.rate_s_per_day / 86_400.0)
            * envx.envelope_rate_hz();
        let profile = fold_envelope(&env, t_osc_env).expect("fold");
        let events_via_anchored = extract_events_anchored(&raw, 0, &env, sr, &profile, 0, 0, 0);

        // Both extractions should detect the same events (same count, same beat indices).
        assert_eq!(
            events_via_wrapper.len(),
            events_via_anchored.len(),
            "wrapper and anchored event counts differ"
        );
        if !events_via_wrapper.is_empty() && !events_via_anchored.is_empty() {
            let max_wrapper = events_via_wrapper
                .iter()
                .map(|e| e.beat_index)
                .max()
                .unwrap();
            let max_anchored = events_via_anchored
                .iter()
                .map(|e| e.beat_index)
                .max()
                .unwrap();
            assert_eq!(
                max_wrapper, max_anchored,
                "wrapper max beat_index {max_wrapper}, anchored {max_anchored}"
            );
        }
    }

    /// Builds a deterministic `(raw, env, profile)` triple with a
    /// unit-amplitude impulse near every Tic/Toc grid position implied by
    /// `t_osc_env`/`env_len` (anchored at `fold_start = 0`) — every cycle is
    /// equally "loud," so `build_template`'s outlier trim can't mistake a
    /// real beat for noise, and the returned event count becomes a direct,
    /// deterministic proxy for how many cycles the extraction actually
    /// walked. This isolates the fold_start/cycle-count ARITHMETIC (what
    /// finding 1 is about) from real signal-detection quality.
    ///
    /// Each impulse is placed `TEMPLATE_PEAK_AT / 2` samples after its
    /// nominal grid position, not exactly on it. `correlate_in_gate`
    /// doesn't search `[center−gate, center+gate)` for the signal itself —
    /// it correlates a `TEMPLATE_LEN`-long template (whose own peak sits at
    /// local offset `TEMPLATE_PEAK_AT`) starting at each `start` in that
    /// range, so the effective search window for a single-sample-delta
    /// template is shifted to `[center−gate+TEMPLATE_PEAK_AT,
    /// center+gate+TEMPLATE_PEAK_AT)`. A caller whose `fold_start` is off
    /// by even one envelope sample (this bug) moves `center` by
    /// `DECIMATION` raw samples; placing impulses exactly on the nominal
    /// grid leaves near-zero margin against that shift on the low side
    /// (only `TEMPLATE_PEAK_AT − gate` samples, which is negative — i.e. NO
    /// margin — whenever `gate < TEMPLATE_PEAK_AT`). Offsetting every
    /// impulse by a fixed `TEMPLATE_PEAK_AT / 2` instead centers it in the
    /// intersection of `build_template`'s symmetric ±gate window and
    /// `correlate_in_gate`'s shifted window — that intersection's midpoint
    /// is `TEMPLATE_PEAK_AT / 2` regardless of `gate` — keeping comfortable
    /// margin on both sides for every `t_osc_env` used by the tests below.
    fn synthetic_cycle_grid(t_osc_env: f64, env_len: usize) -> (Vec<f32>, Vec<f32>, FoldProfile) {
        let env = vec![0.0f32; env_len];
        let profile = FoldProfile {
            bins: vec![0.0f32; NBINS],
            t_osc_env,
            anchor_a_phase: 0.0,
            anchor_b_phase: (NBINS / 2) as f64,
            contrast: 1.0,
        };
        let (off_a, off_b) = profile.anchor_env_offsets();
        let cycles = (env_len as f64 / t_osc_env).floor() as usize;
        let mut raw = vec![0.0f32; env_len * crate::envelope::DECIMATION + 8_192];
        for c in 0..cycles {
            for off in [off_a, off_b] {
                let idx = ((c as f64 * t_osc_env + off) * crate::envelope::DECIMATION as f64)
                    .round() as usize
                    + (TEMPLATE_PEAK_AT / 2);
                raw[idx] = 1.0;
            }
        }
        (raw, env, profile)
    }

    #[test]
    fn extract_events_wrapper_matches_anchored_full_window_exact_counterexample() {
        // Reviewer's exact counterexample for the wrapper's cycle-loss bug
        // (task-11 fix round, finding 1): t_osc_env=100.909, env.len()=100910.
        // In real arithmetic floor(100910/100.909) = 1000, and 1000·100.909
        // is also exactly 100909 — but computed in f64, `cycles as f64 *
        // t_osc_env` rounds to exactly 100909.0 (so `.ceil()` is a no-op)
        // while dividing back, `100909.0 / 100.909_f64`, rounds DOWN below
        // 1000 because the f64 representation of 100.909 used by the
        // division isn't the same value the multiplication effectively
        // used. A one-shot ceil-based fold_start can't see this; only
        // replaying anchored's own division and self-correcting can.
        let t_osc_env = 100.909;
        let env_len = 100_910usize;
        let (raw, env, profile) = synthetic_cycle_grid(t_osc_env, env_len);
        let sr = 8_000.0;

        let via_wrapper = extract_events(&raw, 0, &env, sr, &profile);
        let via_anchored_full = extract_events_anchored(&raw, 0, &env, sr, &profile, 0, 0, 0);

        assert!(
            via_anchored_full.len() > 1_900,
            "sanity: full-window extraction should find close to 2*1000 events, got {}",
            via_anchored_full.len()
        );
        assert_eq!(
            via_wrapper.len(),
            via_anchored_full.len(),
            "wrapper found {} events but the full-window (fold_start=0) extraction found {} \
             — the wrapper dropped the trailing cycle",
            via_wrapper.len(),
            via_anchored_full.len()
        );
    }

    #[test]
    fn cycle_grid_fold_start_recovers_exact_cycle_count_for_exact_counterexample() {
        // Same counterexample as the DSP-level test above, but pinned
        // directly to the arithmetic that finding 1 is actually about:
        // `cycle_grid_fold_start`'s own claimed invariant is that
        // re-deriving cycles from its result always recovers the same
        // `cycles` that `floor(env_len / t_osc_env)` gives directly.
        let t_osc_env = 100.909;
        let env_len = 100_910usize;
        let expected_cycles = (env_len as f64 / t_osc_env).floor() as usize;
        assert_eq!(expected_cycles, 1000, "sanity on the counterexample itself");

        let fold_start = cycle_grid_fold_start(env_len, t_osc_env);
        let recomputed = (env_len.saturating_sub(fold_start) as f64 / t_osc_env).floor() as usize;
        assert_eq!(
            recomputed, expected_cycles,
            "fold_start={fold_start} recomputed {recomputed} cycles, want {expected_cycles}"
        );
    }

    #[test]
    fn cycle_grid_fold_start_recovers_exact_cycle_count_across_fractional_t_and_len_sweep() {
        // Broader regression net for the same rounding class: 20 fractional
        // t_osc_env values (found by numeric search to trip the pre-fix
        // ceil-only formula at at least one of the 3 lengths below) times 3
        // env lengths bracketing the exact multiply/divide rounding
        // boundary each — 40 of these 60 cases fail against the pre-fix
        // ceil-only formula. `cycle_grid_fold_start` must never cost a
        // trailing cycle for any of them.
        let cases: [(f64, usize); 20] = [
            (70.15, 4210),
            (73.4, 2203),
            (76.65, 4600),
            (79.9, 2398),
            (83.15, 4990),
            (83.54, 4178),
            (86.4, 2593),
            (87.18, 4360),
            (89.65, 5380),
            (92.9, 2788),
            (93.68, 2343),
            (139.05, 8344),
            (142.3, 4270),
            (145.55, 8734),
            (148.8, 4465),
            (152.05, 9124),
            (155.3, 4660),
            (158.55, 9514),
            (161.8, 4855),
            (165.05, 9904),
        ];
        for (t_osc_env, base_len) in cases {
            for delta in [-1i64, 0, 1] {
                let env_len = (base_len as i64 + delta) as usize;
                let expected_cycles = (env_len as f64 / t_osc_env).floor() as usize;
                let fold_start = cycle_grid_fold_start(env_len, t_osc_env);
                let recomputed =
                    (env_len.saturating_sub(fold_start) as f64 / t_osc_env).floor() as usize;
                assert_eq!(
                    recomputed, expected_cycles,
                    "t_osc_env={t_osc_env} env_len={env_len}: fold_start={fold_start} \
                     recomputed {recomputed} cycles, want {expected_cycles}"
                );
            }
        }
    }
}
