//! Simple backtest engine — replay historical OHLCV and emit performance metrics.

pub mod data_loader;
pub mod engine;
pub mod metrics;

pub use data_loader::load_csv;
pub use engine::{BacktestEngine, BacktestResult};
pub use metrics::PerformanceMetrics;
