//! Engine integration tests (T7): headless, no audio hardware, no window.
//! Exercises the real spawned DSP thread end to end through the public
//! `Engine` API — control channel, triple_buffer snapshot publishing, and
//! clean shutdown.

use chrona_app::engine::*;

/// M4 Task 4: guards every test in this file that (directly, or via
/// `ControlMsg::StartRecording`) calls `chrona_session::SessionWriter::
/// create` against `recording_write_failure_surfaces_warn_banner_and_stops_
/// recording` below, which arms chrona-session's `test-fault-injection`
/// fault seam via the process-global `CHRONA_TEST_FAIL_PUSH_AFTER` env var
/// — `create` (chrona-session, this crate's own dependency) reads it
/// whenever the feature is on, which it unconditionally is for this crate's
/// test builds (see task-4-report.md). `cargo test` runs the test functions
/// in this binary as parallel threads of the *same process* by default, so
/// without this lock, another thread's `create` call could transiently
/// observe the env var set and get spuriously fault-injected — empirically
/// confirmed during development (two unrelated tests failed with "…never
/// took" once in ~5 unlocked runs; see task-4-report.md's RED/race
/// evidence). `Mutex<()>` guards nothing but mutual exclusion itself, so a
/// poisoned lock (an earlier holder panicking) is recovered from rather
/// than cascaded into every later test.
static SESSION_WRITER_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn lock_session_writer_tests() -> std::sync::MutexGuard<'static, ()> {
    SESSION_WRITER_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// RAII cleanup for `CHRONA_TEST_FAIL_PUSH_AFTER`: clears it on drop,
/// including on an early return via a panicking `assert!` unwinding through
/// it (libtest catches the unwind per-test, so this scope's locals — this
/// guard included — still run their `Drop` before the *next* test gets a
/// turn). Without this, a failure in the fault-injection test itself (e.g.
/// its deadline actually elapsing) would leave the var stuck "on" for
/// whichever `SESSION_WRITER_TEST_LOCK`-holding test runs next in this same
/// process, turning one genuine failure into several confusing ones.
struct ClearFailPushAfterOnDrop;

