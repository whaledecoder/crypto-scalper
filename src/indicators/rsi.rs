//! Relative Strength Index (Wilder smoothing).

#[derive(Debug, Clone)]
pub struct Rsi {
    period: usize,
    prev_close: Option<f64>,
    avg_gain: Option<f64>,
    avg_loss: Option<f64>,
    seed_gains: f64,
    seed_losses: f64,
    seed_count: usize,
}

impl Rsi {
    pub fn new(period: usize) -> Self {
        assert!(period > 1);
        Self {
            period,
            prev_close: None,
            avg_gain: None,
            avg_loss: None,
            seed_gains: 0.0,
            seed_losses: 0.0,
            seed_count: 0,
        }
    }

    pub fn next(&mut self, close: f64) -> Option<f64> {
        let prev = match self.prev_close {
            Some(p) => p,
            None => {
                self.prev_close = Some(close);
                return None;
            }
        };
        let change = close - prev;
        let gain = change.max(0.0);
        let loss = (-change).max(0.0);
        self.prev_close = Some(close);

        if self.avg_gain.is_none() {
            self.seed_gains += gain;
            self.seed_losses += loss;
            self.seed_count += 1;
            if self.seed_count == self.period {
                self.avg_gain = Some(self.seed_gains / self.period as f64);
                self.avg_loss = Some(self.seed_losses / self.period as f64);
                return Some(self.compute_rsi());
            }
            return None;
        }
        let p = self.period as f64;
        self.avg_gain = Some((self.avg_gain.unwrap() * (p - 1.0) + gain) / p);
        self.avg_loss = Some((self.avg_loss.unwrap() * (p - 1.0) + loss) / p);
        Some(self.compute_rsi())
    }

    fn compute_rsi(&self) -> f64 {
        let g = self.avg_gain.unwrap();
        let l = self.avg_loss.unwrap();
        if l == 0.0 {
            return 100.0;
        }
        let rs = g / l;
        100.0 - 100.0 / (1.0 + rs)
    }

    pub fn compute(values: &[f64], period: usize) -> Option<f64> {
        let mut r = Rsi::new(period);
        let mut last = None;
        for v in values {
            if let Some(x) = r.next(*v) {
                last = Some(x);
            }
        }
        last
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rsi_full_up_is_100() {
        let series: Vec<f64> = (1..=20).map(|i| i as f64).collect();
        let v = Rsi::compute(&series, 14).unwrap();
        approx::assert_abs_diff_eq!(v, 100.0, epsilon = 1e-9);
    }

    #[test]
    fn rsi_oscillating_in_range() {
        let s = [
            44.0, 44.34, 44.09, 44.15, 43.61, 44.33, 44.83, 45.10, 45.42, 45.84, 46.08, 45.89,
            46.03, 45.61, 46.28, 46.28,
        ];
        let v = Rsi::compute(&s, 14).unwrap();
        assert!(v > 50.0 && v < 100.0);
    }
}
