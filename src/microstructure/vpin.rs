use std::collections::VecDeque;

#[derive(Debug, Clone)]
pub struct Vpin {
    bucket_size: f64,
    window: usize,
    current_buy: f64,
    current_sell: f64,
    buckets: VecDeque<f64>,
}

impl Vpin {
    pub fn new(bucket_size: f64, window: usize) -> Self {
        Self {
            bucket_size: bucket_size.max(1e-9),
            window: window.max(1),
            current_buy: 0.0,
            current_sell: 0.0,
            buckets: VecDeque::with_capacity(window.max(1)),
        }
    }

    pub fn update(&mut self, buy_volume: f64, sell_volume: f64) -> Option<f64> {
        self.current_buy += buy_volume.max(0.0);
        self.current_sell += sell_volume.max(0.0);
        let total = self.current_buy + self.current_sell;
        if total < self.bucket_size {
            return self.value();
        }
        let imbalance = (self.current_buy - self.current_sell).abs() / total.max(1e-9);
        self.buckets.push_back(imbalance);
        if self.buckets.len() > self.window {
            self.buckets.pop_front();
        }
        self.current_buy = 0.0;
        self.current_sell = 0.0;
        self.value()
    }

    pub fn value(&self) -> Option<f64> {
        if self.buckets.len() < self.window {
            return None;
        }
        Some(self.buckets.iter().sum::<f64>() / self.buckets.len() as f64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn computes_vpin_after_window_fills() {
        let mut vpin = Vpin::new(10.0, 2);
        assert!(vpin.update(10.0, 0.0).is_none());
        let value = vpin.update(5.0, 5.0).unwrap();
        approx::assert_abs_diff_eq!(value, 0.5);
    }
}
