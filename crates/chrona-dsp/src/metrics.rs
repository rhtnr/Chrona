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
        extract_events(&raw, 0, &env, 0, sr, &profile)
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
}
