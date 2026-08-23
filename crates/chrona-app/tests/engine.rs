//! Engine integration tests (T7): headless, no audio hardware, no window.
//! Exercises the real spawned DSP thread end to end through the public
//! `Engine` API — control channel, triple_buffer snapshot publishing, and
//! clean shutdown.

use chrona_app::engine::*;

#[test]
fn simulate_source_reaches_tier3_and_publishes_tape() {
    // The engine paces itself by wall clock (~50 ms/tick) so a real-time run
    // of this test would need ~40 s of simulated audio to reach steady
    // state. `start_with_turbo(.., true)` is the test-only knob (R2/T7
    // controller ruling) that skips that pacing — the DSP thread still
    // generates the same 50 ms-equivalent chunks per tick, it just doesn't
    // sleep between them, so the same steady state arrives in well under a
    // second of wall time instead.
    let mut eng = Engine::start_with_turbo(
        SourceSpec::Simulate {
            rate: 12.0,
            beat_error: 0.8,
            amplitude: 270.0,
            snr: 30.0,
        },
        EngineConfig {
            lift_angle_deg: 52.0,
            averaging_s: 30.0,
            bph_mode: chrona_dsp::BphMode::Auto,
            ppm_correction: 0.0,
        },
        None,
        true,
    );
    // Break on T3 *with* a tape past the 100-event bar the assertions below
    // check, not on the first T3 sighting: at 30 dB SNR the tier-3 quality
    // gates (detection ratio, jitter, unlocking ratio, amplitude) can be
    // satisfied from just a handful of simulated seconds — that's the
    // analyzer correctly reporting "this window is good enough", not a
    // promise that much history has accumulated yet. `tape_events()` is
    // bounded by how much audio has actually been pushed, so it can still
    // be thin at that exact instant; waiting for both is what this test
    // actually wants to verify (a T3 read backed by a healthy tape), not a
    // race between the two.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    let snap = loop {
        let s = eng.snapshot();
        if s.metrics
            .as_ref()
            .is_some_and(|m| m.tier == chrona_dsp::Tier::T3)
            && s.tape.len() > 100
        {
            break s;
        }
        assert!(std::time::Instant::now() < deadline, "never reached T3");
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    let m = snap.metrics.unwrap();
    assert!((m.rate_s_per_day.unwrap() - 12.0).abs() < 0.5);
    assert!((m.beat_error_ms.unwrap() - 0.8).abs() <= 0.15);
    assert!(snap.tape.len() > 100);
    assert!(matches!(snap.source_kind, SourceKind::Simulate));
}

#[test]
fn control_messages_rebuild_without_deadlock() {
    let mut eng = Engine::start_with_turbo(
        SourceSpec::Simulate {
            rate: 0.0,
            beat_error: 0.0,
            amplitude: 270.0,
            snr: 30.0,
        },
        EngineConfig {
            lift_angle_deg: 52.0,
            averaging_s: 30.0,
            bph_mode: chrona_dsp::BphMode::Auto,
            ppm_correction: 0.0,
        },
        None,
        true,
    );
    eng.send(ControlMsg::SetLift(38.0));
    eng.send(ControlMsg::SetAveraging(10.0));
    eng.send(ControlMsg::SetPpm(25.0));
    std::thread::sleep(std::time::Duration::from_millis(500));
    let _ = eng.snapshot(); // must not hang
    drop(eng); // Shutdown + join must complete (test would hang otherwise)
}
