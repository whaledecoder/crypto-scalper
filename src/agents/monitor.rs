//! Monitor agent — fans out events to the metrics state, the trade
//! journal, and Telegram. The other agents stay focused on their
//! domain; the Monitor is the only place where observability concerns
//! live.

use crate::agents::messages::{AgentEvent, BrainOutcome, ControlCommand, ManagerAction};
use crate::agents::MessageBus;
use crate::llm::engine::Decision;
use crate::monitoring::{MetricsState, TelegramNotifier, TradeJournal, TradeRecord};
use crate::strategy::state::StrategyName;
use chrono::{DateTime, Utc};
use parking_lot::Mutex as PlMutex;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::task::JoinHandle;
use tracing::{info, warn};

/// Per-symbol snapshot used by the status log so the operator can see
/// current price and how stale the last tick is.
#[derive(Default, Clone, Copy)]
struct PriceSnapshot {
    price: f64,
    ts: Option<DateTime<Utc>>,
    ticks: u64,
}

/// Running counters used by the periodic status log so the operator
/// can see at a glance that the bot is actually receiving market
/// data and evaluating strategies (it is otherwise silent for minutes
/// at a time on 5m timeframes).
#[derive(Default)]
struct StatusCounters {
    prices: HashMap<String, PriceSnapshot>,
    candles: HashMap<String, u64>,
    signals: u64,
    risk_allowed: u64,
    risk_blocked: u64,
    brain_calls: u64,
    brain_go: u64,
    brain_nogo: u64,
    brain_wait: u64,
    manager_calls: u64,
    manager_vetoes: u64,
    orders_filled: u64,
    last_signal: Option<SignalSnapshot>,
    last_block: Option<DecisionSnapshot>,
    last_brain: Option<DecisionSnapshot>,
    last_manager: Option<DecisionSnapshot>,
}

#[derive(Clone)]
struct SignalSnapshot {
    symbol: String,
    strategy: StrategyName,
    side: crate::data::Side,
    confidence: u8,
}

#[derive(Clone)]
struct DecisionSnapshot {
    symbol: String,
    stage: &'static str,
    reason: String,
}

fn fmt_counts(map: &HashMap<String, u64>) -> String {
    if map.is_empty() {
        return "-".to_string();
    }
    let mut entries: Vec<(&String, &u64)> = map.iter().collect();
    entries.sort_by(|a, b| a.0.cmp(b.0));
    entries
        .iter()
        .map(|(s, n)| format!("{}:{}", s, n))
        .collect::<Vec<_>>()
        .join(",")
}

/// Trim the common "USDT" suffix so the log stays readable: BTCUSDT -> BTC.
fn short_sym(s: &str) -> &str {
    s.strip_suffix("USDT").unwrap_or(s)
}

