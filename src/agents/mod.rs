//! Multi-agent runtime.
//!
//! Each layer of the stack is exposed as an *agent* — a long-running tokio
//! task that consumes typed messages from a shared `MessageBus` and emits
//! results back onto the same bus. Most agents are deterministic Rust
//! actors (`DataAgent`, `SignalAgent`, `RiskAgent`, ...). One agent —
//! `TraderManagerAgent` — has its own LLM brain and acts as the
//! head-of-desk: it watches every other agent's output and gives the
//! final `Approve` / `Veto` / `Adjust` verdict before any order is sent.
//!
//! The fast path stays deterministic and synchronous-feeling, but the
//! manager is invoked between `BrainAgent` and `ExecutionAgent` so it can
//! veto or modulate trades. Disabling the manager (via config) reduces
//! the system back to pure actor mode with no extra LLM cost.

pub mod brain;
pub mod bus;
pub mod control;
pub mod data;
pub mod execution;
pub mod feeds;
pub mod learning;
pub mod manager;
pub mod messages;
pub mod monitor;
pub mod risk;
pub mod signal;
pub mod survival;
pub mod watchdog;

pub use bus::MessageBus;
pub use messages::{
    AgentEvent, AgentId, BrainOutcome, ControlCommand, FeedsSnapshotMsg, ManagerAction,
    ManagerProposal, ManagerVerdict, RiskOutcome, RiskVerdictMsg, SurvivalMode, SurvivalState,
};
