//! Paper exchange — instant fills at the requested price, no network.

use crate::errors::Result;
use crate::execution::exchange::{Exchange, OrderAck};
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
}

impl PaperExchange {
    pub fn new(fee_bps: f64) -> Self {
        Self {
            fee_bps,
            orders: Mutex::new(HashMap::new()),
        }
    }

    pub fn open_orders(&self) -> Vec<OpenPaperOrder> {
        self.orders.lock().values().cloned().collect()
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
}
