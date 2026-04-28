//! Average True Range (Wilder).

use crate::data::Candle;

#[derive(Debug, Clone)]
pub struct Atr {
    period: usize,
    prev_close: Option<f64>,
    value: Option<f64>,
    seed_sum: f64,
    seed_count: usize,
}

impl Atr {
    pub fn new(period: usize) -> Self {
        assert!(period > 0);
        Self {
            period,
            prev_close: None,
            value: None,
            seed_sum: 0.0,
            seed_count: 0,
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
        if self.value.is_none() {
            self.seed_sum += tr;
            self.seed_count += 1;
            if self.seed_count == self.period {
                let v = self.seed_sum / self.period as f64;
                self.value = Some(v);
                return Some(v);
            }
            return None;
        }
        let p = self.period as f64;
        let new_val = (self.value.unwrap() * (p - 1.0) + tr) / p;
        self.value = Some(new_val);
        Some(new_val)
    }

    pub fn value(&self) -> Option<f64> {
        self.value
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn c(h: f64, l: f64, cl: f64) -> Candle {
        Candle {
            open_time: Utc::now(),
            close_time: Utc::now(),
            open: l,
            high: h,
            low: l,
            close: cl,
            volume: 0.0,
        }
    }

    #[test]
    fn atr_steady_range() {
        let mut a = Atr::new(3);
        a.next(&c(10.0, 8.0, 9.0));
        a.next(&c(10.0, 8.0, 9.0));
        let v = a.next(&c(10.0, 8.0, 9.0)).unwrap();
        approx::assert_abs_diff_eq!(v, 2.0, epsilon = 1e-9);
    }
}
