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
    pub llm: LlmCfg,
    pub risk: RiskCfg,
    pub schedule: Schedule,
    pub feeds: Feeds,
    pub monitoring: Monitoring,
    pub backtest: Backtest,
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
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RiskCfg {
    pub risk_per_trade_pct: f64,
    pub max_open_positions: u32,
    pub max_daily_loss_pct: f64,
    pub max_drawdown_pct: f64,
    pub max_leverage: u32,
    pub max_spread_pct: f64,
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
    #[serde(default)]
    pub rss_feeds: Vec<String>,
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
        if let Ok(v) = std::env::var("ANTHROPIC_API_KEY") {
            self.llm.api_key = v;
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
        if self.mode.run_mode == "live" && !self.mode.dry_run
            && (self.exchange.api_key.is_empty() || self.exchange.api_secret.is_empty()) {
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
}
