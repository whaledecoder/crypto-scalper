//! Build the Market Context Packet from TA + external feeds.

use crate::feeds::ExternalSnapshot;
use crate::strategy::state::{PreSignal, SymbolState};
use crate::strategy::Regime;
use serde::{Deserialize, Serialize};
use std::fmt::Write;

/// Snapshot handed to the LLM. Cloneable so it can be logged after the call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketContext {
    pub symbol: String,
    pub current_price: f64,
    pub pre_signal_direction: String,
    pub ta_confidence: u8,
    pub regime: String,
    pub strategy: String,
    pub proposed_entry: f64,
    pub proposed_sl: f64,
    pub proposed_tp: f64,
    pub rsi: Option<f64>,
    pub adx: Option<f64>,
    pub di_plus: Option<f64>,
    pub di_minus: Option<f64>,
    pub atr: Option<f64>,
    pub vwap: Option<f64>,
    pub vwap_slope: Option<f64>,
    pub choppiness: Option<f64>,
    pub ema_8: Option<f64>,
    pub ema_21: Option<f64>,
    pub ema_50: Option<f64>,
    pub ema_200: Option<f64>,
    pub bb_upper: Option<f64>,
    pub bb_lower: Option<f64>,
    pub bb_mid: Option<f64>,
    pub spread_pct: Option<f64>,
    pub best_bid: Option<f64>,
    pub best_ask: Option<f64>,
    pub external: ExternalSnapshot,
}

pub struct ContextBuilder;

impl ContextBuilder {
    pub fn build(
        state: &SymbolState,
        regime: Regime,
        signal: &PreSignal,
        external: ExternalSnapshot,
    ) -> MarketContext {
        let price = state.last_candle().map(|c| c.close).unwrap_or(0.0);
        MarketContext {
            symbol: state.symbol.clone(),
            current_price: price,
            pre_signal_direction: signal.side.as_str().to_string(),
            ta_confidence: signal.ta_confidence,
            regime: regime.as_str().to_string(),
            strategy: signal.strategy.as_str().to_string(),
            proposed_entry: signal.entry,
            proposed_sl: signal.stop_loss,
            proposed_tp: signal.take_profit,
            rsi: state.last_rsi,
            adx: state.last_adx,
            di_plus: state.last_di_plus,
            di_minus: state.last_di_minus,
            atr: state.last_atr,
            vwap: state.last_vwap,
            vwap_slope: state.last_vwap_slope,
            choppiness: state.last_choppiness,
            ema_8: state.ema_8.value(),
            ema_21: state.ema_21.value(),
            ema_50: state.ema_50.value(),
            ema_200: state.ema_200.value(),
            bb_upper: state.last_bb.map(|b| b.upper),
            bb_lower: state.last_bb.map(|b| b.lower),
            bb_mid: state.last_bb.map(|b| b.mid),
            spread_pct: state.order_book.spread_pct(),
            best_bid: state.order_book.best_bid(),
            best_ask: state.order_book.best_ask(),
            external,
        }
    }
}

