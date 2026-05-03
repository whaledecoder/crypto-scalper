//! Message types exchanged on the agent bus.

use crate::data::{Candle, Side, Trade};
use crate::execution::exchange::OrderAck;
use crate::execution::PositionExitReason;
use crate::feeds::ExternalSnapshot;
use crate::llm::engine::TradeDecision;
use crate::strategy::state::PreSignal;
use crate::strategy::{Regime, StrategyName};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AgentId {
    Data,
    Feeds,
    Signal,
    Risk,
    Brain,
    Learning,
    Manager,
    Execution,
    Monitor,
    Survival,
    Control,
}

impl AgentId {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Data => "data",
            Self::Feeds => "feeds",
            Self::Signal => "signal",
            Self::Risk => "risk",
            Self::Brain => "brain",
            Self::Learning => "learning",
            Self::Manager => "manager",
            Self::Execution => "execution",
            Self::Monitor => "monitor",
            Self::Survival => "survival",
            Self::Control => "control",
        }
    }
}

#[derive(Debug, Clone)]
pub enum AgentEvent {
    /// Raw market trade from `DataAgent`.
    Tick { symbol: String, trade: Trade },
    /// Best-bid/ask updated.
    BookTicker {
        symbol: String,
        best_bid: f64,
        bid_qty: f64,
        best_ask: f64,
        ask_qty: f64,
    },
    /// `DataAgent` finalized a candle for a symbol.
    CandleClosed {
        symbol: String,
        timeframe_secs: i64,
        candle: Candle,
    },
    /// `SignalAgent` evaluated a closed candle but did not emit a tradeable
    /// pre-signal. Used by monitor/control surfaces to explain quiet runs.
    SignalEvaluation(SignalEvaluationMsg),
    /// `FeedsAgent` published an updated external snapshot for a symbol.
    FeedsSnapshot(FeedsSnapshotMsg),
    /// `SignalAgent` produced a pre-signal candidate.
    PreSignalEmitted {
        signal: Box<PreSignal>,
        regime: Regime,
    },
    /// `RiskAgent` evaluated a pre-signal.
    RiskVerdict(RiskVerdictMsg),
    /// `BrainAgent` analysed a vetted signal.
    BrainOutcomeReady(BrainOutcome),
    /// `LearningAgent` rebuilt the policy.
    PolicyRefreshed {
        lessons_count: usize,
        ts: DateTime<Utc>,
    },
    /// `TraderManagerAgent` final verdict.
    ManagerVerdictEmitted(ManagerVerdict),
    /// `ExecutionAgent` filled an order.
    OrderFilled {
        client_id: String,
        symbol: String,
        side: Side,
        size: f64,
        ack: OrderAck,
    },
    /// Position closed (by SL, TP or trailing stop).
    PositionClosed {
        client_id: String,
        symbol: String,
        side: Side,
        size: f64,
        entry_price: f64,
        exit_price: f64,
        pnl_usd: f64,
        reason: PositionExitReason,
    },
    /// Heartbeat for liveness monitoring.
    Heartbeat { from: AgentId, ts: DateTime<Utc> },
    /// `SurvivalAgent` published a new survival state. All downstream
    /// agents (manager prompt, risk sizing, execution gate) consume this
    /// to decide how aggressive to be.
    SurvivalUpdated(SurvivalState),
    /// Equity reconciled from the exchange (or paper synthetic).
    EquityReconciled { equity_usd: f64, ts: DateTime<Utc> },
    /// External operator command (Telegram, CLI). Routed by the
    /// `ControlAgent` and consumed by the relevant downstream agent.
    ControlCommand(ControlCommand),
    /// Graceful shutdown notice.
    Shutdown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ControlCommand {
    /// Freeze new entries (idempotent).
    Freeze { reason: String },
    /// Resume trading after a freeze.
    Unfreeze,
    /// Close every open position at market right now and freeze.
    FlatAll { reason: String },
    /// Reset daily pnl counters (cron 00:00 UTC). Internal use.
    ResetDaily,
    /// External request to publish a fresh /status snapshot to Telegram.
    StatusRequest,
}

/// SurvivalAgent's verdict, broadcast continuously. Other agents
/// derive their behaviour from this:
///
/// - `RiskAgent`        — uses `size_multiplier`
/// - `ExecutionAgent`   — refuses when `mode == Frozen` or `Dead`
/// - `ManagerAgent`     — prompt embeds score + reasons
/// - `MonitorAgent`     — exposes via `/survival` + Telegram
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SurvivalState {
    /// 0 = death imminent, 100 = peak fitness.
    pub score: u8,
    pub mode: SurvivalMode,
    pub equity_usd: f64,
    pub initial_equity_usd: f64,
    pub death_line_usd: f64,
    pub peak_equity_usd: f64,
    pub realized_pnl_today: f64,
    pub realized_pnl_pct_today: f64,
    pub drawdown_pct: f64,
    pub open_positions: u32,
    pub consecutive_losses: u32,
    pub last_loss_at: Option<DateTime<Utc>>,
    pub size_multiplier: f64,
    /// Human-readable list of currently active survival rules ("loss-streak",
    /// "vol-spike", etc.). Used by the /survival endpoint and Manager prompt.
    pub reasons: Vec<String>,
    pub ts: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SurvivalMode {
    /// Score >= 80, full risk authority.
    Healthy,
    /// 50 <= score < 80, moderate caution (≈0.6× size).
    Cautious,
    /// 25 <= score < 50, defensive mode (≈0.3× size, manager skews to veto).
    Defensive,
    /// score < 25 OR cooldown active — pause new entries.
    Frozen,
    /// equity <= death_line — bot is "dead". Auto-flat all positions
    /// and refuse trading until manually unfrozen by the operator.
    Dead,
}

impl SurvivalMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Healthy => "healthy",
            Self::Cautious => "cautious",
            Self::Defensive => "defensive",
            Self::Frozen => "frozen",
            Self::Dead => "dead",
        }
    }
}

