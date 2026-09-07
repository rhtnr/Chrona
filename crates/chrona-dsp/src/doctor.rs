//! Mic Doctor signal-analysis core (binding spec §3.2 steps 1–3 + score,
//! Task 9): pure fns over already-captured buffers only — silence/hum, the
//! tick test, AGC/gate pumping detection, and the setup score. No
//! capture/OS-level logic here (that's M5, per the M4 design doc §5/§10).

use crate::analyzer::{Analyzer, AnalyzerConfig};
use crate::tier::Tier;
use realfft::RealFftPlanner;

/// Step 1 (silence test): broadband noise floor + optional mains-hum report.
#[derive(Debug, Clone, Copy)]
pub struct SilenceReport {
    pub noise_floor_dbfs: f64,
    pub hum: Option<HumReport>,
}

/// A detected mains-hum family (fundamental + 2nd/3rd harmonics all clearing
/// the local floor — see `silence_report`'s doc comment for the exact gate).
#[derive(Debug, Clone, Copy)]
pub struct HumReport {
    pub freq_hz: f64,
    /// Fundamental's level over the local median floor, in dB.
    pub strength_db: f64,
}

/// RMS noise floor (dBFS) and mains-hum detection over a silence-test
/// capture (no watch present — spec §3.2 step 1).
///
/// Hum detection: Hann-windowed rFFT of `samples`. Candidate fundamentals
/// at 50/60 Hz, each checked in its own ±1 Hz band together with the same
/// ±1 Hz band around its 2nd/3rd harmonics. A "local median floor" is the
/// median magnitude of the 20–300 Hz region EXCLUDING every ±1 Hz
/// candidate/harmonic band for BOTH 50 and 60 Hz families (so a real hum
/// peak can never bias its own reference floor upward). Hum is reported
/// when the fundamental AND both harmonics each clear that floor by
/// ≥ 12 dB (spec §3.2). `freq_hz` is the fundamental's own peak-bin
/// frequency (within the searched ±1 Hz); `strength_db` is the
/// fundamental's dB-over-floor. When both 50 and 60 Hz families clear the
/// gate, the stronger one is reported.
pub fn silence_report(samples: &[f32], sample_rate_hz: f64) -> SilenceReport {
    SilenceReport {
        noise_floor_dbfs: rms_dbfs(samples),
        hum: detect_hum(samples, sample_rate_hz),
    }
}

fn rms_dbfs(samples: &[f32]) -> f64 {
    if samples.is_empty() {
        return f64::NEG_INFINITY;
    }
    let ms = samples.iter().map(|&s| (s as f64).powi(2)).sum::<f64>() / samples.len() as f64;
    20.0 * ms.sqrt().max(1e-12).log10()
}

/// "Upper" median (matches this crate's existing convention, e.g.
/// `metrics::median`): sorts and takes the middle element.
fn median(v: &mut [f64]) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(f64::total_cmp);
    Some(v[v.len() / 2])
}

fn hz_to_bin(hz: f64, bin_hz: f64, len: usize) -> usize {
    if bin_hz <= 0.0 {
        return 0;
    }
    ((hz / bin_hz).round().max(0.0) as usize).min(len)
}

/// Inclusive-lo/exclusive-hi bin range covering `center_hz ± tol_hz`, clamped to `[0, len]`.
fn band_range(center_hz: f64, tol_hz: f64, bin_hz: f64, len: usize) -> (usize, usize) {
    let lo = hz_to_bin(center_hz - tol_hz, bin_hz, len);
    let hi = hz_to_bin(center_hz + tol_hz, bin_hz, len).max(lo);
    (lo, hi)
}

/// (index, magnitude) of the largest value in `mag[lo..hi]`; `(lo, 0.0)` on an empty range.
fn peak_in(mag: &[f64], lo: usize, hi: usize) -> (usize, f64) {
    if lo >= hi || lo >= mag.len() {
        return (lo.min(mag.len().saturating_sub(1)), 0.0);
    }
    let hi = hi.min(mag.len());
    let mut best_i = lo;
    let mut best_v = mag[lo];
    for (i, &v) in mag.iter().enumerate().take(hi).skip(lo) {
        if v > best_v {
            best_v = v;
            best_i = i;
        }
    }
    (best_i, best_v)
}

