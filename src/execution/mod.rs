//! Layer 4 — execution, risk management, and position tracking.

pub mod binance;
pub mod exchange;
pub mod limit_order;
pub mod orders;
pub mod paper;
pub mod position;
pub mod quality;
pub mod risk;
pub mod tcm;

pub use exchange::{Exchange, OrderAck};
pub use orders::{OrderRequest, OrderType};
pub use paper::PaperExchange;
pub use position::{Position, PositionBook, PositionConfig, PositionExitReason};
pub use risk::{RiskManager, RiskSnapshot};
pub use tcm::TransactionCostModel;
