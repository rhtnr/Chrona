use assert_cmd::Command;
use predicates::str::contains;
#[test]
fn headless_flag_parses_and_exits_cleanly() {
    // T7: the engine is wired. Headless drives a real (synchronous) Simulate
    // run and prints the tier badge plus the rate line, then exits 0.
    Command::cargo_bin("chrona-app")
        .unwrap()
        .args([
            "--simulate",
            "--rate=12.0",
            "--beat-error",
            "0.8",
            "--headless-seconds",
            "5",
        ])
        .assert()
        .code(0)
        .stdout(contains("TIER"))
        .stdout(contains("s/d"));
}
#[test]
fn headless_without_simulate_is_a_usage_error() {
    Command::cargo_bin("chrona-app")
        .unwrap()
        .args(["--headless-seconds", "5"])
        .assert()
        .code(1); // validated in main: headless requires --simulate in M3
}
