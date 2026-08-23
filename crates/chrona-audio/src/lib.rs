//! Chrona's capture layer: cpal input streams behind a lock-free ring
//! (spec §4). Deliberately DSP-ignorant — no dependency on `chrona-dsp`.

pub mod capture;
pub mod devices;
pub mod monoize;

pub use capture::{CaptureError, CaptureHealth, CaptureStream, StreamInfo};
pub use devices::{DeviceInfo, list_input_devices};
