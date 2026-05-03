//! Strategy A — Mean Reversion.
//!
//! Fade the tails: BB band touch + RSI extreme + volume confirmation, only in
//! non-trending regimes.
//!
//! Tuned for HFT scalping: conditions are deliberately more permissive so the
//! strategy fires multiple times per session rather than once a week.

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

        // Allow mild trends (up to ADX 35) — only skip strong breakouts.
        if adx > 35.0 {
            return None;
        }
        if s.volume_sma <= 0.0 {
            return None;
        }
        let vol_ratio = c.volume / s.volume_sma;
        // Volume confirmation: even 0.8× is acceptable — the signal quality
        // comes from BB + RSI alignment, not volume alone.
        if vol_ratio < 0.8 {
            return None;
        }

        // --- Long: price touched or pierced lower BB + RSI oversold ---
        // Use RSI < 35 (was 28) to catch more setups.  For a 5-minute
        // scalper, RSI 35 is already significantly oversold.
        let (side, reason, entry, sl, tp) = if c.low <= bb.lower && rsi < 35.0 {
            // SL = 1.0× half-band below (was 1.2×) — tighter for scalping
            let sl = bb.lower - bb.width * 0.5;
            let sl = sl.min(c.close - atr * 0.8);
            (
                Side::Long,
                format!("BB lower touch, RSI {rsi:.1}<35, vol×{vol_ratio:.2}"),
                c.close,
                sl,
                // TP: mid-band OR 1.5× ATR, whichever is closer (faster exit)
                bb.mid.min(c.close + atr * 1.5),
            )
        // --- Short: price touched or pierced upper BB + RSI overbought ---
        } else if c.high >= bb.upper && rsi > 65.0 {
            let sl = bb.upper + bb.width * 0.5;
            let sl = sl.max(c.close + atr * 0.8);
            (
                Side::Short,
                format!("BB upper touch, RSI {rsi:.1}>65, vol×{vol_ratio:.2}"),
                c.close,
                sl,
                bb.mid.max(c.close - atr * 1.5),
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
    let mut score: f64 = 62.0; // Base slightly above the 60 threshold
                               // RSI extremes → higher confidence
    if !(20.0..=80.0).contains(&rsi) {
        score += 15.0;
    } else if !(30.0..=70.0).contains(&rsi) {
        score += 10.0;
    }
    // Volume spike → higher confidence
    if vol_ratio >= 2.0 {
        score += 12.0;
    } else if vol_ratio >= 1.2 {
        score += 5.0;
    }
    // Low ADX (ranging market) → higher confidence for mean reversion
    if adx < 15.0 {
        score += 10.0;
    } else if adx < 20.0 {
        score += 5.0;
    }
    score.clamp(0.0, 100.0) as u8
}
