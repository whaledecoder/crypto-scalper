//! Order models.

use crate::data::Side;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum OrderType {
    Market,
    Limit,
    StopLoss,
    TakeProfit,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderRequest {
    pub client_id: String,
    pub symbol: String,
    pub side: Side,
    pub size: f64,
    pub price: Option<f64>,
    /// Trigger price for STOP_MARKET / TAKE_PROFIT_MARKET orders.
    /// Ignored for `Market` / `Limit`.
    #[serde(default)]
    pub stop_price: Option<f64>,
    /// Reference SL price (informational; persisted on the entry order
    /// so the position book can populate trailing-stop state).
    pub stop_loss: f64,
    /// Reference TP price.
    pub take_profit: f64,
    pub order_type: OrderType,
    /// True for protective SL/TP orders (sent with `reduceOnly=true` and
    /// `closePosition=true` on Binance). Defaults to `false` for the
    /// entry order.
    #[serde(default)]
    pub reduce_only: bool,
}
