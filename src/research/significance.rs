use crate::research::ic::pearson;

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
}
