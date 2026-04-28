//! Signal agent — listens for `CandleClosed` events, updates per-symbol
//! state, runs the regime detector + active strategies, and emits a
//! `PreSignalEmitted` event for the best candidate.

use crate::agents::messages::AgentEvent;
use crate::agents::MessageBus;
use crate::config::Schedule;
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
use chrono::{Timelike, Utc};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tracing::{debug, info};

pub fn spawn(
    bus: MessageBus,
    states: Arc<Mutex<HashMap<String, SymbolState>>>,
    active: Vec<StrategyName>,
    schedule: Schedule,
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
                    if in_dead_zone(&schedule) {
                        debug!(
                            symbol = %symbol,
                            "signal: skipping candle (dead zone {}–{} WIB)",
                            schedule.dead_zone_start_hour_wib,
                            schedule.dead_zone_end_hour_wib
                        );
                        continue;
                    }
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

/// True iff the current UTC time falls inside the configured WIB
/// dead zone (e.g. 03:00–07:00 WIB == 20:00–00:00 UTC). The window
/// is wrap-aware: `start > end` means it crosses midnight.
fn in_dead_zone(s: &Schedule) -> bool {
    if s.dead_zone_start_hour_wib == s.dead_zone_end_hour_wib {
        return false;
    }
    // WIB == UTC+7. Convert via subtraction.
    let now_wib_hour = (Utc::now().hour() as i32 + 7).rem_euclid(24) as u8;
    let start = s.dead_zone_start_hour_wib;
    let end = s.dead_zone_end_hour_wib;
    if start < end {
        now_wib_hour >= start && now_wib_hour < end
    } else {
        now_wib_hour >= start || now_wib_hour < end
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dead_zone_disabled_when_start_eq_end() {
        let s = Schedule {
            dead_zone_start_hour_wib: 3,
            dead_zone_end_hour_wib: 3,
        };
        // Just check the function doesn't say true for a degenerate config.
        assert!(!in_dead_zone(&s));
    }
}
