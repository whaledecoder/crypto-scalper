//! Rate of Change — percent change vs N periods ago.

use std::collections::VecDeque;

#[derive(Debug, Clone)]
pub struct Roc {
    period: usize,
    buf: VecDeque<f64>,
}

impl Roc {
    pub fn new(period: usize) -> Self {
        assert!(period > 0);
        Self {
            period,
            buf: VecDeque::with_capacity(period + 1),
        }
    }

    pub fn next(&mut self, x: f64) -> Option<f64> {
        self.buf.push_back(x);
        if self.buf.len() > self.period + 1 {
            self.buf.pop_front();
        }
        if self.buf.len() <= self.period {
            return None;
        }
        let oldest = *self.buf.front().unwrap();
        if oldest == 0.0 {
            return None;
        }
        Some((x - oldest) / oldest * 100.0)
    }
}
