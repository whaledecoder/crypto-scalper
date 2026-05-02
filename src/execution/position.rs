//! Position tracker — manages open positions, trailing stops, PnL.

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
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum PositionExitReason {
    StopLoss,
    TakeProfit,
    Trailing,
    TimeExit,
    Manual,
}

impl PositionExitReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::StopLoss => "SL",
            Self::TakeProfit => "TP",
            Self::Trailing => "TRAILING",
            Self::TimeExit => "TIME",
            Self::Manual => "MANUAL",
        }
    }
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

    /// Snapshot of every open position. Alias for `all()` — used in
    /// `flat-all` flows where the call site wants iteration semantics.
    pub fn snapshot(&self) -> Vec<Position> {
        self.all()
    }

    pub fn close_by_id(&self, client_id: &str) -> Option<Position> {
        self.close(client_id)
    }

    /// Replace the in-memory book with the given list of positions.
    /// Used by `ExecutionAgent` at startup to reconcile the in-memory
    /// state against what the exchange actually holds.
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

    /// Given a fresh mark price, return the set of positions that should close
    /// and the reason. Also mutates trailing stops on remaining positions.
    pub fn check_exits(&self, symbol: &str, price: f64) -> Vec<(Position, PositionExitReason)> {
        let mut out = Vec::new();
        let mut to_remove = Vec::new();
        let mut book = self.inner.lock();
        for (id, p) in book.iter_mut() {
            if p.symbol != symbol {
                continue;
            }
            let reason = match p.side {
                Side::Long => {
                    if p.stop_loss > 0.0 && price <= p.stop_loss {
                        Some(PositionExitReason::StopLoss)
                    } else if p.take_profit > 0.0 && price >= p.take_profit {
                        Some(PositionExitReason::TakeProfit)
                    } else if p.trailing_activated
                        && price <= p.peak_price - (p.peak_price - p.entry_price) * 0.5
                    {
                        Some(PositionExitReason::Trailing)
                    } else {
                        // activate trailing once profit > 1R
                        let r = (p.entry_price - p.stop_loss).abs();
                        if !p.trailing_activated && price >= p.entry_price + r {
                            p.trailing_activated = true;
                        }
                        None
                    }
                }
                Side::Short => {
                    if p.stop_loss > 0.0 && price >= p.stop_loss {
                        Some(PositionExitReason::StopLoss)
                    } else if p.take_profit > 0.0 && price <= p.take_profit {
                        Some(PositionExitReason::TakeProfit)
                    } else if p.trailing_activated
                        && price >= p.trough_price + (p.entry_price - p.trough_price) * 0.5
                    {
                        Some(PositionExitReason::Trailing)
                    } else {
                        let r = (p.entry_price - p.stop_loss).abs();
                        if !p.trailing_activated && price <= p.entry_price - r {
                            p.trailing_activated = true;
                        }
                        None
                    }
                }
            };
            if let Some(r) = reason {
                out.push((p.clone(), r));
                to_remove.push(id.clone());
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
