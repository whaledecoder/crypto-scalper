use crate::backtest::{drawdown_confidence_intervals, BacktestResult};
use crate::research::report::{StrategyHealth, StrategyResearchSummary};

#[derive(Debug, Clone)]
pub struct ResearchReport {
    pub symbol: String,
    pub trades: u32,
    pub win_rate: f64,
    pub profit_factor: f64,
    pub net_pnl: f64,
    pub sharpe: f64,
    pub max_drawdown_pct: f64,
    pub monte_carlo_drawdown_p95: Option<f64>,
    pub monte_carlo_drawdown_p99: Option<f64>,
    pub health: StrategyHealth,
}

impl ResearchReport {
    pub fn from_backtest(result: &BacktestResult) -> Self {
        let pnls: Vec<f64> = result.trades.iter().map(|t| t.pnl).collect();
        let mc = drawdown_confidence_intervals(&pnls, 256);
        let summary =
            StrategyResearchSummary::new(result.symbol.clone(), result.metrics.clone(), None, None);
        Self {
            symbol: result.symbol.clone(),
            trades: result.metrics.trades,
            win_rate: result.metrics.win_rate,
            profit_factor: result.metrics.profit_factor,
            net_pnl: result.metrics.net_pnl,
            sharpe: result.metrics.sharpe,
            max_drawdown_pct: result.metrics.max_drawdown_pct,
            monte_carlo_drawdown_p95: mc.as_ref().map(|x| x.p95),
            monte_carlo_drawdown_p99: mc.as_ref().map(|x| x.p99),
            health: summary.health,
        }
    }
}

pub fn reports_to_markdown(reports: &[ResearchReport]) -> String {
    let mut out = String::from(
        "| Symbol | Trades | Win rate | PF | Net PnL | Sharpe | Max DD | MC DD p95 | Health |\n\
         |---|---:|---:|---:|---:|---:|---:|---:|---|\n",
    );
    for r in reports {
        out.push_str(&format!(
            "| {} | {} | {:.2}% | {:.2} | {:.2} | {:.2} | {:.2}% | {} | {:?} |\n",
            r.symbol,
            r.trades,
            r.win_rate * 100.0,
            r.profit_factor,
            r.net_pnl,
            r.sharpe,
            r.max_drawdown_pct,
            r.monte_carlo_drawdown_p95
                .map(|x| format!("{x:.2}%"))
                .unwrap_or_else(|| "n/a".into()),
            r.health
        ));
    }
    out
}

pub fn reports_to_json(reports: &[ResearchReport]) -> String {
    let rows: Vec<serde_json::Value> = reports
        .iter()
        .map(|r| {
            serde_json::json!({
                "symbol": r.symbol,
                "trades": r.trades,
                "win_rate": json_f64(r.win_rate),
                "profit_factor": json_f64(r.profit_factor),
                "net_pnl": json_f64(r.net_pnl),
                "sharpe": json_f64(r.sharpe),
                "max_drawdown_pct": json_f64(r.max_drawdown_pct),
                "monte_carlo_drawdown_p95": json_opt_f64(r.monte_carlo_drawdown_p95),
                "monte_carlo_drawdown_p99": json_opt_f64(r.monte_carlo_drawdown_p99),
                "health": format!("{:?}", r.health),
            })
        })
        .collect();
    serde_json::to_string_pretty(&rows).unwrap_or_else(|_| "[]".into())
}

fn json_f64(value: f64) -> serde_json::Value {
    if value.is_finite() {
        serde_json::json!(value)
    } else {
        serde_json::json!(format!("{value}"))
    }
}

fn json_opt_f64(value: Option<f64>) -> serde_json::Value {
    match value {
        Some(x) => json_f64(x),
        None => serde_json::Value::Null,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backtest::{BacktestResult, PerformanceMetrics};

    #[test]
    fn renders_markdown_report() {
        let result = BacktestResult {
            symbol: "BTCUSDT".into(),
            trades: Vec::new(),
            metrics: PerformanceMetrics {
                trades: 1,
                win_rate: 1.0,
                profit_factor: f64::INFINITY,
                ..PerformanceMetrics::default()
            },
        };
        let report = ResearchReport::from_backtest(&result);
        assert!(reports_to_markdown(&[report]).contains("BTCUSDT"));
    }

    #[test]
    fn json_report_preserves_non_finite_metrics_as_strings() {
        let report = ResearchReport {
            symbol: "BTCUSDT".into(),
            trades: 3,
            win_rate: 1.0,
            profit_factor: f64::INFINITY,
            net_pnl: 10.0,
            sharpe: f64::NAN,
            max_drawdown_pct: 0.0,
            monte_carlo_drawdown_p95: Some(f64::INFINITY),
            monte_carlo_drawdown_p99: None,
            health: StrategyHealth::Observe,
        };
        let json = reports_to_json(&[report]);
        assert!(json.contains("\"profit_factor\": \"inf\""));
        assert!(json.contains("\"sharpe\": \"NaN\""));
        assert!(json.contains("\"monte_carlo_drawdown_p95\": \"inf\""));
    }
}
