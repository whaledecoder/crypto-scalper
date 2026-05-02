//! Configuration loader. Reads `config/default.toml` + optional overlay and
//! environment variables.

use crate::errors::{Result, ScalperError};
use serde::{Deserialize, Serialize};
use std::{fs, path::Path};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Config {
    pub mode: Mode,
    pub exchange: Exchange,
    pub pairs: Pairs,
    pub strategy: StrategyCfg,
    #[serde(default)]
    pub advanced_alpha: AdvancedAlphaCfg,
    pub llm: LlmCfg,
    #[serde(default)]
    pub manager: ManagerCfg,
    pub risk: RiskCfg,
    pub schedule: Schedule,
    pub feeds: Feeds,
    pub monitoring: Monitoring,
    pub backtest: Backtest,
    #[serde(default)]
    pub survival: SurvivalCfg,
    #[serde(default)]
    pub control: ControlCfg,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Mode {
    pub run_mode: String,
    pub dry_run: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Exchange {
    pub name: String,
    pub market: String,
    pub rest_base_url: String,
    pub ws_base_url: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub api_secret: String,
    pub recv_window_ms: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Pairs {
    pub symbols: Vec<String>,
    pub timeframes: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct StrategyCfg {
    pub mode: String,
    pub active: Vec<String>,
    pub min_ta_confidence: u8,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AdvancedAlphaCfg {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_alpha_min_abs_score")]
    pub min_abs_score: f64,
    #[serde(default = "default_alpha_reduce_confidence_delta")]
    pub reduce_confidence_delta: u8,
    #[serde(default = "default_alpha_feed_max_age_secs")]
    pub feed_max_age_secs: u64,
    #[serde(default = "default_alpha_process_noise")]
    pub kalman_process_noise: f64,
    #[serde(default = "default_alpha_measurement_noise")]
    pub kalman_measurement_noise: f64,
}

fn default_alpha_min_abs_score() -> f64 {
    0.2
}
fn default_alpha_reduce_confidence_delta() -> u8 {
    5
}
fn default_alpha_feed_max_age_secs() -> u64 {
    180
}
fn default_alpha_process_noise() -> f64 {
    0.01
}
fn default_alpha_measurement_noise() -> f64 {
    1.0
}

impl Default for AdvancedAlphaCfg {
    fn default() -> Self {
        Self {
            enabled: false,
            min_abs_score: default_alpha_min_abs_score(),
            reduce_confidence_delta: default_alpha_reduce_confidence_delta(),
            feed_max_age_secs: default_alpha_feed_max_age_secs(),
            kalman_process_noise: default_alpha_process_noise(),
            kalman_measurement_noise: default_alpha_measurement_noise(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LlmCfg {
    pub provider: String,
    pub model: String,
    #[serde(default)]
    pub api_key: String,
    pub api_base: String,
    pub timeout_secs: u64,
    pub min_confidence: u8,
    pub fallback_ta_threshold: u8,
    pub max_tokens: u32,
    /// Optional HTTP-Referer for OpenRouter (used for analytics & rate-limit
    /// boosts on free models).
    #[serde(default)]
    pub http_referer: String,
    /// Optional X-Title shown in OpenRouter dashboards.
    #[serde(default)]
    pub http_app_title: String,
}

/// Configuration for the TraderManagerAgent (multi-agent overseer LLM).
/// Disabled by default — the bot runs in single-LLM mode unless this is
/// explicitly turned on.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ManagerCfg {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_manager_provider")]
    pub provider: String,
    #[serde(default = "default_manager_api_base")]
    pub api_base: String,
    #[serde(default = "default_manager_model")]
    pub model: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default = "default_manager_timeout_secs")]
    pub timeout_secs: u64,
    #[serde(default = "default_manager_max_tokens")]
    pub max_tokens: u32,
    #[serde(default = "default_manager_fast_approve")]
    pub fast_approve_min_conf: u8,
    #[serde(default)]
    pub http_referer: String,
    #[serde(default)]
    pub http_app_title: String,
    #[serde(default)]
    pub fail_open_on_error: bool,
}

fn default_manager_provider() -> String {
    "openrouter".into()
}
fn default_manager_api_base() -> String {
    "https://openrouter.ai/api/v1/chat/completions".into()
}
fn default_manager_model() -> String {
    "anthropic/claude-3.5-haiku".into()
}
fn default_manager_timeout_secs() -> u64 {
    6
}
fn default_manager_max_tokens() -> u32 {
    600
}
fn default_manager_fast_approve() -> u8 {
    90
}

impl Default for ManagerCfg {
    fn default() -> Self {
        Self {
            enabled: false,
            provider: default_manager_provider(),
            api_base: default_manager_api_base(),
            model: default_manager_model(),
            api_key: String::new(),
            timeout_secs: default_manager_timeout_secs(),
            max_tokens: default_manager_max_tokens(),
            fast_approve_min_conf: default_manager_fast_approve(),
            http_referer: String::new(),
            http_app_title: String::new(),
            fail_open_on_error: false,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RiskCfg {
    pub risk_per_trade_pct: f64,
    pub max_open_positions: u32,
    pub max_daily_loss_pct: f64,
    pub max_drawdown_pct: f64,
    pub max_leverage: u32,
    pub max_spread_pct: f64,
    pub min_reward_risk: f64,
    pub max_position_notional_pct: f64,
    pub min_net_edge_bps: f64,
    pub assumed_daily_volume_usd: f64,
    pub equity_usd: f64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Schedule {
    pub dead_zone_start_hour_wib: u8,
    pub dead_zone_end_hour_wib: u8,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Feeds {
    #[serde(default)]
    pub cryptopanic_api_key: String,
    #[serde(default)]
    pub lunarcrush_api_key: String,
    #[serde(default)]
    pub glassnode_api_key: String,
    #[serde(default)]
    pub whalealert_api_key: String,
    #[serde(default = "default_deribit_base_url")]
    pub deribit_base_url: String,
    #[serde(default)]
    pub rss_feeds: Vec<String>,
}

fn default_deribit_base_url() -> String {
    "https://www.deribit.com".into()
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Monitoring {
    #[serde(default)]
    pub telegram_bot_token: String,
    #[serde(default)]
    pub telegram_chat_id: String,
    pub log_level: String,
    pub db_path: String,
    pub metrics_bind: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Backtest {
    pub data_dir: String,
    #[serde(default)]
    pub from_ts: String,
    #[serde(default)]
    pub to_ts: String,
    #[serde(default = "default_backtest_fee_bps")]
    pub fee_bps: f64,
    #[serde(default = "default_backtest_slippage_bps")]
    pub slippage_bps: f64,
    #[serde(default = "default_backtest_market_impact_bps")]
    pub market_impact_bps: f64,
    #[serde(default = "default_backtest_trading_days_per_year")]
    pub trading_days_per_year: f64,
    #[serde(default = "default_backtest_trades_per_day")]
    pub trades_per_day: f64,
}

fn default_backtest_fee_bps() -> f64 {
    4.0
}

fn default_backtest_slippage_bps() -> f64 {
    2.0
}
fn default_backtest_market_impact_bps() -> f64 {
    1.0
}
fn default_backtest_trading_days_per_year() -> f64 {
    365.0
}
fn default_backtest_trades_per_day() -> f64 {
    12.0
}

/// "Trade for Life" survival mechanics. Defaults are calibrated for
/// capital preservation: low risk per trade, hard equity floor, and
/// aggressive cooldowns. The bot's job is to **stay alive**.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SurvivalCfg {
    /// Master switch. Default ON — disable only for unit tests / backtests.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Equity floor as a fraction of `risk.equity_usd`. When current
    /// equity falls below `equity_usd * death_line_pct`, mode flips to
    /// `Dead`: bot auto-flats every position and refuses to open new
    /// ones until the operator manually unfreezes.
    #[serde(default = "default_death_line")]
    pub death_line_pct: f64,
    /// Number of consecutive losses that triggers a 30-minute cooldown.
    #[serde(default = "default_loss_streak_short")]
    pub loss_streak_short: u32,
    #[serde(default = "default_loss_streak_short_minutes")]
    pub loss_streak_short_cooldown_min: u64,
    /// Number of losses *within `loss_streak_long_window_min` minutes*
    /// that triggers a 4-hour cooldown.
    #[serde(default = "default_loss_streak_long")]
    pub loss_streak_long: u32,
    #[serde(default = "default_loss_streak_long_window_min")]
    pub loss_streak_long_window_min: u64,
    #[serde(default = "default_loss_streak_long_cooldown_min")]
    pub loss_streak_long_cooldown_min: u64,
    /// Daily loss count that triggers a 24-hour cooldown.
    #[serde(default = "default_daily_loss_count")]
    pub daily_loss_count: u32,
    /// Auto-flat threshold (% of peak equity) — if drawdown crosses this
    /// inside the rolling window, every open position is closed.
    #[serde(default = "default_auto_flat_pct")]
    pub auto_flat_drawdown_pct: f64,
    /// Refresh cadence for the SurvivalAgent (seconds).
    #[serde(default = "default_survival_refresh")]
    pub refresh_secs: u64,
    /// Equity reconciliation cadence (seconds). Set to 0 to disable.
    #[serde(default = "default_equity_refresh")]
    pub equity_refresh_secs: u64,
    /// Volatility multiplier (vs 24h ATR moving average). Above this
    /// multiplier, sizes halve. Above 2× this multiplier, signals are skipped.
    #[serde(default = "default_vol_spike_mult")]
    pub vol_spike_mult: f64,
    /// News blackout — net news score below this triggers a freeze.
    #[serde(default = "default_news_panic")]
    pub news_panic_threshold: f64,
    /// News blackout — net news score above this triggers half-size mode
    /// (avoid FOMO chasing tops).
    #[serde(default = "default_news_euphoria")]
    pub news_euphoria_threshold: f64,
    /// Daily PnL ratchet — once today's gain reaches this %, lock half
    /// the gain (bot stops trading until ratchet eases).
    #[serde(default = "default_daily_ratchet_pct")]
    pub daily_pnl_ratchet_pct: f64,
}

fn default_true() -> bool {
    true
}
fn default_death_line() -> f64 {
    0.70
}
fn default_loss_streak_short() -> u32 {
    3
}
fn default_loss_streak_short_minutes() -> u64 {
    30
}
fn default_loss_streak_long() -> u32 {
    5
}
fn default_loss_streak_long_window_min() -> u64 {
    60
}
fn default_loss_streak_long_cooldown_min() -> u64 {
    240
}
fn default_daily_loss_count() -> u32 {
    10
}
fn default_auto_flat_pct() -> f64 {
    8.0
}
fn default_survival_refresh() -> u64 {
    15
}
fn default_equity_refresh() -> u64 {
    60
}
fn default_vol_spike_mult() -> f64 {
    2.0
}
fn default_news_panic() -> f64 {
    -0.6
}
fn default_news_euphoria() -> f64 {
    0.8
}
fn default_daily_ratchet_pct() -> f64 {
    2.0
}

impl Default for SurvivalCfg {
    fn default() -> Self {
        Self {
            enabled: true,
            death_line_pct: default_death_line(),
            loss_streak_short: default_loss_streak_short(),
            loss_streak_short_cooldown_min: default_loss_streak_short_minutes(),
            loss_streak_long: default_loss_streak_long(),
            loss_streak_long_window_min: default_loss_streak_long_window_min(),
            loss_streak_long_cooldown_min: default_loss_streak_long_cooldown_min(),
            daily_loss_count: default_daily_loss_count(),
            auto_flat_drawdown_pct: default_auto_flat_pct(),
            refresh_secs: default_survival_refresh(),
            equity_refresh_secs: default_equity_refresh(),
            vol_spike_mult: default_vol_spike_mult(),
            news_panic_threshold: default_news_panic(),
            news_euphoria_threshold: default_news_euphoria(),
            daily_pnl_ratchet_pct: default_daily_ratchet_pct(),
        }
    }
}

/// External-control surface (Telegram bot, CLI). Disabled by default —
/// the bot must be safe to run unattended without exposing a remote
/// command interface.
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct ControlCfg {
    /// Master switch for the Telegram command panel.
    #[serde(default)]
    pub telegram_commands_enabled: bool,
    /// Comma-separated list of Telegram user IDs allowed to issue
    /// commands. Empty = lock down to chat owner only.
    #[serde(default)]
    pub allowed_user_ids: Vec<i64>,
    /// Poll interval for Telegram getUpdates (long-poll); seconds.
    #[serde(default = "default_telegram_poll")]
    pub poll_secs: u64,
}

fn default_telegram_poll() -> u64 {
    3
}

impl Config {
    /// Load default + optional overlay TOML, then apply environment variable
    /// overrides for secrets.
    pub fn load(default_path: &Path, overlay_path: Option<&Path>) -> Result<Self> {
        let default_str = fs::read_to_string(default_path)
            .map_err(|e| ScalperError::Config(format!("read {default_path:?}: {e}")))?;
        let mut value: toml::Value = toml::from_str(&default_str)
            .map_err(|e| ScalperError::Config(format!("parse default toml: {e}")))?;

        if let Some(overlay) = overlay_path {
            if overlay.exists() {
                let overlay_str = fs::read_to_string(overlay)
                    .map_err(|e| ScalperError::Config(format!("read {overlay:?}: {e}")))?;
                let overlay_val: toml::Value = toml::from_str(&overlay_str)
                    .map_err(|e| ScalperError::Config(format!("parse overlay toml: {e}")))?;
                merge_toml(&mut value, overlay_val);
            }
        }

        let mut cfg: Config = value
            .try_into()
            .map_err(|e| ScalperError::Config(format!("deserialize: {e}")))?;

        cfg.apply_env();
        cfg.validate()?;
        Ok(cfg)
    }

    fn apply_env(&mut self) {
        if let Ok(v) = std::env::var("BINANCE_API_KEY") {
            self.exchange.api_key = v;
        }
        if let Ok(v) = std::env::var("BINANCE_API_SECRET") {
            self.exchange.api_secret = v;
        }
        // Brain LLM provider/model/api_base overrides — applied BEFORE the
        // api-key lookup so the right `*_API_KEY` env var is picked.
        if let Ok(v) = std::env::var("ARIA_LLM_PROVIDER") {
            if !v.is_empty() {
                self.llm.provider = v;
            }
        }
        if let Ok(v) = std::env::var("ARIA_LLM_MODEL") {
            if !v.is_empty() {
                self.llm.model = v;
            }
        }
        if let Ok(v) = std::env::var("ARIA_LLM_API_BASE") {
            if !v.is_empty() {
                self.llm.api_base = v;
            }
        }
        // Manager LLM enabled toggle. Accepts truthy ("1", "true", "yes",
        // "on") and falsy ("0", "false", "no", "off"); anything else is
        // ignored.
        if let Ok(v) = std::env::var("ARIA_MANAGER_ENABLED") {
            match v.trim().to_ascii_lowercase().as_str() {
                "1" | "true" | "yes" | "on" => self.manager.enabled = true,
                "0" | "false" | "no" | "off" => self.manager.enabled = false,
                _ => {}
            }
        }
        // Manager LLM provider/model/api_base overrides.
        if let Ok(v) = std::env::var("ARIA_MANAGER_PROVIDER") {
            if !v.is_empty() {
                self.manager.provider = v;
            }
        }
        if let Ok(v) = std::env::var("ARIA_MANAGER_MODEL") {
            if !v.is_empty() {
                self.manager.model = v;
            }
        }
        if let Ok(v) = std::env::var("ARIA_MANAGER_API_BASE") {
            if !v.is_empty() {
                self.manager.api_base = v;
            }
        }
        // LLM key — checked in priority order. The first non-empty match wins,
        // so a user can have multiple keys exported simultaneously and the
        // active provider just picks its own.
        let llm_env_var = match self.llm.provider.to_ascii_lowercase().as_str() {
            "anthropic" | "claude" => "ANTHROPIC_API_KEY",
            "openai" => "OPENAI_API_KEY",
            "together" => "TOGETHER_API_KEY",
            "groq" => "GROQ_API_KEY",
            // openrouter, custom, etc.
            _ => "OPENROUTER_API_KEY",
        };
        if let Ok(v) = std::env::var(llm_env_var) {
            if !v.is_empty() {
                self.llm.api_key = v;
            }
        }
        // Fallbacks for users who export a generic LLM key.
        if self.llm.api_key.is_empty() {
            for k in [
                "OPENROUTER_API_KEY",
                "ANTHROPIC_API_KEY",
                "OPENAI_API_KEY",
                "LLM_API_KEY",
            ] {
                if let Ok(v) = std::env::var(k) {
                    if !v.is_empty() {
                        self.llm.api_key = v;
                        break;
                    }
                }
            }
        }
        // Manager LLM key (`MANAGER_API_KEY`) with fallback to the
        // brain LLM key — usually you want the same provider for both.
        if let Ok(v) = std::env::var("MANAGER_API_KEY") {
            if !v.is_empty() {
                self.manager.api_key = v;
            }
        }
        if self.manager.api_key.is_empty() && !self.llm.api_key.is_empty() {
            self.manager.api_key = self.llm.api_key.clone();
        }
        if let Ok(v) = std::env::var("CRYPTOPANIC_API_KEY") {
            self.feeds.cryptopanic_api_key = v;
        }
        if let Ok(v) = std::env::var("LUNARCRUSH_API_KEY") {
            self.feeds.lunarcrush_api_key = v;
        }
        if let Ok(v) = std::env::var("GLASSNODE_API_KEY") {
            self.feeds.glassnode_api_key = v;
        }
        if let Ok(v) = std::env::var("WHALE_ALERT_API_KEY") {
            self.feeds.whalealert_api_key = v;
        }
        if let Ok(v) = std::env::var("TELEGRAM_BOT_TOKEN") {
            self.monitoring.telegram_bot_token = v;
        }
        if let Ok(v) = std::env::var("TELEGRAM_CHAT_ID") {
            self.monitoring.telegram_chat_id = v;
        }
    }

    fn validate(&self) -> Result<()> {
        if !["paper", "live", "backtest"].contains(&self.mode.run_mode.as_str()) {
            return Err(ScalperError::Config(format!(
                "invalid run_mode `{}`",
                self.mode.run_mode
            )));
        }
        if self.pairs.symbols.is_empty() {
            return Err(ScalperError::Config("pairs.symbols is empty".into()));
        }
        if self.risk.risk_per_trade_pct <= 0.0 || self.risk.risk_per_trade_pct > 5.0 {
            return Err(ScalperError::Config(
                "risk.risk_per_trade_pct must be in (0, 5]".into(),
            ));
        }
        if self.risk.min_reward_risk <= 0.0 {
            return Err(ScalperError::Config(
                "risk.min_reward_risk must be positive".into(),
            ));
        }
        if self.risk.max_position_notional_pct <= 0.0
            || self.risk.max_position_notional_pct > self.risk.max_leverage as f64 * 100.0
        {
            return Err(ScalperError::Config(
                "risk.max_position_notional_pct must be positive and within leverage cap".into(),
            ));
        }
        if self.risk.min_net_edge_bps < 0.0 {
            return Err(ScalperError::Config(
                "risk.min_net_edge_bps must be non-negative".into(),
            ));
        }
        if self.risk.assumed_daily_volume_usd <= 0.0 {
            return Err(ScalperError::Config(
                "risk.assumed_daily_volume_usd must be positive".into(),
            ));
        }
        if self.backtest.trading_days_per_year <= 0.0 || self.backtest.trades_per_day <= 0.0 {
            return Err(ScalperError::Config(
                "backtest annualization settings must be positive".into(),
            ));
        }
        if self.mode.run_mode == "live"
            && !self.mode.dry_run
            && (self.exchange.api_key.is_empty() || self.exchange.api_secret.is_empty())
        {
            return Err(ScalperError::Config(
                "live mode requires BINANCE_API_KEY / BINANCE_API_SECRET".into(),
            ));
        }
        Ok(())
    }
}

/// Recursive merge of TOML tables — `overlay` wins.
fn merge_toml(base: &mut toml::Value, overlay: toml::Value) {
    match (base, overlay) {
        (toml::Value::Table(b), toml::Value::Table(o)) => {
            for (k, v) in o {
                merge_toml(b.entry(k).or_insert(toml::Value::Boolean(false)), v);
            }
        }
        (b, o) => {
            *b = o;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn default_config_loads() {
        let p = std::path::PathBuf::from("config/default.toml");
        let cfg = Config::load(&p, None).expect("default config must parse");
        assert!(!cfg.pairs.symbols.is_empty());
        assert_eq!(cfg.mode.run_mode, "paper");
    }

    #[test]
    fn overlay_overrides_base() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().join("base.toml");
        let overlay = tmp.path().join("overlay.toml");
        let mut f = std::fs::File::create(&base).unwrap();
        write!(
            f,
            r#"
[mode]
run_mode = "paper"
dry_run = true

[exchange]
name = "binance"
market = "futures"
rest_base_url = ""
ws_base_url = ""
recv_window_ms = 5000

[pairs]
symbols = ["BTCUSDT"]
timeframes = ["5m"]

[strategy]
mode = "adaptive"
active = ["mean_reversion"]
min_ta_confidence = 65

[llm]
provider = "anthropic"
model = "haiku"
api_base = "https://api.anthropic.com/v1/messages"
timeout_secs = 5
min_confidence = 70
fallback_ta_threshold = 75
max_tokens = 1024

[risk]
risk_per_trade_pct = 0.8
max_open_positions = 3
max_daily_loss_pct = 3.0
max_drawdown_pct = 10.0
max_leverage = 5
max_spread_pct = 0.03
min_reward_risk = 1.2
max_position_notional_pct = 35.0
min_net_edge_bps = 1.0
assumed_daily_volume_usd = 1000000000.0
equity_usd = 5000.0

[schedule]
dead_zone_start_hour_wib = 3
dead_zone_end_hour_wib = 7

[feeds]

[monitoring]
log_level = "info"
db_path = "trades.db"
metrics_bind = "127.0.0.1:0"

[backtest]
data_dir = "data"
"#
        )
        .unwrap();

        let mut of = std::fs::File::create(&overlay).unwrap();
        write!(
            of,
            r#"
[risk]
risk_per_trade_pct = 0.5
equity_usd = 1000.0
"#
        )
        .unwrap();

        let cfg = Config::load(&base, Some(&overlay)).unwrap();
        approx::assert_abs_diff_eq!(cfg.risk.risk_per_trade_pct, 0.5);
        approx::assert_abs_diff_eq!(cfg.risk.equity_usd, 1000.0);
    }

    #[test]
    fn env_overrides_llm_model_and_provider() {
        // Run serially-friendly: clear afterwards even on panic.
        struct EnvGuard(&'static [&'static str]);
        impl Drop for EnvGuard {
            fn drop(&mut self) {
                for k in self.0 {
                    std::env::remove_var(k);
                }
            }
        }
        let _guard = EnvGuard(&[
            "ARIA_LLM_PROVIDER",
            "ARIA_LLM_MODEL",
            "ARIA_LLM_API_BASE",
            "ARIA_MANAGER_MODEL",
            "ARIA_MANAGER_ENABLED",
        ]);
        std::env::set_var("ARIA_LLM_PROVIDER", "openrouter");
        std::env::set_var("ARIA_LLM_MODEL", "deepseek/deepseek-chat");
        std::env::set_var(
            "ARIA_LLM_API_BASE",
            "https://api.deepseek.com/v1/chat/completions",
        );
        std::env::set_var("ARIA_MANAGER_MODEL", "anthropic/claude-3.5-sonnet");
        std::env::set_var("ARIA_MANAGER_ENABLED", "true");

        let p = std::path::PathBuf::from("config/default.toml");
        let cfg = Config::load(&p, None).expect("default config must parse");

        assert_eq!(cfg.llm.provider, "openrouter");
        assert_eq!(cfg.llm.model, "deepseek/deepseek-chat");
        assert_eq!(
            cfg.llm.api_base,
            "https://api.deepseek.com/v1/chat/completions"
        );
        assert_eq!(cfg.manager.model, "anthropic/claude-3.5-sonnet");
        assert!(cfg.manager.enabled, "ARIA_MANAGER_ENABLED=true must flip");
    }
}