/// dB gate for hum detection (spec §3.2): fundamental AND both harmonics
/// must each clear the local median floor by this much.
const HUM_DB_GATE: f64 = 12.0;

fn detect_hum(samples: &[f32], sample_rate_hz: f64) -> Option<HumReport> {
    let n = samples.len();
    if n < 64 || sample_rate_hz <= 0.0 {
        return None;
    }
    // Hann window: reduces spectral leakage. The main-lobe widening this
    // causes is fine against the ±1 Hz search band at the multi-second
    // buffer lengths a silence-test capture uses (sub-Hz bin resolution).
    let windowed: Vec<f32> = samples
        .iter()
        .enumerate()
        .map(|(i, &s)| {
            let w = 0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / (n - 1) as f64).cos();
            s * w as f32
        })
        .collect();
    let mut planner = RealFftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(n);
    let mut buf = windowed;
    let mut spectrum = fft.make_output_vec();
    // Invariant: buffer lengths come from the planner itself, so process cannot fail.
    fft.process(&mut buf, &mut spectrum)
        .expect("planner-sized buffers");
    let mag: Vec<f64> = spectrum
        .iter()
        .map(|c| ((c.re as f64).powi(2) + (c.im as f64).powi(2)).sqrt())
        .collect();
    let bin_hz = sample_rate_hz / n as f64;

    let candidates = [50.0f64, 60.0];
    let harmonics = [1.0f64, 2.0, 3.0];
    let tol_hz = 1.0;

    // Exclude every candidate/harmonic band (both families) from the floor
    // estimate so a real hum peak can't bias its own reference upward.
    let excluded: Vec<(usize, usize)> = candidates
        .iter()
        .flat_map(|&f0| harmonics.iter().map(move |&h| f0 * h))
        .map(|f| band_range(f, tol_hz, bin_hz, mag.len()))
        .collect();
    let floor_lo = hz_to_bin(20.0, bin_hz, mag.len());
    let floor_hi = hz_to_bin(300.0, bin_hz, mag.len()).max(floor_lo);
    let mut floor_samples: Vec<f64> = (floor_lo..floor_hi)
        .filter(|i| !excluded.iter().any(|&(lo, hi)| *i >= lo && *i < hi))
        .map(|i| mag[i])
        .collect();
    let local_floor = median(&mut floor_samples).unwrap_or(0.0).max(1e-12);

    let mut best: Option<HumReport> = None;
    for &f0 in &candidates {
        let (lo0, hi0) = band_range(f0, tol_hz, bin_hz, mag.len());
        let (peak_idx, peak_mag) = peak_in(&mag, lo0, hi0);
        let db0 = 20.0 * (peak_mag.max(1e-12) / local_floor).log10();
        if db0 < HUM_DB_GATE {
            continue;
        }
        let harmonics_clear = harmonics[1..].iter().all(|&h| {
            let (lo, hi) = band_range(f0 * h, tol_hz, bin_hz, mag.len());
            let (_, hmag) = peak_in(&mag, lo, hi);
            20.0 * (hmag.max(1e-12) / local_floor).log10() >= HUM_DB_GATE
        });
        if !harmonics_clear {
            continue;
        }
        let report = HumReport {
            freq_hz: peak_idx as f64 * bin_hz,
            strength_db: db0,
        };
        if best.is_none_or(|b: HumReport| report.strength_db > b.strength_db) {
            best = Some(report);
        }
    }
    best
}

