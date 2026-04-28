//! Choppiness Index — measures trendiness vs sidewaysness.
//!
//! CI = 100 * log10( sum(TR,n) / (max(high,n) - min(low,n)) ) / log10(n)

use crate::data::Candle;
use std::collections::VecDeque;

#[derive(Debug, Clone)]
pub struct Choppiness {
    period: usize,
    buf: VecDeque<Candle>,
    tr_sum: f64,
    prev_close: Option<f64>,
    tr_hist: VecDeque<f64>,
}

impl Choppiness {
    pub fn new(period: usize) -> Self {
        assert!(period > 1);
        Self {
            period,
            buf: VecDeque::with_capacity(period),
            tr_sum: 0.0,
            prev_close: None,
            tr_hist: VecDeque::with_capacity(period),
        }
    }

    fn true_range(c: &Candle, prev_close: f64) -> f64 {
        let hl = c.high - c.low;
        let hc = (c.high - prev_close).abs();
        let lc = (c.low - prev_close).abs();
        hl.max(hc).max(lc)
    }

    pub fn next(&mut self, c: &Candle) -> Option<f64> {
        let tr = match self.prev_close {
            None => c.high - c.low,
            Some(pc) => Self::true_range(c, pc),
        };
        self.prev_close = Some(c.close);

        self.tr_hist.push_back(tr);
        self.tr_sum += tr;
        if self.tr_hist.len() > self.period {
            self.tr_sum -= self.tr_hist.pop_front().unwrap();
        }

        self.buf.push_back(*c);
        if self.buf.len() > self.period {
            self.buf.pop_front();
        }
        if self.buf.len() < self.period {
            return None;
        }

        let (mut hi, mut lo) = (f64::NEG_INFINITY, f64::INFINITY);
        for cc in &self.buf {
            if cc.high > hi {
                hi = cc.high;
            }
            if cc.low < lo {
                lo = cc.low;
            }
        }
        let range = (hi - lo).max(1e-9);
        let v = 100.0 * (self.tr_sum / range).log10() / (self.period as f64).log10();
        Some(v)
    }
}
