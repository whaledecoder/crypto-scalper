//! State that is shared across all strategies for a given symbol.

use crate::data::{Candle, OrderBook, Side};
use crate::indicators::{
    Adx, Atr, Bollinger, BollingerBand, Choppiness, Ema, Keltner, Roc, Rsi, Vwap,
};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum StrategyName {
    MeanReversion,
    Momentum,
    VwapScalp,
    EmaRibbon,
    Squeeze,
}

impl StrategyName {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::MeanReversion => "mean_reversion",
            Self::Momentum => "momentum",
            Self::VwapScalp => "vwap_scalp",
            Self::EmaRibbon => "ema_ribbon",
            Self::Squeeze => "squeeze",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "mean_reversion" => Some(Self::MeanReversion),
            "momentum" => Some(Self::Momentum),
            "vwap_scalp" => Some(Self::VwapScalp),
            "ema_ribbon" => Some(Self::EmaRibbon),
            "squeeze" => Some(Self::Squeeze),
            _ => None,
        }
    }
}

/// A candidate trade signal from a strategy, before LLM / gate validation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreSignal {
    pub symbol: String,
    pub strategy: StrategyName,
    pub side: Side,
    pub entry: f64,
    pub stop_loss: f64,
    pub take_profit: f64,
    pub ta_confidence: u8,
    pub reason: String,
}

impl PreSignal {
    pub fn rr(&self) -> f64 {
        let risk = (self.entry - self.stop_loss).abs();
        if risk <= 0.0 {
            return 0.0;
        }
        let reward = (self.take_profit - self.entry).abs();
        reward / risk
    }
}

/// Rolling state per symbol — wraps indicator pipelines + recent candles.
pub struct SymbolState {
    pub symbol: String,
    pub candles: VecDeque<Candle>,
    pub max_candles: usize,

    pub ema_8: Ema,
    pub ema_21: Ema,
    pub ema_50: Ema,
    pub ema_200: Ema,

    pub rsi: Rsi,
    pub bollinger: Bollinger,
    pub atr: Atr,
    pub adx: Adx,
    pub vwap: Vwap,
    pub choppiness: Choppiness,
    pub keltner: Keltner,
    pub roc: Roc,

    pub last_bb: Option<BollingerBand>,
    pub last_adx: Option<f64>,
    pub last_di_plus: Option<f64>,
    pub last_di_minus: Option<f64>,
    pub last_rsi: Option<f64>,
    pub last_atr: Option<f64>,
    pub last_vwap: Option<f64>,
    pub last_vwap_slope: Option<f64>,
    pub last_choppiness: Option<f64>,
    pub last_keltner_upper: Option<f64>,
    pub last_keltner_lower: Option<f64>,
    pub last_roc: Option<f64>,

    pub order_book: OrderBook,
    pub volume_sma: f64,
    pub volume_sma_count: usize,
}

impl SymbolState {
    pub fn new(symbol: impl Into<String>) -> Self {
        Self {
            symbol: symbol.into(),
            candles: VecDeque::with_capacity(512),
            max_candles: 512,
            ema_8: Ema::new(8),
            ema_21: Ema::new(21),
            ema_50: Ema::new(50),
            ema_200: Ema::new(200),
            rsi: Rsi::new(14),
            bollinger: Bollinger::new(20, 2.0),
            atr: Atr::new(14),
            adx: Adx::new(14),
            vwap: Vwap::new(),
            choppiness: Choppiness::new(14),
            keltner: Keltner::new(20, 1.5),
            roc: Roc::new(3),
            last_bb: None,
            last_adx: None,
            last_di_plus: None,
            last_di_minus: None,
            last_rsi: None,
            last_atr: None,
            last_vwap: None,
            last_vwap_slope: None,
            last_choppiness: None,
            last_keltner_upper: None,
            last_keltner_lower: None,
            last_roc: None,
            order_book: OrderBook::default(),
            volume_sma: 0.0,
            volume_sma_count: 0,
        }
    }

    /// Ingest a closed candle and update every indicator.
    pub fn on_closed(&mut self, c: Candle) {
        self.candles.push_back(c);
        if self.candles.len() > self.max_candles {
            self.candles.pop_front();
        }

        let _ = self.ema_8.next(c.close);
        let _ = self.ema_21.next(c.close);
        let _ = self.ema_50.next(c.close);
        let _ = self.ema_200.next(c.close);

        self.last_rsi = self.rsi.next(c.close).or(self.last_rsi);
        if let Some(bb) = self.bollinger.next(c.close) {
            self.last_bb = Some(bb);
        }
        if let Some(a) = self.atr.next(&c) {
            self.last_atr = Some(a);
        }
        if let Some(adx) = self.adx.next(&c) {
            self.last_adx = Some(adx.adx);
            self.last_di_plus = Some(adx.di_plus);
            self.last_di_minus = Some(adx.di_minus);
        }
        if let Some(v) = self.vwap.next(&c) {
            self.last_vwap = Some(v);
            self.last_vwap_slope = self.vwap.slope();
        }
        if let Some(ch) = self.choppiness.next(&c) {
            self.last_choppiness = Some(ch);
        }
        if let Some(k) = self.keltner.next(&c) {
            self.last_keltner_lower = Some(k.lower);
            self.last_keltner_upper = Some(k.upper);
        }
        if let Some(r) = self.roc.next(c.close) {
            self.last_roc = Some(r);
        }

        // Running volume SMA (last 20 candles)
        let window = 20usize.min(self.candles.len());
        let sum: f64 = self
            .candles
            .iter()
            .rev()
            .take(window)
            .map(|x| x.volume)
            .sum();
        self.volume_sma = sum / window as f64;
        self.volume_sma_count = window;
    }

    pub fn last_candle(&self) -> Option<&Candle> {
        self.candles.back()
    }
}
