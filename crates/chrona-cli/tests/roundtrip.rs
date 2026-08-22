//! synth → WAV → analyze roundtrips (spec §10: the CLI is the dev harness).

use assert_cmd::Command;

fn chrona() -> Command {
    Command::cargo_bin("chrona").expect("binary builds")
}

#[test]
fn synth_then_analyze_recovers_the_configured_rate() {
    let dir = tempfile::tempdir().unwrap();
    let wav = dir.path().join("t.wav");
    chrona()
        .args([
            "synth",
            wav.to_str().unwrap(),
            "--bph",
            "21600",
            "--rate",
            "12.5",
            "--snr",
            "30",
        ])
        .assert()
        .success();
    let out = chrona()
        .args(["analyze", wav.to_str().unwrap(), "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(v["status"], "ok");
    assert_eq!(v["bph_nominal"], 21_600);
    let rate = v["rate_s_per_day"].as_f64().unwrap();
    assert!((rate - 12.5).abs() < 0.5, "rate {rate}");
    assert_eq!(v["calibrated"], false);
}

#[test]
fn analyze_honors_ppm_and_fixed_bph() {
    let dir = tempfile::tempdir().unwrap();
    let wav = dir.path().join("t.wav");
    // Apparent −4.32 s/d caused by a 50 ppm-fast clock (see analyzer unit tests).
    chrona()
        .args(["synth", wav.to_str().unwrap(), "--rate=-4.32"])
        .assert()
        .success();
    let out = chrona()
        .args([
            "analyze",
            wav.to_str().unwrap(),
            "--bph",
            "28800",
            "--ppm",
            "50",
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
    let rate = v["rate_s_per_day"].as_f64().unwrap();
    assert!(rate.abs() < 0.5, "corrected rate {rate}");
    assert_eq!(v["calibrated"], true);
}

#[test]
fn analyze_noise_exits_2_with_no_beat() {
    let dir = tempfile::tempdir().unwrap();
    let wav = dir.path().join("noise.wav");
    chrona()
        .args([
            "synth",
            wav.to_str().unwrap(),
            "--snr=-40",
            "--duration",
            "10",
        ])
        .assert()
        .success();
    let out = chrona()
        .args(["analyze", wav.to_str().unwrap(), "--json"])
        .assert()
        .code(2)
        .get_output()
        .stdout
        .clone();
    let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(v["status"], "no_beat");
}

#[test]
fn missing_file_is_an_error() {
    chrona()
        .args(["analyze", "does-not-exist.wav"])
        .assert()
        .code(1);
}

#[test]
fn pcm16_synth_roundtrips_through_the_int_read_path() {
    let dir = tempfile::tempdir().unwrap();
    let wav = dir.path().join("t16.wav");
    chrona()
        .args(["synth", wav.to_str().unwrap(), "--rate", "10.0", "--pcm16"])
        .assert()
        .success();
    let out = chrona()
        .args(["analyze", wav.to_str().unwrap(), "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(v["status"], "ok");
    let rate = v["rate_s_per_day"].as_f64().unwrap();
    assert!((rate - 10.0).abs() < 0.5, "rate {rate}");
}

#[test]
fn full_metrics_appear_in_json_at_tier3() {
    let dir = tempfile::tempdir().unwrap();
    let wav = dir.path().join("full.wav");
    chrona()
        .args([
            "synth",
            wav.to_str().unwrap(),
            "--rate",
            "12.0",
            "--beat-error",
            "0.8",
            "--amplitude",
            "270",
        ])
        .assert()
        .success();
    let out = chrona()
        .args(["analyze", wav.to_str().unwrap(), "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(v["status"], "ok");
    assert_eq!(v["tier"], "T3");
    assert_eq!(v["rate_source"], "unlocking_regression");
    let be = v["beat_error_ms"].as_f64().unwrap();
    assert!((be - 0.8).abs() <= 0.1, "be {be}");
    let amp = v["amplitude_deg"].as_f64().unwrap();
    assert!((amp - 270.0).abs() <= 5.0, "amp {amp}");
    assert!(v["detection_ratio"].as_f64().unwrap() > 0.8);
}

#[test]
fn lift_flag_scales_amplitude_only() {
    let dir = tempfile::tempdir().unwrap();
    let wav = dir.path().join("lift.wav");
    chrona()
        .args(["synth", wav.to_str().unwrap(), "--amplitude", "270"])
        .assert()
        .success();
    let get = |lift: &str| {
        let out = chrona()
            .args(["analyze", wav.to_str().unwrap(), "--lift", lift, "--json"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        serde_json::from_slice::<serde_json::Value>(&out).unwrap()
    };
    let (v52, v40) = (get("52"), get("40"));
    let (a52, a40) = (
        v52["amplitude_deg"].as_f64().unwrap(),
        v40["amplitude_deg"].as_f64().unwrap(),
    );
    // Amplitude scales ~linearly with lift; rate must not move.
    assert!((a52 - a40).abs() > 30.0, "a52 {a52} a40 {a40}");
    assert!(
        (v52["rate_s_per_day"].as_f64().unwrap() - v40["rate_s_per_day"].as_f64().unwrap()).abs()
            < 0.2
    );
}
