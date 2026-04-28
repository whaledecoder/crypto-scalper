//! Risk agent — listens for `PreSignalEmitted`, applies the existing
//! 8-gate `RiskManager` plus the `LearningPolicy` verdict, sizes the
//! trade, and publishes a `RiskVerdict` event.

use crate::agents::messages::{AgentEvent, RiskOutcome, RiskVerdictMsg};
use crate::agents::MessageBus;
use crate::execution::RiskManager;
use crate::learning::LearningPolicy;
use std::sync::Arc;
use tokio::task::JoinHandle;
use tracing::{info, warn};

pub fn spawn(
    bus: MessageBus,
    risk: Arc<RiskManager>,
    policy: LearningPolicy,
    base_min_ta_threshold: u8,
    base_min_llm_floor: u8,
) -> JoinHandle<()> {
    let mut rx = bus.subscribe();
    tokio::spawn(async move {
        info!("risk agent starting");
        while let Ok(ev) = rx.recv().await {
            if matches!(ev, AgentEvent::Shutdown) {
                break;
            }
            let AgentEvent::PreSignalEmitted { signal, regime } = ev else {
                continue;
            };
            let verdict =
                policy.evaluate(signal.strategy.as_str(), regime.as_str(), &signal.symbol);
            let effective_ta_threshold = (base_min_ta_threshold as i32
                + verdict.ta_threshold_delta as i32)
                .clamp(0, 100) as u8;
            let llm_floor = verdict
                .llm_min_confidence_floor
                .unwrap_or(base_min_llm_floor)
                .max(base_min_llm_floor);

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
                        "TA {} < threshold {}",
                        signal.ta_confidence, effective_ta_threshold
                    )),
                }));
                continue;
            }

            if let Err(e) = risk.can_open_position() {
                warn!(symbol = %signal.symbol, reason = %e, "risk gate blocked");
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

            let base_size = risk.calculate_size(signal.entry, signal.stop_loss);
            let size = base_size * verdict.size_multiplier;
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
    })
}
