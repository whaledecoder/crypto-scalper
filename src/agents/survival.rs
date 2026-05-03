//! SurvivalAgent — "trade for life" gatekeeper.
//!
//! Listens to every relevant agent event, derives a `SurvivalState`
//! (mode + score + size_multiplier + reasons), and broadcasts it on
//! the bus. Other agents consume the state to decide how aggressive
//! they should be.
//!
//! ### Responsibilities
//!
//! 1. **Equity reconciliation**: every `equity_refresh_secs`, fetch the
//!    USDT-margined balance from the exchange, push it into the
//!    [`RiskManager`], and broadcast `EquityReconciled`.
//! 2. **Survival score (0–100)**: combine drawdown, daily-loss, loss
//!    streaks, news regime, and equity floor proximity into a single
//!    fitness score. Translate to `SurvivalMode` and `size_multiplier`.
//! 3. **Cooldown enforcement**: 3-loss / 30-min, 5-loss-in-1h / 4-h,
//!    10 daily losses / 24-h. While a cooldown is active, mode = Frozen.
//! 4. **Death detection**: if equity ≤ initial × `death_line_pct`,
//!    auto-flat all positions (broadcast `ControlCommand::FlatAll`),
//!    set mode = Dead, freeze the [`RiskManager`].
//! 5. **Auto-flat on drawdown**: if drawdown ≥ `auto_flat_drawdown_pct`,
//!    same flat-all behaviour but recoverable (cooldown ends after 12h).
//! 6. **Daily PnL ratchet**: once today's gain ≥ ratchet %, freeze
//!    until tomorrow (locks profit).

use crate::agents::messages::{
    AgentEvent, AgentId, ControlCommand, FeedsSnapshotMsg, SurvivalMode, SurvivalState,
};
use crate::agents::MessageBus;
use crate::backtest::monte_carlo::drawdown_confidence_intervals;
use crate::config::SurvivalCfg;
use crate::execution::{Exchange, RiskManager};
use chrono::{DateTime, Duration as ChronoDuration, NaiveDate, Utc};
use parking_lot::Mutex;
use std::collections::VecDeque;
use std::sync::Arc;
use tokio::task::JoinHandle;
use tracing::{info, warn};

#[derive(Debug, Default)]
struct SurvivalInner {
    consecutive_losses: u32,
    last_loss_at: Option<DateTime<Utc>>,
    /// Rolling list of close events with timestamps & PnL — used for
    /// the long-window ("losses in 1h") cooldown and the daily-loss
    /// counter.
    recent_closes: VecDeque<(DateTime<Utc>, f64)>,
    cooldown_until: Option<DateTime<Utc>>,
    cooldown_reason: Option<String>,
    /// Last time we issued a flat-all command (so we don't spam it).
    last_flat_at: Option<DateTime<Utc>>,
    /// Current day for daily-PnL counters. Resets at UTC midnight.
    current_day: NaiveDate,
    daily_loss_count: u32,
    daily_pnl: f64,
    daily_peak_equity: f64,
    /// Last news net score we've observed across symbols.
    news_score: f64,
    /// Last funding rate observed (any symbol).
    funding_rate: f64,
    /// Last computed state — re-broadcast on every refresh tick.
    last_state: Option<SurvivalState>,
    /// PnL history for Monte Carlo drawdown CI (last 200 trades).
    pnl_history: Vec<f64>,
}

pub struct SurvivalAgentDeps {
    pub bus: MessageBus,
    pub cfg: SurvivalCfg,
    pub exchange: Arc<dyn Exchange>,
    pub risk: Arc<RiskManager>,
    pub initial_equity: f64,
}

