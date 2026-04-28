//! Performance metrics: WR, PF, Sharpe, Sortino, max drawdown.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PerformanceMetrics {
    pub trades: u32,
    pub wins: u32,
    pub losses: u32,
    pub win_rate: f64,
    pub gross_profit: f64,
    pub gross_loss: f64,
    pub net_pnl: f64,
    pub profit_factor: f64,
    pub avg_win: f64,
    pub avg_loss: f64,
    pub max_drawdown_pct: f64,
    pub sharpe: f64,
    pub sortino: f64,
    pub expectancy: f64,
}

impl PerformanceMetrics {
    /// Compute from a series of realized PnL per trade (in account currency).
    pub fn from_trades(pnls: &[f64]) -> Self {
        if pnls.is_empty() {
            return Self::default();
        }
        let wins: Vec<f64> = pnls.iter().copied().filter(|&x| x > 0.0).collect();
        let losses: Vec<f64> = pnls.iter().copied().filter(|&x| x < 0.0).collect();
        let trades = pnls.len() as u32;
        let gross_profit: f64 = wins.iter().copied().sum();
        let gross_loss_abs: f64 = losses.iter().copied().map(|x| -x).sum();
        let net = gross_profit - gross_loss_abs;
        let win_rate = wins.len() as f64 / trades as f64;
        let avg_win = if wins.is_empty() {
            0.0
        } else {
            gross_profit / wins.len() as f64
        };
        let avg_loss = if losses.is_empty() {
            0.0
        } else {
            gross_loss_abs / losses.len() as f64
        };
        let pf = if gross_loss_abs > 0.0 {
            gross_profit / gross_loss_abs
        } else {
            f64::INFINITY
        };
        let expectancy = net / trades as f64;

        // Max drawdown on equity curve.
        let mut equity = 0.0;
        let mut peak = 0.0;
        let mut max_dd = 0.0;
        let mut returns = Vec::with_capacity(pnls.len());
        for p in pnls {
            equity += p;
            if equity > peak {
                peak = equity;
            }
            let dd = if peak > 0.0 {
                (peak - equity) / peak * 100.0
            } else {
                0.0
            };
            if dd > max_dd {
                max_dd = dd;
            }
            returns.push(*p);
        }

        let mean_r = returns.iter().copied().sum::<f64>() / returns.len() as f64;
        let var = returns.iter().map(|x| (x - mean_r).powi(2)).sum::<f64>() / returns.len() as f64;
        let sd = var.sqrt();
        let sharpe = if sd > 0.0 {
            mean_r / sd * (returns.len() as f64).sqrt()
        } else {
            0.0
        };

        let downside_var = losses.iter().map(|x| x * x).sum::<f64>() / returns.len().max(1) as f64;
        let downside_sd = downside_var.sqrt();
        let sortino = if downside_sd > 0.0 {
            mean_r / downside_sd * (returns.len() as f64).sqrt()
        } else {
            0.0
        };

        Self {
            trades,
            wins: wins.len() as u32,
            losses: losses.len() as u32,
            win_rate,
            gross_profit,
            gross_loss: gross_loss_abs,
            net_pnl: net,
            profit_factor: pf,
            avg_win,
            avg_loss,
            max_drawdown_pct: max_dd,
            sharpe,
            sortino,
            expectancy,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_metrics() {
        let pnls = [10.0, -5.0, 20.0, -8.0, 15.0];
        let m = PerformanceMetrics::from_trades(&pnls);
        assert_eq!(m.trades, 5);
        assert_eq!(m.wins, 3);
        approx::assert_abs_diff_eq!(m.win_rate, 0.6, epsilon = 1e-9);
        approx::assert_abs_diff_eq!(m.net_pnl, 32.0, epsilon = 1e-9);
    }
}
