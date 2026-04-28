//! Strategy C — VWAP Order Flow Scalping.

use super::state::{PreSignal, StrategyName, SymbolState};
use super::Strategy;
use crate::data::{Candle, Side};

pub struct VwapScalp;

impl Strategy for VwapScalp {
    fn name(&self) -> StrategyName {
        StrategyName::VwapScalp
    }

    fn evaluate(&self, s: &SymbolState, c: &Candle) -> Option<PreSignal> {
        let vwap = s.last_vwap?;
        let slope = s.last_vwap_slope.unwrap_or(0.0);
        let atr = s.last_atr?;

        let dist_pct = (c.close - vwap) / vwap.max(1e-9) * 100.0;

        let side = if slope > 0.0 && c.close <= vwap * 1.0005 && c.close >= vwap * 0.997 {
            Side::Long
        } else if slope < 0.0 && c.close >= vwap * 0.9995 && c.close <= vwap * 1.003 {
            Side::Short
        } else {
            return None;
        };

        let (sl, tp) = match side {
            Side::Long => (c.close - 0.5 * atr, vwap + 0.5 * atr),
            Side::Short => (c.close + 0.5 * atr, vwap - 0.5 * atr),
        };

        let mut score: f64 = 60.0;
        if slope.abs() > 0.0005 {
            score += 10.0;
        }
        if dist_pct.abs() < 0.1 {
            score += 5.0;
        }

        Some(PreSignal {
            symbol: s.symbol.clone(),
            strategy: StrategyName::VwapScalp,
            side,
            entry: c.close,
            stop_loss: sl,
            take_profit: tp,
            ta_confidence: score.clamp(0.0, 100.0) as u8,
            reason: format!("VWAP {vwap:.4} slope {slope:.5} dist {dist_pct:.2}%"),
        })
    }
}
