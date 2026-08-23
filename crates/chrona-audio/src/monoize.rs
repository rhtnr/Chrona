//! Pure interleaved-to-mono averaging, split out so it is testable without
//! any audio hardware or cpal types.

/// Average interleaved multi-channel samples down to mono, appending nothing —
/// `out` is cleared first, then filled frame-by-frame via `extend`. Callers on
/// the audio callback path pre-reserve `out`'s capacity once at stream build
/// time; `clear()` does not release capacity, and `extend` from an
/// `ExactSizeIterator`-backed chunk iterator fills in place without growing,
/// so steady-state calls here perform no allocation.
///
/// A trailing partial frame (fewer than `channels` samples) is dropped, not
/// panicked on. `channels == 0` yields an empty `out`.
pub fn average_into(interleaved: &[f32], channels: usize, out: &mut Vec<f32>) {
    out.clear();
    if channels == 0 {
        return;
    }
    out.extend(
        interleaved
            .chunks_exact(channels)
            .map(|frame| frame.iter().sum::<f32>() / channels as f32),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn averages_channels() {
        let mut out = Vec::new();
        average_into(&[1.0, 3.0, 5.0, 7.0], 2, &mut out); // frames: (1,3) (5,7)
        assert_eq!(out, vec![2.0, 6.0]);
        average_into(&[1.0, 2.0, 3.0], 1, &mut out);
        assert_eq!(out, vec![1.0, 2.0, 3.0]);
    }

    #[test]
    fn ragged_tail_is_dropped_not_panicked() {
        let mut out = Vec::new();
        average_into(&[1.0, 3.0, 5.0], 2, &mut out); // ragged last frame
        assert_eq!(out, vec![2.0]);
    }

    #[test]
    fn zero_channels_yields_empty() {
        let mut out = vec![9.0];
        average_into(&[1.0, 2.0], 0, &mut out);
        assert!(out.is_empty());
    }
}
