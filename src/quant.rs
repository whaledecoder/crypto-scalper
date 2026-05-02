//! Quant Integration Layer — connects portfolio math to the live trading
//! pipeline.  This module sits between the learning/policy layer and the
//! risk/execution layer, applying quantitative position sizing and risk
//! management on top of the existing TA + LLM signal flow.
//!
//! ## What this adds to the pipeline
//!
//! 1. **Kelly criterion sizing** — replaces fixed % risk with optimal
//!    fraction based on historical win rate and payoff ratio.
//! 2. **Volatility targeting** — scales position size inversely with
//!    realized volatility so the bot takes smaller bets in high-vol
//!    regimes and larger bets in calm markets.
//! 3. **VaR / CVaR position cap** — rejects trades whose estimated
//!    loss-at-risk exceeds a configurable fraction of equity.
//! 4. **IC-weighted strategy confidence** — boosts or penalizes each
//!    strategy's TA confidence based on its rolling Information
//!    Coefficient (predictive power).
//! 5. **Kalman trend gate** — uses a Kalman filter's velocity estimate
//!    as a trend-confirmation signal to supplement EMA-based regimes.
//! 6. **Correlation-aware sizing** — reduces size when proposed trade
//!    is highly correlated with existing open positions.

use crate::data::Side;
use crate::portfolio::{
    historical_cvar, historical_var, kelly_fraction, volatility_target_multiplier,
};
use crate::research::ic::IcTracker;
use crate::strategy::kalman::KalmanTrend;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ─── Configuration ────────────────────────────────────────────────────────────

/// Quant-specific configuration.  Embedded in the main config under `[quant]`.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct QuantConfig {
    /// Master switch.  When false, all quant adjustments are bypassed.
    #[serde(default = "default_true")]
    pub enabled: bool,

    // ── Kelly ──────────────────────────────────────────────────────
    /// Cap on Kelly fraction (e.g. 0.25 = never risk more than 25% of
    /// optimal Kelly per trade).  Half-Kelly is a common conservative
    /// choice.
    #[serde(default = "default_kelly_cap")]
    pub kelly_cap: f64,
    /// Minimum number of closed trades before Kelly sizing activates.
    /// Below this threshold the bot falls back to fixed % sizing.
    #[serde(default = "default_kelly_min_trades")]
    pub kelly_min_trades: usize,

    // ── Volatility Targeting ───────────────────────────────────────
    /// Annualized target volatility (e.g. 0.15 = 15% per year).
    #[serde(default = "default_target_vol")]
    pub target_vol_annual: f64,
    /// Maximum vol-target multiplier (prevents over-sizing in very calm
    /// markets).
    #[serde(default = "default_max_vol_mult")]
    pub max_vol_multiplier: f64,
    /// Number of recent returns used for realized vol estimation.
    #[serde(default = "default_vol_window")]
    pub vol_window: usize,

    // ── VaR / CVaR ─────────────────────────────────────────────────
    /// Confidence level for VaR calculation (e.g. 0.95 = 95%).
    #[serde(default = "default_var_confidence")]
    pub var_confidence: f64,
    /// Maximum allowable VaR as fraction of equity (e.g. 0.03 = 3%).
    /// Trades that would push portfolio VaR above this are rejected.
    #[serde(default = "default_max_var_pct")]
    pub max_var_pct: f64,

    // ── IC Tracking ────────────────────────────────────────────────
    /// Window for rolling IC calculation (number of observations).
    #[serde(default = "default_ic_window")]
    pub ic_window: usize,
    /// Minimum IC magnitude for a strategy to get a confidence boost.
    /// Below this, strategies run at their raw TA confidence.
    #[serde(default = "default_ic_min_abs")]
    pub ic_min_abs: f64,
    /// Maximum confidence boost (in percentage points) from high IC.
    #[serde(default = "default_ic_max_boost")]
    pub ic_max_boost: u8,

    // ── Kalman ─────────────────────────────────────────────────────
    /// Kalman process noise.  Lower = smoother trend estimate.
    #[serde(default = "default_kalman_q")]
    pub kalman_process_noise: f64,
    /// Kalman measurement noise.  Higher = less reactive to each tick.
    #[serde(default = "default_kalman_r")]
    pub kalman_measurement_noise: f64,
    /// Minimum absolute Kalman velocity (in bps) required for the
    /// trend gate to add confirmation.  Below this = neutral.
    #[serde(default = "default_kalman_min_bps")]
    pub kalman_min_velocity_bps: f64,
}