/// Render the price snapshot map. Includes price, staleness (how old
/// the last tick is in seconds at log-time) and tick count.
fn fmt_prices(map: &HashMap<String, PriceSnapshot>, now: DateTime<Utc>) -> String {
    if map.is_empty() {
        return "-".to_string();
    }
    let mut entries: Vec<(&String, &PriceSnapshot)> = map.iter().collect();
    entries.sort_by(|a, b| a.0.cmp(b.0));
    entries
        .iter()
        .map(|(sym, snap)| {
            let age = snap
                .ts
                .map(|t| (now - t).num_seconds().max(0))
                .unwrap_or(-1);
            format!(
                "{}={:.2}({}s/{}t)",
                short_sym(sym),
                snap.price,
                age,
                snap.ticks,
            )
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn fmt_signal(s: &Option<SignalSnapshot>) -> String {
    match s {
        Some(s) => format!(
            "{}:{}:{}:{}",
            s.symbol,
            s.strategy.as_str(),
            s.side.as_str(),
            s.confidence
        ),
        None => "-".to_string(),
    }
}

fn fmt_decision(s: &Option<DecisionSnapshot>) -> String {
    match s {
        Some(s) => format!("{}:{}:{}", s.symbol, s.stage, s.reason),
        None => "-".to_string(),
    }
}

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
    let counters: Arc<PlMutex<StatusCounters>> = Arc::new(PlMutex::new(StatusCounters::default()));

    // Periodic status log — every 30s emit a single INFO line summarising
    // ws/data/signal/brain/manager activity. Quiet bot does not mean dead bot.
    {
        let counters = counters.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(30));
            // Skip the immediate first tick — counters would all be zero.
            tick.tick().await;
            loop {
                tick.tick().await;
                let c = counters.lock();
                let now = Utc::now();
                info!(
                    prices = %fmt_prices(&c.prices, now),
                    candles = %fmt_counts(&c.candles),
                    signals = c.signals,
                    risk_allowed = c.risk_allowed,
                    risk_blocked = c.risk_blocked,
                    brain = c.brain_calls,
                    brain_go = c.brain_go,
                    brain_nogo = c.brain_nogo,
                    brain_wait = c.brain_wait,
                    manager = c.manager_calls,
                    vetoes = c.manager_vetoes,
                    fills = c.orders_filled,
                    last_signal = %fmt_signal(&c.last_signal),
                    last_block = %fmt_decision(&c.last_block),
                    last_brain = %fmt_decision(&c.last_brain),
                    last_manager = %fmt_decision(&c.last_manager),
                    "status"
                );
            }
        });
    }

    tokio::spawn(async move {
        info!("monitor agent starting");
        while let Ok(ev) = rx.recv().await {
            match ev {
                AgentEvent::Shutdown => break,
                AgentEvent::Tick { symbol, trade } => {
                    let mut c = counters.lock();
                    let snap = c.prices.entry(symbol).or_default();
                    snap.price = trade.price;
                    snap.ts = Some(trade.ts);
                    snap.ticks += 1;
                }
                AgentEvent::CandleClosed { symbol, .. } => {
                    *counters.lock().candles.entry(symbol).or_insert(0) += 1;
                }
                AgentEvent::PreSignalEmitted { signal, .. } => {
                    metrics.update(|m| m.signals_today += 1);
                    let mut c = counters.lock();
                    c.signals += 1;
                    c.last_signal = Some(SignalSnapshot {
                        symbol: signal.symbol.clone(),
                        strategy: signal.strategy,
                        side: signal.side,
                        confidence: signal.ta_confidence,
                    });
                }
                AgentEvent::RiskVerdict(risk) => {
                    let mut c = counters.lock();
                    match risk.outcome {
                        crate::agents::messages::RiskOutcome::Allowed => {
                            c.risk_allowed += 1;
                            c.last_block = None;
                            info!(
                                symbol = %risk.signal.symbol,
                                strategy = %risk.signal.strategy.as_str(),
                                side = %risk.signal.side.as_str(),
                                size = risk.size,
                                ta = risk.signal.ta_confidence,
                                ta_threshold = risk.effective_ta_threshold,
                                llm_floor = risk.effective_llm_floor,
                                "risk: allowed signal"
                            );
                        }
                        crate::agents::messages::RiskOutcome::Blocked => {
                            c.risk_blocked += 1;
                            let reason = risk.reason.clone().unwrap_or_else(|| "blocked".into());
                            c.last_block = Some(DecisionSnapshot {
                                symbol: risk.signal.symbol.clone(),
                                stage: "risk",
                                reason: reason.clone(),
                            });
                            info!(
                                symbol = %risk.signal.symbol,
                                strategy = %risk.signal.strategy.as_str(),
                                side = %risk.signal.side.as_str(),
                                ta = risk.signal.ta_confidence,
                                ta_threshold = risk.effective_ta_threshold,
                                llm_floor = risk.effective_llm_floor,
                                reason = %reason,
                                "risk: blocked signal"
                            );
                        }
                    }
                }
                AgentEvent::BrainOutcomeReady(brain) => {
                    last_brain
                        .lock()
                        .insert(brain.signal.symbol.clone(), brain.clone());
                    record_brain(&metrics, &brain);
                    let mut c = counters.lock();
                    c.brain_calls += 1;
                    match brain.decision.decision {
                        Decision::Go => c.brain_go += 1,
                        Decision::NoGo => c.brain_nogo += 1,
                        Decision::Wait => c.brain_wait += 1,
                    }
                    c.last_brain = Some(DecisionSnapshot {
                        symbol: brain.signal.symbol.clone(),
                        stage: "brain",
                        reason: format!(
                            "{:?}/{}: {}",
                            brain.decision.decision,
                            brain.decision.confidence,
                            brain.decision.reasoning.summary
                        ),
                    });
                }
                AgentEvent::ManagerVerdictEmitted(v) => {
                    {
                        let mut c = counters.lock();
                        c.manager_calls += 1;
                        if matches!(v.action, ManagerAction::Veto { .. }) {
                            c.manager_vetoes += 1;
                        }
                        c.last_manager = Some(DecisionSnapshot {
                            symbol: v.proposal.symbol.clone(),
                            stage: "manager",
                            reason: manager_action_summary(&v.action),
                        });
                    }
                    if matches!(v.action, ManagerAction::Veto { .. }) {
                        info!(
                            symbol = %v.proposal.symbol,
                            reason = %manager_action_summary(&v.action),
                            "monitor: trade vetoed by manager"
                        );
                    }
                }
                AgentEvent::ControlCommand(ControlCommand::StatusRequest) => {
                    let c = counters.lock();
                    let now = Utc::now();
                    info!(
                        prices = %fmt_prices(&c.prices, now),
                        candles = %fmt_counts(&c.candles),
                        signals = c.signals,
                        risk_allowed = c.risk_allowed,
                        risk_blocked = c.risk_blocked,
                        brain = c.brain_calls,
                        brain_go = c.brain_go,
                        brain_nogo = c.brain_nogo,
                        brain_wait = c.brain_wait,
                        manager = c.manager_calls,
                        vetoes = c.manager_vetoes,
                        fills = c.orders_filled,
                        last_signal = %fmt_signal(&c.last_signal),
                        last_block = %fmt_decision(&c.last_block),
                        last_brain = %fmt_decision(&c.last_brain),
                        last_manager = %fmt_decision(&c.last_manager),
                        "status requested"
                    );
                }
                AgentEvent::OrderFilled {
                    client_id,
                    symbol,
                    side,
                    size,
                    ack,
                } => {
                    counters.lock().orders_filled += 1;
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

fn manager_action_summary(action: &ManagerAction) -> String {
    match action {
        ManagerAction::Approve => "approve".to_string(),
        ManagerAction::Veto { reason } => format!("veto: {reason}"),
        ManagerAction::Adjust {
            size_multiplier,
            sl_offset_bps,
            tp_offset_bps,
            reason,
        } => format!(
            "adjust size={size_multiplier:.2} sl={sl_offset_bps:.1}bps tp={tp_offset_bps:.1}bps: {reason}"
        ),
    }
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
