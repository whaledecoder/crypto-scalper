//! Layer 5 — trade journal, Telegram alerts, HTTP metrics dashboard.

pub mod logger;
pub mod metrics;
pub mod telegram;

pub use logger::{TradeJournal, TradeRecord};
pub use metrics::{spawn_metrics_server, MetricsSnapshot, MetricsState};
pub use telegram::TelegramNotifier;
