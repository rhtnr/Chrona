use assert_cmd::Command;

fn chrona() -> Command {
    Command::cargo_bin("chrona").expect("binary builds")
}

#[test]
fn verify_passes_a_good_expectation_and_fails_a_bad_one() {
    let dir = tempfile::tempdir().unwrap();
    let wav = dir.path().join("w.wav");
    chrona()
        .args(["synth", wav.to_str().unwrap(), "--rate", "10.0"])
        .assert()
        .success();
    let good = r#"{"file":"w.wav","bph":"auto","expect_rate_s_per_day":10.0,"tol_rate":1.0}"#;
    std::fs::write(dir.path().join("good.json"), good).unwrap();
    chrona()
        .args(["verify", dir.path().to_str().unwrap()])
        .assert()
        .success();

    let bad = r#"{"file":"w.wav","bph":"auto","expect_rate_s_per_day":-25.0,"tol_rate":1.0}"#;
    std::fs::write(dir.path().join("bad.json"), bad).unwrap();
    chrona()
        .args(["verify", dir.path().to_str().unwrap()])
        .assert()
        .code(1);
}

#[test]
fn verify_empty_dir_succeeds_with_notice() {
    let dir = tempfile::tempdir().unwrap();
    chrona()
        .args(["verify", dir.path().to_str().unwrap()])
        .assert()
        .success();
}

#[test]
fn verify_checks_beat_error_and_amplitude_when_present() {
    let dir = tempfile::tempdir().unwrap();
    let wav = dir.path().join("m.wav");
    chrona()
        .args([
            "synth",
            wav.to_str().unwrap(),
            "--rate",
            "5.0",
            "--beat-error",
            "0.8",
            "--amplitude",
            "270",
        ])
        .assert()
        .success();
    let good = r#"{"file":"m.wav","bph":"auto","expect_rate_s_per_day":5.0,"tol_rate":1.0,
        "expect_beat_error_ms":0.8,"tol_beat_error":0.15,
        "expect_amplitude_deg":270.0,"tol_amplitude":6.0}"#;
    std::fs::write(dir.path().join("good.json"), good).unwrap();
    chrona()
        .args(["verify", dir.path().to_str().unwrap()])
        .assert()
        .success();

    let bad = r#"{"file":"m.wav","bph":"auto","expect_rate_s_per_day":5.0,"tol_rate":1.0,
        "expect_amplitude_deg":330.0,"tol_amplitude":5.0}"#;
    std::fs::write(dir.path().join("zbad.json"), bad).unwrap();
    chrona()
        .args(["verify", dir.path().to_str().unwrap()])
        .assert()
        .code(1);
}