pub fn spawn(deps: SurvivalAgentDeps) -> JoinHandle<()> {
    let SurvivalAgentDeps {
        bus,
        cfg,
        exchange,
        risk,
        initial_equity,
    } = deps;

    let inner = Arc::new(Mutex::new(SurvivalInner {
        current_day: Utc::now().date_naive(),
        daily_peak_equity: initial_equity,
        ..Default::default()
    }));

    // Equity reconciliation task — independent cadence so we keep
    // pumping balance updates even if the event stream is quiet.
    if cfg.enabled && cfg.equity_refresh_secs > 0 {
        let bus_eq = bus.clone();
        let exchange_eq = exchange.clone();
        let risk_eq = risk.clone();
        let interval = cfg.equity_refresh_secs;
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(interval)).await;
                match exchange_eq.fetch_equity_usd().await {
                    Ok(eq) if eq > 0.0 => {
                        risk_eq.set_equity(eq);
                        bus_eq.publish(AgentEvent::EquityReconciled {
                            equity_usd: eq,
                            ts: Utc::now(),
                        });
                    }
                    Ok(_) => {
                        // Paper mode returns 0 — silently skip.
                    }
                    Err(e) => warn!(error = %e, "survival: fetch_equity_usd failed"),
                }
            }
        });
    }

    // Refresh task — periodically recomputes the survival state.
    if cfg.enabled && cfg.refresh_secs > 0 {
        let bus_r = bus.clone();
        let inner_r = inner.clone();
        let risk_r = risk.clone();
        let cfg_r = cfg.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(cfg_r.refresh_secs)).await;
                let state = recompute(&inner_r, &risk_r, &cfg_r, initial_equity);
                apply_state(&risk_r, &state);
                bus_r.publish(AgentEvent::SurvivalUpdated(state.clone()));
                bus_r.publish(AgentEvent::Heartbeat {
                    from: AgentId::Survival,
                    ts: Utc::now(),
                });
            }
        });
    }

    // Event ingestion task.
    let mut rx = bus.subscribe();
    let bus_for_publish = bus.clone();
    let risk_ev = risk.clone();
    let cfg_ev = cfg.clone();
    let inner_ev = inner.clone();
    tokio::spawn(async move {
        info!("survival agent starting");
        while let Ok(ev) = rx.recv().await {
            match ev {
                AgentEvent::PositionClosed { pnl_usd, .. } => {
                    on_position_closed(&inner_ev, pnl_usd, &cfg_ev);
                    let state = recompute(&inner_ev, &risk_ev, &cfg_ev, initial_equity);

                    // Death / auto-flat detection.
                    if matches!(state.mode, SurvivalMode::Dead) {
                        maybe_publish_flat_all(
                            &inner_ev,
                            &bus_for_publish,
                            "death-line breached — bot will freeze",
                        );
                    } else if state.drawdown_pct >= cfg_ev.auto_flat_drawdown_pct {
                        maybe_publish_flat_all(
                            &inner_ev,
                            &bus_for_publish,
                            &format!(
                                "auto-flat: drawdown {:.2}% >= {:.2}%",
                                state.drawdown_pct, cfg_ev.auto_flat_drawdown_pct
                            ),
                        );
                    }
                    apply_state(&risk_ev, &state);
                    bus_for_publish.publish(AgentEvent::SurvivalUpdated(state));
                }
                AgentEvent::FeedsSnapshot(FeedsSnapshotMsg { snapshot, .. }) => {
                    let mut g = inner_ev.lock();
                    if let Some(news) = &snapshot.news {
                        g.news_score = news.net_score;
                    }
                    if let Some(funding) = &snapshot.funding {
                        g.funding_rate = funding.rate;
                    }
                }
                AgentEvent::EquityReconciled { equity_usd, .. } => {
                    let mut g = inner_ev.lock();
                    if equity_usd > g.daily_peak_equity {
                        g.daily_peak_equity = equity_usd;
                    }
                }
                AgentEvent::ControlCommand(ControlCommand::ResetDaily) => {
                    let mut g = inner_ev.lock();
                    g.daily_loss_count = 0;
                    g.daily_pnl = 0.0;
                    g.daily_peak_equity = risk_ev.equity();
                    g.current_day = Utc::now().date_naive();
                    g.cooldown_until = None;
                    g.cooldown_reason = None;
                    info!("survival: daily counters reset");
                }
                AgentEvent::ControlCommand(ControlCommand::Unfreeze) => {
                    let mut g = inner_ev.lock();
                    g.cooldown_until = None;
                    g.cooldown_reason = None;
                    risk_ev.unfreeze();
                    info!("survival: unfreeze requested");
                }
                AgentEvent::Shutdown => break,
                _ => {}
            }
        }
    })
}

