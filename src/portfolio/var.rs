pub fn historical_var(returns: &[f64], confidence: f64) -> Option<f64> {
    if returns.is_empty() || !(0.0..1.0).contains(&confidence) {
        return None;
    }
    let mut sorted = returns.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let tail = 1.0 - confidence;
    let idx = ((sorted.len() as f64 * tail).floor() as usize).min(sorted.len() - 1);
    Some(-sorted[idx].min(0.0))
}

pub fn historical_cvar(returns: &[f64], confidence: f64) -> Option<f64> {
    if returns.is_empty() || !(0.0..1.0).contains(&confidence) {
        return None;
    }
    let mut sorted = returns.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let tail = 1.0 - confidence;
    let count = ((sorted.len() as f64 * tail).ceil() as usize)
        .max(1)
        .min(sorted.len());
    let avg = sorted.iter().take(count).sum::<f64>() / count as f64;
    Some(-avg.min(0.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn computes_var_and_cvar() {
        let returns = [-0.05, -0.02, 0.01, 0.03, 0.04];
        assert!(historical_var(&returns, 0.8).unwrap() >= 0.02);
        assert!(historical_cvar(&returns, 0.8).unwrap() >= historical_var(&returns, 0.8).unwrap());
    }
}
