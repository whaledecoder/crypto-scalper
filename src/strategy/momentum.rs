//! Strategy B — Momentum Breakout with retest preference.
//!
//! Tuned for HFT scalping: lower volume threshold, relaxed ROC, tighter SL/TP.

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
        let ema50 = s.ema_50.value();
        let ema200 = s.ema_200.value();

        // Volume: 1.2× average (was 2.0×) — still elevated but not extreme
        if vol_ratio < 1.2 {
            return None;
        }

        // For EMA alignment, be flexible: if EMAs aren't warm yet, allow
        // the trade on breakout + ROC alone (ema50/ema200 are None during warmup).
        let ema_aligned_long = ema50.zip(ema200).map(|(a, b)| a > b).unwrap_or(true);
        let ema_aligned_short = ema50.zip(ema200).map(|(a, b)| a < b).unwrap_or(true);

        let (side, reason, sl, tp) = if c.close > highest && roc > 0.2 && ema_aligned_long {
            (
                Side::Long,
                format!("Long breakout > {highest:.4} vol×{vol_ratio:.2} ROC {roc:.2}%"),
                // Tighter SL for scalping: 0.8× ATR (was 1.0×)
                c.close - 0.8 * atr,
                // Closer TP: 1.5× ATR (was 2.0×) — faster profit capture
                c.close + 1.5 * atr,
            )
        } else if c.close < lowest && roc < -0.2 && ema_aligned_short {
            (
                Side::Short,
                format!("Short breakout < {lowest:.4} vol×{vol_ratio:.2} ROC {roc:.2}%"),
                c.close + 0.8 * atr,
                c.close - 1.5 * atr,
            )
        } else {
            return None;
        };

        let mut score: f64 = 65.0;
        if vol_ratio >= 2.0 {
            score += 10.0;
        } else if vol_ratio >= 1.5 {
            score += 5.0;
        }
        if (side == Side::Long && s.last_ofi.unwrap_or(0.0) > 0.0)
            || (side == Side::Short && s.last_ofi.unwrap_or(0.0) < 0.0)
        {
            score += 5.0;
        }
        if roc.abs() > 0.5 {
            score += 5.0;
        }
        if roc.abs() > 1.0 {
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
