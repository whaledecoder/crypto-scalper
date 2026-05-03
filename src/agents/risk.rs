//! Risk agent — listens for `PreSignalEmitted`, applies the existing
//! 8-gate `RiskManager` plus the `LearningPolicy` verdict, sizes the
//! trade, and publishes a `RiskVerdict` event.
//!
//! The agent additionally reads the latest [`SurvivalState`] (set by
//! the SurvivalAgent) and the latest funding rate from `FeedsSnapshot`
//! to apply two extra gates before sizing:
//!
//! * **Survival gate** — if the bot is in `Frozen` or `Dead` mode it
//!   refuses every entry.
//! * **Funding gate** — extreme funding rates are a strong sign of a
//!   one-sided crowd; we block longs when funding > +0.1% and shorts
//!   when funding < -0.1% (configurable). Default thresholds are
//!   wide enough to never bite under normal conditions but tight
//!   enough to dodge a funding-flush.

use crate::agents::messages::{
    AgentEvent, FeedsSnapshotMsg, RiskOutcome, RiskVerdictMsg, SurvivalMode, SurvivalState,
};
use crate::agents::MessageBus;
use crate::data::Side;
use crate::execution::tcm::TransactionCostModel;
use crate::execution::RiskManager;
use crate::learning::LearningPolicy;
use crate::quant::{QuantEngine, QuantSizingInput};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::task::JoinHandle;
use tracing::{info, warn};

#[derive(Debug, Clone)]
pub struct RiskAgentConfig {
    pub base_min_ta_threshold: u8,
    pub base_min_llm_floor: u8,
    /// Funding rate threshold beyond which we reject same-direction
    /// trades. Binance reports funding as a fraction (e.g. 0.0005 ==
    /// 0.05%). Default 0.001 = 0.1%.
    pub funding_block_threshold: f64,
    pub tcm: TransactionCostModel,
    /// Base risk per trade % — passed to the quant engine for Kelly
    /// comparison.  Default 0.5%.
    pub base_risk_pct: f64,
}

impl Default for RiskAgentConfig {
    fn default() -> Self {
        Self {
            // Lower thresholds for HFT scalping — the strategies already
            // score conservatively (62-68 base), so a 60 TA threshold
            // lets most valid signals through.
            base_min_ta_threshold: 60,
            // LLM floor: accept signals where LLM confidence is >= 50.
            // The brain LLM prompt now defaults to GO for composite >= 50.
            base_min_llm_floor: 50,
            funding_block_threshold: 0.001,
            tcm: TransactionCostModel {
                taker_fee_bps: 4.0,
                maker_fee_bps: -1.0,
                avg_slippage_bps: 2.0,
                market_impact_bps: 1.0,
            },
            base_risk_pct: 0.5,
        }
    }
}

