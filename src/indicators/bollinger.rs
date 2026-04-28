//! Bollinger Bands (SMA ± k*stddev).

use std::collections::VecDeque;

#[derive(Debug, Clone, Copy)]
pub struct BollingerBand {
    pub lower: f64,
    pub mid: f64,
    pub upper: f64,
    pub width: f64,
}

#[derive(Debug, Clone)]
pub struct Bollinger {
    period: usize,
    k: f64,
    buf: VecDeque<f64>,
    sum: f64,
    sum_sq: f64,
}

impl Bollinger {
    pub fn new(period: usize, k: f64) -> Self {
        assert!(period > 1);
        Self {
            period,
            k,
            buf: VecDeque::with_capacity(period),
            sum: 0.0,
            sum_sq: 0.0,
        }
    }

    pub fn next(&mut self, x: f64) -> Option<BollingerBand> {
        self.buf.push_back(x);
        self.sum += x;
        self.sum_sq += x * x;
        if self.buf.len() > self.period {
            let old = self.buf.pop_front().unwrap();
            self.sum -= old;
            self.sum_sq -= old * old;
        }
        if self.buf.len() < self.period {
            return None;
        }
        let n = self.period as f64;
        let mean = self.sum / n;
        let var = (self.sum_sq / n) - mean * mean;
        let sd = var.max(0.0).sqrt();
        let lower = mean - self.k * sd;
        let upper = mean + self.k * sd;
        Some(BollingerBand {
            lower,
            mid: mean,
            upper,
            width: upper - lower,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bb_constant_width_zero() {
        let mut bb = Bollinger::new(5, 2.0);
        for _ in 0..10 {
            if let Some(b) = bb.next(100.0) {
                approx::assert_abs_diff_eq!(b.width, 0.0, epsilon = 1e-9);
                approx::assert_abs_diff_eq!(b.mid, 100.0, epsilon = 1e-9);
            }
        }
    }
}