#[derive(Debug, Clone)]
pub struct FeedsSnapshotMsg {
    pub symbol: String,
    pub snapshot: ExternalSnapshot,
    pub ts: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct SignalEvaluationMsg {
    pub symbol: String,
    pub timeframe_secs: i64,
    pub regime: Option<Regime>,
    pub candles: usize,
    pub strategies: Vec<StrategyName>,
    pub reason: String,
    pub best_strategy: Option<StrategyName>,
    pub best_confidence: Option<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RiskOutcome {
    /// Pre-signal cleared the risk gate; size and effective threshold attached.
    Allowed,
    /// Risk gate or learning policy blocked this signal.
    Blocked,
}

#[derive(Debug, Clone)]
pub struct RiskVerdictMsg {
    pub signal: Box<PreSignal>,
    pub regime: Regime,
    pub outcome: RiskOutcome,
    pub size: f64,
    pub size_multiplier: f64,
    pub effective_ta_threshold: u8,
    pub effective_llm_floor: u8,
    pub matched_lessons: Vec<String>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone)]
pub struct BrainOutcome {
    pub signal: Box<PreSignal>,
    pub regime: Regime,
    pub risk: RiskVerdictMsg,
    pub decision: TradeDecision,
    pub latency_ms: u64,
    pub offline_fallback: bool,
}

#[derive(Debug, Clone)]
pub struct ManagerProposal {
    pub symbol: String,
    pub side: Side,
    pub strategy: String,
    pub regime: String,
    pub entry: f64,
    pub stop_loss: f64,
    pub take_profit: f64,
    pub size: f64,
    pub ta_confidence: u8,
    pub llm_confidence: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "action")]
pub enum ManagerAction {
    Approve,
    Veto {
        reason: String,
    },
    Adjust {
        size_multiplier: f64,
        sl_offset_bps: f64,
        tp_offset_bps: f64,
        reason: String,
    },
}

impl ManagerAction {
    pub fn is_blocking(&self) -> bool {
        matches!(self, Self::Veto { .. })
    }
}

#[derive(Debug, Clone)]
pub struct ManagerVerdict {
    pub proposal: ManagerProposal,
    pub action: ManagerAction,
    pub latency_ms: u64,
    pub offline_fallback: bool,
    pub brain_outcome: BrainOutcome,
}
