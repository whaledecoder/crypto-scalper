//! Risk manager — position sizing, circuit breakers, daily loss / drawdown limits.

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RiskSnapshot {
    pub equity: f64,
    pub peak_equity: f64,
    pub open_positions: u32,
    pub realized_pnl_today: f64,
    pub daily_loss_pct: f64,
    pub drawdown_pct: f64,
    pub tripped: bool,
    pub trip_reason: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RiskLimits {
    pub risk_per_trade_pct: f64,
    pub max_open_positions: u32,
    pub max_daily_loss_pct: f64,
    pub max_drawdown_pct: f64,
    pub max_leverage: u32,
    pub max_spread_pct: f64,
}

#[derive(Debug)]
struct Inner {
    limits: RiskLimits,
    equity: f64,
    peak_equity: f64,
    open_positions: u32,
    realized_pnl_today: f64,
    tripped: bool,
    trip_reason: Option<String>,
}

#[derive(Clone)]
pub struct RiskManager {
    inner: Arc<Mutex<Inner>>,
}

impl RiskManager {
    pub fn new(limits: RiskLimits, equity: f64) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                limits,
                equity,
                peak_equity: equity,
                open_positions: 0,
                realized_pnl_today: 0.0,
                tripped: false,
                trip_reason: None,
            })),
        }
    }

    pub fn snapshot(&self) -> RiskSnapshot {
        let i = self.inner.lock();
        let dd = if i.peak_equity > 0.0 {
            ((i.peak_equity - i.equity) / i.peak_equity * 100.0).max(0.0)
        } else {
            0.0
        };
        let daily_loss_pct = if i.equity > 0.0 && i.realized_pnl_today < 0.0 {
            -i.realized_pnl_today / i.equity * 100.0
        } else {
            0.0
        };
        RiskSnapshot {
            equity: i.equity,
            peak_equity: i.peak_equity,
            open_positions: i.open_positions,
            realized_pnl_today: i.realized_pnl_today,
            daily_loss_pct,
            drawdown_pct: dd,
            tripped: i.tripped,
            trip_reason: i.trip_reason.clone(),
        }
    }

    pub fn limits(&self) -> RiskLimits {
        self.inner.lock().limits.clone()
    }

    /// True iff a new position can be opened under current risk state.
    pub fn can_open_position(&self) -> std::result::Result<(), String> {
        let i = self.inner.lock();
        if i.tripped {
            return Err(format!(
                "circuit tripped: {}",
                i.trip_reason.clone().unwrap_or_default()
            ));
        }
        if i.open_positions >= i.limits.max_open_positions {
            return Err(format!(
                "open positions {} / {}",
                i.open_positions, i.limits.max_open_positions
            ));
        }
        let dd = if i.peak_equity > 0.0 {
            ((i.peak_equity - i.equity) / i.peak_equity * 100.0).max(0.0)
        } else {
            0.0
        };
        if dd >= i.limits.max_drawdown_pct {
            return Err(format!("drawdown {dd:.2}% >= limit"));
        }
        if i.realized_pnl_today < 0.0
            && (-i.realized_pnl_today / i.equity * 100.0) >= i.limits.max_daily_loss_pct
        {
            return Err("daily loss limit reached".into());
        }
        Ok(())
    }

    /// Calculate qty so that (entry - sl).abs() * qty == equity * risk%.
    pub fn calculate_size(&self, entry: f64, stop_loss: f64) -> f64 {
        let i = self.inner.lock();
        let risk_amount = i.equity * i.limits.risk_per_trade_pct / 100.0;
        let risk_per_unit = (entry - stop_loss).abs();
        if risk_per_unit <= 0.0 {
            return 0.0;
        }
        let raw = risk_amount / risk_per_unit;
        let leverage_cap = i.equity * i.limits.max_leverage as f64 / entry.max(1e-9);
        raw.min(leverage_cap)
    }

    pub fn on_position_opened(&self) {
        self.inner.lock().open_positions += 1;
    }

    pub fn on_position_closed(&self, realized_pnl: f64) {
        let mut i = self.inner.lock();
        if i.open_positions > 0 {
            i.open_positions -= 1;
        }
        i.realized_pnl_today += realized_pnl;
        i.equity += realized_pnl;
        if i.equity > i.peak_equity {
            i.peak_equity = i.equity;
        }
        let dd = if i.peak_equity > 0.0 {
            ((i.peak_equity - i.equity) / i.peak_equity * 100.0).max(0.0)
        } else {
            0.0
        };
        if dd >= i.limits.max_drawdown_pct && !i.tripped {
            i.tripped = true;
            i.trip_reason = Some(format!("max drawdown {dd:.2}%"));
        }
        let loss_pct = if i.equity > 0.0 && i.realized_pnl_today < 0.0 {
            -i.realized_pnl_today / i.equity * 100.0
        } else {
            0.0
        };
        if loss_pct >= i.limits.max_daily_loss_pct && !i.tripped {
            i.tripped = true;
            i.trip_reason = Some(format!("daily loss {loss_pct:.2}%"));
        }
    }

    pub fn reset_daily(&self) {
        let mut i = self.inner.lock();
        i.realized_pnl_today = 0.0;
        i.tripped = false;
        i.trip_reason = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_limits() -> RiskLimits {
        RiskLimits {
            risk_per_trade_pct: 1.0,
            max_open_positions: 3,
            max_daily_loss_pct: 3.0,
            max_drawdown_pct: 10.0,
            max_leverage: 5,
            max_spread_pct: 0.03,
        }
    }

    #[test]
    fn size_calculation() {
        let r = RiskManager::new(default_limits(), 10_000.0);
        let size = r.calculate_size(100.0, 99.0);
        // 1% of 10000 = 100 USD risk; risk per unit = 1 → size = 100
        approx::assert_abs_diff_eq!(size, 100.0, epsilon = 1e-6);
    }

    #[test]
    fn circuit_trips_on_daily_loss() {
        let r = RiskManager::new(default_limits(), 1000.0);
        r.on_position_closed(-40.0); // 4% loss > 3% limit
        let s = r.snapshot();
        assert!(s.tripped);
        assert!(r.can_open_position().is_err());
    }

    #[test]
    fn circuit_trips_on_drawdown() {
        let r = RiskManager::new(default_limits(), 1000.0);
        r.on_position_closed(100.0); // peak 1100
        r.on_position_closed(-120.0); // dd ~ 11%
        let s = r.snapshot();
        assert!(s.tripped);
    }
}
