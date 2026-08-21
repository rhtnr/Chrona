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
