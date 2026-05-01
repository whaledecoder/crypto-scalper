use crate::research::ic::pearson;

pub fn win_rate_significance(wins: u32, total: u32) -> Option<f64> {
    if total == 0 || wins > total {
        return None;
    }
    let observed = wins.max(total - wins);
    let mut tail = 0.0;
    for k in observed..=total {
        tail += binomial_probability(total, k, 0.5)?;
    }
    Some((tail * 2.0).min(1.0))
}

pub fn permutation_p_value(observations: &[(f64, f64)], permutations: usize) -> Option<f64> {
    if observations.len() < 4 || permutations == 0 {
        return None;
    }
    let observed = pearson(observations)?.abs();
    let mut shuffled: Vec<(f64, f64)> = observations.to_vec();
    let mut extreme = 0usize;
    for step in 0..permutations {
        rotate_returns(&mut shuffled, step + 1);
        if pearson(&shuffled).unwrap_or(0.0).abs() >= observed {
            extreme += 1;
        }
    }
    Some((extreme as f64 + 1.0) / (permutations as f64 + 1.0))
}

fn binomial_probability(n: u32, k: u32, p: f64) -> Option<f64> {
    if k > n || !(0.0..=1.0).contains(&p) {
        return None;
    }
    let k_small = k.min(n - k);
    let mut coeff = 1.0;
    for i in 0..k_small {
        coeff *= (n - i) as f64 / (i + 1) as f64;
    }
    Some(coeff * p.powi(k as i32) * (1.0 - p).powi((n - k) as i32))
}

fn rotate_returns(values: &mut [(f64, f64)], shift: usize) {
    let n = values.len();
    if n == 0 {
        return;
    }
    let returns: Vec<f64> = values.iter().map(|(_, r)| *r).collect();
    for (i, (_, r)) in values.iter_mut().enumerate() {
        *r = returns[(i + shift) % n];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_valid_p_value() {
        let observations: Vec<(f64, f64)> = (0..20)
            .map(|i| {
                let x = i as f64;
                (x, x * 0.01)
            })
            .collect();
        let p = permutation_p_value(&observations, 19).unwrap();
        assert!(p > 0.0 && p <= 1.0);
    }

    #[test]
    fn computes_win_rate_significance() {
        let p = win_rate_significance(8, 10).unwrap();
        assert!(p > 0.0 && p <= 1.0);
        assert_eq!(win_rate_significance(11, 10), None);
    }

    #[test]
    fn binomial_probability_preserves_original_success_count() {
        let p = binomial_probability(10, 8, 0.3).unwrap();
        approx::assert_abs_diff_eq!(p, 0.0014467005, epsilon = 1e-12);
    }
}
