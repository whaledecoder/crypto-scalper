#[derive(Debug, Clone, Copy)]
pub struct KalmanTrend {
    pub estimate: f64,
    pub velocity: f64,
    covariance: f64,
    process_noise: f64,
    measurement_noise: f64,
}

impl KalmanTrend {
    pub fn new(initial_price: f64, process_noise: f64, measurement_noise: f64) -> Self {
        Self {
            estimate: initial_price,
            velocity: 0.0,
            covariance: 1.0,
            process_noise: process_noise.max(1e-12),
            measurement_noise: measurement_noise.max(1e-12),
        }
    }

    pub fn update(&mut self, price: f64) -> f64 {
        let prior = self.estimate + self.velocity;
        let prior_cov = self.covariance + self.process_noise;
        let gain = prior_cov / (prior_cov + self.measurement_noise);
        let updated = prior + gain * (price - prior);
        self.velocity = updated - self.estimate;
        self.estimate = updated;
        self.covariance = (1.0 - gain) * prior_cov;
        self.estimate
    }

    pub fn trend_score(&self, price: f64) -> f64 {
        if price <= 0.0 {
            return 0.0;
        }
        (self.velocity / price * 10_000.0).clamp(-100.0, 100.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracks_positive_trend() {
        let mut k = KalmanTrend::new(100.0, 0.01, 0.1);
        for price in [101.0, 102.0, 103.0, 104.0] {
            k.update(price);
        }
        assert!(k.velocity > 0.0);
        assert!(k.trend_score(104.0) > 0.0);
    }
}
