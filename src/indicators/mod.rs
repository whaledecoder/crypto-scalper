//! Technical indicators used by the signal engine.
//!
//! All indicators are implemented as small incremental structs that update on
//! each new value. Most also expose a `from_candles` helper to bootstrap from
//! historical data.

pub mod adx;
pub mod atr;
pub mod bollinger;
pub mod choppiness;
pub mod ema;
pub mod keltner;
pub mod roc;
pub mod rsi;
pub mod vwap;

pub use adx::Adx;
pub use atr::Atr;
pub use bollinger::{Bollinger, BollingerBand};
pub use choppiness::Choppiness;
pub use ema::Ema;
pub use keltner::Keltner;
pub use roc::Roc;
pub use rsi::Rsi;
pub use vwap::Vwap;
