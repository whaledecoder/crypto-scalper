//! Data ingestion agent — owns the WebSocket client and the per-symbol
//! OHLCV builders. Publishes `Tick`, `BookTicker` and `CandleClosed`
//! events on the bus.

use crate::agents::messages::AgentEvent;
use crate::agents::MessageBus;
use crate::data::{ws_client::WsClient, OhlcvBuilder, WsEvent};
use std::collections::HashMap;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::{info, warn};

pub struct DataAgentConfig {
    pub ws_base_url: String,
    pub symbols: Vec<String>,
    pub interval_secs: i64,
}

pub fn spawn(bus: MessageBus, cfg: DataAgentConfig) -> JoinHandle<()> {
    tokio::spawn(async move { run(bus, cfg).await })
}

async fn run(bus: MessageBus, cfg: DataAgentConfig) {
    info!(symbols = ?cfg.symbols, "data agent starting");
    let mut builders: HashMap<String, OhlcvBuilder> = cfg
        .symbols
        .iter()
        .map(|s| (s.clone(), OhlcvBuilder::new(cfg.interval_secs)))
        .collect();

    let (tx, mut rx) = mpsc::channel::<WsEvent>(4096);
    let ws = WsClient::new(cfg.ws_base_url, cfg.symbols.clone());
    tokio::spawn(async move { ws.run(tx).await });

    while let Some(event) = rx.recv().await {
        match event {
            WsEvent::Trade { symbol, trade } => {
                let candle = builders.get_mut(&symbol).and_then(|b| b.ingest(trade));
                bus.publish(AgentEvent::Tick {
                    symbol: symbol.clone(),
                    trade,
                });
                if let Some(c) = candle {
                    bus.publish(AgentEvent::CandleClosed { symbol, candle: c });
                }
            }
            WsEvent::BookTicker {
                symbol,
                best_bid,
                best_ask,
            } => {
                bus.publish(AgentEvent::BookTicker {
                    symbol,
                    best_bid,
                    best_ask,
                });
            }
            WsEvent::Heartbeat => {}
            WsEvent::Disconnected(reason) => {
                warn!(%reason, "data agent: ws disconnected — reconnect pending");
            }
        }
    }
}
