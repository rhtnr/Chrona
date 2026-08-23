//! Chrona session I/O (spec M3): WAV + JSON-sidecar recording, replay
//! through a fresh `chrona_dsp::Analyzer`, and the TOML app config store.

pub mod civil;
pub mod config;
pub mod reader;
pub mod replay;
pub mod sidecar;
pub mod writer;

pub use config::ConfigStore;
pub use reader::SessionReader;
pub use replay::{ReplayOverrides, ReplayResult, replay};
pub use sidecar::SessionMeta;
pub use writer::SessionWriter;
