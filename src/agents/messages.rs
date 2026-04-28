//! Message types exchanged on the agent bus.

use crate::data::{Candle, Side, Trade};
use crate::execution::exchange::OrderAck;
use crate::execution::PositionExitReason;
use crate::feeds::ExternalSnapshot;
use crate::llm::engine::TradeDecision;
use crate::strategy::state::PreSignal;
use crate::strategy::Regime;
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
        best_ask: f64,
    },
    /// `DataAgent` finalized a candle for a symbol.
    CandleClosed { symbol: String, candle: Candle },
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
    /// Graceful shutdown notice.
    Shutdown,
}

#[derive(Debug, Clone)]
pub struct FeedsSnapshotMsg {
    pub symbol: String,
    pub snapshot: ExternalSnapshot,
    pub ts: DateTime<Utc>,
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
