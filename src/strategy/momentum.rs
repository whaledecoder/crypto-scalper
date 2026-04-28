//! Strategy B — Momentum Breakout with retest preference.

use super::state::{PreSignal, StrategyName, SymbolState};
use super::Strategy;
use crate::data::{Candle, Side};

pub struct Momentum;

impl Strategy for Momentum {
    fn name(&self) -> StrategyName {
        StrategyName::Momentum
    }

    fn evaluate(&self, s: &SymbolState, c: &Candle) -> Option<PreSignal> {
        if s.candles.len() < 21 {
            return None;
        }
        let recent: Vec<&Candle> = s.candles.iter().rev().skip(1).take(20).collect();
        let highest = recent
            .iter()
            .map(|x| x.high)
            .fold(f64::NEG_INFINITY, f64::max);
        let lowest = recent.iter().map(|x| x.low).fold(f64::INFINITY, f64::min);
        let vol_ratio = if s.volume_sma > 0.0 {
            c.volume / s.volume_sma
        } else {
            0.0
        };
        let roc = s.last_roc.unwrap_or(0.0);
        let atr = s.last_atr?;
        let ema50 = s.ema_50.value()?;
        let ema200 = s.ema_200.value()?;

        if vol_ratio < 2.0 {
            return None;
        }

        let (side, reason, sl, tp) = if c.close > highest && roc > 0.5 && ema50 > ema200 {
            (
                Side::Long,
                format!("Long breakout > {highest:.4} vol×{vol_ratio:.2} ROC {roc:.2}%"),
                c.close - 1.0 * atr,
                c.close + 2.0 * atr,
            )
        } else if c.close < lowest && roc < -0.5 && ema50 < ema200 {
            (
                Side::Short,
                format!("Short breakout < {lowest:.4} vol×{vol_ratio:.2} ROC {roc:.2}%"),
                c.close + 1.0 * atr,
                c.close - 2.0 * atr,
            )
        } else {
            return None;
        };

        let mut score: f64 = 60.0;
        if vol_ratio >= 2.5 {
            score += 8.0;
        }
        if roc.abs() > 1.0 {
            score += 7.0;
        }
        if (side == Side::Long && ema50 > ema200) || (side == Side::Short && ema50 < ema200) {
            score += 5.0;
        }

        Some(PreSignal {
            symbol: s.symbol.clone(),
            strategy: StrategyName::Momentum,
            side,
            entry: c.close,
            stop_loss: sl,
            take_profit: tp,
            ta_confidence: score.clamp(0.0, 100.0) as u8,
            reason,
        })
    }
}