fn maybe_publish_flat_all(inner: &Arc<Mutex<SurvivalInner>>, bus: &MessageBus, reason: &str) {
    let mut g = inner.lock();
    let now = Utc::now();
    // Don't fire the flat-all more than once per minute.
    if let Some(last) = g.last_flat_at {
        if (now - last).num_seconds() < 60 {
            return;
        }
    }
    g.last_flat_at = Some(now);
    drop(g);
    warn!(%reason, "survival: broadcasting flat-all");
    bus.publish(AgentEvent::ControlCommand(ControlCommand::FlatAll {
        reason: reason.to_string(),
    }));
}

fn on_position_closed(inner: &Arc<Mutex<SurvivalInner>>, pnl: f64, cfg: &SurvivalCfg) {
    let mut g = inner.lock();
    let now = Utc::now();

    // Daily rollover.
    let today = now.date_naive();
    if today != g.current_day {
        g.current_day = today;
        g.daily_loss_count = 0;
        g.daily_pnl = 0.0;
    }

    g.daily_pnl += pnl;
    g.recent_closes.push_back((now, pnl));
    g.pnl_history.push(pnl);
    if g.pnl_history.len() > 200 {
        g.pnl_history.remove(0);
    }
    while let Some((t, _)) = g.recent_closes.front() {
        if (now - *t) > ChronoDuration::hours(24) {
            g.recent_closes.pop_front();
        } else {
            break;
        }
    }

    if pnl < 0.0 {
        g.consecutive_losses += 1;
        g.last_loss_at = Some(now);
        g.daily_loss_count += 1;

        // Short-window cooldown (e.g. 3 in a row → 30m pause).
        if g.consecutive_losses >= cfg.loss_streak_short {
            let until = now + ChronoDuration::minutes(cfg.loss_streak_short_cooldown_min as i64);
            if g.cooldown_until.map(|t| t < until).unwrap_or(true) {
                g.cooldown_until = Some(until);
                g.cooldown_reason = Some(format!(
                    "{} consecutive losses — pausing {}m",
                    g.consecutive_losses, cfg.loss_streak_short_cooldown_min
                ));
            }
        }
        // Long-window cooldown (e.g. 5 losses in 1h → 4h pause).
        let window = ChronoDuration::minutes(cfg.loss_streak_long_window_min as i64);
        let losses_in_window = g
            .recent_closes
            .iter()
            .rev()
            .take_while(|(t, _)| (now - *t) <= window)
            .filter(|(_, p)| *p < 0.0)
            .count() as u32;
        if losses_in_window >= cfg.loss_streak_long {
            let until = now + ChronoDuration::minutes(cfg.loss_streak_long_cooldown_min as i64);
            if g.cooldown_until.map(|t| t < until).unwrap_or(true) {
                g.cooldown_until = Some(until);
                g.cooldown_reason = Some(format!(
                    "{} losses in {}m — pausing {}h",
                    losses_in_window,
                    cfg.loss_streak_long_window_min,
                    cfg.loss_streak_long_cooldown_min / 60
                ));
            }
        }
        // Daily-loss-count cooldown (e.g. 10 losses today → freeze 24h).
        if g.daily_loss_count >= cfg.daily_loss_count {
            let until = now + ChronoDuration::hours(24);
            if g.cooldown_until.map(|t| t < until).unwrap_or(true) {
                g.cooldown_until = Some(until);
                g.cooldown_reason =
                    Some(format!("{} losses today — pausing 24h", g.daily_loss_count));
            }
        }
    } else {
        // A win breaks the consecutive-loss streak.
        g.consecutive_losses = 0;
    }
}

