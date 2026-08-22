use assert_cmd::Command;

fn chrona() -> Command {
    Command::cargo_bin("chrona").expect("binary builds")
}

fn write_f32_wav(path: &std::path::Path, samples: &[f32], sr: u32) {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: sr,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut w = hound::WavWriter::create(path, spec).unwrap();
    for &s in samples {
        w.write_sample(s).unwrap();
    }
    w.finalize().unwrap();
}

#[test]
fn calibrate_recovers_ppm_and_reports_usage() {
    let dir = tempfile::tempdir().unwrap();
    let wav = dir.path().join("quartz.wav");
    let x = chrona_dsp::synth::synthesize_quartz(320.0, 48_000.0, 42.0, 30.0, 7).unwrap();
    write_f32_wav(&wav, &x, 48_000);
    let out = chrona()
        .args(["calibrate", wav.to_str().unwrap(), "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
    let ppm = v["ppm"].as_f64().unwrap();
    assert!((ppm - 42.0).abs() <= 0.5, "ppm {ppm}");
}

#[test]
fn calibrate_short_recording_exits_2() {
    let dir = tempfile::tempdir().unwrap();
    let wav = dir.path().join("short.wav");
    let x = chrona_dsp::synth::synthesize_quartz(30.0, 48_000.0, 0.0, 30.0, 1).unwrap();
    write_f32_wav(&wav, &x, 48_000);
    chrona()
        .args(["calibrate", wav.to_str().unwrap()])
        .assert()
        .code(2);
}
