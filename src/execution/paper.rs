//! Paper exchange — instant fills at the requested price, no network.

use crate::errors::Result;
use crate::execution::exchange::{Exchange, OrderAck, PositionSnapshot};
use crate::execution::orders::OrderRequest;
use chrono::Utc;
use parking_lot::Mutex;
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct OpenPaperOrder {
    pub req: OrderRequest,
    pub filled_at: chrono::DateTime<chrono::Utc>,
}

pub struct PaperExchange {
    fee_bps: f64,
    orders: Mutex<HashMap<String, OpenPaperOrder>>,
    /// Synthetic balance the paper exchange "holds". Updated by callers
    /// (RiskAgent / SurvivalAgent) so they can simulate equity drift.
    equity_usd: Mutex<f64>,
}

impl PaperExchange {
    pub fn new(fee_bps: f64, equity_usd: f64) -> Self {
        Self {
            fee_bps,
            orders: Mutex::new(HashMap::new()),
            equity_usd: Mutex::new(equity_usd),
        }
    }

    pub fn open_orders(&self) -> Vec<OpenPaperOrder> {
        self.orders.lock().values().cloned().collect()
    }

    pub fn set_equity(&self, equity: f64) {
        *self.equity_usd.lock() = equity;
    }
}

impl Exchange for PaperExchange {
    fn name(&self) -> &'static str {
        "paper"
    }

    fn place_order<'a>(
        &'a self,
        req: &'a OrderRequest,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<OrderAck>> + Send + 'a>> {
        Box::pin(async move {
            let price = req.price.unwrap_or(0.0);
            let notional = price * req.size;
            let fee = notional * self.fee_bps / 10_000.0;
            self.orders.lock().insert(
                req.client_id.clone(),
                OpenPaperOrder {
                    req: req.clone(),
                    filled_at: Utc::now(),
                },
            );
            Ok(OrderAck {
                client_id: req.client_id.clone(),
                exchange_order_id: format!("paper-{}", req.client_id),
                symbol: req.symbol.clone(),
                filled_qty: req.size,
                avg_fill_price: price,
                fee_usd: fee,
                ts_ms: Utc::now().timestamp_millis(),
            })
        })
    }

    fn cancel_order<'a>(
        &'a self,
        _symbol: &'a str,
        client_id: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(async move {
            self.orders.lock().remove(client_id);
            Ok(())
        })
    }

    fn cancel_all<'a>(
        &'a self,
        symbol: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(async move {
            self.orders.lock().retain(|_, o| o.req.symbol != symbol);
            Ok(())
        })
    }

    fn set_leverage<'a>(
        &'a self,
        _symbol: &'a str,
        _leverage: u8,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(async move { Ok(()) })
    }

    fn fetch_equity_usd<'a>(
        &'a self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<f64>> + Send + 'a>> {
        Box::pin(async move { Ok(*self.equity_usd.lock()) })
    }

    fn fetch_open_positions<'a>(
        &'a self,
        _symbols: &'a [String],
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Vec<PositionSnapshot>>> + Send + 'a>,
    > {
        // Paper exchange has no broker-side positions — the in-memory
        // PositionBook is the source of truth.
        Box::pin(async move { Ok(Vec::new()) })
    }
}