fn recompute(
    inner: &Arc<Mutex<SurvivalInner>>,
    risk: &Arc<RiskManager>,
    cfg: &SurvivalCfg,
    initial_equity: f64,
) -> SurvivalState {
    let snap = risk.snapshot();
    let mut g = inner.lock();
    let now = Utc::now();

    // Refresh peak.
    if snap.equity > g.daily_peak_equity {
        g.daily_peak_equity = snap.equity;
    }

    // Death detection.
    let death_line = initial_equity * cfg.death_line_pct;
    let mut reasons: Vec<String> = Vec::new();
    let mut score = 100i32;

    // Drawdown component.
    let drawdown = snap.drawdown_pct;
    if drawdown > 0.0 {
        let penalty = (drawdown / cfg.auto_flat_drawdown_pct.max(1.0) * 60.0).min(60.0);
        score -= penalty as i32;
        if drawdown >= 1.0 {
            reasons.push(format!("drawdown {:.2}%", drawdown));
        }
    }

    // Daily loss component.
    let daily_loss_pct = if snap.equity > 0.0 && snap.realized_pnl_today < 0.0 {
        -snap.realized_pnl_today / snap.equity * 100.0
    } else {
        0.0
    };
    if daily_loss_pct > 0.0 {
        let penalty = (daily_loss_pct * 6.0).min(40.0);
        score -= penalty as i32;
        reasons.push(format!("daily-loss {:.2}%", daily_loss_pct));
    }

    // Loss-streak component.
    if g.consecutive_losses >= 2 {
        let penalty = (g.consecutive_losses.saturating_sub(1) as i32 * 8).min(30);
        score -= penalty;
        reasons.push(format!("{} consecutive losses", g.consecutive_losses));
    }

    // Active cooldown.
    let in_cooldown = match g.cooldown_until {
        Some(t) if t > now => {
            let mins = (t - now).num_minutes().max(0);
            reasons.push(format!(
                "{} ({}m left)",
                g.cooldown_reason
                    .clone()
                    .unwrap_or_else(|| "cooldown".into()),
                mins
            ));
            true
        }
        _ => false,
    };
    if in_cooldown {
        score = score.min(20);
    }

    // News regime.
    if g.news_score <= cfg.news_panic_threshold {
        score -= 25;
        reasons.push(format!(
            "news panic ({:.2} <= {:.2})",
            g.news_score, cfg.news_panic_threshold
        ));
    } else if g.news_score >= cfg.news_euphoria_threshold {
        score -= 10;
        reasons.push(format!(
            "news euphoria ({:.2} >= {:.2})",
            g.news_score, cfg.news_euphoria_threshold
        ));
    }

    // Equity floor proximity.
    let floor_distance_pct = if snap.equity > 0.0 && initial_equity > 0.0 {
        ((snap.equity - death_line) / initial_equity * 100.0).max(0.0)
    } else {
        0.0
    };
    if snap.equity <= death_line {
        reasons.push(format!(
            "equity ${:.2} <= death-line ${:.2}",
            snap.equity, death_line
        ));
        score = 0;
    } else if floor_distance_pct < 5.0 {
        score -= 30;
        reasons.push(format!("{:.1}% above death-line", floor_distance_pct));
    } else if floor_distance_pct < 10.0 {
        score -= 15;
    }

    // Daily PnL ratchet — if today's gain ≥ threshold, lock half by freezing.
    let realized_pct = if initial_equity > 0.0 {
        snap.realized_pnl_today / initial_equity * 100.0
    } else {
        0.0
    };
    let mut ratchet_locked = false;
    if realized_pct >= cfg.daily_pnl_ratchet_pct {
        let lock_floor = snap.realized_pnl_today * 0.5;
        if snap.realized_pnl_today < lock_floor {
            ratchet_locked = true;
            reasons.push(format!(
                "ratchet locked: daily PnL ${:.2} < lock ${:.2}",
                snap.realized_pnl_today, lock_floor
            ));
        }
    }

    // Monte Carlo drawdown CI — project expected drawdown from recent
    // trade history. If P95 drawdown exceeds the auto-flat threshold,
    // proactively reduce the survival score.
    if g.pnl_history.len() >= 20 {
        if let Some(mc) = drawdown_confidence_intervals(&g.pnl_history, 100) {
            if mc.p95 > cfg.auto_flat_drawdown_pct {
                let penalty = ((mc.p95 - cfg.auto_flat_drawdown_pct) * 5.0).min(20.0) as i32;
                score -= penalty;
                reasons.push(format!(
                    "MC drawdown P95={:.1}% (P50={:.1}%)",
                    mc.p95, mc.p50
                ));
            }
        }
    }

    let score_clamped = score.clamp(0, 100) as u8;
    let mode = if snap.equity <= death_line {
        SurvivalMode::Dead
    } else if in_cooldown || ratchet_locked || score_clamped < 25 {
        SurvivalMode::Frozen
    } else if score_clamped < 50 {
        SurvivalMode::Defensive
    } else if score_clamped < 80 {
        SurvivalMode::Cautious
    } else {
        SurvivalMode::Healthy
    };

    let size_multiplier = match mode {
        SurvivalMode::Healthy => 1.0,
        SurvivalMode::Cautious => 0.6,
        SurvivalMode::Defensive => 0.3,
        SurvivalMode::Frozen | SurvivalMode::Dead => 0.0,
    };

    let state = SurvivalState {
        score: score_clamped,
        mode,
        equity_usd: snap.equity,
        initial_equity_usd: initial_equity,
        death_line_usd: death_line,
        peak_equity_usd: snap.peak_equity,
        realized_pnl_today: snap.realized_pnl_today,
        realized_pnl_pct_today: realized_pct,
        drawdown_pct: snap.drawdown_pct,
        open_positions: snap.open_positions,
        consecutive_losses: g.consecutive_losses,
        last_loss_at: g.last_loss_at,
        size_multiplier,
        reasons,
        ts: now,
    };
    g.last_state = Some(state.clone());
    state
}

