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
    pub stop_loss: f64,
    pub take_profit: f64,
    pub order_type: OrderType,
}
