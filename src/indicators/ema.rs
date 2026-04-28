//! Exponential Moving Average — incremental implementation.

#[derive(Debug, Clone)]
pub struct Ema {
    period: usize,
    alpha: f64,
    value: Option<f64>,
    seed_sum: f64,
    seed_count: usize,
}

impl Ema {
    pub fn new(period: usize) -> Self {
        assert!(period > 0, "period must be > 0");
        Self {
            period,
            alpha: 2.0 / (period as f64 + 1.0),
            value: None,
            seed_sum: 0.0,
            seed_count: 0,
        }
    }

    /// Feed the next observation. Returns current EMA once seeded.
    pub fn next(&mut self, x: f64) -> Option<f64> {
        if self.value.is_none() {
            // Seed with SMA over the first `period` observations.
            self.seed_sum += x;
            self.seed_count += 1;
            if self.seed_count == self.period {
                self.value = Some(self.seed_sum / self.period as f64);
            }
            return self.value;
        }
        let prev = self.value.unwrap();
        let new = self.alpha * x + (1.0 - self.alpha) * prev;
        self.value = Some(new);
        Some(new)
    }

    pub fn value(&self) -> Option<f64> {
        self.value
    }

    pub fn period(&self) -> usize {
        self.period
    }

    /// Convenience: EMA of a full slice.
    pub fn compute(values: &[f64], period: usize) -> Option<f64> {
        let mut e = Ema::new(period);
        let mut last = None;
        for v in values {
            last = e.next(*v);
        }
        last
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ema_matches_pandas_style() {
        // EMA(3) of [1,2,3,4,5] with SMA seed:
        // seed at i=2: (1+2+3)/3 = 2.0
        // i=3: 0.5*4 + 0.5*2 = 3.0
        // i=4: 0.5*5 + 0.5*3 = 4.0
        let e = Ema::compute(&[1.0, 2.0, 3.0, 4.0, 5.0], 3).unwrap();
        approx::assert_abs_diff_eq!(e, 4.0, epsilon = 1e-9);
    }

    #[test]
    fn ema_needs_period_values_to_seed() {
        let mut e = Ema::new(4);
        assert!(e.next(1.0).is_none());
        assert!(e.next(2.0).is_none());
        assert!(e.next(3.0).is_none());
        let v = e.next(4.0);
        assert!(v.is_some());
    }
}
