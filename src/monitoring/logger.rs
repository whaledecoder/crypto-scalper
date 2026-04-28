//! SQLite-backed trade journal.

use crate::errors::Result;
use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;

pub const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS trades (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    client_order_id TEXT UNIQUE NOT NULL,
    symbol TEXT NOT NULL,
    direction TEXT NOT NULL,
    strategy TEXT NOT NULL,
    market_regime TEXT NOT NULL,
    entry_time DATETIME NOT NULL,
    entry_price REAL NOT NULL,
    size REAL NOT NULL,
    stop_loss REAL NOT NULL,
    take_profit REAL NOT NULL,
    exit_time DATETIME,
    exit_price REAL,
    exit_reason TEXT,
    pnl_usd REAL,
    pnl_pct REAL,
    fees_paid REAL,
    ta_confidence INTEGER,
    rsi REAL,
    adx REAL,
    vwap_delta_pct REAL,
    ema_alignment TEXT,
    llm_model TEXT,
    llm_decision TEXT,
    llm_confidence INTEGER,
    llm_ta_score INTEGER,
    llm_sentiment_score INTEGER,
    llm_fundamental_score INTEGER,
    llm_composite INTEGER,
    llm_summary TEXT,
    llm_ta_analysis TEXT,
    llm_sentiment TEXT,
    llm_fundamental TEXT,
    llm_risks TEXT,
    llm_invalidation TEXT,
    llm_latency_ms INTEGER,
    fear_greed INTEGER,
    social_sentiment REAL,
    news_score REAL,
    funding_rate REAL,
    exchange_flow_btc REAL,
    top_news_titles TEXT,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_trades_symbol     ON trades(symbol);
CREATE INDEX IF NOT EXISTS idx_trades_entry_time ON trades(entry_time);
CREATE INDEX IF NOT EXISTS idx_trades_strategy   ON trades(strategy);
CREATE INDEX IF NOT EXISTS idx_trades_llm_dec    ON trades(llm_decision);

CREATE TABLE IF NOT EXISTS llm_decisions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    ts DATETIME NOT NULL,
    symbol TEXT NOT NULL,
    strategy TEXT NOT NULL,
    regime TEXT NOT NULL,
    direction TEXT NOT NULL,
    ta_confidence INTEGER,
    llm_decision TEXT,
    llm_confidence INTEGER,
    composite_score INTEGER,
    summary TEXT,
    raw_json TEXT,
    latency_ms INTEGER,
    offline_fallback INTEGER DEFAULT 0
);
"#;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TradeRecord {
    pub client_order_id: String,
    pub symbol: String,
    pub direction: String,
    pub strategy: String,
    pub market_regime: String,
    pub entry_time: DateTime<Utc>,
    pub entry_price: f64,
    pub size: f64,
    pub stop_loss: f64,
    pub take_profit: f64,
    pub exit_time: Option<DateTime<Utc>>,
    pub exit_price: Option<f64>,
    pub exit_reason: Option<String>,
    pub pnl_usd: Option<f64>,
    pub pnl_pct: Option<f64>,
    pub fees_paid: Option<f64>,

    pub ta_confidence: Option<u8>,
    pub rsi: Option<f64>,
    pub adx: Option<f64>,
    pub vwap_delta_pct: Option<f64>,
    pub ema_alignment: Option<String>,

    pub llm_model: Option<String>,
    pub llm_decision: Option<String>,
    pub llm_confidence: Option<u8>,
    pub llm_ta_score: Option<u8>,
    pub llm_sentiment_score: Option<u8>,
    pub llm_fundamental_score: Option<u8>,
    pub llm_composite: Option<u8>,
    pub llm_summary: Option<String>,
    pub llm_ta_analysis: Option<String>,
    pub llm_sentiment: Option<String>,
    pub llm_fundamental: Option<String>,
    pub llm_risks: Option<String>,
    pub llm_invalidation: Option<String>,
    pub llm_latency_ms: Option<u64>,

    pub fear_greed: Option<u8>,
    pub social_sentiment: Option<f64>,
    pub news_score: Option<f64>,
    pub funding_rate: Option<f64>,
    pub top_news_titles: Option<String>,
}

pub struct TradeJournal {
    conn: Arc<Mutex<Connection>>,
}

