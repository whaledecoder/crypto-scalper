use std::collections::VecDeque;

#[derive(Debug, Clone)]
pub struct IcTracker {
    observations: Vec<(f64, f64)>,
    window_ics: VecDeque<f64>,
    window: usize,
}

impl IcTracker {
    pub fn new(window: usize) -> Self {
        Self {
            observations: Vec::new(),
            window_ics: VecDeque::new(),
            window: window.max(2),
        }
    }

    pub fn record(&mut self, signal_value: f64, forward_return: f64) {
        if !signal_value.is_finite() || !forward_return.is_finite() {
            return;
        }
        self.observations.push((signal_value, forward_return));
        if self.observations.len() >= self.window {
            let start = self.observations.len() - self.window;
            if let Some(ic) = pearson(&self.observations[start..]) {
                self.window_ics.push_back(ic);
            }
        }
    }

    pub fn len(&self) -> usize {
        self.observations.len()
    }

    pub fn is_empty(&self) -> bool {
        self.observations.is_empty()
    }

    pub fn ic(&self) -> Option<f64> {
        pearson(&self.observations)
    }

    pub fn ir(&self) -> Option<f64> {
        if self.window_ics.len() < 2 {
            return None;
        }
        let mean = self.window_ics.iter().sum::<f64>() / self.window_ics.len() as f64;
        let var = self
            .window_ics
            .iter()
            .map(|x| (x - mean).powi(2))
            .sum::<f64>()
            / self.window_ics.len() as f64;
        let sd = var.sqrt();
        if sd <= 0.0 {
            return None;
        }
        Some(mean / sd)
    }

    pub fn observations(&self) -> &[(f64, f64)] {
        &self.observations
    }
}

pub fn pearson(observations: &[(f64, f64)]) -> Option<f64> {
    if observations.len() < 3 {
        return None;
    }
    let n = observations.len() as f64;
    let mean_s = observations.iter().map(|(s, _)| *s).sum::<f64>() / n;
    let mean_r = observations.iter().map(|(_, r)| *r).sum::<f64>() / n;
    let cov = observations
        .iter()
        .map(|(s, r)| (s - mean_s) * (r - mean_r))
        .sum::<f64>()
        / n;
    let var_s = observations
        .iter()
        .map(|(s, _)| (s - mean_s).powi(2))
        .sum::<f64>()
        / n;
    let var_r = observations
        .iter()
        .map(|(_, r)| (r - mean_r).powi(2))
        .sum::<f64>()
        / n;
    let denom = var_s.sqrt() * var_r.sqrt();
    if denom <= 0.0 {
        return None;
    }
    Some((cov / denom).clamp(-1.0, 1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn computes_positive_ic_and_ir() {
        let mut tracker = IcTracker::new(4);
        for i in 1..=12 {
            let x = i as f64;
            tracker.record(x, x * 0.01);
        }
        assert_eq!(tracker.len(), 12);
        assert!(tracker.ic().unwrap() > 0.99);
        assert!(tracker.ir().unwrap().is_finite());
    }

    #[test]
    fn ignores_non_finite_observations() {
        let mut tracker = IcTracker::new(3);
        tracker.record(f64::NAN, 1.0);
        tracker.record(1.0, 1.0);
        assert_eq!(tracker.len(), 1);
    }
}
