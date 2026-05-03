//! Data ingestion agent — owns the WebSocket client and the per-symbol
//! OHLCV builders. Publishes `Tick`, `BookTicker` and `CandleClosed`
//! events on the bus.

use crate::agents::messages::{AgentEvent, AgentId};
use crate::agents::MessageBus;
use crate::data::{ws_client::WsClient, OhlcvBuilder, Timeframe, WsEvent};
use chrono::Utc;
use std::collections::HashMap;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::{info, warn};

pub struct DataAgentConfig {
    pub ws_base_url: String,
    pub symbols: Vec<String>,
    pub timeframes: Vec<Timeframe>,
}

pub fn spawn(bus: MessageBus, cfg: DataAgentConfig) -> JoinHandle<()> {
    tokio::spawn(async move { run(bus, cfg).await })
}

async fn run(bus: MessageBus, cfg: DataAgentConfig) {
    let timeframes = if cfg.timeframes.is_empty() {
        vec![Timeframe { seconds: 300 }]
    } else {
        cfg.timeframes
    };
    info!(symbols = ?cfg.symbols, timeframes = ?timeframes, "data agent starting");
    let mut builders: HashMap<String, Vec<(i64, OhlcvBuilder)>> = cfg
        .symbols
        .iter()
        .map(|s| {
            (
                s.clone(),
                timeframes
                    .iter()
                    .map(|tf| (tf.seconds, OhlcvBuilder::new(tf.seconds)))
                    .collect(),
            )
        })
        .collect();

    let (tx, mut rx) = mpsc::channel::<WsEvent>(4096);
    let ws = WsClient::new(cfg.ws_base_url, cfg.symbols.clone());
    tokio::spawn(async move { ws.run(tx).await });

    // Independent periodic heartbeat — keeps the watchdog satisfied
    // even during quiet markets or while a reconnect is in progress.
    {
        let bus_hb = bus.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(20));
            loop {
                tick.tick().await;
                bus_hb.publish(AgentEvent::Heartbeat {
                    from: AgentId::Data,
                    ts: Utc::now(),
                });
            }
        });
    }

    while let Some(event) = rx.recv().await {
        match event {
            WsEvent::Trade { symbol, trade } => {
                let candles = builders
                    .get_mut(&symbol)
                    .map(|builders| {
                        builders
                            .iter_mut()
                            .filter_map(|(timeframe_secs, builder)| {
                                builder
                                    .ingest(trade)
                                    .map(|candle| (*timeframe_secs, candle))
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                bus.publish(AgentEvent::Tick {
                    symbol: symbol.clone(),
                    trade,
                });
                for (timeframe_secs, candle) in candles {
                    bus.publish(AgentEvent::CandleClosed {
                        symbol: symbol.clone(),
                        timeframe_secs,
                        candle,
                    });
                }
            }
            WsEvent::BookTicker {
                symbol,
                best_bid,
                bid_qty,
                best_ask,
                ask_qty,
            } => {
                bus.publish(AgentEvent::BookTicker {
                    symbol,
                    best_bid,
                    bid_qty,
                    best_ask,
                    ask_qty,
                });
            }
            WsEvent::Heartbeat => {}
            WsEvent::Disconnected(reason) => {
                warn!(%reason, "data agent: ws disconnected — reconnect pending");
            }
        }
        // Activity-based heartbeat — every event is a sign of life.
        bus.publish(AgentEvent::Heartbeat {
            from: AgentId::Data,
            ts: Utc::now(),
        });
    }
}
