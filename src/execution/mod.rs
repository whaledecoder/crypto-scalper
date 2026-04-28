//! Layer 4 — execution, risk management, and position tracking.

pub mod binance;
pub mod exchange;
pub mod orders;
pub mod paper;
pub mod position;
pub mod risk;

pub use exchange::{Exchange, OrderAck};
pub use orders::{OrderRequest, OrderType};
pub use paper::PaperExchange;
pub use position::{Position, PositionBook, PositionExitReason};
pub use risk::{RiskManager, RiskSnapshot};
