//! Strategy D — EMA Ribbon + RSI pullback entries.
//!
//! Tuned for HFT: works with just EMA8 + EMA21 (no EMA50/200 required).
//! This means it fires during warmup too — critical for day-1 operation.

use super::state::{PreSignal, StrategyName, SymbolState};
use super::Strategy;
use crate::data::{Candle, Side};

pub struct EmaRibbon;

impl Strategy for EmaRibbon {
    fn name(&self) -> StrategyName {
        StrategyName::EmaRibbon
    }

    fn evaluate(&self, s: &SymbolState, c: &Candle) -> Option<PreSignal> {
        // Minimum required: EMA8 + EMA21 (fast pair).
        // EMA50/200 are optional confirmation — don't block if missing.
        let e8 = s.ema_8.value()?;
        let e21 = s.ema_21.value()?;
        let rsi = s.last_rsi.unwrap_or(50.0);
        let atr = s.last_atr?;

        let e50 = s.ema_50.value();
        let e200 = s.ema_200.value();

        // Ribbon alignment — be flexible:
        // - Full alignment (8>21>50>close>200) = strong
        // - Partial alignment (8>21 + close above/below 200) = acceptable
        // - Minimal (8>21) = still valid for scalping
        let bullish_ribbon = e8 > e21;
        let bearish_ribbon = e8 < e21;

        // Extra confirmation if EMA50/200 are available
        let ema50_confirms_bull = e50.map(|e| e21 > e).unwrap_or(true);
        let ema50_confirms_bear = e50.map(|e| e21 < e).unwrap_or(true);
        let ema200_confirms_bull = e200.map(|e| c.close > e).unwrap_or(true);
        let ema200_confirms_bear = e200.map(|e| c.close < e).unwrap_or(true);

        if bullish_ribbon && ema50_confirms_bull && ema200_confirms_bull {
            // Pullback entry: price dipped near EMA21 from above
            let pullback_zone = c.low <= e21 * 1.003 && c.close > e21;
            if pullback_zone && rsi > 35.0 && rsi < 70.0 {
                let sl = (e50.unwrap_or(e21).min(c.low)) - 0.5 * atr;
                let tp = c.close + 1.5 * atr;
                let mut score: f64 = 66.0;
                // Full ribbon alignment bonus
                if e50.is_some() && e200.is_some() {
                    score += 5.0;
                }
                if rsi > 45.0 && rsi < 60.0 {
                    score += 5.0;
                }
                return Some(PreSignal {
                    symbol: s.symbol.clone(),
                    strategy: StrategyName::EmaRibbon,
                    side: Side::Long,
                    entry: c.close,
                    stop_loss: sl,
                    take_profit: tp,
                    ta_confidence: score.clamp(0.0, 100.0) as u8,
                    reason: format!("Ribbon bull + pullback EMA21 {e21:.4} RSI {rsi:.1}"),
                });
            }
        }

        if bearish_ribbon && ema50_confirms_bear && ema200_confirms_bear {
            let pullback_zone = c.high >= e21 * 0.997 && c.close < e21;
            if pullback_zone && rsi > 30.0 && rsi < 65.0 {
                let sl = (e50.unwrap_or(e21).max(c.high)) + 0.5 * atr;
                let tp = c.close - 1.5 * atr;
                let mut score: f64 = 66.0;
                if e50.is_some() && e200.is_some() {
                    score += 5.0;
                }
                if rsi > 40.0 && rsi < 55.0 {
                    score += 5.0;
                }
                return Some(PreSignal {
                    symbol: s.symbol.clone(),
                    strategy: StrategyName::EmaRibbon,
                    side: Side::Short,
                    entry: c.close,
                    stop_loss: sl,
                    take_profit: tp,
                    ta_confidence: score.clamp(0.0, 100.0) as u8,
                    reason: format!("Ribbon bear + pullback EMA21 {e21:.4} RSI {rsi:.1}"),
                });
            }
        }

        None
    }
}
