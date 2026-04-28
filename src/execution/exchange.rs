//! Abstraction over exchange implementations (real or paper).

use crate::data::Side;
use crate::errors::Result;
use crate::execution::orders::OrderRequest;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderAck {
    pub client_id: String,
    pub exchange_order_id: String,
    pub symbol: String,
    pub filled_qty: f64,
    pub avg_fill_price: f64,
    pub fee_usd: f64,
    pub ts_ms: i64,
}

/// Snapshot of an open position as reported by the exchange.
/// Used to reconcile our in-memory `PositionBook` against the truth
/// the broker holds — the bot must never drift from that.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PositionSnapshot {
    pub symbol: String,
    pub side: Side,
    pub size: f64,
    pub entry_price: f64,
    pub mark_price: f64,
    pub unrealized_pnl: f64,
    pub leverage: u8,
}

/// Minimal interface the bot uses to place / cancel orders. We express async
/// via boxed futures so the trait stays dyn-object-safe without relying on the
/// `async-trait` proc-macro crate.
pub trait Exchange: Send + Sync {
    fn place_order<'a>(
        &'a self,
        req: &'a OrderRequest,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<OrderAck>> + Send + 'a>>;

    fn cancel_order<'a>(
        &'a self,
        symbol: &'a str,
        client_id: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + 'a>>;

    /// Cancel all open orders for the given symbol — used on shutdown
    /// or when the watchdog detects the bot lost track of state.
    fn cancel_all<'a>(
        &'a self,
        symbol: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + 'a>>;

    /// Set the per-symbol leverage. Called once at boot per active symbol.
    fn set_leverage<'a>(
        &'a self,
        symbol: &'a str,
        leverage: u8,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + 'a>>;

    /// Fetch the USDT-margined wallet balance (margin balance, not just free).
    /// Used by the SurvivalAgent / RiskAgent to keep `equity_usd` truthful.
    fn fetch_equity_usd<'a>(
        &'a self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<f64>> + Send + 'a>>;

    /// Fetch the list of currently open positions across the configured
    /// symbols. Used on startup to reconcile the in-memory book against
    /// what the exchange actually holds.
    fn fetch_open_positions<'a>(
        &'a self,
        symbols: &'a [String],
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Vec<PositionSnapshot>>> + Send + 'a>,
    >;

    fn name(&self) -> &'static str;
}