impl MarketContext {
    /// Serialize as the human-readable "Market Context Packet" from the blueprint.
    pub fn build_prompt(&self) -> String {
        let mut s = String::new();
        let _ = writeln!(s, "=== MARKET CONTEXT PACKET ===");
        let _ = writeln!(s);
        let _ = writeln!(s, "[ASSET INFO]");
        let _ = writeln!(s, "  Symbol        : {}", self.symbol);
        let _ = writeln!(s, "  Current Price : {:.2}", self.current_price);

        let _ = writeln!(s, "\n[TECHNICAL SNAPSHOT]");
        if let (Some(e8), Some(e21), Some(e50)) = (self.ema_8, self.ema_21, self.ema_50) {
            let _ = writeln!(s, "  EMA 8/21/50   : {e8:.2} / {e21:.2} / {e50:.2}");
        }
        if let Some(e) = self.ema_200 {
            let _ = writeln!(s, "  EMA 200       : {e:.2}");
        }
        if let Some(r) = self.rsi {
            let _ = writeln!(s, "  RSI (14)      : {r:.2}");
        }
        if let (Some(l), Some(m), Some(u)) = (self.bb_lower, self.bb_mid, self.bb_upper) {
            let _ = writeln!(s, "  BB (20,2)     : L:{l:.2} M:{m:.2} U:{u:.2}");
        }
        if let Some(v) = self.vwap {
            let _ = writeln!(s, "  VWAP          : {v:.2}");
        }
        if let Some(v) = self.vwap_slope {
            let _ = writeln!(s, "  VWAP slope    : {v:.6}");
        }
        if let Some(v) = self.atr {
            let _ = writeln!(s, "  ATR (14)      : {v:.2}");
        }
        if let (Some(a), Some(p), Some(m)) = (self.adx, self.di_plus, self.di_minus) {
            let _ = writeln!(s, "  ADX / DI±     : {a:.2} / {p:.2} / {m:.2}");
        }
        if let Some(c) = self.choppiness {
            let _ = writeln!(s, "  Choppiness    : {c:.2}");
        }
        let _ = writeln!(s, "  Regime        : {}", self.regime);
        let _ = writeln!(s, "  Strategy      : {}", self.strategy);
        let _ = writeln!(s, "  Pre-signal    : {}", self.pre_signal_direction);
        let _ = writeln!(s, "  TA Confidence : {}/100", self.ta_confidence);
        let _ = writeln!(s, "  Proposed Entry: {:.2}", self.proposed_entry);
        let _ = writeln!(s, "  Proposed SL   : {:.2}", self.proposed_sl);
        let _ = writeln!(s, "  Proposed TP   : {:.2}", self.proposed_tp);

        let _ = writeln!(s, "\n[ORDER BOOK]");
        if let (Some(b), Some(a)) = (self.best_bid, self.best_ask) {
            let _ = writeln!(s, "  Best bid/ask  : {b:.2} / {a:.2}");
        }
        if let Some(sp) = self.spread_pct {
            let _ = writeln!(s, "  Spread %      : {sp:.4}");
        }

        if let Some(fg) = &self.external.fear_greed {
            let _ = writeln!(s, "\n[FEAR & GREED]");
            let _ = writeln!(s, "  Value         : {} ({})", fg.value, fg.label.as_str());
            if let Some(a) = fg.avg_7d {
                let _ = writeln!(s, "  7-day average : {a}");
            }
        }

        if let Some(f) = &self.external.funding {
            let _ = writeln!(s, "\n[FUNDING]");
            let _ = writeln!(s, "  Rate          : {}", f.rate);
            if let Some(oi) = f.open_interest {
                let _ = writeln!(s, "  Open Interest : {oi}");
            }
        }

        if let Some(o) = &self.external.onchain {
            let _ = writeln!(s, "\n[ON-CHAIN]");
            if let Some(v) = o.exchange_inflow_24h {
                let _ = writeln!(s, "  Exch inflow 24h : {v}");
            }
            if let Some(v) = o.exchange_outflow_24h {
                let _ = writeln!(s, "  Exch outflow 24h: {v}");
            }
            if let Some(v) = o.whale_tx_1h {
                let _ = writeln!(s, "  Whale tx 1h     : {v}");
            }
            if let Some(v) = o.sopr_1h {
                let _ = writeln!(s, "  SOPR 1h         : {v}");
            }
        }

        if let Some(snt) = &self.external.sentiment {
            let _ = writeln!(s, "\n[SOCIAL SENTIMENT]");
            let _ = writeln!(
                s,
                "  Volume 24h    : {} (+{:.1}%)",
                snt.social_volume, snt.social_volume_change_pct
            );
            let _ = writeln!(s, "  Sentiment     : {:.2}", snt.sentiment);
            if let Some(g) = snt.galaxy_score {
                let _ = writeln!(s, "  Galaxy score  : {g:.2}");
            }
        }

        if let Some(n) = &self.external.news {
            let _ = writeln!(s, "\n[NEWS HEADLINES]");
            for item in n.items.iter().take(8) {
                let _ = writeln!(
                    s,
                    "  [{}] {} ({})",
                    item.impact.as_str(),
                    item.title,
                    item.source
                );
            }
            let _ = writeln!(s, "  Net score     : {:+.2}", n.net_score);
        }

        s
    }
}
