//! Monitor agent — fans out events to the metrics state, the trade
//! journal, and Telegram. The other agents stay focused on their
//! domain; the Monitor is the only place where observability concerns
//! live.

use crate::agents::messages::{AgentEvent, BrainOutcome, ManagerAction};
use crate::agents::MessageBus;
use crate::llm::engine::Decision;
use crate::monitoring::{MetricsState, TelegramNotifier, TradeJournal, TradeRecord};
use chrono::Utc;
use parking_lot::Mutex as PlMutex;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::task::JoinHandle;
use tracing::{info, warn};

pub fn spawn(
    bus: MessageBus,
    metrics: Arc<MetricsState>,
    journal: Arc<TradeJournal>,
    telegram: Arc<TelegramNotifier>,
) -> JoinHandle<()> {
    let mut rx = bus.subscribe();
    // Cache the most recent BrainOutcome per symbol so we can attach
    // its reasoning to a trade record once the order actually fills.
    let last_brain: Arc<PlMutex<HashMap<String, BrainOutcome>>> =
        Arc::new(PlMutex::new(HashMap::new()));
    tokio::spawn(async move {
        info!("monitor agent starting");
        while let Ok(ev) = rx.recv().await {
            match ev {
                AgentEvent::Shutdown => break,
                AgentEvent::PreSignalEmitted { .. } => {
                    metrics.update(|m| m.signals_today += 1);
                }
                AgentEvent::BrainOutcomeReady(brain) => {
                    last_brain
                        .lock()
                        .insert(brain.signal.symbol.clone(), brain.clone());
                    record_brain(&metrics, &brain);
                }
                AgentEvent::ManagerVerdictEmitted(v) => {
                    if matches!(v.action, ManagerAction::Veto { .. }) {
                        info!(
                            symbol = %v.proposal.symbol,
                            "monitor: trade vetoed by manager"
                        );
                    }
                }
                AgentEvent::OrderFilled {
                    client_id,
                    symbol,
                    side,
                    size,
                    ack,
                } => {
                    let brain = last_brain.lock().get(&symbol).cloned();
                    if let Some(brain) = brain {
                        if let Err(e) =
                            log_open_trade(&journal, &client_id, &symbol, side, size, &ack, &brain)
                        {
                            warn!(error = %e, "monitor: insert_trade failed");
                        }
                    }
                    let _ = telegram
                        .send(&format!(
                            "🟢 *OPEN* `{}` {} size `{:.4}` @ `{:.2}`",
                            symbol,
                            side.as_str(),
                            size,
                            ack.avg_fill_price
                        ))
                        .await;
                }
                AgentEvent::PositionClosed {
                    client_id,
                    symbol,
                    side,
                    size: _,
                    entry_price,
                    exit_price,
                    pnl_usd,
                    reason,
                } => {
                    let pnl_pct = if entry_price > 0.0 {
                        (exit_price - entry_price) / entry_price * 100.0
                    } else {
                        0.0
                    };
                    if let Err(e) = journal.close_trade(
                        &client_id,
                        Utc::now(),
                        exit_price,
                        &format!("{:?}", reason),
                        pnl_usd,
                        pnl_pct,
                        0.0,
                    ) {
                        warn!(error = %e, "monitor: close_trade failed");
                    }
                    metrics.update(|m| {
                        m.daily_pnl += pnl_usd;
                        m.trades_today += 1;
                    });
                    let _ = telegram
                        .send(&format!(
                            "🔴 *CLOSE* `{}` {} pnl `{:+.2}$` ({:?})",
                            symbol,
                            side.as_str(),
                            pnl_usd,
                            reason
                        ))
                        .await;
                }
                AgentEvent::PolicyRefreshed { lessons_count, .. } => {
                    metrics.update(|m| m.active_lessons = lessons_count as u64);
                }
                _ => {}
            }
        }
    })
}

fn record_brain(metrics: &MetricsState, brain: &BrainOutcome) {
    metrics.update(|m| {
        let n = m.llm_go + m.llm_nogo + m.llm_wait;
        let avg = m.llm_avg_confidence * n as f64 + brain.decision.confidence as f64;
        match brain.decision.decision {
            Decision::Go => m.llm_go += 1,
            Decision::NoGo => m.llm_nogo += 1,
            Decision::Wait => m.llm_wait += 1,
        }
        m.llm_avg_confidence = avg / ((n + 1) as f64).max(1.0);
        let total = m.llm_go + m.llm_nogo + m.llm_wait;
        m.llm_avg_latency_ms =
            (m.llm_avg_latency_ms * total.saturating_sub(1) + brain.latency_ms) / total.max(1);
        if brain.offline_fallback {
            m.llm_offline_fallbacks += 1;
        }
    });
}

fn log_open_trade(
    journal: &TradeJournal,
    client_id: &str,
    symbol: &str,
    side: crate::data::Side,
    size: f64,
    ack: &crate::execution::OrderAck,
    brain: &BrainOutcome,
) -> anyhow::Result<()> {
    let signal = &brain.signal;
    let record = TradeRecord {
        client_order_id: client_id.to_string(),
        symbol: symbol.to_string(),
        direction: side.as_str().to_string(),
        strategy: signal.strategy.as_str().to_string(),
        market_regime: brain.regime.as_str().to_string(),
        entry_time: Utc::now(),
        entry_price: ack.avg_fill_price,
        size,
        stop_loss: signal.stop_loss,
        take_profit: signal.take_profit,
        exit_time: None,
        exit_price: None,
        exit_reason: None,
        pnl_usd: None,
        pnl_pct: None,
        fees_paid: Some(ack.fee_usd),
        ta_confidence: Some(signal.ta_confidence),
        rsi: None,
        adx: None,
        vwap_delta_pct: None,
        ema_alignment: Some(brain.regime.as_str().to_string()),
        llm_model: Some(brain.decision.direction.clone()),
        llm_decision: Some(format!("{:?}", brain.decision.decision)),
        llm_confidence: Some(brain.decision.confidence),
        llm_ta_score: Some(brain.decision.market_context_score.ta_score),
        llm_sentiment_score: Some(brain.decision.market_context_score.sentiment_score),
        llm_fundamental_score: Some(brain.decision.market_context_score.fundamental_score),
        llm_composite: Some(brain.decision.market_context_score.composite_score),
        llm_summary: Some(brain.decision.reasoning.summary.clone()),
        llm_ta_analysis: Some(brain.decision.reasoning.ta_analysis.clone()),
        llm_sentiment: Some(brain.decision.reasoning.sentiment_analysis.clone()),
        llm_fundamental: Some(brain.decision.reasoning.fundamental_analysis.clone()),
        llm_risks: Some(brain.decision.reasoning.risk_factors.clone()),
        llm_invalidation: Some(brain.decision.reasoning.invalidation.clone()),
        llm_latency_ms: Some(brain.latency_ms),
        fear_greed: None,
        social_sentiment: None,
        funding_rate: None,
        news_score: None,
        top_news_titles: None,
    };
    journal.insert_trade(&record)?;
    Ok(())
}
