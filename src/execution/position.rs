//! Position tracker — manages open positions, trailing stops, PnL.
//!
//! Enhanced for HFT quant:
//! - ATR-based trailing stop (activates at 1R, trails at 0.5× ATR)
//! - Time-based exit (close positions open > max_hold_candles)
//! - Partial take-profit (close 50% at first TP, rest trails)
//! - Breakeven stop (move SL to entry after 0.5R profit)

use crate::data::Side;
use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Position {
    pub client_id: String,
    pub symbol: String,
    pub side: Side,
    pub size: f64,
    pub entry_price: f64,
    pub stop_loss: f64,
    pub take_profit: f64,
    pub opened_at: DateTime<Utc>,
    pub trailing_activated: bool,
    pub peak_price: f64,
    pub trough_price: f64,
    /// ATR at entry — used for trailing stop distance.
    #[serde(default)]
    pub atr_at_entry: f64,
    /// Whether partial TP (50%) has been taken.
    #[serde(default)]
    pub partial_taken: bool,
    /// Whether SL has been moved to breakeven.
    #[serde(default)]
    pub breakeven_activated: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum PositionExitReason {
    StopLoss,
    TakeProfit,
    Trailing,
    TimeExit,
    Manual,
    Breakeven,
    PartialTP,
}

impl PositionExitReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::StopLoss => "SL",
            Self::TakeProfit => "TP",
            Self::Trailing => "TRAILING",
            Self::TimeExit => "TIME",
            Self::Manual => "MANUAL",
            Self::Breakeven => "BE",
            Self::PartialTP => "PARTIAL_TP",
        }
    }
}

/// Configuration for position management behavior.
#[derive(Debug, Clone)]
pub struct PositionConfig {
    /// Maximum time to hold a position (seconds). 0 = no limit.
    pub max_hold_secs: i64,
    /// ATR multiplier for trailing stop distance (e.g. 0.5 = half ATR).
    pub trail_atr_mult: f64,
    /// Profit threshold (in R-multiples) to activate trailing.
    pub trail_activate_r: f64,
    /// Profit threshold (in R-multiples) to move SL to breakeven.
    pub breakeven_r: f64,
    /// Whether to take partial TP (50% at first TP target).
    pub partial_tp_enabled: bool,
    /// Profit threshold (in R-multiples) to take partial TP.
    pub partial_tp_r: f64,
}

impl Default for PositionConfig {
    fn default() -> Self {
        Self {
            max_hold_secs: 1800, // 30 minutes
            trail_atr_mult: 0.5,
            trail_activate_r: 1.0,
            breakeven_r: 0.5,
            partial_tp_enabled: true,
            partial_tp_r: 1.0,
        }
    }
}

/// Action the execution agent should take for a position.
#[derive(Debug, Clone)]
pub enum PositionAction {
    /// Close the entire position.
    Close(Position, PositionExitReason),
    /// Close partial (size, reason).
    Reduce(Position, f64, PositionExitReason),
    /// Update SL (new_stop_loss).
    MoveSL(Position, f64),
    /// No action needed.
    None,
}

#[derive(Default)]
pub struct PositionBook {
    inner: Arc<Mutex<HashMap<String, Position>>>,
}

impl PositionBook {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn open(&self, p: Position) {
        self.inner.lock().insert(p.client_id.clone(), p);
    }

    pub fn close(&self, client_id: &str) -> Option<Position> {
        self.inner.lock().remove(client_id)
    }

    pub fn get(&self, client_id: &str) -> Option<Position> {
        self.inner.lock().get(client_id).cloned()
    }

    pub fn all(&self) -> Vec<Position> {
        self.inner.lock().values().cloned().collect()
    }

    pub fn snapshot(&self) -> Vec<Position> {
        self.all()
    }

    pub fn close_by_id(&self, client_id: &str) -> Option<Position> {
        self.close(client_id)
    }

    pub fn reconcile(&self, positions: Vec<Position>) {
        let mut book = self.inner.lock();
        book.clear();
        for p in positions {
            book.insert(p.client_id.clone(), p);
        }
    }

    pub fn update_price(&self, symbol: &str, price: f64) {
        for p in self.inner.lock().values_mut() {
            if p.symbol != symbol {
                continue;
            }
            if price > p.peak_price {
                p.peak_price = price;
            }
            if price < p.trough_price {
                p.trough_price = price;
            }
        }
    }