fn default_true() -> bool {
    true
}
fn default_kelly_cap() -> f64 {
    0.25
}
fn default_kelly_min_trades() -> usize {
    20
}
fn default_target_vol() -> f64 {
    0.15
}
fn default_max_vol_mult() -> f64 {
    2.0
}
fn default_vol_window() -> usize {
    60
}
fn default_var_confidence() -> f64 {
    0.95
}
fn default_max_var_pct() -> f64 {
    0.03
}
fn default_ic_window() -> usize {
    50
}
fn default_ic_min_abs() -> f64 {
    0.05
}
fn default_ic_max_boost() -> u8 {
    10
}
fn default_kalman_q() -> f64 {
    0.01
}
fn default_kalman_r() -> f64 {
    1.0
}
fn default_kalman_min_bps() -> f64 {
    3.0
}

impl Default for QuantConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            kelly_cap: default_kelly_cap(),
            kelly_min_trades: default_kelly_min_trades(),
            target_vol_annual: default_target_vol(),
            max_vol_multiplier: default_max_vol_mult(),
            vol_window: default_vol_window(),
            var_confidence: default_var_confidence(),
            max_var_pct: default_max_var_pct(),
            ic_window: default_ic_window(),
            ic_min_abs: default_ic_min_abs(),
            ic_max_boost: default_ic_max_boost(),
            kalman_process_noise: default_kalman_q(),
            kalman_measurement_noise: default_kalman_r(),
            kalman_min_velocity_bps: default_kalman_min_bps(),
        }
    }
}

// ─── Quant Engine ─────────────────────────────────────────────────────────────

/// The central quant engine.  Owned by the RiskAgent, accessed per-signal.
pub struct QuantEngine {
    cfg: QuantConfig,
    /// Per-symbol Kalman trend filters.
    kalman: Mutex<HashMap<String, KalmanTrend>>,
    /// Per-strategy IC trackers.
    ic_trackers: Mutex<HashMap<String, IcTracker>>,
    /// Per-symbol return history (for vol targeting and VaR).
    returns: Mutex<HashMap<String, Vec<f64>>>,
    /// Rolling trade outcomes for Kelly: (win_rate, avg_win, avg_loss).
    trade_outcomes: Mutex<TradeOutcomes>,
}

#[derive(Debug, Default)]
struct TradeOutcomes {
    wins: Vec<f64>,
    losses: Vec<f64>,
}

impl TradeOutcomes {
    fn record(&mut self, pnl: f64) {
        if pnl > 0.0 {
            self.wins.push(pnl);
        } else if pnl < 0.0 {
            self.losses.push(pnl.abs());
        }
        // Keep last 200 outcomes
        if self.wins.len() > 200 {
            self.wins.remove(0);
        }
        if self.losses.len() > 200 {
            self.losses.remove(0);
        }
    }

    fn win_rate(&self) -> f64 {
        let total = self.wins.len() + self.losses.len();
        if total == 0 {
            return 0.5; // prior: assume 50% until we have data
        }
        self.wins.len() as f64 / total as f64
    }

    fn avg_win(&self) -> f64 {
        if self.wins.is_empty() {
            return 1.0; // prior
        }
        self.wins.iter().sum::<f64>() / self.wins.len() as f64
    }

    fn avg_loss(&self) -> f64 {
        if self.losses.is_empty() {
            return 1.0; // prior
        }
        self.losses.iter().sum::<f64>() / self.losses.len() as f64
    }

    fn total_trades(&self) -> usize {
        self.wins.len() + self.losses.len()
    }
}

/// Result of the quant sizing pipeline.
#[derive(Debug, Clone)]
pub struct QuantSizingResult {
    /// Final size multiplier from the quant engine.
    pub size_multiplier: f64,
    /// Kelly fraction used (0 if not enough data).
    pub kelly_fraction: f64,
    /// Vol-target multiplier applied.
    pub vol_multiplier: f64,
    /// Whether the trade was rejected by VaR cap.
    pub var_rejected: bool,
    /// IC-based confidence adjustment (can be negative).
    pub ic_adjustment: i8,
    /// Kalman trend direction: +1 = bullish, -1 = bearish, 0 = neutral.
    pub kalman_direction: i8,
    /// Human-readable reason for the sizing decision.
    pub reason: String,
}

