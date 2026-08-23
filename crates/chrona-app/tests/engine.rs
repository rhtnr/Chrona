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
        assert!(
            std::time::Instant::now() < deadline,
            "never reached T3 with tape > 100 (last tier {:?}, tape.len() {})",
            s.metrics.as_ref().map(|m| m.tier),
            s.tape.len()
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    let m = snap.metrics.unwrap();
    assert!((m.rate_s_per_day.unwrap() - 12.0).abs() < 0.5);
    assert!((m.beat_error_ms.unwrap() - 0.8).abs() <= 0.15);
    // tape.len() > 100 is already guaranteed by the loop's break condition.
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

#[test]
fn non_turbo_pacing_survives_a_control_message_burst() {
    // I2: the two tests above only ever exercise `start_with_turbo(..,
    // true)`, never the real (non-turbo) wall-clock-paced path this fix
    // touches directly. This doesn't measure the tick cadence precisely
    // (the public snapshot has no elapsed-ticks counter to assert
    // against), but does confirm the pacing rewrite doesn't hang or
    // deadlock under a burst of control messages — the exact scenario I2
    // guards against inflating the cadence for — and, via the invalid
    // message appended below, that the burst is actually drained and
    // published by the non-turbo path rather than merely not wedging it.
    let mut eng = Engine::start(
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
    );
    for _ in 0..20 {
        eng.send(ControlMsg::SetLift(52.0));
    }
    // One invalid message closes out the burst: `averaging_s` must be in
    // [2, 60] (`AnalyzerConfig`'s own validation), so `Analyzer::new`
    // rejects 999 and `apply_config` sets `error_banner`. The default
    // snapshot's banner is always `None`, so seeing it `Some` after the
    // wait below is proof this specific message was actually drained and
    // published by the non-turbo path — not just evidence the thread
    // didn't hang.
    eng.send(ControlMsg::SetAveraging(999.0));
    // ~10 Hz publish cadence: 1 s is generous headroom for at least one
    // real (non-default) snapshot to land even on a loaded machine.
    std::thread::sleep(std::time::Duration::from_secs(1));
    let snap = eng.snapshot();
    assert!(matches!(snap.source_kind, SourceKind::Simulate));
    assert!(
        snap.error_banner.is_some(),
        "invalid SetAveraging never surfaced — burst wasn't drained/published"
    );
    drop(eng); // Shutdown + join must complete (test would hang otherwise)
}

#[test]
fn start_recording_failure_clears_stale_recording_path() {
    // T7 re-review incidental, ported into T10 as an authorized engine.rs
    // edit: `StartRecording`'s Err arm must clear `recording_path`, not
    // leave the previous (now-finalized) recording's path stale beside a
    // `None` writer. Forces the Err arm by starting a *second* recording
    // (while the first is still active) into a directory that's actually a
    // regular file — `SessionWriter::create`'s `dir.join(..)` then can't be
    // created (not a directory).
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

    let dir = tempfile::tempdir().unwrap();
    eng.send(ControlMsg::StartRecording {
        dir: dir.path().to_path_buf(),
        meta_position: None,
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        if eng.snapshot().recording.is_some() {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "first StartRecording never took"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }

    let not_a_dir = dir.path().join("not_a_dir");
    std::fs::write(&not_a_dir, b"x").unwrap();
    eng.send(ControlMsg::StartRecording {
        dir: not_a_dir,
        meta_position: None,
    });

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let snap = loop {
        let s = eng.snapshot();
        if s.error_banner
            .as_deref()
            .is_some_and(|m| m.contains("failed to start recording"))
        {
            break s;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "second (failing) StartRecording never surfaced its error banner"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    assert!(
        snap.recording.is_none(),
        "a failed StartRecording while already recording must clear recording_path, \
         not leave the prior writer's path stale (got {:?})",
        snap.recording
    );
    drop(eng); // Shutdown + join must complete (test would hang otherwise)
}

#[test]
fn switch_to_replay_with_sidecar_surfaces_settings_banner() {
    // T10 edit A (T7-I3), end to end through the public Engine: switching
    // into a recorded session with a sidecar restores its lift/ppm into the
    // live config and surfaces the "using recorded settings" info banner.
    // `SourceRuntime::build`'s own unit tests (in engine.rs, same-crate —
    // it's private) cover the exact mutation in isolation; this confirms
    // the `SwitchSource` control-message call site actually wires it
    // through to a real published snapshot.
    let dir = tempfile::tempdir().unwrap();
    let meta = chrona_session::SessionMeta {
        schema_version: 1,
        device_name: "TestMic".to_string(),
        sample_rate_hz: 48_000.0,
        ppm_correction: 3.0,
        lift_angle_deg: 40.0,
        bph_mode: "auto".to_string(),
        position: None,
        started_unix_s: 1_700_000_000,
        app_version: "test".to_string(),
    };
    let mut w = chrona_session::SessionWriter::create(dir.path(), meta).unwrap();
    w.push(&vec![0.0f32; 4_800]).unwrap();
    let wav_path = w.finalize().unwrap();

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
    eng.send(ControlMsg::SwitchSource(SourceSpec::ReplayFile {
        path: wav_path,
    }));

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let snap = loop {
        let s = eng.snapshot();
        if matches!(s.source_kind, SourceKind::Replay) {
            break s;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "never switched to Replay"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    assert_eq!(
        snap.error_banner.as_deref(),
        Some("replay: using recorded settings (lift 40.0°, ppm 3.0)")
    );
    drop(eng); // Shutdown + join must complete (test would hang otherwise)
}
