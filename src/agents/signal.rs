//! Signal agent — listens for `CandleClosed` events, updates per-symbol
//! state, runs the regime detector + active strategies, and emits a
//! `PreSignalEmitted` event for the best candidate.

use crate::agents::messages::AgentEvent;
use crate::agents::MessageBus;
use crate::config::{AdvancedAlphaCfg, Schedule};
use crate::data::Side;
use crate::feeds::ExternalSnapshot;
use crate::microstructure::Ofi;
use crate::quant::QuantEngine;
use crate::strategy::{
    alpha_gate::{
        advanced_alpha_gate, alt_data_inputs_from_snapshot, funding_rate_from_snapshot,
        kalman_trend_score, AdvancedAlphaInputs, AlphaGateDecision,
    },
    ema_ribbon::EmaRibbon,
    mean_reversion::MeanReversion,
    momentum::Momentum,
    select_strategies,
    squeeze::Squeeze,
    state::{PreSignal, StrategyName, SymbolState},
    vwap_scalp::VwapScalp,
    RegimeDetector, Strategy,
};
use chrono::{DateTime, Duration as ChronoDuration, Timelike, Utc};
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
    advanced_alpha: AdvancedAlphaCfg,
    quant_engine: Option<Arc<QuantEngine>>,
) -> JoinHandle<()> {
    let mut rx = bus.subscribe();
    tokio::spawn(async move {
        info!(?active, "signal agent starting");
        let mut ofi_by_symbol: HashMap<String, Ofi> = HashMap::new();
        let mut feeds_by_symbol: HashMap<String, TimedExternalSnapshot> = HashMap::new();
        while let Ok(ev) = rx.recv().await {
            match ev {
                AgentEvent::FeedsSnapshot(msg) => {
                    feeds_by_symbol.insert(
                        msg.symbol,
                        TimedExternalSnapshot {
                            snapshot: msg.snapshot,
                            ts: msg.ts,
                        },
                    );
                }
                AgentEvent::BookTicker {
                    symbol,
                    best_bid,
                    bid_qty,
                    best_ask,
                    ask_qty,
                } => {
                    let ofi = ofi_by_symbol
                        .entry(symbol.clone())
                        .or_insert_with(|| Ofi::new(20))
                        .update(bid_qty, ask_qty);
                    let mut states = states.lock().await;
                    if let Some(state) = states.get_mut(&symbol) {
                        state
                            .order_book
                            .set_top_with_qty(best_bid, bid_qty, best_ask, ask_qty);
                        if let Some(value) = ofi {
                            state.last_ofi = Some(value);
                        }
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
                        let prev_close = state.candles.back().map(|c| c.close);
                        state.on_closed(candle);

                        // Quant engine: update Kalman filter and record return
                        if let Some(ref qe) = quant_engine {
                            qe.update_kalman(&symbol, candle.close);
                            if let Some(prev) = prev_close {
                                if prev > 0.0 {
                                    let ret = (candle.close - prev) / prev;
                                    qe.record_return(&symbol, ret);
                                }
                            }
                        }

                        let regime = RegimeDetector::detect(state);
                        let chosen = select_strategies(&active, regime);

                        // Debug warm-up status so operator can see why no signals
                        debug!(
                            symbol = %symbol,
                            regime = %regime.as_str(),
                            candles = state.candles.len(),
                            ema200_ready = state.ema_200.value().is_some(),
                            ema50_ready  = state.ema_50.value().is_some(),
                            adx_ready    = state.last_adx.is_some(),
                            rsi_ready    = state.last_rsi.is_some(),
                            bb_ready     = state.last_bb.is_some(),
                            vwap_ready   = state.last_vwap.is_some(),
                            strategies   = ?chosen,
                            "candle closed — evaluating strategies"
                        );

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
                                debug!(
                                    symbol = %symbol,
                                    strategy = %s.strategy.as_str(),
                                    side = %s.side.as_str(),
                                    confidence = s.ta_confidence,
                                    reason = %s.reason,
                                    ofi = state.last_ofi,
                                    "strategy fired pre-signal"
                                );
                                if best
                                    .as_ref()
                                    .map(|b| s.ta_confidence > b.ta_confidence)
                                    .unwrap_or(true)
                                {
                                    best = Some(s);
                                }
                            }
                        }
                        let filtered = apply_advanced_alpha(
                            best,
                            state,
                            feeds_by_symbol.get(&symbol),
                            &advanced_alpha,
                        );
                        (filtered, regime)
                    };
                    if let Some(signal) = best {
                        bus.publish(AgentEvent::PreSignalEmitted {
                            signal: Box::new(signal),
                            regime,
                        });
                    }
                }
                AgentEvent::ControlCommand(crate::agents::messages::ControlCommand::ResetDaily) => {
                    // Reset session-anchored indicators (VWAP) at midnight.
                    let mut states = states.lock().await;
                    for state in states.values_mut() {
                        state.vwap.reset();
                        state.last_vwap = None;
                        state.last_vwap_slope = None;
                    }
                    tracing::info!("signal: VWAP reset for new session");
                }
                AgentEvent::Shutdown => break,
                _ => {}
            }
        }
    })
}