pub fn spawn(
    bus: MessageBus,
    risk: Arc<RiskManager>,
    policy: LearningPolicy,
    cfg: RiskAgentConfig,
    quant_engine: Option<Arc<QuantEngine>>,
) -> JoinHandle<()> {
    let mut rx = bus.subscribe();
    let survival: Arc<Mutex<Option<SurvivalState>>> = Arc::new(Mutex::new(None));
    let funding: Arc<Mutex<HashMap<String, f64>>> = Arc::new(Mutex::new(HashMap::new()));
    let spreads: Arc<Mutex<HashMap<String, f64>>> = Arc::new(Mutex::new(HashMap::new()));

    tokio::spawn(async move {
        info!("risk agent starting");
        while let Ok(ev) = rx.recv().await {
            match ev {
                AgentEvent::Shutdown => break,
                AgentEvent::SurvivalUpdated(s) => {
                    *survival.lock() = Some(s);
                    continue;
                }
                AgentEvent::FeedsSnapshot(FeedsSnapshotMsg {
                    symbol, snapshot, ..
                }) => {
                    if let Some(f) = &snapshot.funding {
                        funding.lock().insert(symbol, f.rate);
                    }
                    continue;
                }
                AgentEvent::BookTicker {
                    symbol,
                    best_bid,
                    bid_qty: _,
                    best_ask,
                    ask_qty: _,
                } => {
                    let mid = (best_bid + best_ask) / 2.0;
                    if mid > 0.0 && best_ask >= best_bid {
                        spreads
                            .lock()
                            .insert(symbol, (best_ask - best_bid) / mid * 100.0);
                    }
                    continue;
                }
                AgentEvent::PreSignalEmitted { signal, regime } => {
                    // Survival hard-gate: refuse outright when frozen or dead.
                    let surv = survival.lock().clone();
                    if let Some(s) = &surv {
                        if matches!(s.mode, SurvivalMode::Frozen | SurvivalMode::Dead) {
                            bus.publish(AgentEvent::RiskVerdict(RiskVerdictMsg {
                                signal: signal.clone(),
                                regime,
                                outcome: RiskOutcome::Blocked,
                                size: 0.0,
                                size_multiplier: 0.0,
                                effective_ta_threshold: cfg.base_min_ta_threshold,
                                effective_llm_floor: cfg.base_min_llm_floor,
                                matched_lessons: vec![],
                                reason: Some(format!("survival {}", s.mode.as_str())),
                            }));
                            continue;
                        }
                    }

                    let verdict =
                        policy.evaluate(signal.strategy.as_str(), regime.as_str(), &signal.symbol);
                    let effective_ta_threshold = (cfg.base_min_ta_threshold as i32
                        + verdict.ta_threshold_delta as i32)
                        .clamp(0, 100) as u8;
                    let llm_floor = verdict
                        .llm_min_confidence_floor
                        .unwrap_or(cfg.base_min_llm_floor)
                        .max(cfg.base_min_llm_floor);

                    // Hard policy block?
                    if !verdict.allowed {
                        bus.publish(AgentEvent::RiskVerdict(RiskVerdictMsg {
                            signal: signal.clone(),
                            regime,
                            outcome: RiskOutcome::Blocked,
                            size: 0.0,
                            size_multiplier: 0.0,
                            effective_ta_threshold,
                            effective_llm_floor: llm_floor,
                            matched_lessons: verdict.matched_lessons,
                            reason: Some("learning policy blocked".into()),
                        }));
                        continue;
                    }

                    if signal.ta_confidence < effective_ta_threshold {
                        bus.publish(AgentEvent::RiskVerdict(RiskVerdictMsg {
                            signal: signal.clone(),
                            regime,
                            outcome: RiskOutcome::Blocked,
                            size: 0.0,
                            size_multiplier: verdict.size_multiplier,
                            effective_ta_threshold,
                            effective_llm_floor: llm_floor,
                            matched_lessons: verdict.matched_lessons,
                            reason: Some(format!(
                                "TA {} < {}",
                                signal.ta_confidence, effective_ta_threshold
                            )),
                        }));
                        continue;
                    }

                    if let Err(e) = risk.can_open_position() {
                        warn!(symbol = %signal.symbol, reason = %e, "risk blocked");
                        bus.publish(AgentEvent::RiskVerdict(RiskVerdictMsg {
                            signal: signal.clone(),
                            regime,
                            outcome: RiskOutcome::Blocked,
                            size: 0.0,
                            size_multiplier: verdict.size_multiplier,
                            effective_ta_threshold,
                            effective_llm_floor: llm_floor,
                            matched_lessons: verdict.matched_lessons,
                            reason: Some(e.to_string()),
                        }));
                        continue;
                    }

                    let spread_pct = spreads.lock().get(&signal.symbol).copied();

                    if let Err(e) = risk.validate_signal(
                        signal.entry,
                        signal.stop_loss,
                        signal.take_profit,
                        spread_pct,
                        &cfg.tcm,
                    ) {
                        warn!(symbol = %signal.symbol, reason = %e, "risk blocked");
                        bus.publish(AgentEvent::RiskVerdict(RiskVerdictMsg {
                            signal: signal.clone(),
                            regime,
                            outcome: RiskOutcome::Blocked,
                            size: 0.0,
                            size_multiplier: verdict.size_multiplier,
                            effective_ta_threshold,
                            effective_llm_floor: llm_floor,
                            matched_lessons: verdict.matched_lessons,
                            reason: Some(e),
                        }));
                        continue;
                    }

                    // Funding-rate gate.
                    let funding_rate = funding.lock().get(&signal.symbol).copied().unwrap_or(0.0);
                    let funding_blocks = match signal.side {
                        Side::Long => funding_rate >= cfg.funding_block_threshold,
                        Side::Short => funding_rate <= -cfg.funding_block_threshold,
                    };
                    if funding_blocks {
                        bus.publish(AgentEvent::RiskVerdict(RiskVerdictMsg {
                            signal: signal.clone(),
                            regime,
                            outcome: RiskOutcome::Blocked,
                            size: 0.0,
                            size_multiplier: verdict.size_multiplier,
                            effective_ta_threshold,
                            effective_llm_floor: llm_floor,
                            matched_lessons: verdict.matched_lessons,
                            reason: Some(format!("funding {:.4}%", funding_rate * 100.0)),
                        }));
                        continue;
                    }

                    // RiskManager.calculate_size already multiplies by
                    // the SurvivalAgent-controlled size_multiplier.
                    let base_size = risk.calculate_size(signal.entry, signal.stop_loss);

                    // Apply quant engine sizing (Kelly, vol-target, VaR, Kalman)
                    let (size, _quant_reason) = if let Some(ref qe) = quant_engine {
                        let qr = qe.compute_sizing(QuantSizingInput {
                            symbol: &signal.symbol,
                            strategy: signal.strategy.as_str(),
                            side: signal.side,
                            entry: signal.entry,
                            stop_loss: signal.stop_loss,
                            equity: risk.equity(),
                            base_risk_pct: cfg.base_risk_pct,
                        });
                        if qr.var_rejected {
                            bus.publish(AgentEvent::RiskVerdict(RiskVerdictMsg {
                                signal: signal.clone(),
                                regime,
                                outcome: RiskOutcome::Blocked,
                                size: 0.0,
                                size_multiplier: verdict.size_multiplier,
                                effective_ta_threshold,
                                effective_llm_floor: llm_floor,
                                matched_lessons: verdict.matched_lessons,
                                reason: Some(format!("quant VaR cap: {}", qr.reason)),
                            }));
                            continue;
                        }
                        let adjusted = base_size * verdict.size_multiplier * qr.size_multiplier;
                        (adjusted, qr.reason)
                    } else {
                        (base_size * verdict.size_multiplier, String::new())
                    };

                    if size <= 0.0 {
                        bus.publish(AgentEvent::RiskVerdict(RiskVerdictMsg {
                            signal: signal.clone(),
                            regime,
                            outcome: RiskOutcome::Blocked,
                            size: 0.0,
                            size_multiplier: verdict.size_multiplier,
                            effective_ta_threshold,
                            effective_llm_floor: llm_floor,
                            matched_lessons: verdict.matched_lessons,
                            reason: Some("size <= 0".into()),
                        }));
                        continue;
                    }
                    bus.publish(AgentEvent::RiskVerdict(RiskVerdictMsg {
                        signal,
                        regime,
                        outcome: RiskOutcome::Allowed,
                        size,
                        size_multiplier: verdict.size_multiplier,
                        effective_ta_threshold,
                        effective_llm_floor: llm_floor,
                        matched_lessons: verdict.matched_lessons,
                        reason: None,
                    }));
                }
                _ => {}
            }
        }
    })
}
