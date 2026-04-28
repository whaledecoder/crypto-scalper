//! Minimal backtest runner. Replays candles through all configured strategies
//! and simulates SL/TP fills on the next candle.

use crate::backtest::metrics::PerformanceMetrics;
use crate::data::{Candle, Side};
use crate::errors::Result;
use crate::strategy::ema_ribbon::EmaRibbon;
use crate::strategy::mean_reversion::MeanReversion;
use crate::strategy::momentum::Momentum;
use crate::strategy::squeeze::Squeeze;
use crate::strategy::state::{PreSignal, StrategyName, SymbolState};
use crate::strategy::vwap_scalp::VwapScalp;
use crate::strategy::{select_strategies, RegimeDetector, Strategy};
use serde::{Deserialize, Serialize};
use tracing::info;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimTrade {
    pub symbol: String,
    pub strategy: String,
    pub side: String,
    pub entry: f64,
    pub exit: f64,
    pub pnl: f64,
    pub pnl_pct: f64,
    pub bars_held: u32,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BacktestResult {
    pub symbol: String,
    pub trades: Vec<SimTrade>,
    pub metrics: PerformanceMetrics,
}

pub struct BacktestEngine {
    pub symbol: String,
    pub active: Vec<StrategyName>,
    pub min_ta_confidence: u8,
    pub risk_per_trade_usd: f64,
}

impl BacktestEngine {
    pub fn run(&self, candles: &[Candle]) -> Result<BacktestResult> {
        let mut state = SymbolState::new(&self.symbol);
        let mut open: Option<(PreSignal, u32)> = None;
        let mut sim_trades: Vec<SimTrade> = Vec::new();

        for (i, c) in candles.iter().enumerate() {
            state.on_closed(*c);

            // Exit check first
            if let Some((sig, bars)) = open.clone() {
                let (exit_price, exit_reason) = match sig.side {
                    Side::Long => {
                        if c.low <= sig.stop_loss {
                            (sig.stop_loss, "SL".to_string())
                        } else if c.high >= sig.take_profit {
                            (sig.take_profit, "TP".to_string())
                        } else {
                            // Noop — still open
                            open = Some((sig.clone(), bars + 1));
                            continue;
                        }
                    }
                    Side::Short => {
                        if c.high >= sig.stop_loss {
                            (sig.stop_loss, "SL".to_string())
                        } else if c.low <= sig.take_profit {
                            (sig.take_profit, "TP".to_string())
                        } else {
                            open = Some((sig.clone(), bars + 1));
                            continue;
                        }
                    }
                };
                let size = self.risk_per_trade_usd / (sig.entry - sig.stop_loss).abs().max(1e-9);
                let pnl = match sig.side {
                    Side::Long => (exit_price - sig.entry) * size,
                    Side::Short => (sig.entry - exit_price) * size,
                };
                let pnl_pct = match sig.side {
                    Side::Long => (exit_price / sig.entry - 1.0) * 100.0,
                    Side::Short => (sig.entry / exit_price - 1.0) * 100.0,
                };
                sim_trades.push(SimTrade {
                    symbol: sig.symbol.clone(),
                    strategy: sig.strategy.as_str().to_string(),
                    side: sig.side.as_str().to_string(),
                    entry: sig.entry,
                    exit: exit_price,
                    pnl,
                    pnl_pct,
                    bars_held: bars + 1,
                    reason: exit_reason,
                });
                open = None;
            }

            // Only look for new signal if no open position
            if open.is_some() {
                continue;
            }
            if i < 205 {
                continue; // warmup indicators
            }
            let regime = RegimeDetector::detect(&state);
            let chosen = select_strategies(&self.active, regime);

            for name in chosen {
                let sig = match name {
                    StrategyName::EmaRibbon => EmaRibbon.evaluate(&state, c),
                    StrategyName::MeanReversion => MeanReversion.evaluate(&state, c),
                    StrategyName::Momentum => Momentum.evaluate(&state, c),
                    StrategyName::VwapScalp => VwapScalp.evaluate(&state, c),
                    StrategyName::Squeeze => Squeeze.evaluate(&state, c),
                };
                if let Some(s) = sig {
                    if s.ta_confidence >= self.min_ta_confidence {
                        open = Some((s, 0));
                        break;
                    }
                }
            }
        }

        let pnls: Vec<f64> = sim_trades.iter().map(|t| t.pnl).collect();
        let metrics = PerformanceMetrics::from_trades(&pnls);
        info!(
            symbol = %self.symbol,
            trades = sim_trades.len(),
            wr = %format!("{:.2}", metrics.win_rate * 100.0),
            pf = %format!("{:.2}", metrics.profit_factor),
            net = %format!("{:.2}", metrics.net_pnl),
            "backtest complete"
        );
        Ok(BacktestResult {
            symbol: self.symbol.clone(),
            trades: sim_trades,
            metrics,
        })
    }
}
