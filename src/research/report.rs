use crate::backtest::metrics::PerformanceMetrics;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StrategyHealth {
    Promote,
    Observe,
    Retire,
}

#[derive(Debug, Clone)]
pub struct StrategyResearchSummary {
    pub strategy: String,
    pub metrics: PerformanceMetrics,
    pub ic: Option<f64>,
    pub p_value: Option<f64>,
    pub health: StrategyHealth,
}

impl StrategyResearchSummary {
    pub fn new(
        strategy: impl Into<String>,
        metrics: PerformanceMetrics,
        ic: Option<f64>,
        p_value: Option<f64>,
    ) -> Self {
        let health = classify_health(&metrics, ic, p_value);
        Self {
            strategy: strategy.into(),
            metrics,
            ic,
            p_value,
            health,
        }
    }
}

pub fn classify_health(
    metrics: &PerformanceMetrics,
    ic: Option<f64>,
    p_value: Option<f64>,
) -> StrategyHealth {
    if metrics.trades >= 30
        && (metrics.profit_factor < 1.0
            || metrics.sharpe < 0.0
            || p_value.map(|p| p > 0.20).unwrap_or(false))
    {
        return StrategyHealth::Retire;
    }
    if metrics.trades >= 30
        && metrics.profit_factor >= 1.2
        && metrics.sharpe > 0.5
        && ic.map(|x| x > 0.03).unwrap_or(false)
        && p_value.map(|p| p < 0.05).unwrap_or(false)
    {
        return StrategyHealth::Promote;
    }
    StrategyHealth::Observe
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn promotes_statistically_valid_strategy() {
        let metrics = PerformanceMetrics {
            trades: 40,
            profit_factor: 1.4,
            sharpe: 0.8,
            ..PerformanceMetrics::default()
        };
        let summary = StrategyResearchSummary::new("momentum", metrics, Some(0.05), Some(0.01));
        assert_eq!(summary.health, StrategyHealth::Promote);
    }
}
