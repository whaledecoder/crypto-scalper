//! Keltner Channel (EMA ± k * ATR). Used for Volatility Squeeze detection.

use crate::data::Candle;
use crate::indicators::{Atr, Ema};

#[derive(Debug, Clone, Copy)]
pub struct KeltnerBand {
    pub lower: f64,
    pub mid: f64,
    pub upper: f64,
}

#[derive(Debug, Clone)]
pub struct Keltner {
    ema: Ema,
    atr: Atr,
    k: f64,
}

impl Keltner {
    pub fn new(period: usize, k: f64) -> Self {
        Self {
            ema: Ema::new(period),
            atr: Atr::new(period),
            k,
        }
    }

    pub fn next(&mut self, c: &Candle) -> Option<KeltnerBand> {
        let tp = c.typical_price();
        let mid = self.ema.next(tp)?;
        let atr = self.atr.next(c)?;
        Some(KeltnerBand {
            lower: mid - self.k * atr,
            mid,
            upper: mid + self.k * atr,
        })
    }
}
