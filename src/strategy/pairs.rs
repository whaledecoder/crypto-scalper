#[derive(Debug, Clone, Copy)]
pub struct HedgeRatio {
    pub beta: f64,
    pub intercept: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairSignal {
    LongSpread,
    ShortSpread,
    HoldSpread,
    Flat,
}

pub fn estimate_hedge_ratio(base: &[f64], hedge: &[f64]) -> Option<HedgeRatio> {
    if base.len() != hedge.len() || base.len() < 3 {
        return None;
    }
    let n = base.len() as f64;
    let mean_x = hedge.iter().sum::<f64>() / n;
    let mean_y = base.iter().sum::<f64>() / n;
    let var_x = hedge.iter().map(|x| (x - mean_x).powi(2)).sum::<f64>();
    if var_x <= 0.0 {
        return None;
    }
    let cov = base
        .iter()
        .zip(hedge.iter())
        .map(|(y, x)| (x - mean_x) * (y - mean_y))
        .sum::<f64>();
    let beta = cov / var_x;
    Some(HedgeRatio {
        beta,
        intercept: mean_y - beta * mean_x,
    })
}

pub fn spread_zscore(base: &[f64], hedge: &[f64], ratio: HedgeRatio) -> Option<f64> {
    if base.len() != hedge.len() || base.len() < 3 {
        return None;
    }
    let spreads: Vec<f64> = base
        .iter()
        .zip(hedge.iter())
        .map(|(b, h)| b - (ratio.intercept + ratio.beta * h))
        .collect();
    let last = *spreads.last()?;
    let mean = spreads.iter().sum::<f64>() / spreads.len() as f64;
    let var = spreads.iter().map(|s| (s - mean).powi(2)).sum::<f64>() / spreads.len() as f64;
    let std = var.sqrt();
    if std <= 0.0 {
        return None;
    }
    Some((last - mean) / std)
}

pub fn pair_signal(zscore: f64, entry_z: f64, exit_z: f64) -> PairSignal {
    if zscore >= entry_z {
        PairSignal::ShortSpread
    } else if zscore <= -entry_z {
        PairSignal::LongSpread
    } else if zscore.abs() <= exit_z {
        PairSignal::Flat
    } else {
        PairSignal::HoldSpread
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estimates_pair_spread() {
        let hedge = [100.0, 101.0, 102.0, 103.0, 104.0, 105.0];
        let base = [200.0, 202.0, 204.0, 206.0, 208.0, 215.0];
        let ratio = estimate_hedge_ratio(&base, &hedge).unwrap();
        assert!(ratio.beta > 2.0);
        assert!(spread_zscore(&base, &hedge, ratio).unwrap().is_finite());
    }
}
