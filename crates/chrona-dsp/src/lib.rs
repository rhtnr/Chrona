//! Chrona's pure DSP core. No audio I/O, no OS dependencies (spec §4):
//! push `f32` mono samples in, read typed estimates out.

pub mod analyzer;
pub mod autocorr;
pub mod bph;
pub mod envelope;
pub mod events;
pub mod filter;
pub mod fold;
pub mod metrics;
pub mod period;
pub mod ring;
pub mod synth;
pub mod tier;

pub use analyzer::{
    Analyzer, AnalyzerConfig, AnalyzerError, BphMode, MetricsSnapshot, Quality, RateSource,
};
pub use period::PeriodEstimate;
pub use tier::Tier;

#[cfg(test)]
mod smoke {
    #[test]
    fn workspace_builds_and_tests_run() {
        assert_eq!(2 + 2, 4);
    }
}
