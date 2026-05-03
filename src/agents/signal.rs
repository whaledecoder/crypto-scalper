//! Signal agent — listens for `CandleClosed` events, updates per-symbol
//! state, runs the regime detector + active strategies, and emits a
//! `PreSignalEmitted` event for the best candidate.

use crate::agents::messages::{AgentEvent, SignalEvaluationMsg};
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
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tracing::{debug, info};

pub struct SignalAgentConfig {
    pub active: Vec<StrategyName>,
    pub schedule: Schedule,
    pub advanced_alpha: AdvancedAlphaCfg,
    pub quant_engine: Option<Arc<QuantEngine>>,
    pub paper_scout_enabled: bool,
    pub entry_timeframe_secs: i64,
}

pub fn spawn(
    bus: MessageBus,
    states: Arc<Mutex<HashMap<String, SymbolState>>>,
    cfg: SignalAgentConfig,
) -> JoinHandle<()> {
    let SignalAgentConfig {
        active,
        schedule,
        advanced_alpha,
        quant_engine,
        paper_scout_enabled,
        entry_timeframe_secs,
    } = cfg;
    let mut rx = bus.subscribe();
    tokio::spawn(async move {
        info!(?active, "signal agent starting");
        let mut ofi_by_symbol: HashMap<String, Ofi> = HashMap::new();
        let mut feeds_by_symbol: HashMap<String, TimedExternalSnapshot> = HashMap::new();
        let mut higher_timeframes: HashMap<String, BTreeMap<i64, HigherTimeframeSnapshot>> =
            HashMap::new();
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
                AgentEvent::CandleClosed {
                    symbol,
                    timeframe_secs,
                    candle,
                } => {
                    if timeframe_secs != entry_timeframe_secs {
                        higher_timeframes
                            .entry(symbol)
                            .or_default()
                            .insert(timeframe_secs, HigherTimeframeSnapshot::from_candle(candle));
                        continue;
                    }
                    if in_dead_zone(&schedule) && !paper_scout_enabled {
                        bus.publish(AgentEvent::SignalEvaluation(SignalEvaluationMsg {
                            symbol,
                            timeframe_secs,
                            regime: None,
                            candles: 0,
                            strategies: Vec::new(),
                            reason: format!(
                                "dead_zone_{}-{}_WIB",
                                schedule.dead_zone_start_hour_wib, schedule.dead_zone_end_hour_wib
                            ),
                            best_strategy: None,
                            best_confidence: None,
                        }));
                        continue;
                    }
                    let htf = higher_timeframes.get(&symbol).cloned().unwrap_or_default();
                    let symbol_for_state = symbol.clone();
                    let (best, regime, candles, chosen, best_seen, forced) = {
                        let mut states = states.lock().await;
                        let state = match states.get_mut(&symbol_for_state) {
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
                        let mut best_seen: Option<(StrategyName, u8)> = None;
                        for &name in &chosen {
                            let sig = match name {
                                StrategyName::EmaRibbon => EmaRibbon.evaluate(state, &candle),
                                StrategyName::MeanReversion => {
                                    MeanReversion.evaluate(state, &candle)
                                }
                                StrategyName::Momentum => Momentum.evaluate(state, &candle),
                                StrategyName::VwapScalp => VwapScalp.evaluate(state, &candle),
                                StrategyName::Squeeze => Squeeze.evaluate(state, &candle),
                            };
                            if let Some(mut s) = sig {
                                if best_seen
                                    .as_ref()
                                    .map(|(_, confidence)| s.ta_confidence > *confidence)
                                    .unwrap_or(true)
                                {
                                    best_seen = Some((s.strategy, s.ta_confidence));
                                }
                                if let Some(ema200) = state.ema_200.value() {
                                    let htf_aligned = match s.side {
                                        Side::Long => candle.close > ema200,
                                        Side::Short => candle.close < ema200,
                                    };
                                    if !htf_aligned {
                                        s.ta_confidence = s.ta_confidence.saturating_sub(8);
                                        s.reason = format!(
                                            "{} | HTF-contradict(ema200={:.2})",
                                            s.reason, ema200
                                        );
                                    } else {
                                        s.ta_confidence = (s.ta_confidence + 3).min(100);
                                        s.reason = format!(
                                            "{} | HTF-confirm(ema200={:.2})",
                                            s.reason, ema200
                                        );
                                    }
                                }
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
                        let mut forced = false;
                        if best.is_none() && paper_scout_enabled {
                            best = paper_scout_signal(state, &candle, &htf);
                            if let Some(s) = &best {
                                best_seen = Some((s.strategy, s.ta_confidence));
                                forced = true;
                            }
                        }
                        let filtered = apply_advanced_alpha(
                            best,
                            state,
                            feeds_by_symbol.get(&symbol),
                            &advanced_alpha,
                        );
                        (
                            filtered,
                            regime,
                            state.candles.len(),
                            chosen,
                            best_seen,
                            forced,
                        )
                    };
                    if let Some(signal) = best {
                        if forced {
                            info!(
                                symbol = %signal.symbol,
                                side = %signal.side.as_str(),
                                entry = signal.entry,
                                sl = signal.stop_loss,
                                tp = signal.take_profit,
                                confidence = signal.ta_confidence,
                                htf = %htf_summary(&htf),
                                "paper scout htf-aware scalp signal"
                            );
                        }
                        bus.publish(AgentEvent::PreSignalEmitted {
                            signal: Box::new(signal),
                            regime,
                        });
                    } else {
                        let (best_strategy, best_confidence) = best_seen
                            .map(|(strategy, confidence)| (Some(strategy), Some(confidence)))
                            .unwrap_or((None, None));
                        bus.publish(AgentEvent::SignalEvaluation(SignalEvaluationMsg {
                            symbol,
                            timeframe_secs,
                            regime: Some(regime),
                            candles,
                            strategies: chosen,
                            reason: no_signal_reason(candles, best_strategy, best_confidence),
                            best_strategy,
                            best_confidence,
                        }));
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

fn paper_scout_signal(
    state: &SymbolState,
    candle: &crate::data::Candle,
    higher_timeframes: &BTreeMap<i64, HigherTimeframeSnapshot>,
) -> Option<PreSignal> {
    if state.candles.len() < 3 {
        return None;
    }
    let atr = state
        .last_atr
        .unwrap_or_else(|| (candle.high - candle.low).abs().max(candle.close * 0.001));
    if candle.close <= 0.0 || atr <= 0.0 {
        return None;
    }
    let vwap = state.last_vwap.unwrap_or(candle.close);
    let bias = higher_timeframe_bias(higher_timeframes);
    let side = if bias > 0.0 || (bias == 0.0 && candle.close >= vwap) {
        Side::Long
    } else {
        Side::Short
    };
    let stop_distance = (0.6 * atr).max(candle.close * 0.001);
    let take_distance = stop_distance * 1.35;
    let (stop_loss, take_profit) = match side {
        Side::Long => (candle.close - stop_distance, candle.close + take_distance),
        Side::Short => (candle.close + stop_distance, candle.close - take_distance),
    };
    Some(PreSignal {
        symbol: state.symbol.clone(),
        strategy: StrategyName::VwapScalp,
        side,
        entry: candle.close,
        stop_loss,
        take_profit,
        ta_confidence: 60,
        reason: format!(
            "paper_scout htf_bias={:.2} close={:.4} vwap={:.4} atr={:.4}",
            bias, candle.close, vwap, atr
        ),
    })
}

#[derive(Debug, Clone, Copy)]
struct HigherTimeframeSnapshot {
    open: f64,
    close: f64,
}

impl HigherTimeframeSnapshot {
    fn from_candle(candle: crate::data::Candle) -> Self {
        Self {
            open: candle.open,
            close: candle.close,
        }
    }
}

fn higher_timeframe_bias(higher_timeframes: &BTreeMap<i64, HigherTimeframeSnapshot>) -> f64 {
    let total_weight: f64 = higher_timeframes.keys().map(|tf| *tf as f64).sum();
    if total_weight <= 0.0 {
        return 0.0;
    }
    let weighted = higher_timeframes
        .iter()
        .map(|(tf, snapshot)| {
            let direction = if snapshot.close > snapshot.open {
                1.0
            } else if snapshot.close < snapshot.open {
                -1.0
            } else {
                0.0
            };
            direction * *tf as f64
        })
        .sum::<f64>();
    (weighted / total_weight).clamp(-1.0, 1.0)
}

fn htf_summary(higher_timeframes: &BTreeMap<i64, HigherTimeframeSnapshot>) -> String {
    if higher_timeframes.is_empty() {
        return "none".to_string();
    }
    higher_timeframes
        .iter()
        .map(|(tf, snapshot)| {
            let direction = if snapshot.close > snapshot.open {
                "bull"
            } else if snapshot.close < snapshot.open {
                "bear"
            } else {
                "flat"
            };
            format!("{}m:{direction}", tf / 60)
        })
        .collect::<Vec<_>>()
        .join(",")
}

fn no_signal_reason(
    candles: usize,
    best_strategy: Option<StrategyName>,
    best_confidence: Option<u8>,
) -> String {
    if candles < 20 {
        return format!("warming_up_candles_{candles}/20");
    }
    match (best_strategy, best_confidence) {
        (Some(strategy), Some(confidence)) => {
            format!("alpha_gate_filtered_{}:{confidence}", strategy.as_str())
        }
        _ => "strategy_conditions_not_met".to_string(),
    }
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
    fn paper_scout_uses_higher_timeframe_bias() {
        let state = warmed_state();
        let candle = *state.last_candle().unwrap();
        let mut htf = BTreeMap::new();
        htf.insert(
            300,
            HigherTimeframeSnapshot {
                open: 100.0,
                close: 99.0,
            },
        );
        htf.insert(
            900,
            HigherTimeframeSnapshot {
                open: 100.0,
                close: 98.0,
            },
        );
        let signal = paper_scout_signal(&state, &candle, &htf).expect("paper scout signal");
        assert_eq!(signal.side, Side::Short);
        assert_eq!(signal.strategy, StrategyName::VwapScalp);
        assert!(signal.reason.contains("htf_bias=-1.00"));
        assert!(signal.rr() >= 1.2);
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
