//! Signal agent — listens for `CandleClosed` events, updates per-symbol
//! state, runs the regime detector + active strategies, and emits a
//! `PreSignalEmitted` event for the best candidate.

use crate::agents::messages::AgentEvent;
use crate::agents::MessageBus;
use crate::strategy::{
    ema_ribbon::EmaRibbon,
    mean_reversion::MeanReversion,
    momentum::Momentum,
    select_strategies,
    squeeze::Squeeze,
    state::{PreSignal, StrategyName, SymbolState},
    vwap_scalp::VwapScalp,
    RegimeDetector, Strategy,
};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tracing::info;

pub fn spawn(
    bus: MessageBus,
    states: Arc<Mutex<HashMap<String, SymbolState>>>,
    active: Vec<StrategyName>,
) -> JoinHandle<()> {
    let mut rx = bus.subscribe();
    tokio::spawn(async move {
        info!(?active, "signal agent starting");
        while let Ok(ev) = rx.recv().await {
            match ev {
                AgentEvent::BookTicker {
                    symbol,
                    best_bid,
                    best_ask,
                } => {
                    let mut states = states.lock().await;
                    if let Some(state) = states.get_mut(&symbol) {
                        state.order_book.set_top(best_bid, best_ask);
                    }
                }
                AgentEvent::CandleClosed { symbol, candle } => {
                    let (best, regime) = {
                        let mut states = states.lock().await;
                        let state = match states.get_mut(&symbol) {
                            Some(s) => s,
                            None => continue,
                        };
                        state.on_closed(candle);
                        let regime = RegimeDetector::detect(state);
                        let chosen = select_strategies(&active, regime);
                        let mut best: Option<PreSignal> = None;
                        for name in chosen {
                            let sig = match name {
                                StrategyName::EmaRibbon => EmaRibbon.evaluate(state, &candle),
                                StrategyName::MeanReversion => {
                                    MeanReversion.evaluate(state, &candle)
                                }
                                StrategyName::Momentum => Momentum.evaluate(state, &candle),
                                StrategyName::VwapScalp => VwapScalp.evaluate(state, &candle),
                                StrategyName::Squeeze => Squeeze.evaluate(state, &candle),
                            };
                            if let Some(s) = sig {
                                if best
                                    .as_ref()
                                    .map(|b| s.ta_confidence > b.ta_confidence)
                                    .unwrap_or(true)
                                {
                                    best = Some(s);
                                }
                            }
                        }
                        (best, regime)
                    };
                    if let Some(signal) = best {
                        bus.publish(AgentEvent::PreSignalEmitted {
                            signal: Box::new(signal),
                            regime,
                        });
                    }
                }
                AgentEvent::Shutdown => break,
                _ => {}
            }
        }
    })
}