/// Step 2 (tick test): reuses the streaming analyzer's own quality path on
/// a throwaway `Analyzer` (spec §3.2) rather than a separate detector.
#[derive(Debug, Clone, Copy)]
pub struct TickReport {
    /// `None` iff the throwaway analysis produced no metrics at all
    /// (`Analyzer::current_metrics() == None` — "Tier 0" in the analyzer's
    /// own vocabulary, spec §5.7). `Tier::T1` implies a defensible rate
    /// estimate exists; reporting it for a no-signal buffer would misstate
    /// capability that isn't there (this crate's honesty rule), so the
    /// no-metrics case is represented as the absence of a tier, not as T1.
    pub tier: Option<Tier>,
    pub band_snr_db: Option<f64>,
    pub clipped: u64,
}

/// Runs `samples` through a fresh `Analyzer` at its default configuration
/// and reports the achieved tier, mean per-beat matched-filter SNR (from
/// `Quality::mean_beat_snr_db`), and clipped-sample count.
///
/// `tier` is `None` when the analyzer can't produce ANY snapshot — the
/// clip count still comes through even then (clipping is meaningful even
/// absent a beat signal, mirroring `Analyzer::clipped_samples`'s own doc
/// comment). An unusable `sample_rate_hz` (analyzer construction failure)
/// degrades the same way, with `clipped: 0` — this function never panics.
pub fn tick_report(samples: &[f32], sample_rate_hz: f64) -> TickReport {
    let no_beat = TickReport {
        tier: None,
        band_snr_db: None,
        clipped: 0,
    };
    let Ok(mut analyzer) = Analyzer::new(AnalyzerConfig {
        sample_rate_hz,
        ..AnalyzerConfig::default()
    }) else {
        return no_beat;
    };
    analyzer.push_samples(samples);
    match analyzer.current_metrics() {
        Some(snap) => TickReport {
            tier: Some(snap.tier),
            band_snr_db: snap.quality.mean_beat_snr_db,
            clipped: snap.quality.clipped_samples,
        },
        None => TickReport {
            clipped: analyzer.clipped_samples(),
            ..no_beat
        },
    }
}

/// Step 3 (AGC/gate detection): two independent post-tick floor
/// signatures, each `None` when not detected.
#[derive(Debug, Clone, Copy)]
pub struct AgcReport {
    /// Measured post-tick dip depth (dB, negative) when the floor dips
    /// after each tick and substantially recovers before the next one
    /// ("pumping").
    pub agc_db: Option<f64>,
    /// Measured floor collapse (dB, negative, ≤ −20 dB) when the floor
    /// stays collapsed through the LATE part of the gap too — i.e. never
    /// recovers before the next tick.
    pub gate_db: Option<f64>,
}

/// Minimum lead-in (seconds) before the first detected tick required to
/// trust it as an undamped reference floor.
const MIN_LEAD_IN_S: f64 = 0.05;
/// ≥ this much floor collapse (dB) relative to the undamped reference,
/// SUSTAINED into the late part of the gap, reads as a gate (spec §3.2).
const GATE_DB_THRESHOLD: f64 = -20.0;
/// A measurable early-vs-late dip below this (dB) that ISN'T sustained
/// (see gate above) reads as AGC pumping.
const AGC_DIP_DB_THRESHOLD: f64 = -1.5;