impl TradeJournal {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    pub fn open_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    pub fn insert_trade(&self, t: &TradeRecord) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO trades (
                client_order_id, symbol, direction, strategy, market_regime,
                entry_time, entry_price, size, stop_loss, take_profit,
                exit_time, exit_price, exit_reason, pnl_usd, pnl_pct, fees_paid,
                ta_confidence, rsi, adx, vwap_delta_pct, ema_alignment,
                llm_model, llm_decision, llm_confidence,
                llm_ta_score, llm_sentiment_score, llm_fundamental_score,
                llm_composite, llm_summary, llm_ta_analysis, llm_sentiment,
                llm_fundamental, llm_risks, llm_invalidation, llm_latency_ms,
                fear_greed, social_sentiment, news_score, funding_rate, top_news_titles
            ) VALUES (
                ?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,
                ?19,?20,?21,?22,?23,?24,?25,?26,?27,?28,?29,?30,?31,?32,?33,?34,?35,
                ?36,?37,?38,?39,?40
            )",
            params![
                t.client_order_id,
                t.symbol,
                t.direction,
                t.strategy,
                t.market_regime,
                t.entry_time,
                t.entry_price,
                t.size,
                t.stop_loss,
                t.take_profit,
                t.exit_time,
                t.exit_price,
                t.exit_reason,
                t.pnl_usd,
                t.pnl_pct,
                t.fees_paid,
                t.ta_confidence,
                t.rsi,
                t.adx,
                t.vwap_delta_pct,
                t.ema_alignment,
                t.llm_model,
                t.llm_decision,
                t.llm_confidence,
                t.llm_ta_score,
                t.llm_sentiment_score,
                t.llm_fundamental_score,
                t.llm_composite,
                t.llm_summary,
                t.llm_ta_analysis,
                t.llm_sentiment,
                t.llm_fundamental,
                t.llm_risks,
                t.llm_invalidation,
                t.llm_latency_ms.map(|x| x as i64),
                t.fear_greed,
                t.social_sentiment,
                t.news_score,
                t.funding_rate,
                t.top_news_titles,
            ],
        )?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn close_trade(
        &self,
        client_id: &str,
        exit_time: DateTime<Utc>,
        exit_price: f64,
        exit_reason: &str,
        pnl_usd: f64,
        pnl_pct: f64,
        fees: f64,
    ) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            "UPDATE trades SET exit_time=?2, exit_price=?3, exit_reason=?4,
                pnl_usd=?5, pnl_pct=?6, fees_paid=?7
             WHERE client_order_id=?1",
            params![
                client_id,
                exit_time,
                exit_price,
                exit_reason,
                pnl_usd,
                pnl_pct,
                fees
            ],
        )?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn log_llm_decision(
        &self,
        symbol: &str,
        strategy: &str,
        regime: &str,
        direction: &str,
        ta_confidence: u8,
        llm_decision: &str,
        llm_confidence: u8,
        composite_score: u8,
        summary: &str,
        raw_json: &str,
        latency_ms: u64,
        offline_fallback: bool,
    ) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO llm_decisions (
                ts, symbol, strategy, regime, direction, ta_confidence,
                llm_decision, llm_confidence, composite_score, summary,
                raw_json, latency_ms, offline_fallback
            ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
            params![
                Utc::now(),
                symbol,
                strategy,
                regime,
                direction,
                ta_confidence,
                llm_decision,
                llm_confidence,
                composite_score,
                summary,
                raw_json,
                latency_ms as i64,
                offline_fallback as i64,
            ],
        )?;
        Ok(())
    }

    pub fn recent_pnl(&self) -> Result<f64> {
        let conn = self.conn.lock();
        let v: Option<f64> = conn.query_row(
            "SELECT SUM(pnl_usd) FROM trades WHERE exit_time IS NOT NULL AND date(exit_time) = date('now')",
            [],
            |r| r.get(0),
        ).unwrap_or(None);
        Ok(v.unwrap_or(0.0))
    }

    pub fn trade_count(&self) -> Result<i64> {
        let conn = self.conn.lock();
        let v: i64 = conn.query_row("SELECT COUNT(*) FROM trades", [], |r| r.get(0))?;
        Ok(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_and_insert() {
        let j = TradeJournal::open_memory().unwrap();
        let t = TradeRecord {
            client_order_id: "abc".into(),
            symbol: "BTCUSDT".into(),
            direction: "LONG".into(),
            strategy: "ema_ribbon".into(),
            market_regime: "TRENDING_BULLISH".into(),
            entry_time: Utc::now(),
            entry_price: 67240.0,
            size: 0.01,
            stop_loss: 66980.0,
            take_profit: 67510.0,
            exit_time: None,
            exit_price: None,
            exit_reason: None,
            pnl_usd: None,
            pnl_pct: None,
            fees_paid: None,
            ta_confidence: Some(74),
            rsi: Some(61.4),
            adx: Some(28.4),
            vwap_delta_pct: Some(0.42),
            ema_alignment: Some("bull".into()),
            llm_model: Some("claude-3-5-haiku".into()),
            llm_decision: Some("GO".into()),
            llm_confidence: Some(78),
            llm_ta_score: Some(74),
            llm_sentiment_score: Some(72),
            llm_fundamental_score: Some(80),
            llm_composite: Some(74),
            llm_summary: Some("summary".into()),
            llm_ta_analysis: None,
            llm_sentiment: None,
            llm_fundamental: None,
            llm_risks: None,
            llm_invalidation: None,
            llm_latency_ms: Some(820),
            fear_greed: Some(71),
            social_sentiment: Some(0.68),
            news_score: Some(0.72),
            funding_rate: Some(0.0082),
            top_news_titles: Some(r#"["ETF inflow"]"#.into()),
        };
        j.insert_trade(&t).unwrap();
        assert_eq!(j.trade_count().unwrap(), 1);
    }
}
