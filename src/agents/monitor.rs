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

#[derive(Default, Clone, Copy)]
struct PriceSnapshot {
    price: f64,
    ts: Option<DateTime<Utc>>,
    ticks: u64,
}

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
    trades_total: u64,
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

fn short_sym(s: &str) -> &str {
    s.strip_suffix("USDT").unwrap_or(s)
}

fn fmt_prices_compact(map: &HashMap<String, PriceSnapshot>, now: DateTime<Utc>) -> String {
    if map.is_empty() {
        return "—".to_string();
    }
    let mut entries: Vec<(&String, &PriceSnapshot)> = map.iter().collect();
    entries.sort_by(|a, b| a.0.cmp(b.0));
    entries
        .iter()
        .map(|(sym, snap)| {
            let age = snap.ts.map(|t| (now - t).num_seconds().max(0)).unwrap_or(-1);
            let stale = if age > 10 { "⚠" } else { "" };
            format!("{}={:.2}{}", short_sym(sym), snap.price, stale)
        })
        .collect::<Vec<_>>()
        .join("  ")
}

fn fmt_candles_compact(map: &HashMap<String, u64>) -> String {
    if map.is_empty() {
        return "0".to_string();
    }
    let mut entries: Vec<(&String, &u64)> = map.iter().collect();
    entries.sort_by(|a, b| a.0.cmp(b.0));
    entries
        .iter()
        .map(|(s, n)| format!("{}:{}", short_sym(s), n))
        .collect::<Vec<_>>()
        .join(" ")
}

fn fmt_signal_compact(s: &Option<SignalSnapshot>) -> String {
    match s {
        Some(s) => format!(
            "{} {} {} @{}%",
            short_sym(&s.symbol),
            s.strategy.as_str(),
            s.side.as_str(),
            s.confidence
        ),
        None => "—".to_string(),
    }
}

fn fmt_block_compact(s: &Option<DecisionSnapshot>) -> String {
    match s {
        Some(s) => format!("{} {}: {}", short_sym(&s.symbol), s.stage, s.reason),
        None => "—".to_string(),
    }
}

fn fmt_brain_compact(s: &Option<DecisionSnapshot>) -> String {
    match s {
        Some(s) => format!("{} {}", short_sym(&s.symbol), s.reason),
        None => "—".to_string(),
    }
}

fn emit_status_line(line: impl std::fmt::Display) {
    info!("{}", line);
}