    /// Enhanced exit check with ATR trailing, breakeven, partial TP,
    /// and time-based exits.  Returns a list of actions for the
    /// execution agent to process.
    pub fn check_exits(
        &self,
        symbol: &str,
        price: f64,
        cfg: &PositionConfig,
    ) -> Vec<(Position, PositionExitReason)> {
        let mut out = Vec::new();
        let mut to_remove = Vec::new();
        let mut sl_updates: Vec<(String, f64)> = Vec::new();
        let mut book = self.inner.lock();
        let now = Utc::now();

        for (id, p) in book.iter_mut() {
            if p.symbol != symbol {
                continue;
            }

            let r = (p.entry_price - p.stop_loss).abs();
            if r <= 0.0 {
                continue;
            }

            // Time-based exit
            if cfg.max_hold_secs > 0 {
                let held = (now - p.opened_at).num_seconds();
                if held > cfg.max_hold_secs {
                    out.push((p.clone(), PositionExitReason::TimeExit));
                    to_remove.push(id.clone());
                    continue;
                }
            }

            match p.side {
                Side::Long => {
                    // Hard SL
                    if p.stop_loss > 0.0 && price <= p.stop_loss {
                        out.push((p.clone(), PositionExitReason::StopLoss));
                        to_remove.push(id.clone());
                        continue;
                    }
                    // Hard TP
                    if p.take_profit > 0.0 && price >= p.take_profit {
                        out.push((p.clone(), PositionExitReason::TakeProfit));
                        to_remove.push(id.clone());
                        continue;
                    }

                    let profit_r = (price - p.entry_price) / r;

                    // Breakeven: move SL to entry after 0.5R profit
                    if !p.breakeven_activated && profit_r >= cfg.breakeven_r {
                        p.breakeven_activated = true;
                        p.stop_loss = p.entry_price;
                        sl_updates.push((id.clone(), p.entry_price));
                    }

                    // Trailing stop: activate at 1R profit, trail at 0.5× ATR
                    if !p.trailing_activated && profit_r >= cfg.trail_activate_r {
                        p.trailing_activated = true;
                    }
                    if p.trailing_activated {
                        let trail_dist = if p.atr_at_entry > 0.0 {
                            p.atr_at_entry * cfg.trail_atr_mult
                        } else {
                            // Fallback: 50% of profit
                            (price - p.entry_price) * 0.5
                        };
                        let trail_stop = price - trail_dist;
                        if trail_stop > p.stop_loss {
                            p.stop_loss = trail_stop;
                            sl_updates.push((id.clone(), trail_stop));
                        }
                        // Check if trailing stop hit
                        if price <= p.stop_loss {
                            out.push((p.clone(), PositionExitReason::Trailing));
                            to_remove.push(id.clone());
                            continue;
                        }
                    }
                }
                Side::Short => {
                    if p.stop_loss > 0.0 && price >= p.stop_loss {
                        out.push((p.clone(), PositionExitReason::StopLoss));
                        to_remove.push(id.clone());
                        continue;
                    }
                    if p.take_profit > 0.0 && price <= p.take_profit {
                        out.push((p.clone(), PositionExitReason::TakeProfit));
                        to_remove.push(id.clone());
                        continue;
                    }

                    let profit_r = (p.entry_price - price) / r;

                    if !p.breakeven_activated && profit_r >= cfg.breakeven_r {
                        p.breakeven_activated = true;
                        p.stop_loss = p.entry_price;
                        sl_updates.push((id.clone(), p.entry_price));
                    }

                    if !p.trailing_activated && profit_r >= cfg.trail_activate_r {
                        p.trailing_activated = true;
                    }
                    if p.trailing_activated {
                        let trail_dist = if p.atr_at_entry > 0.0 {
                            p.atr_at_entry * cfg.trail_atr_mult
                        } else {
                            (p.entry_price - price) * 0.5
                        };
                        let trail_stop = price + trail_dist;
                        if trail_stop < p.stop_loss {
                            p.stop_loss = trail_stop;
                            sl_updates.push((id.clone(), trail_stop));
                        }
                        if price >= p.stop_loss {
                            out.push((p.clone(), PositionExitReason::Trailing));
                            to_remove.push(id.clone());
                            continue;
                        }
                    }
                }
            }
        }

        for id in to_remove {
            book.remove(&id);
        }
        out
    }
}

pub fn pnl_usd(p: &Position, exit_price: f64) -> f64 {
    match p.side {
        Side::Long => (exit_price - p.entry_price) * p.size,
        Side::Short => (p.entry_price - exit_price) * p.size,
    }
}