impl Drop for ClearFailPushAfterOnDrop {
    fn drop(&mut self) {
        // SAFETY: only ever constructed immediately after `set_var`ing the
        // same key, while `SESSION_WRITER_TEST_LOCK` is held — see that
        // static's doc comment for why that makes this safe.
        unsafe {
            std::env::remove_var("CHRONA_TEST_FAIL_PUSH_AFTER");
        }
    }
}

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
    // rejects 999 and `apply_config` sets `banner`. The default
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
        snap.banner.is_some(),
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
    //
    // Holds `SESSION_WRITER_TEST_LOCK` (see its doc comment): this test
    // calls `SessionWriter::create` (via `StartRecording`) and must not run
    // concurrently with the fault-injection test below.
    let _guard = lock_session_writer_tests();
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
        watch: None,
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
        watch: None,
    });

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let snap = loop {
        let s = eng.snapshot();
        if s.banner
            .as_ref()
            .is_some_and(|b| b.text.contains("failed to start recording"))
        {
            break s;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "second (failing) StartRecording never surfaced its error banner"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    let b = snap.banner.as_ref().expect("banner present");
    assert_eq!(b.severity, BannerSeverity::Error);
    assert!(b.text.contains("failed to start recording"));
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
    //
    // Holds `SESSION_WRITER_TEST_LOCK` (see its doc comment): this test
    // calls `SessionWriter::create` directly (setup, below) and must not
    // run concurrently with the fault-injection test below.
    let _guard = lock_session_writer_tests();
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
        watch: None,
        summary: None,
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
    let b = snap.banner.as_ref().expect("banner present");
    assert_eq!(b.severity, BannerSeverity::Info);
    assert_eq!(
        b.text,
        "replay: using recorded settings (lift 40.0°, ppm 3.0)"
    );
    drop(eng); // Shutdown + join must complete (test would hang otherwise)
}

#[test]
fn stop_recording_writes_summary_sidecar() {
    // T3: `StartRecording` carries `watch` through to the sidecar, and
    // `StopRecording` writes a stop-time `SessionSummary` (tier + headline
    // metrics, from the engine's last-published `MetricsSnapshot`) into it.
    //
    // Holds `SESSION_WRITER_TEST_LOCK` (see its doc comment): this test
    // calls `SessionWriter::create` (via `StartRecording`) and must not run
    // concurrently with the fault-injection test below.
    let _guard = lock_session_writer_tests();
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

    // Wait for Tier 3 so `last_metrics` has something real to summarize.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    loop {
        if eng
            .snapshot()
            .metrics
            .as_ref()
            .is_some_and(|m| m.tier == chrona_dsp::Tier::T3)
        {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "never reached Tier 3");
        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    let dir = tempfile::tempdir().unwrap();
    eng.send(ControlMsg::StartRecording {
        dir: dir.path().to_path_buf(),
        meta_position: None,
        watch: Some("Test Watch".to_string()),
    });

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    let wav_path = loop {
        if let Some(path) = eng.snapshot().recording {
            break path;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "StartRecording never took"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    };

    // >= 1 s of (turbo-paced, so far more than 1 simulated second's worth
    // of) audio actually recorded before stopping.
    std::thread::sleep(std::time::Duration::from_secs(1));

    eng.send(ControlMsg::StopRecording);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    loop {
        if eng.snapshot().recording.is_none() {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "StopRecording never took"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    let meta = chrona_session::sidecar::read_meta(&wav_path).expect("sidecar present");
    assert_eq!(meta.watch.as_deref(), Some("Test Watch"));
    let summary = meta.summary.expect("summary present");
    assert_eq!(summary.tier, "T3");
    let rate = summary
        .rate_s_per_day
        .expect("rate_s_per_day present at T3");
    assert!(
        (rate - 12.0).abs() < 1.0,
        "rate_s_per_day should be within 1.0 of 12.0, got {rate}"
    );
    assert!(summary.duration_s > 0.0);
    drop(eng); // Shutdown + join must complete (test would hang otherwise)
}

#[test]
fn engine_survives_total_startup_failure_and_recovers() {
    // T3 (M4): today, when EVERY way of starting a source fails, engine_loop
    // publishes an Error banner and `return`s — the DSP thread exits, and
    // the control channel's receiver half is dropped with it, so every
    // later `eng.send(..)` (including a would-be-recovering SwitchSource) is
    // silently swallowed by `Engine::send`'s best-effort `let _ =
    // self.tx.send(msg)`, and `eng.snapshot()` just keeps replaying the same
    // stale Error snapshot forever. This confirms the loop instead survives
    // that failure (an idle placeholder source in place of a dead thread)
    // and genuinely recovers once a working source arrives.
    //
    // The design brief's suggested trigger for "every way of starting a
    // source fails" was a bogus mic device_id
    // (`Mic { device_id: Some("chrona-bogus-device") }`), which relies on
    // the mic-fallback retry (against the default input device — see
    // `engine_loop`'s initial-build region) ALSO failing, i.e. no audio
    // input hardware at all. Empirically checked in this dev sandbox (a
    // throwaway probe calling `chrona_audio::CaptureStream::start(None,
    // ..)` directly): a real default input device ("MacBook Pro
    // Microphone") is present and negotiates successfully, so that specific
    // trigger does NOT reach total failure here — the retry recovers and
    // the engine starts normally. Using it would make this test pass today
    // for the wrong reason (never RED on this machine) and be flaky
    // depending on whatever hardware happens to be attached to whatever
    // machine runs it. `ReplayFile` with a nonexistent path instead hits
    // `engine_loop`'s *other* source-build fatal arm (the non-mic-or-
    // explicit-default-device Err arm — no retry attempted at all, straight
    // to fatal) — same class of failure (every way of starting a source is
    // exhausted), zero hardware dependency, deterministic on every machine.
    // The mic-double-failure arm shares the identical post-match
    // construction (set the Error banner, yield `SourceRuntime::Idle`) —
    // see task-3-report.md for why that arm is judged low-risk without its
    // own dedicated integration test.
    let mut eng = Engine::start_with_turbo(
        SourceSpec::ReplayFile {
            path: std::path::PathBuf::from("/nonexistent/chrona-total-failure-probe.wav"),
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

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let snap = loop {
        let s = eng.snapshot();
        if s.banner.is_some() {
            break s;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "no banner published within 10s of a total startup failure"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    let b = snap.banner.as_ref().expect("banner present");
    assert_eq!(b.severity, BannerSeverity::Error);
    assert!(
        b.text.contains("failed to start"),
        "unexpected banner text: {}",
        b.text
    );
    assert_eq!(
        snap.source_kind,
        SourceKind::Replay,
        "Idle must report the last-attempted source kind, not SourceKind::default()"
    );
    assert_eq!(
        snap.health.silent_for_s, 0.0,
        "an idle source pulls no samples — it must not feed the silence tracker"
    );

    // Idle a while longer (well past a single publish cycle) and confirm
    // the SAME truthful Error banner is still the only signal — it must
    // not decay into (or get joined by) a silence banner, and nothing
    // clears it on its own.
    std::thread::sleep(std::time::Duration::from_millis(300));
    let still_idle = eng.snapshot();
    assert_eq!(still_idle.banner.as_ref(), Some(b));
    assert_eq!(still_idle.health.silent_for_s, 0.0);
    assert!(still_idle.recording.is_none());

    // Today (pre-fix) this SwitchSource is dropped on the floor (dead
    // channel) and the loop below times out: RED.
    eng.send(ControlMsg::SwitchSource(SourceSpec::Simulate {
        rate: 12.0,
        beat_error: 0.8,
        amplitude: 270.0,
        snr: 30.0,
    }));

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    loop {
        let s = eng.snapshot();
        if s.metrics
            .as_ref()
            .is_some_and(|m| m.tier == chrona_dsp::Tier::T3)
        {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "never recovered to Tier 3 after SwitchSource following a total startup \
             failure (engine loop likely dead) — last banner {:?}, source_kind {:?}",
            s.banner,
            s.source_kind,
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    drop(eng); // Shutdown + join must complete (test would hang otherwise)
}

#[test]
fn engine_survives_initial_analyzer_failure_and_recovers() {
    // Distinct fatal exit from the test above: here the *source* builds
    // fine (Simulate never touches hardware) but the initial `EngineConfig`
    // is analyzer-invalid (`averaging_s` must be in [2, 60] —
    // `AnalyzerConfig`'s own validation, chrona-dsp untouched by this task).
    // Exercises `engine_loop`'s separate initial-analyzer-build fatal arm,
    // including the "heal a poisoned config back to safe defaults" fallback
    // (T3-design: rebuilding the idle analyzer against the SAME invalid
    // config at 48 kHz would fail identically, and every later
    // apply_config/SwitchSource starts from that same `config`).
    let mut eng = Engine::start_with_turbo(
        SourceSpec::Simulate {
            rate: 0.0,
            beat_error: 0.0,
            amplitude: 270.0,
            snr: 30.0,
        },
        EngineConfig {
            lift_angle_deg: 52.0,
            averaging_s: 999.0, // outside AnalyzerConfig's [2, 60]
            bph_mode: chrona_dsp::BphMode::Auto,
            ppm_correction: 0.0,
        },
        None,
        true,
    );

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let snap = loop {
        let s = eng.snapshot();
        if s.banner.is_some() {
            break s;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "no banner published within 10s of a total startup failure"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    let b = snap.banner.as_ref().expect("banner present");
    assert_eq!(b.severity, BannerSeverity::Error);
    assert!(
        b.text.contains("invalid initial config"),
        "unexpected banner text: {}",
        b.text
    );
    assert_eq!(snap.source_kind, SourceKind::Simulate);

    // Recovery: a fresh Simulate with a *valid* config must succeed even
    // though the poisoned `averaging_s: 999.0` was never explicitly reset
    // by this message — proof `config` was healed when the engine fell
    // through to idle, not left poisoned for every later config change to
    // keep tripping over.
    eng.send(ControlMsg::SwitchSource(SourceSpec::Simulate {
        rate: 12.0,
        beat_error: 0.8,
        amplitude: 270.0,
        snr: 30.0,
    }));

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    loop {
        let s = eng.snapshot();
        if s.metrics
            .as_ref()
            .is_some_and(|m| m.tier == chrona_dsp::Tier::T3)
        {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "never recovered to Tier 3 after SwitchSource following an initial analyzer \
             failure — last banner {:?}",
            s.banner,
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    drop(eng);
}

#[test]
fn start_recording_while_idle_fails_without_creating_a_writer() {
    // T3 ruling (see task-3-report.md): a writer created while idle would
    // record silence forever under a name implying real capture — refused
    // instead, via the same "failed to start recording" Error-banner shape
    // `SessionWriter::create` failures already use.
    let mut eng = Engine::start_with_turbo(
        SourceSpec::ReplayFile {
            path: std::path::PathBuf::from("/nonexistent/chrona-total-failure-probe-2.wav"),
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

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        if eng.snapshot().banner.is_some() {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "no banner published within 10s of a total startup failure"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }

    let dir = tempfile::tempdir().unwrap();
    eng.send(ControlMsg::StartRecording {
        dir: dir.path().to_path_buf(),
        meta_position: None,
        watch: None,
    });

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let snap = loop {
        let s = eng.snapshot();
        if s.banner
            .as_ref()
            .is_some_and(|b| b.text.contains("no active source"))
        {
            break s;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "StartRecording-while-idle never surfaced the 'no active source' banner \
             (last banner {:?}) — engine loop likely dead",
            eng.snapshot().banner,
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    let b = snap.banner.as_ref().expect("banner present");
    assert_eq!(b.severity, BannerSeverity::Error);
    assert!(b.text.contains("failed to start recording"));
    assert!(
        snap.recording.is_none(),
        "no writer may exist while idle, got {:?}",
        snap.recording
    );
    drop(eng);
}

#[test]
fn recording_write_failure_surfaces_warn_banner_and_stops_recording() {
    // M4 Task 4: `SessionWriter::push` failing mid-recording (e.g. a full
    // disk) must not be silently swallowed — the engine should drop the
    // writer, clear `recording`, and surface a Warn banner (spec §4: a
    // write failure is a recoverable notice, not a fault — Warn, not
    // Error). Armed via chrona-session's `test-fault-injection` feature
    // (unconditionally enabled for this crate's own test builds by this
    // crate's own [dev-dependencies] entry for chrona-session — see
    // task-4-report.md's feature-unification verification), which makes
    // `SessionWriter::create` read `CHRONA_TEST_FAIL_PUSH_AFTER` (samples)
    // and fail every `push` once that many samples have already been
    // written.
    //
    // `CHRONA_TEST_FAIL_PUSH_AFTER` is a process-global env var read by
    // *any* `SessionWriter::create` call in this process — holding
    // `SESSION_WRITER_TEST_LOCK` for this test's whole body (see its doc
    // comment) is what makes setting it here safe: no other test in this
    // binary can be inside its own `create` call while this one has it
    // set. (An earlier, unlocked version of this test empirically produced
    // exactly the corruption this guards against — two unrelated tests'
    // own recordings silently failed once in ~5 runs; see
    // task-4-report.md's RED/race evidence.)
    let _guard = lock_session_writer_tests();
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

    let dir = tempfile::tempdir().unwrap();
    // SAFETY: `SESSION_WRITER_TEST_LOCK` (held for this whole test, until
    // `_guard` drops at the end) ensures no other thread's
    // `SessionWriter::create` call can read this env var while it's set —
    // the only other possible reader in this process. Deliberately *not*
    // cleared as soon as `SessionWriter::create` has merely run (i.e. once
    // `recording` is next observed `Some`): at a 4800-sample threshold and
    // turbo pacing, the fault can trip — dropping the writer and clearing
    // `recording` straight back to `None` — before this test's own next
    // poll ever observes that transient `Some`, same as it did to two
    // *other* tests during development before this lock existed (see
    // task-4-report.md's RED/race evidence). So this stays set until
    // `clear_env` drops (test end, including on a panicking `assert!`
    // below) — safe precisely because the lock, not the env var's
    // lifetime, is what protects every other test.
    unsafe {
        std::env::set_var("CHRONA_TEST_FAIL_PUSH_AFTER", "4800"); // 0.1s @ 48kHz
    }
    let clear_env = ClearFailPushAfterOnDrop;
    eng.send(ControlMsg::StartRecording {
        dir: dir.path().to_path_buf(),
        meta_position: None,
        watch: None,
    });

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let snap = loop {
        let s = eng.snapshot();
        if s.banner
            .as_ref()
            .is_some_and(|b| b.text.contains("recording write failed"))
        {
            break s;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "no 'recording write failed' banner within 5s of the fault tripping \
             (last banner {:?}, recording {:?})",
            s.banner,
            s.recording,
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    // The fault has now demonstrably already fired — safe to clear early
    // rather than waiting for `clear_env` to drop at the end of the
    // function (no other test can observe the difference either way, since
    // `_guard` is held until then regardless).
    drop(clear_env);
    let b = snap.banner.as_ref().expect("banner present");
    assert_eq!(
        b.severity,
        BannerSeverity::Warn,
        "a write failure is a recoverable notice (spec §4), not a fault — and this \
         must not be the panic-containment 'analysis thread fault' Error banner \
         either (text: {})",
        b.text
    );
    assert!(
        snap.recording.is_none(),
        "recording must clear once the writer is dropped on a write failure"
    );

    // RIDER (Task 4 review): the engine's post-failure state must actually
    // accept new recordings, not just fail cleanly once — a fresh
    // StartRecording, into a fresh temp dir, with the fault seam DISARMED
    // (`clear_env` already dropped above, so `CHRONA_TEST_FAIL_PUSH_AFTER`
    // is unset — "re-arm" here is just "don't set it again"), must start
    // recording normally. Still inside `_guard` (held until the end of this
    // function): this StartRecording calls `SessionWriter::create` too.
    let dir2 = tempfile::tempdir().unwrap();
    eng.send(ControlMsg::StartRecording {
        dir: dir2.path().to_path_buf(),
        meta_position: None,
        watch: None,
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        if eng.snapshot().recording.is_some() {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "recovery StartRecording after a write failure never took — the engine's \
             post-failure state must still accept new recordings"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }

    // Shutdown + join must complete (test would hang otherwise) — also
    // confirms the engine thread is still alive and responsive, not
    // panicked/dead, i.e. no panic occurred.
    drop(eng);
}

#[test]
fn config_changes_do_not_clear_the_idle_error_banner() {
    // T3 design note: `apply_config` (SetLift/SetAveraging/SetBphMode/
    // SetPpm) clears `banner` to `None` on a successful candidate — correct
    // for a live source, but while idle the published Error banner is
    // reporting "there is no active source", a fact no config tweak can
    // change (only a successful SwitchSource can). A *valid* SetLift must
    // not silently wipe that Error into a falsely-healthy `None` banner.
    let mut eng = Engine::start_with_turbo(
        SourceSpec::ReplayFile {
            path: std::path::PathBuf::from("/nonexistent/chrona-total-failure-probe-3.wav"),
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

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        if eng.snapshot().banner.is_some() {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "no banner published within 10s of a total startup failure"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }

    eng.send(ControlMsg::SetLift(44.0)); // valid — must not clear the banner
    std::thread::sleep(std::time::Duration::from_millis(300));
    let snap = eng.snapshot();
    let b = snap
        .banner
        .as_ref()
        .expect("a valid SetLift while idle must not clear the no-source Error banner");
    assert_eq!(b.severity, BannerSeverity::Error);
    assert!(b.text.contains("failed to start"));

    // And prove the loop is genuinely alive (not just "the banner happens
    // to survive because the thread died and nothing ever overwrites
    // anything") by recovering from here too.
    eng.send(ControlMsg::SwitchSource(SourceSpec::Simulate {
        rate: 12.0,
        beat_error: 0.8,
        amplitude: 270.0,
        snr: 30.0,
    }));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    loop {
        if eng
            .snapshot()
            .metrics
            .as_ref()
            .is_some_and(|m| m.tier == chrona_dsp::Tier::T3)
        {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "never recovered to Tier 3 — engine loop likely dead"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    drop(eng);
}

#[test]
fn simulate_source_snapshots_never_carry_a_clock_skew_reading() {
    // M4 Task 7 (binding spec §3.4's NTP cross-check, §10 deviation #4): the
    // system-clock skew regression samples the MIC path only — Simulate's
    // delivery is synthetic (generated in-process, not actually clocked
    // against real audio hardware), so cross-checking it against the system
    // clock would measure nothing real. `SkewTracker`'s own unit tests (in
    // engine.rs, same-crate — it's private) cover the regression math in
    // isolation; this confirms the engine wiring never feeds it on a
    // Simulate source, end to end through a real published snapshot, across
    // several publish cycles (not just the first one).
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
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    let mut checked = 0;
    while checked < 20 {
        let s = eng.snapshot();
        if s.metrics.is_some() {
            assert_eq!(
                s.health.clock_skew, None,
                "a Simulate snapshot published a clock_skew reading — synthetic \
                 delivery must never feed the skew regression"
            );
            checked += 1;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "never observed 20 metrics-bearing snapshots to check"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    drop(eng);
}

/// M4 Task 8 pre-review addition, split into two cases by Task 8b: the
/// design spec's (2026-09-07-m4-trust-features-design.md §3)
/// headless-testability bullet for the calibration wizard — "the capture
/// stage's engine plumbing is tested on the Simulate source (flow, cancel,
/// temp-file cleanup — cal reporting NoTicks/Unstable on a watch signal is
/// itself the honest-failure path under test)" — which the task-8 brief's
/// own dispatch had dropped. End to end through the real public `Engine`,
/// same as every other test in this file: `StartRecording` into a temp dir
/// (same shape `ui::cal_wizard`'s `apply_wizard_action` itself sends: no
/// position, no watch metadata), confirm the path via the snapshot, record
/// real audio, `StopRecording`, confirm the finalize, then run the same
/// read-and-calibrate pipeline `ui::cal_wizard::analyze_wav` wraps
/// (`SessionReader::open` + `calibrate_quartz`, inlined here — that fn is
/// private to the lib crate, unreachable from this separate integration-test
/// binary) and confirm the HONEST failure: a genuine mechanical-watch signal
/// (not a 1 Hz quartz stepper) must be rejected, never given a fabricated
/// ppm. Finally reproduces `TempCaptureGuard::drop`'s own
/// two-`remove_file`-call body (also private to the lib crate) and confirms
/// both files are gone — the Discard/Cancel temp-file-cleanup path the spec
/// also asks this test to cover.
///
/// **Why this is now two functions:** a CLEAN Simulate watch signal (`snr:
/// 30.0`, the value used elsewhere in this file) was tried first, during
/// Task 8's own review, and, surprisingly, did NOT fail —
/// `calibrate_quartz` returned `Ok(QuartzCalResult { ppm: -1.05,
/// residual_ppm: 1.13, events: 381, .. })`. Root cause, worked out from the
/// numbers: `chrona_dsp::synth::synthesize`'s beat grid (the engine's
/// Simulate source always uses `SynthConfig::default()`'s bph, not
/// configurable via `SourceSpec::Simulate`) runs at a rate that divides
/// evenly into 1 second — true of every standard bph this app exposes
/// (18k/21.6k/25.2k/28.8k/36k all give an integer beats/second) — so a beat
/// lands at (very nearly) the same phase every integer second regardless of
/// which second you look at, and `calibrate_quartz`'s ±50 ms gated tracker +
/// per-cycle period re-estimate (`t_hat`) locks onto "the loudest beat
/// nearest each integer second" just as reliably as it locks onto a genuine
/// 1 Hz quartz tick — producing a plausible-looking but MEANINGLESS ppm.
/// Flagged in task-8-report.md as a real, separate concern rather than
/// worked around silently (`chrona-dsp` stayed untouched per Task 8's own
/// constraints) — Task 8b (task-8b-brief.md) is that follow-up: it adds
/// `CalError::NotQuartz` (`cal.rs`) specifically to close this path. This
/// fn keeps the ORIGINAL Task 8 case (`snr: 0.0`: tick-burst amplitude ==
/// noise RMS, an already-precedented "very poor recording" value in
/// `chrona_dsp::synth`'s own tests, burying the beat energy at the noise
/// floor) — at that snr, the seed step may lose tick lock before the
/// NotQuartz gate even has data to measure an off-gate ratio from, so
/// either gate may fire first; this fn accepts all three honest outcomes.
/// The sibling fn below,
/// `cal_wizard_capture_on_simulate_snr30_yields_honest_not_quartz`, is the
/// clean-signal (`snr: 30.0`) case and pins the SPECIFIC `NotQuartz` outcome
/// — the exact silent-wrong path Task 8b closes.
///
/// **Why the capture is POLLED, not slept** (both this fn and its snr-30
/// sibling): `calibrate_quartz` (`cal.rs`) rejects anything under its own
/// `MIN_SECONDS = 300.0` with `CalError::TooShort` BEFORE it ever runs the
/// tick-detection logic that could report `NotQuartz`/`NoTicks`/`Unstable`
/// — so this test needs the captured WAV to hold at least 300 SIMULATED
/// seconds, not merely "some" audio. The original version slept a fixed
/// 45 real seconds budgeted from a dev-machine turbo measurement
/// (~9.8 sim-s/real-s, ~1.3x margin); the first CI run proved that rate
/// does not transfer — shared runners delivered 4.6–5.6 sim-s/real-s and
/// under-filled the capture (206 s/254 s), tripping the duration guard on
/// two OSes. `wait_for_captured_sim_seconds` removes the throughput
/// assumption entirely: it watches the growing WAV until the target is
/// actually reached, finishing FASTER on quick machines and adapting to
/// slow ones. The `duration_s >= 300.0` assertion below still re-checks
/// the finalized WAV before ever calling into `calibrate_quartz` — the
/// belt to the poll's suspenders.
///
/// Holds `SESSION_WRITER_TEST_LOCK` (see its doc comment): calls
/// `SessionWriter::create` via `StartRecording`. Its snr-30 sibling holds
/// the same lock, so the two polled captures serialize rather than
/// overlap — see task-8b-report.md.
/// Polls the in-progress capture WAV until it holds at least `target_s`
/// SIMULATED seconds of audio, so the test never assumes a turbo
/// sim-s/real-s throughput (see the doc comment on the snr-0 test above —
/// the first CI run proved dev-machine rates don't transfer to shared
/// runners). File size tracks delivered audio closely: the engine writes
/// f32 mono at 48 kHz (4 bytes/sample) through hound's BufWriter, whose
/// ≤8 KB buffering lag is noise against the ~58 MB target. The deadline
/// converts a pathologically slow machine (< ~1.3 sim-s/real-s, 4x under
/// the slowest rate ever observed) into a clear self-diagnosing failure
/// rather than a hang.
fn wait_for_captured_sim_seconds(wav_path: &std::path::Path, target_s: f64) {
    const BYTES_PER_SIM_SECOND: f64 = 48_000.0 * 4.0;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(240);
    loop {
        let bytes = std::fs::metadata(wav_path).map(|m| m.len()).unwrap_or(0);
        let sim_s = bytes as f64 / BYTES_PER_SIM_SECOND;
        if sim_s >= target_s {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "only {sim_s:.1}s of simulated audio captured within the 240s deadline \
             (need >= {target_s}s) — turbo throughput on this machine is far below any \
             observed rate; calibrate_quartz would report TooShort, which is not what \
             this test is checking"
        );
        std::thread::sleep(std::time::Duration::from_secs(2));
    }
}

#[test]
fn cal_wizard_capture_on_simulate_snr0_yields_honest_failure() {
    let _guard = lock_session_writer_tests();
    let mut eng = Engine::start_with_turbo(
        SourceSpec::Simulate {
            rate: 12.0,
            beat_error: 0.8,
            amplitude: 270.0,
            snr: 0.0,
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

    // Same StartRecording shape ui::cal_wizard::apply_wizard_action's
    // StartCapture arm sends (a temp dir, no position/watch metadata) — the
    // "chrona-cal"-prefixed name is cosmetic (mirrors the wizard's own
    // std::env::temp_dir().join("chrona-cal")) but this uses a fresh
    // per-test tempdir rather than that literal fixed path, same hermetic-
    // test convention every other test in this file already uses.
    let dir = tempfile::Builder::new()
        .prefix("chrona-cal-test-")
        .tempdir()
        .unwrap();
    eng.send(ControlMsg::StartRecording {
        dir: dir.path().to_path_buf(),
        meta_position: None,
        watch: None,
    });

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let wav_path = loop {
        if let Some(path) = eng.snapshot().recording {
            break path;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "StartRecording never took"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    let sidecar_path = wav_path.with_extension("json");
    assert!(
        wav_path.exists(),
        "WAV should already exist once recording is confirmed"
    );
    assert!(
        sidecar_path.exists(),
        "sidecar should already exist once recording is confirmed"
    );

    // Let enough SIMULATED audio accumulate to clear calibrate_quartz's own
    // 300s minimum — polled, not slept (see this test's doc comment); 305
    // gives margin over the gate plus write-buffering slack.
    wait_for_captured_sim_seconds(&wav_path, 305.0);

    eng.send(ControlMsg::StopRecording);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        if eng.snapshot().recording.is_none() {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "StopRecording never took"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    drop(eng); // Shutdown + join must complete (test would hang otherwise)

    let reader = chrona_session::SessionReader::open(&wav_path).expect("finalized WAV opens");
    let duration_s = reader.samples.len() as f64 / reader.sample_rate_hz;
    assert!(
        duration_s >= 300.0,
        "only captured {duration_s:.1}s of simulated audio (need >= 300s) despite the \
         polled wait reporting success — the WAV shrank or the finalize dropped samples; \
         calibrate_quartz would report TooShort here, not NotQuartz/NoTicks/Unstable, \
         which is not what this test is checking"
    );

    // The honest-failure path itself (design spec §3): calibrate_quartz on
    // a real mechanical-watch tick train — never a fabricated ppm. At this
    // snr (0.0, beat energy buried at the noise floor), the seed step may
    // defeat tick detection before the NotQuartz gate (Task 8b) has tick
    // data to measure an off-gate ratio from, so NotQuartz is also an
    // acceptable honest outcome alongside the original NoTicks/Unstable —
    // see this fn's doc comment.
    let result = chrona_session::SessionReader::open(&wav_path)
        .map_err(|e| e.to_string())
        .map(|r| chrona_dsp::cal::calibrate_quartz(&r.samples, r.sample_rate_hz));
    let result = result.expect("WAV already confirmed to open above");
    assert!(
        matches!(
            result,
            Err(chrona_dsp::cal::CalError::NotQuartz { .. })
                | Err(chrona_dsp::cal::CalError::NoTicks)
                | Err(chrona_dsp::cal::CalError::Unstable { .. })
        ),
        "expected Err(NotQuartz | NoTicks | Unstable) on a mechanical-watch signal, got \
         {result:?}"
    );

    // Temp-file cleanup path (design spec: "the temp WAV is deleted after
    // analysis regardless of outcome") — reproduces
    // ui::cal_wizard::TempCaptureGuard::drop's own two-`remove_file`-call
    // body (private to the lib crate, unreachable here) rather than
    // constructing one.
    let _ = std::fs::remove_file(&wav_path);
    let _ = std::fs::remove_file(&sidecar_path);
    assert!(!wav_path.exists(), "WAV should be gone after cleanup");
    assert!(
        !sidecar_path.exists(),
        "sidecar should be gone after cleanup"
    );
}

/// Task 8b's own addition: the clean-signal (`snr: 30.0`) sibling of
/// `cal_wizard_capture_on_simulate_snr0_yields_honest_failure` above — see
/// that fn's doc comment for the full shared context (setup shape, the
/// polled-capture/300s-minimum rationale, `SESSION_WRITER_TEST_LOCK`). Pins the
/// SPECIFIC outcome Task 8b exists to close: pre-Task-8b, this exact signal
/// silently returned `Ok(QuartzCalResult { ppm: -1.05, residual_ppm: 1.13,
/// events: 381, .. })` — a plausible bogus ppm from a mechanical watch,
/// found during Task 8's own review (see the sibling fn's doc comment).
/// After this task's `CalError::NotQuartz` gate (`cal.rs`), it must be
/// rejected specifically as `NotQuartz`, not merely "some honest error" —
/// the strongest assertion this file can make of the exact silent-wrong
/// path being closed.
#[test]
fn cal_wizard_capture_on_simulate_snr30_yields_honest_not_quartz() {
    let _guard = lock_session_writer_tests();
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

    let dir = tempfile::Builder::new()
        .prefix("chrona-cal-test-")
        .tempdir()
        .unwrap();
    eng.send(ControlMsg::StartRecording {
        dir: dir.path().to_path_buf(),
        meta_position: None,
        watch: None,
    });

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let wav_path = loop {
        if let Some(path) = eng.snapshot().recording {
            break path;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "StartRecording never took"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    let sidecar_path = wav_path.with_extension("json");
    assert!(
        wav_path.exists(),
        "WAV should already exist once recording is confirmed"
    );
    assert!(
        sidecar_path.exists(),
        "sidecar should already exist once recording is confirmed"
    );

    // Let enough SIMULATED audio accumulate to clear calibrate_quartz's own
    // 300s minimum — polled, not slept (see the sibling fn's doc comment).
    wait_for_captured_sim_seconds(&wav_path, 305.0);

    eng.send(ControlMsg::StopRecording);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        if eng.snapshot().recording.is_none() {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "StopRecording never took"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    drop(eng); // Shutdown + join must complete (test would hang otherwise)

    let reader = chrona_session::SessionReader::open(&wav_path).expect("finalized WAV opens");
    let duration_s = reader.samples.len() as f64 / reader.sample_rate_hz;
    assert!(
        duration_s >= 300.0,
        "only captured {duration_s:.1}s of simulated audio (need >= 300s) despite the \
         polled wait reporting success — the WAV shrank or the finalize dropped samples; \
         calibrate_quartz would report TooShort here, not NotQuartz, which is not what \
         this test is checking"
    );

    // The exact silent-wrong path Task 8b closes (task-8b-brief.md): a
    // clean mechanical-watch recording must be rejected as NotQuartz, never
    // silently accepted with a plausible bogus ppm.
    let result = chrona_session::SessionReader::open(&wav_path)
        .map_err(|e| e.to_string())
        .map(|r| chrona_dsp::cal::calibrate_quartz(&r.samples, r.sample_rate_hz));
    let result = result.expect("WAV already confirmed to open above");
    assert!(
        matches!(result, Err(chrona_dsp::cal::CalError::NotQuartz { .. })),
        "expected Err(NotQuartz{{..}}) on a clean mechanical-watch signal, got {result:?}"
    );

    // Temp-file cleanup path (design spec: "the temp WAV is deleted after
    // analysis regardless of outcome") — reproduces
    // ui::cal_wizard::TempCaptureGuard::drop's own two-`remove_file`-call
    // body (private to the lib crate, unreachable here) rather than
    // constructing one.
    let _ = std::fs::remove_file(&wav_path);
    let _ = std::fs::remove_file(&sidecar_path);
    assert!(!wav_path.exists(), "WAV should be gone after cleanup");
    assert!(
        !sidecar_path.exists(),
        "sidecar should be gone after cleanup"
    );
}

/// M4 Task 10 (Mic Doctor engine plumbing, binding spec §3.2): a
/// `ControlMsg::DoctorCapture` on a live Simulate source is delivered to
/// EXACTLY ONE published snapshot, then cleared. `±1 chunk` per the task
/// brief is derived from the DELIVERED sample rate rather than a hardcoded
/// assumption (`sim_chunk_len`/`TICK_S` are private to `engine.rs`, not
/// reachable from this separate integration-test binary) — `chunk_len * 2`
/// gives a little slack around exact tick-boundary overshoot without
/// tolerating a grossly wrong length.
///
/// Deliberately real-time paced (`Engine::start`, NOT `start_with_turbo`),
/// unlike every other test in this file: turbo's uncapped tick rate makes
/// the one-shot window genuinely too narrow to poll reliably. Empirically
/// confirmed during development — turbo mode delivers the buffer within
/// ~2ms of sending the request, but the very next publish (~1 tick later)
/// already overwrites it with `None`; the analyzer's periodic "refold" over
/// its averaging window (see `Engine::snapshot`'s doc comment: "spikes to
/// ~88 ms on the ~1-in-10 polls that trigger a refold") only slows the loop
/// down LATER, by which point the one-shot value is long gone. Even a tight
/// no-sleep spin-poll from the test thread missed it under the CPU
/// contention of the full test-file's parallel run (though it reliably
/// caught it running alone) — a real race, not a slow-poller artifact. At
/// the real ~50 ms/tick pace, the one-shot snapshot instead survives a full
/// ~100 ms publish cycle (`SNAPSHOT_EVERY`) before being overwritten,
/// comfortably longer than this loop's poll interval even under load.
#[test]
fn doctor_capture_on_simulate_delivers_one_shot_buffer_then_clears() {
    let mut eng = Engine::start(
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
    );
    eng.send(ControlMsg::DoctorCapture { seconds: 2.0 });

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let (samples, sr) = loop {
        if let Some(pair) = eng.snapshot().doctor_capture {
            break pair;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "DoctorCapture{{seconds: 2.0}} never delivered a buffer"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    };
    let chunk_len = (sr * 0.05).round();
    let expected = 2.0 * sr;
    assert!(
        (samples.len() as f64 - expected).abs() <= chunk_len * 2.0,
        "expected ~{expected} samples (2.0s @ {sr} Hz), got {} (~{:.2}s)",
        samples.len(),
        samples.len() as f64 / sr
    );

    // One-shot: the NEXT snapshot must carry None — generous sleep past at
    // least one more ~10 Hz publish cycle (SNAPSHOT_EVERY, engine.rs)
    // before checking, so this isn't just re-reading the same still-Some
    // triple_buffer slot.
    std::thread::sleep(std::time::Duration::from_millis(200));
    assert!(
        eng.snapshot().doctor_capture.is_none(),
        "doctor_capture must clear after the one snapshot that carried it"
    );
    drop(eng);
}

/// A second `DoctorCapture` arriving before the first has been delivered
/// discards whatever the first had accumulated and starts fresh against its
/// own `seconds` (task brief: "second replaces first"). Sent back-to-back
/// with no sleep between them so both are drained in (or very near) the
/// same inbox batch, well before the first (20s) request could complete —
/// see `engine_loop`'s inbox-draining loop, `engine.rs`. If replacement
/// didn't happen, the eventual buffer would reflect the first request's 20s
/// target (~10x this test's tolerance), not the second's 2s.
///
/// Real-time paced (`Engine::start`), not turbo — see the sibling
/// one-shot-delivery test's doc comment for why: turbo's one-shot window is
/// too narrow to poll reliably (confirmed racy under this file's own
/// parallel run), while the real ~50 ms/tick pace gives it a full ~100 ms
/// publish cycle to be observed.
#[test]
fn doctor_capture_overlapping_request_replaces_the_first() {
    let mut eng = Engine::start(
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
    );
    eng.send(ControlMsg::DoctorCapture { seconds: 20.0 });
    eng.send(ControlMsg::DoctorCapture { seconds: 2.0 });

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let (samples, sr) = loop {
        if let Some(pair) = eng.snapshot().doctor_capture {
            break pair;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "overlapping DoctorCapture requests never delivered a buffer"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    };
    let chunk_len = (sr * 0.05).round();
    let expected = 2.0 * sr; // the SECOND request's target, not the first's 20.0s
    assert!(
        (samples.len() as f64 - expected).abs() <= chunk_len * 2.0,
        "final buffer should reflect the second (2.0s) request, not the first's \
         20.0s target — got {} samples (~{:.2}s @ {sr} Hz)",
        samples.len(),
        samples.len() as f64 / sr
    );
    drop(eng);
}

/// Defensive clamp (task brief): `seconds` below the 1.0s floor is clamped
/// up to it, never honored as-is (and never produces a near-empty/zero-
/// length buffer).
///
/// Real-time paced (`Engine::start`), not turbo — see `doctor_capture_on_
/// simulate_delivers_one_shot_buffer_then_clears`'s doc comment for why.
#[test]
fn doctor_capture_clamps_seconds_below_minimum() {
    let mut eng = Engine::start(
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
    );
    eng.send(ControlMsg::DoctorCapture { seconds: 0.1 });

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let (samples, sr) = loop {
        if let Some(pair) = eng.snapshot().doctor_capture {
            break pair;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "DoctorCapture{{seconds: 0.1}} never delivered a buffer"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    };
    let chunk_len = (sr * 0.05).round();
    let expected = 1.0 * sr; // clamped up to the 1.0s floor, not the requested 0.1s
    assert!(
        (samples.len() as f64 - expected).abs() <= chunk_len * 2.0,
        "0.1s request should clamp to the 1.0s minimum — expected ~{expected} \
         samples (1.0s @ {sr} Hz), got {} (~{:.2}s)",
        samples.len(),
        samples.len() as f64 / sr
    );
    drop(eng);
}

/// M4 Task 11 (spec §6, "Scope" view): `ControlMsg::SetScope` gates whether
/// `EngineSnapshot::scope` is populated at all. Unlike `DoctorCapture`,
/// this is NOT one-shot — once on, every publish carries a fresh
/// `Some(..)` for as long as the view stays open — so, like the T3/tape
/// test above, this runs on `start_with_turbo` rather than real-time
/// pacing.
#[test]
fn set_scope_gates_the_published_scope_field() {
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
    assert!(
        eng.snapshot().scope.is_none(),
        "scope must default to None (SetScope never sent)"
    );

    eng.send(ControlMsg::SetScope(true));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    let snap = loop {
        let s = eng.snapshot();
        if s.metrics
            .as_ref()
            .is_some_and(|m| m.tier == chrona_dsp::Tier::T3)
            && s.scope.as_ref().is_some_and(|v| !v.is_empty())
        {
            break s;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "never reached T3 with a non-empty scope (last tier {:?}, scope {:?})",
            s.metrics.as_ref().map(|m| m.tier),
            s.scope.as_ref().map(|v| v.len())
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    assert!(!snap.scope.expect("checked Some above").is_empty());

    eng.send(ControlMsg::SetScope(false));
    let deadline2 = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        if eng.snapshot().scope.is_none() {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline2,
            "scope never cleared back to None after SetScope(false)"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    drop(eng);
}