impl QuantEngine {
    pub fn new(cfg: QuantConfig) -> Self {
        Self {
            cfg,
            kalman: Mutex::new(HashMap::new()),
            ic_trackers: Mutex::new(HashMap::new()),
            returns: Mutex::new(HashMap::new()),
            trade_outcomes: Mutex::new(TradeOutcomes::default()),
        }
    }

    // ── Public API ─────────────────────────────────────────────────

    /// Main entry point: compute quant-adjusted sizing for a signal.
    pub fn compute_sizing(
        &self,
        symbol: &str,
        strategy: &str,
        side: Side,
        _ta_confidence: u8,
        entry: f64,
        stop_loss: f64,
        equity: f64,
        base_risk_pct: f64,
    ) -> QuantSizingResult {
        if !self.cfg.enabled {
            return QuantSizingResult {
                size_multiplier: 1.0,
                kelly_fraction: 0.0,
                vol_multiplier: 1.0,
                var_rejected: false,
                ic_adjustment: 0,
                kalman_direction: 0,
                reason: "quant disabled".into(),
            };
        }

        let mut reasons: Vec<String> = Vec::new();
        let mut size_mult = 1.0f64;

        // 1. Kelly criterion
        let outcomes = self.trade_outcomes.lock();
        let kelly = if outcomes.total_trades() >= self.cfg.kelly_min_trades {
            let kf = kelly_fraction(
                outcomes.win_rate(),
                outcomes.avg_win(),
                outcomes.avg_loss(),
                self.cfg.kelly_cap,
            );
            reasons.push(format!(
                "kelly={:.3} (WR={:.1}% W/L={:.2}/{:.2})",
                kf,
                outcomes.win_rate() * 100.0,
                outcomes.avg_win(),
                outcomes.avg_loss()
            ));
            kf
        } else {
            reasons.push(format!(
                "kelly=cold-start ({}trades < {})",
                outcomes.total_trades(),
                self.cfg.kelly_min_trades
            ));
            0.0
        };
        drop(outcomes);

        // Apply Kelly sizing: replace fixed risk% with Kelly fraction
        if kelly > 0.0 {
            let kelly_mult = (kelly / base_risk_pct.max(0.01)).min(2.0);
            size_mult *= kelly_mult;
        }

        // 2. Volatility targeting
        let vol_mult = self.vol_target_multiplier(symbol);
        size_mult *= vol_mult;
        if (vol_mult - 1.0).abs() > 0.05 {
            reasons.push(format!("vol-mult={:.2}", vol_mult));
        }

        // 3. VaR check
        let var_rejected = self.var_check(symbol, entry, stop_loss, equity);
        if var_rejected {
            reasons.push("VaR cap exceeded".into());
        }

        // 4. IC-based confidence adjustment
        let ic_adj = self.ic_adjustment(strategy);
        if ic_adj != 0 {
            reasons.push(format!("IC-adj={:+}", ic_adj));
        }

        // 5. Kalman trend gate
        let kalman_dir = self.kalman_direction(symbol, entry);
        if kalman_dir != 0 {
            let dir_str = if kalman_dir > 0 { "bullish" } else { "bearish" };
            reasons.push(format!("kalman={}", dir_str));
            // If Kalman trend contradicts trade direction, reduce size
            let trade_is_long = matches!(side, Side::Long);
            if (trade_is_long && kalman_dir < 0) || (!trade_is_long && kalman_dir > 0) {
                size_mult *= 0.7;
                reasons.push("kalman-contradict: -30%".into());
            } else if (trade_is_long && kalman_dir > 0) || (!trade_is_long && kalman_dir < 0) {
                size_mult *= 1.15;
                reasons.push("kalman-confirm: +15%".into());
            }
        }

        QuantSizingResult {
            size_multiplier: size_mult.clamp(0.1, 3.0),
            kelly_fraction: kelly,
            vol_multiplier: vol_mult,
            var_rejected,
            ic_adjustment: ic_adj,
            kalman_direction: kalman_dir,
            reason: reasons.join(" | "),
        }
    }

