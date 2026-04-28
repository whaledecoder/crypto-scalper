//! Abstraction over exchange implementations (real or paper).

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

    fn name(&self) -> &'static str;
}
