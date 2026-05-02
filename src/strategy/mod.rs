//! Layer 2 — strategy engine.
//!
//! Each strategy consumes a rolling state + the latest closed candle and
//! returns an optional `PreSignal`. The `Regime` detector picks which
//! strategy gets consulted each candle.

pub mod ab_test;
pub mod alpha_gate;
pub mod ema_ribbon;
pub mod hmm;
pub mod kalman;
pub mod mean_reversion;
pub mod momentum;
pub mod multi_timeframe;
pub mod pairs;
pub mod regime;
pub mod retirement;
pub mod squeeze;
pub mod state;
pub mod vwap_scalp;

pub use regime::{Regime, RegimeDetector};
pub use state::{PreSignal, StrategyName, SymbolState};

use crate::data::Candle;

/// Shared trait for all strategies.
pub trait Strategy {
    fn name(&self) -> StrategyName;
    fn evaluate(&self, state: &SymbolState, closed: &Candle) -> Option<PreSignal>;
}

/// Select the best strategy given the current regime and an active set.
pub fn select_strategies(active: &[StrategyName], regime: Regime) -> Vec<StrategyName> {
    let preferred: &[StrategyName] = match regime {
        Regime::TrendingBullish | Regime::TrendingBearish => {
            &[StrategyName::EmaRibbon, StrategyName::Momentum]
        }
        Regime::Ranging => &[
            StrategyName::MeanReversion,
            StrategyName::VwapScalp,
            StrategyName::EmaRibbon,
        ],
        Regime::Volatile => &[StrategyName::Squeeze, StrategyName::Momentum],
        Regime::Squeeze => &[StrategyName::Squeeze],
        Regime::Unknown => &[
            StrategyName::VwapScalp,
            StrategyName::MeanReversion,
            StrategyName::EmaRibbon,
        ],
    };
    preferred
        .iter()
        .copied()
        .filter(|s| active.contains(s))
        .collect()
}