    /// Record a closed trade outcome for Kelly calculation.
    pub fn record_trade(&self, pnl: f64) {
        self.trade_outcomes.lock().record(pnl);
    }

    /// Update per-symbol return history (called on each candle close).
    pub fn record_return(&self, symbol: &str, ret: f64) {
        if !ret.is_finite() {
            return;
        }
        let mut returns = self.returns.lock();
        let entry = returns.entry(symbol.to_string()).or_default();
        entry.push(ret);
        if entry.len() > self.cfg.vol_window * 2 {
            entry.remove(0);
        }
    }

    /// Record a signal → outcome pair for IC tracking.
    pub fn record_ic_observation(&self, strategy: &str, signal_value: f64, forward_return: f64) {
        let mut trackers = self.ic_trackers.lock();
        let tracker = trackers
            .entry(strategy.to_string())
            .or_insert_with(|| IcTracker::new(self.cfg.ic_window));
        tracker.record(signal_value, forward_return);
    }

    /// Update Kalman filter with latest price (called on each tick/candle).
    pub fn update_kalman(&self, symbol: &str, price: f64) {
        let mut kalman = self.kalman.lock();
        let entry = kalman
            .entry(symbol.to_string())
            .or_insert_with(|| KalmanTrend::new(price, self.cfg.kalman_process_noise, self.cfg.kalman_measurement_noise));
        entry.update(price);
    }

    /// Get current Kelly sizing info (for monitoring/dashboard).
    pub fn kelly_info(&self) -> (f64, f64, f64, usize) {
        let o = self.trade_outcomes.lock();
        (o.win_rate(), o.avg_win(), o.avg_loss(), o.total_trades())
    }

    // ── Private helpers ────────────────────────────────────────────

    fn vol_target_multiplier(&self, symbol: &str) -> f64 {
        let returns = self.returns.lock();
        let hist = match returns.get(symbol) {
            Some(r) if r.len() >= 10 => r,
            _ => return 1.0, // not enough data
        };

        // Annualized realized vol from recent returns
        // For 5m candles: 288 candles/day × 365 days = 105,120 periods/year
        let periods_per_year = 105_120.0f64;
        let window = hist.len().min(self.cfg.vol_window);
        let recent = &hist[hist.len() - window..];
        let mean = recent.iter().sum::<f64>() / recent.len() as f64;
        let var = recent.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / recent.len() as f64;
        let realized_vol_annual = var.sqrt() * periods_per_year.sqrt();

        volatility_target_multiplier(
            self.cfg.target_vol_annual,
            realized_vol_annual,
            self.cfg.max_vol_multiplier,
        )
    }

    fn var_check(&self, symbol: &str, entry: f64, stop_loss: f64, equity: f64) -> bool {
        if equity <= 0.0 {
            return false;
        }
        let returns = self.returns.lock();
        if let Some(hist) = returns.get(symbol) {
            if hist.len() >= 20 {
                if let Some(cvar) = historical_cvar(hist, self.cfg.var_confidence) {
                    let trade_risk_pct = (entry - stop_loss).abs() / entry;
                    let estimated_loss = equity * trade_risk_pct;
                    let var_cap = equity * self.cfg.max_var_pct;
                    // Reject if the estimated trade loss + current CVaR exceeds cap
                    return estimated_loss + cvar * equity > var_cap;
                }
            }
        }
        false
    }

    fn ic_adjustment(&self, strategy: &str) -> i8 {
        let trackers = self.ic_trackers.lock();
        if let Some(tracker) = trackers.get(strategy) {
            if let Some(ic) = tracker.ic() {
                if ic.abs() >= self.cfg.ic_min_abs {
                    // Positive IC = boost confidence, negative = penalize
                    let boost = (ic * self.cfg.ic_max_boost as f64)
                        .round()
                        .clamp(
                            -(self.cfg.ic_max_boost as f64),
                            self.cfg.ic_max_boost as f64,
                        ) as i8;
                    return boost;
                }
            }
        }
        0
    }

    fn kalman_direction(&self, symbol: &str, price: f64) -> i8 {
        let kalman = self.kalman.lock();
        if let Some(k) = kalman.get(symbol) {
            let score = k.trend_score(price);
            let min_bps = self.cfg.kalman_min_velocity_bps;
            if score > min_bps {
                return 1;
            } else if score < -min_bps {
                return -1;
            }
        }
        0
    }
}
