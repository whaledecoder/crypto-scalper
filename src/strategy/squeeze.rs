//! Strategy E — Volatility Squeeze & Expansion.
//!
//! Tuned for HFT: lower ROC threshold for expansion detection, tighter SL.

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

        // Check if we're still inside the squeeze (BB inside Keltner)
        let in_squeeze = bb.upper < ku && bb.lower > kl;
        if in_squeeze {
            return None; // wait for expansion
        }

        // Expansion detected — use ROC for direction.
        // Lower threshold: 0.1% (was 0.3%) — crypto moves fast in squeeze release.
        let (side, reason, sl, tp) = if roc > 0.1 && c.close > bb.mid {
            (
                Side::Long,
                format!("Squeeze expand up, ROC {roc:.2}%"),
                // Tighter SL: 0.5× ATR from mid-band
                bb.mid.min(c.close - 0.5 * atr),
                // Closer TP: 1.2× ATR (was 1.5×) — capture expansion quickly
                c.close + 1.2 * atr,
            )
        } else if roc < -0.1 && c.close < bb.mid {
            (
                Side::Short,
                format!("Squeeze expand down, ROC {roc:.2}%"),
                bb.mid.max(c.close + 0.5 * atr),
                c.close - 1.2 * atr,
            )
        } else {
            return None;
        };

        let mut score: f64 = 64.0;
        // Stronger ROC = higher confidence
        score += roc.abs().min(3.0) * 4.0;
        // OFI alignment
        let aligned_ofi = (side == Side::Long && s.last_ofi.unwrap_or(0.0) > 0.0)
            || (side == Side::Short && s.last_ofi.unwrap_or(0.0) < 0.0);
        if aligned_ofi {
            score += 5.0;
        }
        // Price beyond BB band = stronger expansion
        if (side == Side::Long && c.close > bb.upper) || (side == Side::Short && c.close < bb.lower)
        {
            score += 3.0;
        }

        Some(PreSignal {
            symbol: s.symbol.clone(),
            strategy: StrategyName::Squeeze,
            side,
            entry: c.close,
            stop_loss: sl,
            take_profit: tp,
            ta_confidence: score.clamp(0.0, 100.0) as u8,
            reason,
        })
    }
}
