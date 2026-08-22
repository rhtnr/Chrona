//! Bounded sample ring with absolute (since-start) indexing, shared by the
//! fold and event stages (spec §5.4–§5.5).

use std::collections::VecDeque;

pub struct SampleRing {
    buf: VecDeque<f32>,
    capacity: usize,
    total: u64,
}

impl SampleRing {
    pub fn new(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        SampleRing {
            buf: VecDeque::with_capacity(capacity),
            capacity,
            total: 0,
        }
    }

    pub fn push_slice(&mut self, s: &[f32]) {
        for &v in s {
            if self.buf.len() == self.capacity {
                self.buf.pop_front();
            }
            self.buf.push_back(v);
        }
        self.total += s.len() as u64;
    }

    pub fn len(&self) -> usize {
        self.buf.len()
    }
    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }
    /// Samples ever pushed; the newest retained sample has absolute index `total_pushed() - 1`.
    pub fn total_pushed(&self) -> u64 {
        self.total
    }
    /// Absolute index of the oldest retained sample.
    pub fn start_index(&self) -> u64 {
        self.total - self.buf.len() as u64
    }

    /// Copy the newest `n` samples (clamped to `len`) into `out`, oldest-first.
    pub fn copy_last(&self, n: usize, out: &mut Vec<f32>) {
        out.clear();
        let n = n.min(self.buf.len());
        out.extend(self.buf.iter().skip(self.buf.len() - n));
    }

    /// Copy `len` samples starting at absolute index `start_abs`. Returns false
    /// (with `out` cleared) if the range touches evicted or not-yet-pushed samples.
    pub fn copy_range_abs(&self, start_abs: u64, len: usize, out: &mut Vec<f32>) -> bool {
        out.clear();
        let end_abs = start_abs + len as u64;
        if start_abs < self.start_index() || end_abs > self.total {
            return false;
        }
        let offset = (start_abs - self.start_index()) as usize;
        out.extend(self.buf.iter().skip(offset).take(len));
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eviction_and_absolute_indexing() {
        let mut r = SampleRing::new(4);
        r.push_slice(&[1.0, 2.0, 3.0]);
        assert_eq!((r.len(), r.total_pushed(), r.start_index()), (3, 3, 0));
        r.push_slice(&[4.0, 5.0]); // evicts 1.0
        assert_eq!((r.len(), r.total_pushed(), r.start_index()), (4, 5, 1));
        let mut out = Vec::new();
        r.copy_last(2, &mut out);
        assert_eq!(out, vec![4.0, 5.0]);
        r.copy_last(10, &mut out); // clamped to len
        assert_eq!(out, vec![2.0, 3.0, 4.0, 5.0]);
    }

    #[test]
    fn copy_range_abs_detects_eviction_and_future() {
        let mut r = SampleRing::new(4);
        r.push_slice(&[0.0, 1.0, 2.0, 3.0, 4.0, 5.0]); // retains abs 2..=5
        let mut out = Vec::new();
        assert!(r.copy_range_abs(2, 3, &mut out));
        assert_eq!(out, vec![2.0, 3.0, 4.0]);
        assert!(!r.copy_range_abs(1, 3, &mut out), "evicted start must fail");
        assert!(
            !r.copy_range_abs(4, 3, &mut out),
            "reaching past newest must fail"
        );
        assert!(out.is_empty());
    }

    #[test]
    fn zero_capacity_is_clamped() {
        let mut r = SampleRing::new(0);
        r.push_slice(&[7.0]);
        assert_eq!(r.len(), 1);
    }
}
