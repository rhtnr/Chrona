//! Stage 5 (spec §5.5): learned matched-filter drop detection on the
//! band-passed signal, plus envelope-edge extraction (Task 7).

use crate::envelope::DECIMATION;
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
///
/// **Alignment invariant:** `raw` and `env` come from rings fed in lockstep
/// from stream start, so envelope absolute index `j` corresponds to raw
/// absolute index `j · envelope::DECIMATION`; callers pass `env_start_abs`
/// in envelope-domain units and `raw_start_abs` in raw units.
///
/// **Fold-window invariant:** the fold in `fold_envelope` starts at
/// `env.len() − (cycles·t_osc_env) as usize` computed from the SAME env
/// slice length; this function recomputes that offset with the identical
/// two lines (kept in sync by this comment on both sites).
pub fn extract_events(
    raw: &[f32],
    raw_start_abs: u64,
    env: &[f32],
    env_start_abs: u64,
    sample_rate_hz: f64,
    profile: &FoldProfile,
) -> Vec<BeatEvent> {
    let _ = env_start_abs; // env used by the Task-7 edge pass; alignment documented above
    let t_osc_env = profile.t_osc_env;
    if !(t_osc_env.is_finite() && t_osc_env > 1.0) || env.is_empty() {
        return Vec::new();
    }
    // Must match fold_envelope's window arithmetic (see invariant note).
    let cycles = (env.len() as f64 / t_osc_env).floor() as usize;
    if cycles < 8 {
        return Vec::new();
    }
    let usable = (cycles as f64 * t_osc_env) as usize;
    let fold_start = env.len() - usable;

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
        let predictions: Vec<f64> = (0..cycles)
            .map(|c| (fold_start as f64 + c as f64 * t_osc_env + off) * DECIMATION as f64)
            .collect();
        let Some(template) = build_template(raw, &predictions, gate) else {
            continue;
        };
        for (c, &pred) in predictions.iter().enumerate() {
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
                beat_index: 2 * c as i64 + slot,
                parity,
                t_drop_corr_s: (raw_start_abs as f64 + idx + TEMPLATE_PEAK_AT as f64)
                    / sample_rate_hz,
                snr_db,
                t_drop_edge_s: None,
                t_unlock_s: None,
            });
        }
    }
    fill_edges(&mut out, env, env_start_abs, sample_rate_hz, t_osc_env);
    out.sort_by(|a, b| a.t_drop_corr_s.total_cmp(&b.t_drop_corr_s));
    out
}

/// 3-sample box smooth (≈1 ms at 3 kHz envelope rate), spec §5.5.
fn smooth3(env: &[f32]) -> Vec<f32> {
    let n = env.len();
    let mut out = vec![0.0f32; n];
    for i in 0..n {
        let a = env[i.saturating_sub(1)];
        let b = env[i];
        let c = env[(i + 1).min(n - 1)];
        out[i] = (a + b + c) / 3.0;
    }
    out
}

/// Half-height leading-edge time (fractional env index) of the pulse whose
/// local maximum is at `peak_idx`. Scans backward for the 0.5·peak crossing.
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

fn fill_edges(
    events: &mut [BeatEvent],
    env: &[f32],
    env_start_abs: u64,
    sample_rate_hz: f64,
    t_osc_env: f64,
) {
    if env.is_empty() {
        return;
    }
    let sm = smooth3(env);
    let global_max = sm.iter().fold(0.0f32, |m, &v| m.max(v));
    if global_max <= 0.0 {
        return;
    }
    let env_rate = sample_rate_hz / DECIMATION as f64;
    let to_env_idx = |t_s: f64| t_s * env_rate - env_start_abs as f64;
    let to_seconds = |idx: f64| (env_start_abs as f64 + idx) / env_rate;
    let tbeat8 = (t_osc_env / 16.0) as usize; // T_beat/8 in env samples
    let ms3 = (3.0e-3 * env_rate) as usize;

    for e in events.iter_mut() {
        let center = to_env_idx(e.t_drop_corr_s);
        if !center.is_finite() || center < 0.0 {
            continue;
        }
        let center = center as usize;
        let lo = center.saturating_sub(ms3);
        let hi = (center + ms3).min(sm.len());
        if hi <= lo + 2 {
            continue;
        }
        let peak_idx = (lo..hi)
            .max_by(|&i, &j| sm[i].total_cmp(&sm[j]))
            .unwrap_or(center);
        let Some(drop_edge_idx) = half_height_edge(&sm, peak_idx) else {
            continue;
        };
        e.t_drop_edge_s = Some(to_seconds(drop_edge_idx));

        // Unlocking search window: T_beat/8 ending 1 ms before the drop edge.
        let one_ms = (1.0e-3 * env_rate).ceil() as usize;
        let w_end = (drop_edge_idx as usize).saturating_sub(one_ms);
        let w_start = w_end.saturating_sub(tbeat8);
        if w_end <= w_start + 2 {
            continue;
        }
        // Noise floor: mean over the T_beat/8 window AFTER the drop peak (spec §5.5).
        let n_start = (peak_idx + 1).min(sm.len());
        let n_end = (peak_idx + 1 + tbeat8).min(sm.len());
        if n_end <= n_start {
            continue;
        }
        let noise =
            sm[n_start..n_end].iter().map(|&v| v as f64).sum::<f64>() / (n_end - n_start) as f64;
        let mut thr = (0.01 * global_max as f64).max(1.4 * noise) as f32;
        let mut crossings = upward_crossings(&sm, w_start, w_end, thr);
        while crossings.len() > 3 && (thr as f64) < 0.2 * global_max as f64 {
            thr *= 1.4;
            crossings = upward_crossings(&sm, w_start, w_end, thr);
        }
        let Some(&first) = crossings.first() else {
            continue;
        };
        // The unlocking pulse's own local max after the crossing, inside the window.
        let u_peak = (first..w_end)
            .max_by(|&i, &j| sm[i].total_cmp(&sm[j]))
            .unwrap_or(first);
        let Some(u_edge_idx) = half_height_edge(&sm, u_peak) else {
            continue;
        };
        let t_unlock = to_seconds(u_edge_idx);
        let dt = e.t_drop_edge_s.unwrap_or(f64::NAN) - t_unlock;
        let t_beat_s = t_osc_env / 2.0 / env_rate;
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
    use crate::fold::fold_envelope;
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
        let events = extract_events(&raw, 0, &env, 0, sr, &profile);
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
        assert!(unlocked <= events.len());
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
}
