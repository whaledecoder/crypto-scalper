//! Binance Futures REST client — HMAC-SHA256 signed requests.
//!
//! This is a minimal, defensive implementation: it supports `POST /fapi/v1/order`
//! for market + limit and `DELETE /fapi/v1/order` for cancellation.

use crate::errors::{Result, ScalperError};
use crate::execution::exchange::{Exchange, OrderAck};
use crate::execution::orders::{OrderRequest, OrderType};
use hmac::{Hmac, Mac};
use reqwest::Client;
use sha2::Sha256;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::debug;

type HmacSha256 = Hmac<Sha256>;

pub struct BinanceFutures {
    client: Client,
    base_url: String,
    api_key: String,
    api_secret: String,
    recv_window_ms: u64,
}

impl BinanceFutures {
    pub fn new(
        base_url: impl Into<String>,
        api_key: impl Into<String>,
        api_secret: impl Into<String>,
        recv_window_ms: u64,
    ) -> Self {
        Self {
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(5))
                .build()
                .unwrap_or_default(),
            base_url: base_url.into(),
            api_key: api_key.into(),
            api_secret: api_secret.into(),
            recv_window_ms,
        }
    }

    fn timestamp_ms(&self) -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
    }

    fn sign(&self, qs: &str) -> Result<String> {
        let mut mac = HmacSha256::new_from_slice(self.api_secret.as_bytes())
            .map_err(|e| ScalperError::Exchange(format!("hmac: {e}")))?;
        mac.update(qs.as_bytes());
        Ok(hex::encode(mac.finalize().into_bytes()))
    }
}

impl Exchange for BinanceFutures {
    fn name(&self) -> &'static str {
        "binance-futures"
    }

    fn place_order<'a>(
        &'a self,
        req: &'a OrderRequest,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<OrderAck>> + Send + 'a>> {
        Box::pin(async move {
            let side = match req.side {
                crate::data::Side::Long => "BUY",
                crate::data::Side::Short => "SELL",
            };
            let order_type = match req.order_type {
                OrderType::Market => "MARKET",
                OrderType::Limit => "LIMIT",
                OrderType::StopLoss => "STOP_MARKET",
                OrderType::TakeProfit => "TAKE_PROFIT_MARKET",
            };

            let ts = self.timestamp_ms();
            let mut params = vec![
                ("symbol".to_string(), req.symbol.clone()),
                ("side".to_string(), side.to_string()),
                ("type".to_string(), order_type.to_string()),
                ("quantity".to_string(), format!("{:.6}", req.size)),
                ("newClientOrderId".to_string(), req.client_id.clone()),
                ("recvWindow".to_string(), self.recv_window_ms.to_string()),
                ("timestamp".to_string(), ts.to_string()),
            ];
            if let Some(p) = req.price {
                params.push(("price".to_string(), format!("{:.2}", p)));
                params.push(("timeInForce".to_string(), "GTC".to_string()));
            }

            let qs = encode_query(&params);
            let sig = self.sign(&qs)?;
            let url = format!(
                "{}/fapi/v1/order?{qs}&signature={sig}",
                self.base_url.trim_end_matches('/')
            );
            debug!(url = %url, "binance place_order");

            let resp = self
                .client
                .post(&url)
                .header("X-MBX-APIKEY", &self.api_key)
                .send()
                .await?;
            if !resp.status().is_success() {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                return Err(ScalperError::Exchange(format!(
                    "place_order http {status}: {body}"
                )));
            }
            let body: serde_json::Value = resp.json().await?;
            let exchange_order_id = body
                .get("orderId")
                .map(|v| v.to_string())
                .unwrap_or_default();
            let filled_qty: f64 = body
                .get("executedQty")
                .and_then(|v| v.as_str())
                .unwrap_or("0")
                .parse()
                .unwrap_or(0.0);
            let avg_price: f64 = body
                .get("avgPrice")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse::<f64>().ok())
                .unwrap_or(req.price.unwrap_or(0.0));

            Ok(OrderAck {
                client_id: req.client_id.clone(),
                exchange_order_id,
                symbol: req.symbol.clone(),
                filled_qty,
                avg_fill_price: avg_price,
                fee_usd: 0.0,
                ts_ms: ts,
            })
        })
    }

    fn cancel_order<'a>(
        &'a self,
        symbol: &'a str,
        client_id: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(async move {
            let ts = self.timestamp_ms();
            let params = vec![
                ("symbol".to_string(), symbol.to_string()),
                ("origClientOrderId".to_string(), client_id.to_string()),
                ("timestamp".to_string(), ts.to_string()),
                ("recvWindow".to_string(), self.recv_window_ms.to_string()),
            ];
            let qs = encode_query(&params);
            let sig = self.sign(&qs)?;
            let url = format!(
                "{}/fapi/v1/order?{qs}&signature={sig}",
                self.base_url.trim_end_matches('/')
            );
            let resp = self
                .client
                .delete(&url)
                .header("X-MBX-APIKEY", &self.api_key)
                .send()
                .await?;
            if !resp.status().is_success() {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                return Err(ScalperError::Exchange(format!(
                    "cancel_order http {status}: {body}"
                )));
            }
            Ok(())
        })
    }
}

fn encode_query(params: &[(String, String)]) -> String {
    params
        .iter()
        .map(|(k, v)| format!("{k}={}", urlencode(v)))
        .collect::<Vec<_>>()
        .join("&")
}

fn urlencode(s: &str) -> String {
    // Minimal URL encoder — only the characters Binance cares about.
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => {
                out.push_str(&format!("%{b:02X}"));
            }
        }
    }
    out
}
