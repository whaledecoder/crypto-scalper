use std::collections::VecDeque;

#[derive(Debug, Clone)]
pub struct Ofi {
    prev_bid_qty: Option<f64>,
    prev_ask_qty: Option<f64>,
    rolling: VecDeque<f64>,
    window: usize,
}

impl Ofi {
    pub fn new(window: usize) -> Self {
        Self {
            prev_bid_qty: None,
            prev_ask_qty: None,
            rolling: VecDeque::with_capacity(window.max(1)),
            window: window.max(1),
        }
    }

    pub fn update(&mut self, bid_qty: f64, ask_qty: f64) -> Option<f64> {
        let (prev_bid, prev_ask) = match (self.prev_bid_qty, self.prev_ask_qty) {
            (Some(b), Some(a)) => (b, a),
            _ => {
                self.prev_bid_qty = Some(bid_qty);
                self.prev_ask_qty = Some(ask_qty);
                return None;
            }
        };
        self.prev_bid_qty = Some(bid_qty);
        self.prev_ask_qty = Some(ask_qty);
        let value = (bid_qty - prev_bid) - (ask_qty - prev_ask);
        self.rolling.push_back(value);
        if self.rolling.len() > self.window {
            self.rolling.pop_front();
        }
        if self.rolling.len() < self.window {
            return None;
        }
        Some(self.rolling.iter().sum())
    }

    pub fn z_score(&self) -> Option<f64> {
        if self.rolling.len() < 2 {
            return None;
        }
        let n = self.rolling.len() as f64;
        let mean = self.rolling.iter().sum::<f64>() / n;
        let var = self.rolling.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / n;
        let sd = var.sqrt();
        if sd <= 0.0 {
            return None;
        }
        Some(mean / sd)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn computes_rolling_ofi() {
        let mut ofi = Ofi::new(3);
        assert!(ofi.update(10.0, 10.0).is_none());
        assert!(ofi.update(12.0, 9.0).is_none());
        assert!(ofi.update(13.0, 9.5).is_none());
        let value = ofi.update(12.0, 8.0).unwrap();
        approx::assert_abs_diff_eq!(value, 4.0);
        assert!(ofi.z_score().is_some());
    }
}
