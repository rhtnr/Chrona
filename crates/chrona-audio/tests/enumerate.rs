// Hardware-independent integration test: must pass on CI runners WITHOUT
// audio hardware. Does not open a stream (that would require a microphone
// permission grant on macOS) — enumeration only.
#[test]
fn enumeration_never_panics() {
    let devices = chrona_audio::list_input_devices();
    for d in &devices {
        assert!(!d.id.is_empty() && !d.name.is_empty());
    }
    // No assertion on count: CI machines may have zero input devices.
}
