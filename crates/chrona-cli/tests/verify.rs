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