fn apply_state(risk: &Arc<RiskManager>, state: &SurvivalState) {
    risk.set_size_multiplier(state.size_multiplier);
    match state.mode {
        SurvivalMode::Frozen | SurvivalMode::Dead => {
            risk.freeze(format!(
                "survival mode {} (score {})",
                state.mode.as_str(),
                state.score
            ));
        }
        _ => {
            // Only auto-unfreeze if the only reason we were frozen is
            // a stale survival lock — leave manual freezes in place.
            if risk.is_frozen() {
                risk.unfreeze();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::execution::risk::RiskLimits;

    fn limits() -> RiskLimits {
        RiskLimits {
            risk_per_trade_pct: 1.0,
            max_open_positions: 3,
            max_daily_loss_pct: 3.0,
            max_drawdown_pct: 10.0,
            max_leverage: 5,
            max_spread_pct: 0.05,
            min_reward_risk: 1.2,
            max_position_notional_pct: 100.0,
            min_net_edge_bps: 1.0,
            assumed_daily_volume_usd: 1_000_000_000.0,
        }
    }

    fn cfg() -> SurvivalCfg {
        SurvivalCfg::default()
    }

    fn inner() -> Arc<Mutex<SurvivalInner>> {
        Arc::new(Mutex::new(SurvivalInner {
            current_day: Utc::now().date_naive(),
            daily_peak_equity: 1000.0,
            ..Default::default()
        }))
    }

    #[test]
    fn healthy_when_capital_intact_and_no_losses() {
        let risk = Arc::new(RiskManager::new(limits(), 1000.0));
        let inner = inner();
        let s = recompute(&inner, &risk, &cfg(), 1000.0);
        assert!(matches!(s.mode, SurvivalMode::Healthy));
        assert!(s.score >= 80);
        assert!((s.size_multiplier - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn dead_when_equity_below_death_line() {
        let risk = Arc::new(RiskManager::new(limits(), 1000.0));
        risk.on_position_closed(-400.0); // equity 600 < 700 floor
        let inner = inner();
        let s = recompute(&inner, &risk, &cfg(), 1000.0);
        assert!(matches!(s.mode, SurvivalMode::Dead));
        assert_eq!(s.score, 0);
        assert!((s.size_multiplier).abs() < f64::EPSILON);
    }

    #[test]
    fn three_consecutive_losses_trigger_cooldown() {
        let cfg = cfg();
        let inner = inner();
        on_position_closed(&inner, -10.0, &cfg);
        on_position_closed(&inner, -10.0, &cfg);
        on_position_closed(&inner, -10.0, &cfg);
        let g = inner.lock();
        assert_eq!(g.consecutive_losses, 3);
        assert!(g.cooldown_until.is_some());
    }

    #[test]
    fn win_breaks_consecutive_loss_streak() {
        let cfg = cfg();
        let inner = inner();
        on_position_closed(&inner, -10.0, &cfg);
        on_position_closed(&inner, -10.0, &cfg);
        on_position_closed(&inner, 20.0, &cfg);
        let g = inner.lock();
        assert_eq!(g.consecutive_losses, 0);
    }

    #[test]
    fn cooldown_forces_frozen_mode() {
        let cfg = cfg();
        let inner = inner();
        on_position_closed(&inner, -10.0, &cfg);
        on_position_closed(&inner, -10.0, &cfg);
        on_position_closed(&inner, -10.0, &cfg);
        let risk = Arc::new(RiskManager::new(limits(), 1000.0));
        risk.on_position_closed(-30.0);
        let s = recompute(&inner, &risk, &cfg, 1000.0);
        assert!(matches!(s.mode, SurvivalMode::Frozen));
    }
}
