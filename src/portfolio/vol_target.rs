pub fn volatility_target_multiplier(
    target_vol: f64,
    realized_vol: f64,
    max_multiplier: f64,
) -> f64 {
    if target_vol <= 0.0 || realized_vol <= 0.0 || max_multiplier <= 0.0 {
        return 0.0;
    }
    (target_vol / realized_vol).min(max_multiplier)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scales_down_high_volatility() {
        approx::assert_abs_diff_eq!(volatility_target_multiplier(0.10, 0.20, 2.0), 0.5);
        approx::assert_abs_diff_eq!(volatility_target_multiplier(0.10, 0.02, 2.0), 2.0);
    }
}
