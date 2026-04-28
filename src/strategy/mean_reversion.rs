//! Strategy A — Mean Reversion.
//!
//! Fade the tails: BB band touch + RSI extreme + volume confirmation, only in
//! non-trending regimes.

use super::state::{PreSignal, StrategyName, SymbolState};
use super::Strategy;
use crate::data::{Candle, Side};

pub struct MeanReversion;

impl Strategy for MeanReversion {
    fn name(&self) -> StrategyName {
        StrategyName::MeanReversion
    }

    fn evaluate(&self, s: &SymbolState, c: &Candle) -> Option<PreSignal> {
        let bb = s.last_bb?;
        let rsi = s.last_rsi?;
        let atr = s.last_atr?;
        let adx = s.last_adx.unwrap_or(0.0);

        if adx > 25.0 {
            return None; // strong trend — don't fade
        }
        if s.volume_sma <= 0.0 {
            return None;
        }
        let vol_ratio = c.volume / s.volume_sma;
        if vol_ratio < 1.5 {
            return None;
        }

        let (side, reason, entry, sl, tp) = if c.low <= bb.lower && rsi < 28.0 {
            let sl = bb.lower - 1.2 * bb.width * 0.5; // 1.2× half-width below band
            (
                Side::Long,
                format!("BB lower touch, RSI {rsi:.1}<28, vol×{vol_ratio:.2}"),
                c.close,
                sl.min(c.close - atr),
                bb.mid,
            )
        } else if c.high >= bb.upper && rsi > 72.0 {
            let sl = bb.upper + 1.2 * bb.width * 0.5;
            (
                Side::Short,
                format!("BB upper touch, RSI {rsi:.1}>72, vol×{vol_ratio:.2}"),
                c.close,
                sl.max(c.close + atr),
                bb.mid,
            )
        } else {
            return None;
        };

        let confidence = score_confidence(rsi, vol_ratio, adx);
        Some(PreSignal {
            symbol: s.symbol.clone(),
            strategy: StrategyName::MeanReversion,
            side,
            entry,
            stop_loss: sl,
            take_profit: tp,
            ta_confidence: confidence,
            reason,
        })
    }
}

fn score_confidence(rsi: f64, vol_ratio: f64, adx: f64) -> u8 {
    let mut score: f64 = 60.0;
    if !(20.0..=80.0).contains(&rsi) {
        score += 15.0;
    } else if !(25.0..=75.0).contains(&rsi) {
        score += 8.0;
    }
    if vol_ratio >= 2.0 {
        score += 10.0;
    }
    if adx < 15.0 {
        score += 10.0;
    }
    score.clamp(0.0, 100.0) as u8
}
