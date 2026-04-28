//! Average Directional Index (Wilder). Returns ADX + DI+ + DI-.

use crate::data::Candle;

#[derive(Debug, Clone, Copy)]
pub struct AdxValue {
    pub adx: f64,
    pub di_plus: f64,
    pub di_minus: f64,
}

#[derive(Debug, Clone)]
pub struct Adx {
    period: usize,
    prev_candle: Option<Candle>,

    // Smoothed accumulators over `period`
    smooth_tr: f64,
    smooth_plus_dm: f64,
    smooth_minus_dm: f64,

    dx_window: Vec<f64>,
    adx: Option<f64>,

    warmup: usize,
    seeded: bool,
}

impl Adx {
    pub fn new(period: usize) -> Self {
        assert!(period > 1);
        Self {
            period,
            prev_candle: None,
            smooth_tr: 0.0,
            smooth_plus_dm: 0.0,
            smooth_minus_dm: 0.0,
            dx_window: Vec::with_capacity(period),
            adx: None,
            warmup: 0,
            seeded: false,
        }
    }

    pub fn next(&mut self, c: &Candle) -> Option<AdxValue> {
        let prev = match self.prev_candle {
            Some(p) => p,
            None => {
                self.prev_candle = Some(*c);
                return None;
            }
        };
        self.prev_candle = Some(*c);

        let up_move = c.high - prev.high;
        let down_move = prev.low - c.low;
        let plus_dm = if up_move > down_move && up_move > 0.0 {
            up_move
        } else {
            0.0
        };
        let minus_dm = if down_move > up_move && down_move > 0.0 {
            down_move
        } else {
            0.0
        };

        let tr = {
            let hl = c.high - c.low;
            let hc = (c.high - prev.close).abs();
            let lc = (c.low - prev.close).abs();
            hl.max(hc).max(lc)
        };

        let p = self.period as f64;

        if !self.seeded {
            self.smooth_tr += tr;
            self.smooth_plus_dm += plus_dm;
            self.smooth_minus_dm += minus_dm;
            self.warmup += 1;
            if self.warmup == self.period {
                self.seeded = true;
            } else {
                return None;
            }
        } else {
            self.smooth_tr = self.smooth_tr - self.smooth_tr / p + tr;
            self.smooth_plus_dm = self.smooth_plus_dm - self.smooth_plus_dm / p + plus_dm;
            self.smooth_minus_dm = self.smooth_minus_dm - self.smooth_minus_dm / p + minus_dm;
        }

        if self.smooth_tr == 0.0 {
            return None;
        }
        let di_plus = 100.0 * self.smooth_plus_dm / self.smooth_tr;
        let di_minus = 100.0 * self.smooth_minus_dm / self.smooth_tr;
        let dx = if di_plus + di_minus == 0.0 {
            0.0
        } else {
            100.0 * (di_plus - di_minus).abs() / (di_plus + di_minus)
        };

        if self.dx_window.len() < self.period {
            self.dx_window.push(dx);
            if self.dx_window.len() == self.period {
                self.adx = Some(self.dx_window.iter().sum::<f64>() / p);
            }
        } else {
            let prev_adx = self.adx.unwrap();
            self.adx = Some((prev_adx * (p - 1.0) + dx) / p);
        }

        self.adx.map(|adx| AdxValue {
            adx,
            di_plus,
            di_minus,
        })
    }
}