/// Detects tick times via a throwaway `Analyzer`, then measures how the
/// short-window RMS floor behaves in the GAPS between ticks (spec §3.2
/// step 3). For each gap, an adaptive guard band (8 % of the gap, clamped
/// to 5–20 ms) is excluded at both ends to skip each tick's own burst
/// tail/lead; inside what remains, a narrow (3 ms) window right after the
/// guard ("early") and one right before the next guard ("late") are
/// RMS-measured. Across all gaps:
/// - **Gate/suppressor**: median `late` level ≤ 20 dB below the undamped
///   reference (see below) — the floor never recovered even by the LATE
///   part of the gap. `gate_db` is that measured collapse.
/// - **AGC ("pumping")**: not gated, but median `early` is measurably (≥
///   1.5 dB) quieter than median `late` — a dip that recovers within the
///   gap. `agc_db` is that measured early-vs-late delta.
///
/// The undamped reference is the RMS of whatever precedes the first
/// detected tick (silence/lead-in — undamped by construction). Too little
/// lead-in, too few gaps, or too few detected ticks reports both `None`
/// (no reference or insufficient evidence, never a guess).
pub fn agc_report(samples: &[f32], sample_rate_hz: f64) -> AgcReport {
    let none = AgcReport {
        agc_db: None,
        gate_db: None,
    };
    let Ok(mut analyzer) = Analyzer::new(AnalyzerConfig {
        sample_rate_hz,
        ..AnalyzerConfig::default()
    }) else {
        return none;
    };
    analyzer.push_samples(samples);
    let _ = analyzer.current_metrics();
    let mut drops: Vec<f64> = analyzer
        .tape_events()
        .iter()
        .map(|e| e.t_drop_corr_s)
        .collect();
    drops.sort_by(f64::total_cmp);
    drops.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
    if drops.len() < 6 {
        return none;
    }

    let sr = sample_rate_hz;
    let rms_at = |lo_s: f64, hi_s: f64| -> Option<f64> {
        if !(lo_s.is_finite() && hi_s.is_finite()) || hi_s <= lo_s {
            return None;
        }
        let lo = (lo_s * sr).round();
        let hi = (hi_s * sr).round();
        if lo < 0.0 || hi > samples.len() as f64 {
            return None;
        }
        let (lo, hi) = (lo as usize, hi as usize);
        if hi <= lo {
            return None;
        }
        let seg = &samples[lo..hi];
        let ms = seg.iter().map(|&s| (s as f64).powi(2)).sum::<f64>() / seg.len() as f64;
        Some(ms.sqrt())
    };

    // Undamped reference: whatever precedes the first detected tick.
    if drops[0] < MIN_LEAD_IN_S {
        return none;
    }
    let Some(floor_ref) = rms_at(0.0, drops[0]).filter(|&f| f > 0.0) else {
        return none;
    };

    let mut early = Vec::new();
    let mut late = Vec::new();
    for w in drops.windows(2) {
        let (t0, t1) = (w[0], w[1]);
        let gap = t1 - t0;
        let guard = (gap * 0.08).clamp(0.005, 0.02);
        let inner_lo = t0 + guard;
        let inner_hi = t1 - guard;
        let seg = 0.003; // narrow (3 ms) sampling window at each extreme
        if inner_hi - inner_lo < seg {
            continue;
        }
        if let Some(r) = rms_at(inner_lo, inner_lo + seg) {
            early.push(r);
        }
        if let Some(r) = rms_at(inner_hi - seg, inner_hi) {
            late.push(r);
        }
    }
    if early.len() < 4 || late.len() < 4 {
        return none;
    }
    let early_m = median(&mut early).unwrap_or(0.0);
    let late_m = median(&mut late).unwrap_or(0.0);

    let late_db = 20.0 * (late_m.max(1e-12) / floor_ref).log10();
    if late_db <= GATE_DB_THRESHOLD {
        return AgcReport {
            agc_db: None,
            gate_db: Some(late_db),
        };
    }
    let dip_db = 20.0 * (early_m.max(1e-12) / late_m.max(1e-12)).log10();
    if dip_db <= AGC_DIP_DB_THRESHOLD {
        return AgcReport {
            agc_db: Some(dip_db),
            gate_db: None,
        };
    }
    none
}

/// Setup score + ranked fixes.
#[derive(Debug, Clone)]
pub struct DoctorScore {
    pub score: u8,
    pub advice: Vec<&'static str>,
}

