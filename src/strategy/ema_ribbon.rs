//! Strategy D — EMA Ribbon + RSI pullback entries.

use super::state::{PreSignal, StrategyName, SymbolState};
use super::Strategy;
use crate::data::{Candle, Side};

pub struct EmaRibbon;

impl Strategy for EmaRibbon {
    fn name(&self) -> StrategyName {
        StrategyName::EmaRibbon
    }

    fn evaluate(&self, s: &SymbolState, c: &Candle) -> Option<PreSignal> {
        let e8 = s.ema_8.value()?;
        let e21 = s.ema_21.value()?;
        let e50 = s.ema_50.value()?;
        let e200 = s.ema_200.value()?;
        let rsi = s.last_rsi.unwrap_or(50.0);
        let atr = s.last_atr?;

        let bullish_aligned = e8 > e21 && e21 > e50 && c.close > e200;
        let bearish_aligned = e8 < e21 && e21 < e50 && c.close < e200;

        if bullish_aligned {
            // pullback entry: price near EMA21 from above
            if c.low <= e21 * 1.001 && c.close > e21 && rsi > 40.0 && rsi < 65.0 {
                let sl = (e50.min(c.low)) - 0.5 * atr;
                let tp = c.close + 1.5 * atr;
                let mut score: f64 = 68.0;
                if rsi > 50.0 {
                    score += 5.0;
                }
                if (c.close - e200) / e200 * 100.0 > 0.5 {
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
        } else if bearish_aligned
            && c.high >= e21 * 0.999
            && c.close < e21
            && rsi > 35.0
            && rsi < 60.0
        {
            let sl = (e50.max(c.high)) + 0.5 * atr;
            let tp = c.close - 1.5 * atr;
            let mut score: f64 = 68.0;
            if rsi < 50.0 {
                score += 5.0;
            }
            if (e200 - c.close) / e200 * 100.0 > 0.5 {
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
        None
    }
}
