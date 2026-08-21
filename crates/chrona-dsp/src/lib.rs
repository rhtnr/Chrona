//! Chrona's pure DSP core. No audio I/O, no OS dependencies (spec §4):
//! push `f32` mono samples in, read typed estimates out.

pub mod autocorr;
pub mod bph;
pub mod envelope;
pub mod filter;
pub mod period;
pub mod synth;

#[cfg(test)]
mod smoke {
    #[test]
    fn workspace_builds_and_tests_run() {
        assert_eq!(2 + 2, 4);
    }
}
