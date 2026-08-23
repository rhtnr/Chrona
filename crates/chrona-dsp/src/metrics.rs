//! Stage 6 (spec §5.6, §2.3): unlocking-referenced rate + beat error via
//! least squares (Watch-O-Scope's regression, not naive averaging), and
//! amplitude with validity gates.

use crate::events::{BeatEvent, Parity};

#[derive(Debug, Clone, Copy)]
pub struct RegressionResult {
    /// Regressed beat period, nominal-clock seconds.
    pub t_beat_s: f64,
    /// BE = 2·|c| (alternate unlocking intervals are T ∓ 2c), milliseconds.
    pub beat_error_ms: f64,
    /// Residual σ of the fit, milliseconds.
    pub jitter_ms: f64,
    pub events_used: usize,
}

/// Three-parameter LS over unlocking timestamps: t = a + T·k + c·s (spec §5.6).
pub fn regress_unlocking(events: &[BeatEvent]) -> Option<RegressionResult> {
    let usable: Vec<(f64, f64, f64)> = events
        .iter()
        .filter_map(|e| {
            let t = e.t_unlock_s?;
            let s = match e.parity {
                Parity::Tic => 1.0,
                Parity::Toc => -1.0,
            };
            Some((e.beat_index as f64, s, t))
        })
        .collect();
    let n = usable.len();
    let tics = usable.iter().filter(|(_, s, _)| *s > 0.0).count();
    if n < 12 || tics < 4 || n - tics < 4 {
        return None;
    }

    // Normal equations for [a, T, c]:
    // [ n   Σk   Σs  ] [a]   [ Σt  ]
    // [ Σk  Σk²  Σks ] [T] = [ Σkt ]
    // [ Σs  Σks  Σs² ] [c]   [ Σst ]
    let (mut sk, mut ss, mut st, mut sk2, mut sks, mut ss2, mut skt, mut sst) =
        (0.0f64, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
    for &(k, s, t) in &usable {
        sk += k;
        ss += s;
        st += t;
        sk2 += k * k;
        sks += k * s;
        ss2 += s * s;
        skt += k * t;
        sst += s * t;
    }
    let nf = n as f64;
    let det3 = |m: [[f64; 3]; 3]| {
        m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
            - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
            + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
    };
    let m = [[nf, sk, ss], [sk, sk2, sks], [ss, sks, ss2]];
    let d = det3(m);
    if d.abs() < 1e-9 {
        return None;
    }
    let rhs = [st, skt, sst];
    let col = |j: usize| {
        let mut mm = m;
        for i in 0..3 {
            mm[i][j] = rhs[i];
        }
        det3(mm) / d
    };
    let (a, t_beat, c) = (col(0), col(1), col(2));

    let ssr: f64 = usable
        .iter()
        .map(|&(k, s, t)| (t - (a + t_beat * k + c * s)).powi(2))
        .sum();
    let jitter_s = (ssr / (nf - 3.0)).sqrt();

    Some(RegressionResult {
        t_beat_s: t_beat,
        beat_error_ms: 2.0 * c.abs() * 1_000.0,
        jitter_ms: jitter_s * 1_000.0,
        events_used: n,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AmplitudeGateFail {
    /// Fewer than 8 unlock→drop intervals on either parity.
    InsufficientEvents,
    /// A_tic or A_toc outside the 135–360° validity band (spec §2.3).
    OutOfRange,
    /// |A_tic − A_toc| ≥ 60° (spec §2.3 agreement gate).
    TicTocDisagree,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AmplitudeResult {
    pub degrees: f64,
    pub tic_degrees: f64,
    pub toc_degrees: f64,
}

fn median(v: &mut [f64]) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(f64::total_cmp);
    Some(v[v.len() / 2])
}

/// Spec §2.3: A = L / (2·sin(π·Δt/T_osc)), gated 135–360° with 60° tic/toc agreement.
pub fn amplitude_from_events(
    events: &[BeatEvent],
    t_osc_s: f64,
    lift_angle_deg: f64,
) -> Result<AmplitudeResult, AmplitudeGateFail> {
    let mut dt_tic = Vec::new();
    let mut dt_toc = Vec::new();
    for e in events {
        if let (Some(u), Some(d)) = (e.t_unlock_s, e.t_drop_edge_s) {
            let dt = d - u;
            if dt > 0.0 {
                match e.parity {
                    Parity::Tic => dt_tic.push(dt),
                    Parity::Toc => dt_toc.push(dt),
                }
            }
        }
    }
    if dt_tic.len() < 8 || dt_toc.len() < 8 {
        return Err(AmplitudeGateFail::InsufficientEvents);
    }
    let amp_of = |dt: f64| {
        let s = (std::f64::consts::PI * dt / t_osc_s).sin();
        if s <= 0.0 {
            f64::INFINITY
        } else {
            lift_angle_deg / (2.0 * s)
        }
    };
    // Invariant: the vectors were just checked non-empty (len ≥ 8).
    let tic = amp_of(median(&mut dt_tic).expect("non-empty by guard"));
    let toc = amp_of(median(&mut dt_toc).expect("non-empty by guard"));
    let in_range = |a: f64| (135.0..=360.0).contains(&a);
    if !in_range(tic) || !in_range(toc) {
        return Err(AmplitudeGateFail::OutOfRange);
    }
    if (tic - toc).abs() >= 60.0 {
        return Err(AmplitudeGateFail::TicTocDisagree);
    }
    Ok(AmplitudeResult {
        degrees: (tic + toc) / 2.0,
        tic_degrees: tic,
        toc_degrees: toc,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::EnvelopeExtractor;
    use crate::events::{BeatEvent, Parity, extract_events};
    use crate::filter::{Butterworth, DcBlocker};
    use crate::fold::fold_envelope;
    use crate::synth::{SynthConfig, synthesize};

    /// synth → precondition → envelope → fold → events (duplicated from
    /// events.rs tests by design — each module's tests stand alone).
    fn pipeline_to_events(cfg: &SynthConfig) -> Vec<BeatEvent> {
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
        extract_events(&raw, 0, &env, sr, &profile)
    }

    #[test]
    fn beat_error_sweep_meets_the_bar() {
        // M2 bar (Global Constraints): BE within ±0.1 ms at 30 dB.
        for be_true in [0.0f64, 0.3, 0.8, 2.0] {
            let cfg = SynthConfig {
                beat_error_ms: be_true,
                rate_s_per_day: 12.0,
                snr_db: 30.0,
                ..SynthConfig::default()
            };
            let events = pipeline_to_events(&cfg);
            let r = regress_unlocking(&events).expect("regression");
            assert!(
                (r.beat_error_ms - be_true).abs() <= 0.1,
                "be {be_true}: got {}",
                r.beat_error_ms
            );
            // Rate from the regressed beat period: ±0.3 s/d bar.
            let rate = 86_400.0 * (crate::bph::t_beat_s(cfg.bph) - r.t_beat_s)
                / crate::bph::t_beat_s(cfg.bph);
            assert!((rate - 12.0).abs() <= 0.3, "be {be_true}: rate {rate}");
            assert!(r.jitter_ms < 0.5, "jitter {}", r.jitter_ms);
            assert!(r.events_used >= 12);
        }
    }

    #[test]
    fn regression_refuses_thin_or_one_sided_data() {
        let mk = |i: i64, p: Parity, t: f64| BeatEvent {
            beat_index: i,
            parity: p,
            t_drop_corr_s: t,
            snr_db: 20.0,
            t_drop_edge_s: Some(t),
            t_unlock_s: Some(t - 0.007),
        };
        // 11 events → too few.
        let few: Vec<BeatEvent> = (0..11)
            .map(|i| {
                mk(
                    i,
                    if i % 2 == 0 { Parity::Tic } else { Parity::Toc },
                    0.125 * i as f64,
                )
            })
            .collect();
        assert!(regress_unlocking(&few).is_none());
        // 20 events but only 2 tocs → one-sided.
        let sided: Vec<BeatEvent> = (0..20)
            .map(|i| {
                mk(
                    i,
                    if i < 18 { Parity::Tic } else { Parity::Toc },
                    0.125 * i as f64,
                )
            })
            .collect();
        assert!(regress_unlocking(&sided).is_none());
        // Events without unlocking timestamps don't count.
        let no_unlock: Vec<BeatEvent> = (0..30)
            .map(|i| BeatEvent {
                t_unlock_s: None,
                ..mk(
                    i,
                    if i % 2 == 0 { Parity::Tic } else { Parity::Toc },
                    0.125 * i as f64,
                )
            })
            .collect();
        assert!(regress_unlocking(&no_unlock).is_none());
    }

    #[test]
    fn synthetic_exact_model_recovers_parameters() {
        // Noise-free fabricated grid: t = 5.0 + 0.125·k + 0.0004·s_k
        // (BE = 2·0.0004 s = 0.8 ms). Regression must recover it near-exactly.
        let events: Vec<BeatEvent> = (0..100)
            .map(|k| {
                let parity = if k % 2 == 0 { Parity::Tic } else { Parity::Toc };
                let s = if k % 2 == 0 { 1.0 } else { -1.0 };
                let t = 5.0 + 0.125 * k as f64 + 0.0004 * s;
                BeatEvent {
                    beat_index: k,
                    parity,
                    t_drop_corr_s: t + 0.007,
                    snr_db: 20.0,
                    t_drop_edge_s: Some(t + 0.007),
                    t_unlock_s: Some(t),
                }
            })
            .collect();
        let r = regress_unlocking(&events).expect("regression");
        assert!((r.t_beat_s - 0.125).abs() < 1e-12, "T {}", r.t_beat_s);
        assert!(
            (r.beat_error_ms - 0.8).abs() < 1e-9,
            "BE {}",
            r.beat_error_ms
        );
        assert!(r.jitter_ms < 1e-9);
    }

    #[test]
    fn amplitude_sweep_meets_the_bar() {
        // M2 bar: ±5° at 30 dB, averaged output.
        for amp_true in [200.0f64, 240.0, 270.0, 300.0, 330.0] {
            let cfg = SynthConfig {
                amplitude_deg: amp_true,
                snr_db: 30.0,
                ..SynthConfig::default()
            };
            let events = pipeline_to_events(&cfg);
            let t_osc = crate::bph::t_osc_s(cfg.bph);
            let a = amplitude_from_events(&events, t_osc, cfg.lift_angle_deg)
                .unwrap_or_else(|g| panic!("amp {amp_true}: gate {g:?}"));
            assert!(
                (a.degrees - amp_true).abs() <= 5.0,
                "amp {amp_true}: got {}",
                a.degrees
            );
            assert!((a.tic_degrees - a.toc_degrees).abs() < 60.0);
        }
    }

    fn fabricated(dt_tic_s: f64, dt_toc_s: f64, n_per_parity: usize) -> Vec<BeatEvent> {
        (0..2 * n_per_parity)
            .map(|k| {
                let (parity, dt) = if k % 2 == 0 {
                    (Parity::Tic, dt_tic_s)
                } else {
                    (Parity::Toc, dt_toc_s)
                };
                let t = 0.125 * k as f64;
                BeatEvent {
                    beat_index: k as i64,
                    parity,
                    t_drop_corr_s: t,
                    snr_db: 20.0,
                    t_drop_edge_s: Some(t),
                    t_unlock_s: Some(t - dt),
                }
            })
            .collect()
    }

    #[test]
    fn amplitude_gates_fire_correctly() {
        let t_osc = 0.25;
        let lift = 52.0;
        let dt_for = |a: f64| crate::synth::unlock_to_drop_dt_s(a, lift, t_osc);
        // Too few events per parity.
        assert_eq!(
            amplitude_from_events(&fabricated(dt_for(270.0), dt_for(270.0), 5), t_osc, lift),
            Err(AmplitudeGateFail::InsufficientEvents)
        );
        // A = 100° is physically valid input but below the 135° display gate.
        assert_eq!(
            amplitude_from_events(&fabricated(dt_for(100.0), dt_for(100.0), 20), t_osc, lift),
            Err(AmplitudeGateFail::OutOfRange)
        );
        // Tic 270° vs toc 200° → 70° disagreement > 60° gate.
        assert_eq!(
            amplitude_from_events(&fabricated(dt_for(270.0), dt_for(200.0), 20), t_osc, lift),
            Err(AmplitudeGateFail::TicTocDisagree)
        );
        // Healthy case passes.
        let ok = amplitude_from_events(&fabricated(dt_for(280.0), dt_for(275.0), 20), t_osc, lift)
            .expect("healthy");
        assert!((ok.degrees - 277.5).abs() < 1.0, "got {}", ok.degrees);
    }
}
