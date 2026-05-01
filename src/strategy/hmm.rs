use crate::strategy::Regime;

#[derive(Debug, Clone)]
pub struct HmmRegimeModel {
    states: Vec<Regime>,
    transition: Vec<Vec<f64>>,
    emission_mean: Vec<f64>,
    emission_std: Vec<f64>,
    prior: Vec<f64>,
}

impl HmmRegimeModel {
    pub fn new(
        states: Vec<Regime>,
        transition: Vec<Vec<f64>>,
        emission_mean: Vec<f64>,
        emission_std: Vec<f64>,
        prior: Vec<f64>,
    ) -> Option<Self> {
        let n = states.len();
        if n == 0
            || transition.len() != n
            || transition.iter().any(|row| row.len() != n)
            || emission_mean.len() != n
            || emission_std.len() != n
            || prior.len() != n
        {
            return None;
        }
        Some(Self {
            states,
            transition,
            emission_mean,
            emission_std,
            prior: normalize(&prior),
        })
    }

    pub fn infer(&self, observations: &[f64]) -> Vec<(Regime, f64)> {
        if observations.is_empty() {
            return self
                .states
                .iter()
                .copied()
                .zip(self.prior.iter().copied())
                .collect();
        }
        let n = self.states.len();
        let mut probs = self.prior.clone();
        for obs in observations {
            let mut next = vec![0.0; n];
            for (to, slot) in next.iter_mut().enumerate() {
                let transition_prob = (0..n)
                    .map(|from| probs[from] * self.transition[from][to])
                    .sum::<f64>();
                *slot = transition_prob
                    * gaussian_likelihood(*obs, self.emission_mean[to], self.emission_std[to]);
            }
            probs = normalize(&next);
        }
        self.states.iter().copied().zip(probs).collect()
    }

    pub fn most_likely(&self, observations: &[f64]) -> Option<(Regime, f64)> {
        self.infer(observations)
            .into_iter()
            .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
    }
}

fn gaussian_likelihood(x: f64, mean: f64, std: f64) -> f64 {
    let std = std.max(1e-9);
    let z = (x - mean) / std;
    (-0.5 * z * z).exp() / (std * (2.0 * std::f64::consts::PI).sqrt())
}

fn normalize(values: &[f64]) -> Vec<f64> {
    let sum: f64 = values
        .iter()
        .copied()
        .filter(|x| x.is_finite() && *x > 0.0)
        .sum();
    if sum <= 0.0 {
        return vec![1.0 / values.len() as f64; values.len()];
    }
    values
        .iter()
        .map(|x| {
            if x.is_finite() && *x > 0.0 {
                *x / sum
            } else {
                0.0
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn infers_high_vol_regime() {
        let model = HmmRegimeModel::new(
            vec![Regime::Ranging, Regime::Volatile],
            vec![vec![0.9, 0.1], vec![0.2, 0.8]],
            vec![0.01, 0.05],
            vec![0.01, 0.02],
            vec![0.5, 0.5],
        )
        .unwrap();
        let (regime, probability) = model.most_likely(&[0.06, 0.05, 0.04]).unwrap();
        assert_eq!(regime, Regime::Volatile);
        assert!(probability > 0.5);
    }
}