/// Step 5 (setup score, minus the OS-specific checks which land in M5):
/// starts at 100 and applies fixed deductions for whatever evidence is
/// present — a `None` input contributes no penalty ("not run" is never
/// scored as a failure, honesty gate).
///
/// Advice ranking rule (causes rank first, symptom ranks last): the five
/// CAUSE deductions (clipping, AGC, gate, hum, noise floor) each carry an
/// advice string from the binding spec §3.2 fix list and rank AMONG
/// THEMSELVES by deduction size (when two share a fix — AGC and gate both
/// being "the OS is processing the signal" — the string appears once, at
/// its higher rank). `tier < T2` (or no metrics at all, `tier: None` —
/// scored identically, see below) is the single biggest deduction but is
/// a SYMPTOM, not an independent cause: it's what a weak/missing pickup
/// looks like, and its own advice
/// (`"try pressing wired earbuds against the crown"` — the spec's
/// remedial for weak pickup) is always appended LAST, after every cause
/// advice, regardless of its deduction being the largest. This keeps
/// specific, actionable findings ahead of the general "signal is weak"
/// conclusion, and is what makes a combined case (e.g. a gated signal
/// that's also below T2) list the gate fix first.
pub fn doctor_score(
    silence: Option<&SilenceReport>,
    tick: Option<&TickReport>,
    agc: Option<&AgcReport>,
) -> DoctorScore {
    let mut score: i32 = 100;
    let mut cause_hits: Vec<(i32, &'static str)> = Vec::new();
    let mut weak_tier = false;

    if let Some(t) = tick {
        if t.clipped > 0 {
            score -= 20;
            cause_hits.push((20, "lower input gain"));
        }
        // No metrics at all is at least as bad as a defensible-but-weak T1
        // — both score as the same symptom.
        let below_t2 = t.tier.is_none_or(|tier| tier < Tier::T2);
        if below_t2 {
            score -= 30;
            weak_tier = true;
        }
    }
    if let Some(a) = agc {
        if a.gate_db.is_some() {
            score -= 25;
            cause_hits.push((25, "disable enhancements"));
        }
        if a.agc_db.is_some() {
            score -= 15;
            cause_hits.push((15, "disable enhancements"));
        }
    }
    if let Some(s) = silence {
        if s.hum.as_ref().is_some_and(|h| h.strength_db >= 20.0) {
            score -= 10;
            cause_hits.push((10, "use wired mic"));
        }
        if s.noise_floor_dbfs > -50.0 {
            score -= 10;
            cause_hits.push((10, "a $10 piezo contact pickup reaches Tier 3"));
        }
    }

    cause_hits.sort_by_key(|&(deduction, _)| std::cmp::Reverse(deduction)); // stable: ties keep insertion order
    let mut advice: Vec<&'static str> = Vec::new();
    for (_, text) in cause_hits {
        if !advice.contains(&text) {
            advice.push(text);
        }
    }
    if weak_tier {
        advice.push("try pressing wired earbuds against the crown");
    }

    DoctorScore {
        score: score.clamp(0, 100) as u8,
        advice,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synth::{Rng, SynthConfig, synthesize};

    /// A "silence test" ground-truth buffer: white noise (+ optional mains
    /// hum) with NO ticks at all — mirrors `synth::synthesize`'s own
    /// noise/hum formulas exactly (same `noise_rms` convention, same
    /// harmonic ratios) so this is a faithful silence-only fixture, not an
    /// approximation via tick suppression.
    fn noise_and_hum(
        n: usize,
        sr: f64,
        noise_rms: f64,
        hum_hz: Option<f64>,
        hum_level: f64,
        seed: u64,
    ) -> Vec<f32> {
        let mut rng = Rng::new(seed);
        let noise_gain = noise_rms * 3f64.sqrt();
        let mut x: Vec<f32> = (0..n)
            .map(|_| (noise_gain * rng.next_f32() as f64) as f32)
            .collect();
        if let Some(f) = hum_hz {
            for (i, s) in x.iter_mut().enumerate() {
                let t = i as f64 / sr;
                let hum = hum_level
                    * noise_rms
                    * ((2.0 * std::f64::consts::PI * f * t).sin()
                        + 0.6 * (2.0 * std::f64::consts::PI * 2.0 * f * t).sin()
                        + 0.4 * (2.0 * std::f64::consts::PI * 3.0 * f * t).sin());
                *s += hum as f32;
            }
        }
        x
    }

    #[test]
    fn silence_report_detects_strong_hum_near_50hz() {
        let sr = 48_000.0;
        let x = noise_and_hum((5.0 * sr) as usize, sr, 0.02, Some(50.0), 5.0, 1);
        let r = silence_report(&x, sr);
        let hum = r.hum.expect("strong 50 Hz hum must be detected");
        assert!((hum.freq_hz - 50.0).abs() <= 1.0, "freq {}", hum.freq_hz);
        assert!(hum.strength_db >= 12.0, "strength {}", hum.strength_db);
    }

    #[test]
    fn silence_report_reports_no_hum_when_hum_free() {
        let sr = 48_000.0;
        let x = noise_and_hum((5.0 * sr) as usize, sr, 0.02, None, 0.0, 1);
        let r = silence_report(&x, sr);
        assert!(
            r.hum.is_none(),
            "hum-free buffer must not report hum: {:?}",
            r.hum
        );
    }

    #[test]
    fn silence_report_noise_floor_matches_configured_rms() {
        let sr = 48_000.0;
        let noise_rms = 0.001; // −60 dBFS
        let x = noise_and_hum((5.0 * sr) as usize, sr, noise_rms, None, 0.0, 2);
        let r = silence_report(&x, sr);
        let expected = 20.0 * noise_rms.log10();
        assert!(
            (r.noise_floor_dbfs - expected).abs() < 1.0,
            "got {} want ~{}",
            r.noise_floor_dbfs,
            expected
        );
    }

    #[test]
    fn tick_report_on_tier3_synth_reports_t3_and_snr() {
        let cfg = SynthConfig {
            beat_error_ms: 0.8,
            amplitude_deg: 270.0,
            rate_s_per_day: 12.0,
            snr_db: 30.0,
            ..SynthConfig::default()
        };
        let x = synthesize(&cfg).expect("valid synth config");
        let r = tick_report(&x, cfg.sample_rate_hz);
        assert_eq!(r.tier, Some(Tier::T3), "report: {r:?}");
        assert!(r.band_snr_db.is_some(), "band_snr_db must be Some at T3");
        assert_eq!(r.clipped, 0);
    }

    #[test]
    fn agc_report_on_defaults_is_none() {
        let cfg = SynthConfig {
            duration_s: 15.0,
            ..SynthConfig::default()
        };
        let x = synthesize(&cfg).expect("valid synth config");
        let r = agc_report(&x, cfg.sample_rate_hz);
        assert!(r.agc_db.is_none(), "report: {r:?}");
        assert!(r.gate_db.is_none(), "report: {r:?}");
    }

    #[test]
    fn agc_report_detects_pumping_at_moderate_depth() {
        let cfg = SynthConfig {
            duration_s: 15.0,
            agc_depth: 0.4,
            agc_recovery_s: 0.05,
            ..SynthConfig::default()
        };
        let x = synthesize(&cfg).expect("valid synth config");
        let r = agc_report(&x, cfg.sample_rate_hz);
        let agc_db = r
            .agc_db
            .unwrap_or_else(|| panic!("agc_db must be Some: {r:?}"));
        assert!(
            (agc_db - (-4.4)).abs() <= 2.0,
            "agc_db {agc_db} want -4.4 ± 2"
        );
        assert!(r.gate_db.is_none(), "report: {r:?}");
    }

    #[test]
    fn agc_report_detects_gate_at_extreme_depth_and_slow_recovery() {
        let cfg = SynthConfig {
            duration_s: 15.0,
            agc_depth: 0.99,
            agc_recovery_s: 5.0,
            ..SynthConfig::default()
        };
        let x = synthesize(&cfg).expect("valid synth config");
        let r = agc_report(&x, cfg.sample_rate_hz);
        assert!(r.gate_db.is_some(), "report: {r:?}");
        assert!(r.agc_db.is_none(), "report: {r:?}");
    }

    #[test]
    fn doctor_score_all_good_scores_high_with_empty_advice() {
        let cfg = SynthConfig {
            beat_error_ms: 0.8,
            amplitude_deg: 270.0,
            rate_s_per_day: 12.0,
            snr_db: 30.0,
            duration_s: 15.0,
            ..SynthConfig::default()
        };
        let x = synthesize(&cfg).expect("valid synth config");
        let tick = tick_report(&x, cfg.sample_rate_hz);
        let agc = agc_report(&x, cfg.sample_rate_hz);
        let sr = cfg.sample_rate_hz;
        let quiet = noise_and_hum((5.0 * sr) as usize, sr, 0.001, None, 0.0, 3);
        let silence = silence_report(&quiet, sr);

        let ds = doctor_score(Some(&silence), Some(&tick), Some(&agc));
        assert!(ds.score >= 90, "score {} advice {:?}", ds.score, ds.advice);
        assert!(
            ds.advice.is_empty(),
            "expected empty-ish advice, got {:?}",
            ds.advice
        );
    }

    #[test]
    fn doctor_score_gated_signal_scores_low_with_gate_advice_first_and_earbuds_last() {
        // agc_depth 0.99 / recovery 5 s alone (at the default 30 dB SNR)
        // only tanks the AGC/gate signature — tier stays T3, since ducking
        // scales signal and noise together and largely preserves LOCAL
        // relative SNR (measured: score 75, gate-only). A realistically
        // weak base signal (10 dB SNR) combined with the same gating drops
        // tier to T1 too (measured via a probe sweep — see task-9-report.md),
        // giving the −30 (tier<T2) + −25 (gate) combination this test needs.
        let cfg = SynthConfig {
            duration_s: 15.0,
            snr_db: 10.0,
            agc_depth: 0.99,
            agc_recovery_s: 5.0,
            ..SynthConfig::default()
        };
        let x = synthesize(&cfg).expect("valid synth config");
        let tick = tick_report(&x, cfg.sample_rate_hz);
        let agc = agc_report(&x, cfg.sample_rate_hz);
        assert!(
            tick.tier.is_none_or(|t| t < Tier::T2),
            "fixture sanity: tier {:?}",
            tick.tier
        );
        assert!(agc.gate_db.is_some(), "fixture sanity: {agc:?}");

        let ds = doctor_score(None, Some(&tick), Some(&agc));
        assert!(
            ds.score <= 45,
            "score {} tier {:?} advice {:?}",
            ds.score,
            tick.tier,
            ds.advice
        );
        // Causes-rank-first, symptom-ranks-last: gate's fix leads even
        // though tier<T2's own deduction (−30) is bigger than gate's
        // (−25); the tier-driven "earbuds" advice is always appended last.
        assert_eq!(
            ds.advice.first(),
            Some(&"disable enhancements"),
            "advice {:?}",
            ds.advice
        );
        assert_eq!(
            ds.advice.last(),
            Some(&"try pressing wired earbuds against the crown"),
            "advice {:?}",
            ds.advice
        );
    }

    #[test]
    fn doctor_score_weak_tier_alone_scores_70_with_only_earbuds_advice() {
        // No clipping, no AGC/gate, no hum, no bad noise floor — just a
        // tier that never reaches T2 (silence/agc not run at all here, so
        // neither can contribute a deduction or advice).
        let cfg = SynthConfig {
            duration_s: 4.0,
            snr_db: 10.0,
            ..SynthConfig::default()
        };
        let x = synthesize(&cfg).expect("valid synth config");
        let tick = tick_report(&x, cfg.sample_rate_hz);
        assert!(
            tick.tier.is_none_or(|t| t < Tier::T2),
            "fixture sanity: tier {:?}",
            tick.tier
        );
        assert_eq!(tick.clipped, 0, "fixture sanity: {tick:?}");

        let ds = doctor_score(None, Some(&tick), None);
        assert_eq!(ds.score, 70, "tier {:?} advice {:?}", tick.tier, ds.advice);
        assert_eq!(
            ds.advice,
            vec!["try pressing wired earbuds against the crown"],
            "advice {:?}",
            ds.advice
        );
    }
}
