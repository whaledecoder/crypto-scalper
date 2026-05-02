//! Strategy C — VWAP Order Flow Scalping.
//!
//! Tuned for HFT: wider zones, more permissive slope check, tighter SL for
//! faster risk resolution.

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

        // Wider zones: ±1.0% from VWAP (was ±0.5%) — crypto is volatile
        let long_zone = dist_pct >= -1.0 && dist_pct <= 0.3;
        let short_zone = dist_pct <= 1.0 && dist_pct >= -0.3;

        // Allow trades even with flat slope — the zone itself is the edge
        let side = if long_zone && slope >= -0.001 {
            Side::Long
        } else if short_zone && slope <= 0.001 {
            Side::Short
        } else {
            return None;
        };

        // Tighter SL (0.4× ATR) for scalping — faster stop-out = smaller losses
        let (sl, tp) = match side {
            Side::Long => (
                c.close - 0.4 * atr,
                // TP: VWAP + small buffer OR 1.2× ATR, whichever is closer
                vwap.min(c.close + atr * 1.2),
            ),
            Side::Short => (
                c.close + 0.4 * atr,
                vwap.max(c.close - atr * 1.2),
            ),
        };

        let mut score: f64 = 62.0; // Above the 60 threshold
        if slope.abs() > 0.0003 {
            score += 8.0;
        }
        // Closer to VWAP = higher confidence
        if dist_pct.abs() < 0.15 {
            score += 10.0;
        } else if dist_pct.abs() < 0.3 {
            score += 5.0;
        }
        // OFI confirmation
        if (side == Side::Long && s.last_ofi.unwrap_or(0.0) > 0.0)
            || (side == Side::Short && s.last_ofi.unwrap_or(0.0) < 0.0)
        {
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
