//! Binance WebSocket client with auto-reconnect.
//!
//! Subscribes to combined streams for the configured symbols and emits
//! strongly-typed events to an async channel. When disconnected, retries with
//! exponential backoff up to a bounded ceiling.

use crate::data::types::Trade;
use anyhow::{anyhow, Context};
use chrono::{TimeZone, Utc};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::sleep;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{debug, error, info, warn};
use url::Url;

#[derive(Debug)]
pub enum WsEvent {
    Trade {
        symbol: String,
        trade: Trade,
    },
    BookTicker {
        symbol: String,
        best_bid: f64,
        best_ask: f64,
    },
    Heartbeat,
    Disconnected(String),
}

pub struct WsClient {
    base_url: String,
    symbols: Vec<String>,
}

impl WsClient {
    pub fn new(base_url: impl Into<String>, symbols: Vec<String>) -> Self {
        Self {
            base_url: base_url.into(),
            symbols,
        }
    }

    /// Build the combined-stream URL for this client.
    fn build_url(&self) -> anyhow::Result<Url> {
        let streams: Vec<String> = self
            .symbols
            .iter()
            .flat_map(|s| {
                let lower = s.to_lowercase();
                vec![format!("{lower}@trade"), format!("{lower}@bookTicker")]
            })
            .collect();
        let joined = streams.join("/");
        let url = format!("{}?streams={joined}", self.base_url.trim_end_matches('/'));
        Url::parse(&url).context("invalid ws url")
    }

    /// Run the client indefinitely, reconnecting on failure. Events are sent on
    /// `tx`. Errors are logged; the loop exits only when `tx` is dropped.
    pub async fn run(self, tx: mpsc::Sender<WsEvent>) {
        let mut backoff_ms: u64 = 500;
        loop {
            if tx.is_closed() {
                break;
            }
            let url = match self.build_url() {
                Ok(u) => u,
                Err(e) => {
                    error!(error = %e, "failed to build ws url, aborting");
                    return;
                }
            };

            info!(url = %url, "ws connect");
            match connect_async(url.as_str()).await {
                Ok((mut stream, _)) => {
                    backoff_ms = 500;
                    loop {
                        tokio::select! {
                            msg = stream.next() => {
                                match msg {
                                    Some(Ok(Message::Text(txt))) => {
                                        if let Err(e) = handle_text(&txt, &tx).await {
                                            debug!(error = %e, "parse error");
                                        }
                                    }
                                    Some(Ok(Message::Ping(p))) => {
                                        let _ = stream.send(Message::Pong(p)).await;
                                    }
                                    Some(Ok(Message::Close(f))) => {
                                        warn!(frame = ?f, "ws closed by peer");
                                        break;
                                    }
                                    Some(Ok(_)) => {}
                                    Some(Err(e)) => {
                                        warn!(error = %e, "ws read error");
                                        break;
                                    }
                                    None => {
                                        warn!("ws stream ended");
                                        break;
                                    }
                                }
                            }
                            _ = sleep(Duration::from_secs(30)) => {
                                // heartbeat ping
                                let _ = stream.send(Message::Ping(vec![])).await;
                                let _ = tx.send(WsEvent::Heartbeat).await;
                            }
                        }
                    }
                }
                Err(e) => {
                    warn!(error = %e, "ws connect failed");
                }
            }

            let _ = tx.send(WsEvent::Disconnected("reconnecting".into())).await;
            sleep(Duration::from_millis(backoff_ms)).await;
            backoff_ms = (backoff_ms * 2).min(30_000);
        }
    }
}

#[derive(Debug, Deserialize)]
struct CombinedMsg<T> {
    #[allow(dead_code)]
    stream: String,
    data: T,
}

#[derive(Debug, Deserialize)]
struct BinanceTrade {
    #[serde(rename = "E")]
    event_time_ms: i64,
    #[serde(rename = "s")]
    symbol: String,
    #[serde(rename = "p")]
    price: String,
    #[serde(rename = "q")]
    qty: String,
    #[serde(rename = "m")]
    is_buyer_maker: bool,
}

#[derive(Debug, Deserialize)]
struct BinanceBookTicker {
    #[serde(rename = "s")]
    symbol: String,
    #[serde(rename = "b")]
    best_bid: String,
    #[serde(rename = "a")]
    best_ask: String,
}

async fn handle_text(txt: &str, tx: &mpsc::Sender<WsEvent>) -> anyhow::Result<()> {
    let value: serde_json::Value = serde_json::from_str(txt).context("ws: text is not json")?;
    let stream = value
        .get("stream")
        .and_then(|s| s.as_str())
        .ok_or_else(|| anyhow!("no stream field"))?;

    if stream.ends_with("@trade") {
        let parsed: CombinedMsg<BinanceTrade> =
            serde_json::from_value(value).context("parse trade")?;
        let trade = Trade {
            ts: Utc
                .timestamp_millis_opt(parsed.data.event_time_ms)
                .single()
                .ok_or_else(|| anyhow!("bad ts"))?,
            price: parsed.data.price.parse()?,
            qty: parsed.data.qty.parse()?,
            is_buyer_maker: parsed.data.is_buyer_maker,
        };
        let _ = tx
            .send(WsEvent::Trade {
                symbol: parsed.data.symbol,
                trade,
            })
            .await;
    } else if stream.ends_with("@bookTicker") {
        let parsed: CombinedMsg<BinanceBookTicker> =
            serde_json::from_value(value).context("parse book ticker")?;
        let _ = tx
            .send(WsEvent::BookTicker {
                symbol: parsed.data.symbol,
                best_bid: parsed.data.best_bid.parse()?,
                best_ask: parsed.data.best_ask.parse()?,
            })
            .await;
    }
    Ok(())
}
