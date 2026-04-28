//! Market regime detection (trending / ranging / volatile / squeeze).

use super::state::SymbolState;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum Regime {
    TrendingBullish,
    TrendingBearish,
    Ranging,
    Volatile,
    Squeeze,
    Unknown,
}

impl Regime {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::TrendingBullish => "TRENDING_BULLISH",
            Self::TrendingBearish => "TRENDING_BEARISH",
            Self::Ranging => "RANGING",
            Self::Volatile => "VOLATILE",
            Self::Squeeze => "SQUEEZE",
            Self::Unknown => "UNKNOWN",
        }
    }
}

pub struct RegimeDetector;

impl RegimeDetector {
    /// Derive the regime from the latest indicator snapshot.
    pub fn detect(state: &SymbolState) -> Regime {
        let adx = match state.last_adx {
            Some(a) => a,
            None => return Regime::Unknown,
        };
        let chop = state.last_choppiness.unwrap_or(50.0);
        let bb = state.last_bb;
        let kupper = state.last_keltner_upper;
        let klower = state.last_keltner_lower;

        // Squeeze: BB inside Keltner
        if let (Some(bb), Some(u), Some(l)) = (bb, kupper, klower) {
            if bb.upper < u && bb.lower > l && chop > 55.0 {
                return Regime::Squeeze;
            }
        }

        if adx >= 40.0 {
            return Regime::Volatile;
        }

        if adx >= 25.0 && chop < 38.2 {
            // Lean on DI+/DI- to decide bull vs bear.
            let plus = state.last_di_plus.unwrap_or(0.0);
            let minus = state.last_di_minus.unwrap_or(0.0);
            if plus >= minus {
                return Regime::TrendingBullish;
            }
            return Regime::TrendingBearish;
        }

        if adx < 20.0 || chop > 61.8 {
            return Regime::Ranging;
        }

        Regime::Unknown
    }
}