fn apply_advanced_alpha(
    signal: Option<PreSignal>,
    state: &SymbolState,
    snapshot: Option<&TimedExternalSnapshot>,
    cfg: &AdvancedAlphaCfg,
) -> Option<PreSignal> {
    if !cfg.enabled {
        return signal;
    }
    let mut signal = signal?;
    let Some(snapshot) = fresh_snapshot(snapshot, cfg, Utc::now()) else {
        return Some(signal);
    };
    let prices: Vec<f64> = state.candles.iter().map(|c| c.close).collect();
    let decision = advanced_alpha_gate(
        AdvancedAlphaInputs {
            alt_data: alt_data_inputs_from_snapshot(snapshot),
            funding_rate: funding_rate_from_snapshot(snapshot),
            trend_score: kalman_trend_score(
                &prices,
                cfg.kalman_process_noise,
                cfg.kalman_measurement_noise,
            ),
            min_abs_score: cfg.min_abs_score,
        },
        matches!(signal.side, Side::Long),
    );
    match decision {
        AlphaGateDecision::Allow => Some(signal),
        AlphaGateDecision::Reduce => {
            signal.ta_confidence = signal
                .ta_confidence
                .saturating_sub(cfg.reduce_confidence_delta);
            signal.reason = format!("{} | alpha_gate=reduce", signal.reason);
            Some(signal)
        }
        AlphaGateDecision::Block => None,
    }
}

#[derive(Debug, Clone)]
struct TimedExternalSnapshot {
    snapshot: ExternalSnapshot,
    ts: DateTime<Utc>,
}

fn fresh_snapshot<'a>(
    snapshot: Option<&'a TimedExternalSnapshot>,
    cfg: &AdvancedAlphaCfg,
    now: DateTime<Utc>,
) -> Option<&'a ExternalSnapshot> {
    let snapshot = snapshot?;
    if cfg.feed_max_age_secs == 0 {
        return Some(&snapshot.snapshot);
    }
    let max_age = ChronoDuration::seconds(cfg.feed_max_age_secs as i64);
    if now - snapshot.ts <= max_age {
        Some(&snapshot.snapshot)
    } else {
        None
    }
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
    use crate::data::{Candle, Side};
    use crate::feeds::sentiment::SentimentSnapshot;
    use crate::strategy::state::{PreSignal, StrategyName, SymbolState};

    fn test_signal() -> PreSignal {
        PreSignal {
            symbol: "BTCUSDT".into(),
            strategy: StrategyName::Momentum,
            side: Side::Long,
            entry: 100.0,
            stop_loss: 99.0,
            take_profit: 103.0,
            ta_confidence: 80,
            reason: "unit".into(),
        }
    }

    fn warmed_state() -> SymbolState {
        let mut state = SymbolState::new("BTCUSDT");
        let now = Utc::now();
        for i in 0..5 {
            let price = 100.0 + i as f64;
            state.on_closed(Candle {
                open_time: now,
                close_time: now,
                open: price - 0.5,
                high: price + 1.0,
                low: price - 1.0,
                close: price,
                volume: 100.0,
            });
        }
        state
    }

    #[test]
    fn dead_zone_disabled_when_start_eq_end() {
        let s = Schedule {
            dead_zone_start_hour_wib: 3,
            dead_zone_end_hour_wib: 3,
        };
        // Just check the function doesn't say true for a degenerate config.
        assert!(!in_dead_zone(&s));
    }

    #[test]
    fn advanced_alpha_disabled_is_noop() {
        let signal = test_signal();
        let state = warmed_state();
        let filtered = apply_advanced_alpha(
            Some(signal.clone()),
            &state,
            None,
            &AdvancedAlphaCfg::default(),
        );
        assert_eq!(filtered.unwrap().ta_confidence, signal.ta_confidence);
    }

    #[test]
    fn advanced_alpha_can_reduce_confidence() {
        let signal = test_signal();
        let state = warmed_state();
        let fresh = TimedExternalSnapshot {
            snapshot: ExternalSnapshot::default(),
            ts: Utc::now(),
        };
        let filtered = apply_advanced_alpha(
            Some(signal),
            &state,
            Some(&fresh),
            &AdvancedAlphaCfg {
                enabled: true,
                min_abs_score: 0.6,
                reduce_confidence_delta: 7,
                ..AdvancedAlphaCfg::default()
            },
        )
        .expect("neutral alpha context should reduce, not block");
        assert_eq!(filtered.ta_confidence, 73);
        assert!(filtered.reason.contains("alpha_gate=reduce"));
    }

    #[test]
    fn advanced_alpha_skips_when_feed_missing_or_stale() {
        let signal = test_signal();
        let state = warmed_state();
        let cfg = AdvancedAlphaCfg {
            enabled: true,
            feed_max_age_secs: 60,
            ..AdvancedAlphaCfg::default()
        };
        let missing = apply_advanced_alpha(Some(signal.clone()), &state, None, &cfg)
            .expect("missing feed should bypass alpha gate");
        assert_eq!(missing.ta_confidence, signal.ta_confidence);

        let stale = TimedExternalSnapshot {
            snapshot: ExternalSnapshot {
                sentiment: Some(SentimentSnapshot {
                    symbol: "BTCUSDT".into(),
                    social_volume: 1,
                    social_volume_change_pct: 0.0,
                    galaxy_score: None,
                    sentiment: -1.0,
                    top_keywords: vec![],
                }),
                ..ExternalSnapshot::default()
            },
            ts: Utc::now() - ChronoDuration::seconds(120),
        };
        let filtered = apply_advanced_alpha(Some(signal.clone()), &state, Some(&stale), &cfg)
            .expect("stale feed should bypass alpha gate");
        assert_eq!(filtered.ta_confidence, signal.ta_confidence);
    }
}
