use assert_cmd::Command;
#[test]
fn headless_flag_parses_and_exits_cleanly() {
    // Engine unwired in this task: exit code 2 with the stub message proves the
    // flag surface without opening a window. T7 flips this test to expect 0 + metrics.
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
        .code(2);
}
#[test]
fn headless_without_simulate_is_a_usage_error() {
    Command::cargo_bin("chrona-app")
        .unwrap()
        .args(["--headless-seconds", "5"])
        .assert()
        .code(1); // validated in main: headless requires --simulate in M3
}
