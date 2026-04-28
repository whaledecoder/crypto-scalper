//! Brain agent — owns the existing LLM specialist. Listens for allowed
//! `RiskVerdict` events, builds a `MarketContext` (with the historical
//! summary injected), calls the LLM, and emits `BrainOutcomeReady`.

use crate::agents::messages::{
    AgentEvent, BrainOutcome, FeedsSnapshotMsg, ManagerProposal, RiskOutcome,
};
use crate::agents::MessageBus;
use crate::feeds::ExternalSnapshot;
use crate::learning::LearningPolicy;
use crate::llm::engine::LlmEngine;
use crate::llm::{ContextBuilder, Decision};
use crate::strategy::state::SymbolState;
use parking_lot::RwLock as PlRwLock;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tracing::{info, warn};

pub fn spawn(
    bus: MessageBus,
    llm: Arc<LlmEngine>,
    states: Arc<Mutex<HashMap<String, SymbolState>>>,
    policy: LearningPolicy,
    feeds_cache: Arc<PlRwLock<HashMap<String, ExternalSnapshot>>>,
) -> JoinHandle<()> {
    let mut rx = bus.subscribe();
    tokio::spawn(async move {
        info!("brain agent starting");
        while let Ok(ev) = rx.recv().await {
            match ev {
                AgentEvent::FeedsSnapshot(FeedsSnapshotMsg {
                    symbol, snapshot, ..
                }) => {
                    feeds_cache.write().insert(symbol, snapshot);
                }
                AgentEvent::RiskVerdict(risk) => {
                    if risk.outcome != RiskOutcome::Allowed {
                        continue;
                    }
                    let signal = (*risk.signal).clone();
                    let regime = risk.regime;
                    let symbol = signal.symbol.clone();

                    let external = feeds_cache.read().get(&symbol).cloned().unwrap_or_default();

                    let mut ctx = {
                        let states = states.lock().await;
                        match states.get(&symbol) {
                            Some(s) => ContextBuilder::build(s, regime, &signal, external),
                            None => continue,
                        }
                    };
                    ctx.historical_summary = policy.historical_summary(
                        signal.strategy.as_str(),
                        regime.as_str(),
                        &symbol,
                    );

                    let llm_out = match llm.analyze(&ctx).await {
                        Ok(o) => o,
                        Err(e) => {
                            warn!(error = %e, "brain agent: LLM call failed");
                            continue;
                        }
                    };

                    let _proposal = ManagerProposal {
                        symbol: symbol.clone(),
                        side: signal.side,
                        strategy: signal.strategy.as_str().to_string(),
                        regime: regime.as_str().to_string(),
                        entry: llm_out.decision.entry_price.unwrap_or(signal.entry),
                        stop_loss: signal.stop_loss,
                        take_profit: signal.take_profit,
                        size: risk.size,
                        ta_confidence: signal.ta_confidence,
                        llm_confidence: llm_out.decision.confidence,
                    };

                    if llm_out.decision.decision != Decision::Go {
                        info!(
                            symbol = %symbol,
                            decision = ?llm_out.decision.decision,
                            "brain agent: rejected"
                        );
                        // Still publish the outcome for observability.
                    }

                    bus.publish(AgentEvent::BrainOutcomeReady(BrainOutcome {
                        signal: Box::new(signal),
                        regime,
                        risk: risk.clone(),
                        decision: llm_out.decision,
                        latency_ms: llm_out.latency_ms,
                        offline_fallback: llm_out.offline_fallback,
                    }));
                }
                AgentEvent::Shutdown => break,
                _ => {}
            }
        }
    })
}
