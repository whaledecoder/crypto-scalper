//! Session VWAP (Volume-Weighted Average Price) with rolling slope.

use crate::data::Candle;

#[derive(Debug, Clone)]
pub struct Vwap {
    cum_pv: f64,
    cum_vol: f64,
    prev_value: Option<f64>,
    slope: Option<f64>,
}

impl Default for Vwap {
    fn default() -> Self {
        Self::new()
    }
}

impl Vwap {
    pub fn new() -> Self {
        Self {
            cum_pv: 0.0,
            cum_vol: 0.0,
            prev_value: None,
            slope: None,
        }
    }

    pub fn reset(&mut self) {
        self.cum_pv = 0.0;
        self.cum_vol = 0.0;
        self.prev_value = None;
        self.slope = None;
    }

    /// Ingest a candle and return current VWAP.
    pub fn next(&mut self, c: &Candle) -> Option<f64> {
        let tp = c.typical_price();
        self.cum_pv += tp * c.volume;
        self.cum_vol += c.volume;
        if self.cum_vol <= 0.0 {
            return None;
        }
        let v = self.cum_pv / self.cum_vol;
        if let Some(prev) = self.prev_value {
            // slope expressed as fraction change per candle
            self.slope = Some((v - prev) / prev.max(1e-9));
        }
        self.prev_value = Some(v);
        Some(v)
    }

    pub fn value(&self) -> Option<f64> {
        self.prev_value
    }

    /// Slope as fractional change per candle.
    pub fn slope(&self) -> Option<f64> {
        self.slope
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn c(tp: f64, vol: f64) -> Candle {
        Candle {
            open_time: Utc::now(),
            close_time: Utc::now(),
            open: tp,
            high: tp,
            low: tp,
            close: tp,
            volume: vol,
        }
    }

    #[test]
    fn vwap_equal_to_price_when_vol_constant() {
        let mut v = Vwap::new();
        v.next(&c(100.0, 10.0));
        v.next(&c(100.0, 10.0));
        let out = v.next(&c(100.0, 10.0)).unwrap();
        approx::assert_abs_diff_eq!(out, 100.0, epsilon = 1e-9);
    }
}