pub fn spawn(
    bus: MessageBus,
    metrics: Arc<MetricsState>,
    journal: Arc<TradeJournal>,
    telegram: Arc<TelegramNotifier>,
) -> JoinHandle<()> {
    let mut rx = bus.subscribe();
    let last_brain: Arc<PlMutex<HashMap<String, BrainOutcome>>> =
        Arc::new(PlMutex::new(HashMap::new()));
    let counters: Arc<PlMutex<StatusCounters>> = Arc::new(PlMutex::new(StatusCounters::default()));

    // Periodic status — every 60s, emit 4 compact lines instead of one giant blob
    {
        let counters = counters.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(60));
            tick.tick().await;
            loop {
                tick.tick().await;
                let c = counters.lock();
                let now = Utc::now();
                emit_status_line("┌─ ARIA STATUS ─────────────────────────────");
                emit_status_line(format_args!("│ 💹 {}", fmt_prices_compact(&c.prices, now)));
                emit_status_line(format_args!(
                    "│ 📊 candles={}  signals={}  allowed={}  blocked={}  fills={}  trades={}",
                    fmt_candles_compact(&c.candles),
                    c.signals,
                    c.risk_allowed,
                    c.risk_blocked,
                    c.orders_filled,
                    c.trades_total,
                ));
                emit_status_line(format_args!(
                    "│ 🧠 brain={}  go={}  nogo={}  wait={}  manager={}  vetoes={}",
                    c.brain_calls,
                    c.brain_go,
                    c.brain_nogo,
                    c.brain_wait,
                    c.manager_calls,
                    c.manager_vetoes,
                ));
                if c.last_signal.is_some() {
                    emit_status_line(format_args!("│ 🔍 last_signal : {}", fmt_signal_compact(&c.last_signal)));
                }
                if c.last_block.is_some() {
                    emit_status_line(format_args!("│ 🚫 last_block  : {}", fmt_block_compact(&c.last_block)));
                }
                if c.last_brain.is_some() {
                    emit_status_line(format_args!("│ 🤖 last_brain  : {}", fmt_brain_compact(&c.last_brain)));
                }
                if let Some(m) = c.last_manager.as_ref() {
                    emit_status_line(format_args!("│ 👔 last_manager: {} {}: {}", short_sym(&m.symbol), m.stage, m.reason));
                }
                emit_status_line("└───────────────────────────────────────────");
            }
        });
    }

    tokio::spawn(async move {
        info!("monitor agent starting");
        while let Ok(ev) = rx.recv().await {
            match ev {
                AgentEvent::Shutdown => break,
                AgentEvent::Tick { symbol, trade } => {
                    if trade.price <= 0.0 {
                        continue; // drop zero-price WS artifacts
                    }
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
                                symbol   = %risk.signal.symbol,
                                strategy = %risk.signal.strategy.as_str(),
                                side     = %risk.signal.side.as_str(),
                                size     = risk.size,
                                ta       = risk.signal.ta_confidence,
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
                                symbol   = %risk.signal.symbol,
                                strategy = %risk.signal.strategy.as_str(),
                                side     = %risk.signal.side.as_str(),
                                ta       = risk.signal.ta_confidence,
                                reason   = %reason,
                                "risk: blocked signal"
                            );
                        }
                    }
                }
                AgentEvent::BrainOutcomeReady(brain) => {
                    last_brain.lock().insert(brain.signal.symbol.clone(), brain.clone());
                    record_brain(&metrics, &brain);
                    let mut c = counters.lock();
                    c.brain_calls += 1;
                    match brain.decision.decision {
                        Decision::Go   => c.brain_go   += 1,
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
                    emit_status_line("┌─ ARIA STATUS (on-demand) ──────────────────");
                    emit_status_line(format_args!("│ 💹 {}", fmt_prices_compact(&c.prices, now)));
                    emit_status_line(format_args!(
                        "│ 📊 candles={}  signals={}  allowed={}  blocked={}  fills={}  trades={}",
                        fmt_candles_compact(&c.candles),
                        c.signals, c.risk_allowed, c.risk_blocked,
                        c.orders_filled, c.trades_total,
                    ));
                    emit_status_line(format_args!(
                        "│ 🧠 brain={}  go={}  nogo={}  wait={}  manager={}  vetoes={}",
                        c.brain_calls, c.brain_go, c.brain_nogo,
                        c.brain_wait, c.manager_calls, c.manager_vetoes,
                    ));
                    emit_status_line(format_args!("│ 🔍 last_signal : {}", fmt_signal_compact(&c.last_signal)));
                    emit_status_line(format_args!("│ 🚫 last_block  : {}", fmt_block_compact(&c.last_block)));
                    emit_status_line(format_args!("│ 🤖 last_brain  : {}", fmt_brain_compact(&c.last_brain)));
                    emit_status_line("└────────────────────────────────────────────");
                }
                AgentEvent::OrderFilled {
                    client_id,
                    symbol,
                    side,
                    size,
                    ack,
                } => {
                    // Scope lock strictly — must not cross .await boundary
                    let trade_no = {
                        let mut c = counters.lock();
                        c.orders_filled += 1;
                        c.trades_total  += 1;
                        c.trades_total
                    };

                    let brain = { last_brain.lock().get(&symbol).cloned() };
                    let (sl, tp, strategy) = brain.as_ref().map(|b| {
                        (b.signal.stop_loss, b.signal.take_profit, b.signal.strategy.as_str().to_string())
                    }).unwrap_or((0.0, 0.0, "—".to_string()));

                    if let Some(b) = &brain {
                        if let Err(e) = log_open_trade(
                            &journal, &client_id, &symbol, side, size, &ack, b,
                        ) {
                            warn!(error = %e, "monitor: insert_trade failed");
                        }
                    }

                    let side_label = if side == crate::data::Side::Long { "BUY" } else { "SELL" };
                    let sl_line = if sl > 0.0 { format!("{:.4}", sl) } else { "—".to_string() };
                    let tp_line = if tp > 0.0 { format!("{:.4}", tp) } else { "—".to_string() };

                    let msg = format!(
                        "🟢 <b>POSISI DIBUKA</b>\n\
                         ──────────\n\
                         📊 {} #{} · {}\n\
                         📍 Entry: <code>{:.4}</code>\n\
                         🛡 SL: <code>{}</code>\n\
                         🎯 TP: <code>{}</code>\n\
                         💼 Size: <code>{:.4}</code> {}\n\
                         🔧 Strategi: {}",
                        side_label,
                        trade_no,
                        short_sym(&symbol),
                        ack.avg_fill_price,
                        sl_line,
                        tp_line,
                        size,
                        short_sym(&symbol),
                        strategy,
                    );
                    let _ = telegram.send(&msg).await;
                }
                AgentEvent::PositionClosed {
                    client_id,
                    symbol,
                    side,
                    size,
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
                        reason.as_str(),
                        pnl_usd,
                        pnl_pct,
                        0.0,
                    ) {
                        warn!(error = %e, "monitor: close_trade failed");
                    }
                    metrics.update(|m| {
                        m.daily_pnl   += pnl_usd;
                        m.trades_today += 1;
                    });

                    let trade_no = { counters.lock().trades_total };
                    let side_label = if side == crate::data::Side::Long { "BUY" } else { "SELL" };
                    let is_win = pnl_usd > 0.0;

                    let header = match reason {
                        crate::execution::PositionExitReason::TakeProfit => "✅ <b>TAKE PROFIT HIT!</b>",
                        crate::execution::PositionExitReason::StopLoss   => "❌ <b>STOP LOSS HIT</b>",
                        crate::execution::PositionExitReason::Trailing   => "🔄 <b>TRAILING STOP</b>",
                        crate::execution::PositionExitReason::TimeExit   => "⏰ <b>TIME EXIT</b>",
                        crate::execution::PositionExitReason::Manual     => "🔧 <b>MANUAL CLOSE</b>",
                        crate::execution::PositionExitReason::Breakeven  => "🔒 <b>BREAKEVEN EXIT</b>",
                        crate::execution::PositionExitReason::PartialTP  => "🎯 <b>PARTIAL TAKE PROFIT</b>",
                    };
                    let result_line = if is_win {
                        "🏆 Result: <b>WIN</b>"
                    } else {
                        "📉 Result: <b>LOSS</b>"
                    };
                    let pnl_sign = if pnl_usd >= 0.0 { "+" } else { "" };

                    let msg = format!(
                        "{}\n\
                         ──────────\n\
                         📊 {} #{} · {}\n\
                         📍 Entry: <code>{:.4}</code>\n\
                         🏁 Exit:  <code>{:.4}</code>\n\
                         💼 Size:  <code>{:.4}</code> {}\n\
                         💰 PnL:   <code>{}{:.2}$</code> ({}{:.4}%)\n\
                         {}\n\
                         🤖 ARIA v1.0",
                        header,
                        side_label,
                        trade_no,
                        short_sym(&symbol),
                        entry_price,
                        exit_price,
                        size,
                        short_sym(&symbol),
                        pnl_sign,
                        pnl_usd,
                        pnl_sign,
                        pnl_pct.abs(),
                        result_line,
                    );
                    let _ = telegram.send(&msg).await;
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
            Decision::Go   => m.llm_go   += 1,
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
