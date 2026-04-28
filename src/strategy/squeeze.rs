//! Strategy E — Volatility Squeeze & Expansion.

use super::state::{PreSignal, StrategyName, SymbolState};
use super::Strategy;
use crate::data::{Candle, Side};

pub struct Squeeze;

impl Strategy for Squeeze {
    fn name(&self) -> StrategyName {
        StrategyName::Squeeze
    }

    fn evaluate(&self, s: &SymbolState, c: &Candle) -> Option<PreSignal> {
        let bb = s.last_bb?;
        let ku = s.last_keltner_upper?;
        let kl = s.last_keltner_lower?;
        let atr = s.last_atr?;
        let roc = s.last_roc.unwrap_or(0.0);
        let in_squeeze = bb.upper < ku && bb.lower > kl;
        if in_squeeze {
            return None; // wait for expansion
        }

        // Expansion — use ROC to direction.
        let (side, reason, sl, tp) = if roc > 0.3 && c.close > bb.upper {
            (
                Side::Long,
                format!("Squeeze fired up, ROC {roc:.2}%"),
                bb.mid.min(c.close - 0.5 * atr),
                c.close + 1.5 * atr,
            )
        } else if roc < -0.3 && c.close < bb.lower {
            (
                Side::Short,
                format!("Squeeze fired down, ROC {roc:.2}%"),
                bb.mid.max(c.close + 0.5 * atr),
                c.close - 1.5 * atr,
            )
        } else {
            return None;
        };

        let score: f64 = (60.0_f64 + roc.abs().min(5.0) * 3.0).clamp(0.0, 100.0);
        let score = score as u8;
        Some(PreSignal {
            symbol: s.symbol.clone(),
            strategy: StrategyName::Squeeze,
            side,
            entry: c.close,
            stop_loss: sl,
            take_profit: tp,
            ta_confidence: score,
            reason,
        })
    }
}
