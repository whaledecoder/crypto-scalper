//! Bootstrap historical candles from Binance REST API on startup.
//!
//! Fetches the last N closed klines per symbol and feeds them into
//! SymbolState so that all indicators (especially EMA200) are warm
//! before the live WebSocket stream begins.

use crate::data::types::{Candle, Timeframe};
use crate::strategy::state::SymbolState;
use anyhow::{Context, Result};
use chrono::{TimeZone, Utc};
use reqwest::Client;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{info, warn};

/// How many candles to fetch. 220 covers EMA200 seed (200) + a few extra
/// for ADX/Choppiness/Keltner multi-pass warm-up.
const BOOTSTRAP_LIMIT: u32 = 220;

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct RawKline(
    i64,    // 0  open_time ms
    String, // 1  open
    String, // 2  high
    String, // 3  low
    String, // 4  close
    String, // 5  volume
    i64,    // 6  close_time ms
    String, // 7  quote asset volume (ignored)
    u32,    // 8  number of trades (ignored)
    String, // 9  taker buy base (ignored)
    String, // 10 taker buy quote (ignored)
    String, // 11 ignore
);

impl RawKline {
    fn into_candle(self) -> Result<Candle> {
        let open_time = Utc
            .timestamp_millis_opt(self.0)
            .single()
            .context("bad open_time")?;
        let close_time = Utc
            .timestamp_millis_opt(self.6)
            .single()
            .context("bad close_time")?;
        Ok(Candle {
            open_time,
            close_time,
            open: self.1.parse().context("open")?,
            high: self.2.parse().context("high")?,
            low: self.3.parse().context("low")?,
            close: self.4.parse().context("close")?,
            volume: self.5.parse().context("volume")?,
        })
    }
}

/// Fetch historical klines for one symbol from Binance REST.
///
/// `rest_base_url` — e.g. `https://fapi.binance.com` (futures) or
///                       `https://api.binance.com` (spot)
pub async fn fetch_klines(
    client: &Client,
    rest_base_url: &str,
    symbol: &str,
    timeframe: &Timeframe,
    limit: u32,
) -> Result<Vec<Candle>> {
    // Futures uses /fapi/v1/klines, spot uses /api/v3/klines
    let path = if rest_base_url.contains("fapi") || rest_base_url.contains("dapi") {
        "/fapi/v1/klines"
    } else {
        "/api/v3/klines"
    };

    let url = format!("{}{}", rest_base_url.trim_end_matches('/'), path);

    let raw: Vec<RawKline> =
        fetch_klines_from(client, rest_base_url, path, symbol, timeframe, limit)
            .await
            .with_context(|| format!("primary {url}"))?;

    candles_from_raw(raw)
}

async fn fetch_klines_from(
    client: &Client,
    rest_base_url: &str,
    path: &str,
    symbol: &str,
    timeframe: &Timeframe,
    limit: u32,
) -> Result<Vec<RawKline>> {
    let url = format!("{}{}", rest_base_url.trim_end_matches('/'), path);
    let interval = timeframe.as_str();

    client
        .get(&url)
        .query(&[
            ("symbol", symbol),
            ("interval", &interval),
            ("limit", &limit.to_string()),
        ])
        .send()
        .await
        .context("kline request failed")?
        .error_for_status()
        .context("kline http error")?
        .json()
        .await
        .context("kline json parse")
}

fn candles_from_raw(raw: Vec<RawKline>) -> Result<Vec<Candle>> {
    let mut candles = Vec::with_capacity(raw.len());
    for r in raw {
        match r.into_candle() {
            Ok(c) => candles.push(c),
            Err(e) => warn!("skipping malformed kline: {e}"),
        }
    }

    // Drop the last (currently-forming) candle — it isn't closed yet
    if candles
        .last()
        .map(|c| c.close_time > Utc::now())
        .unwrap_or(false)
    {
        candles.pop();
    }

    Ok(candles)
}

async fn fetch_bootstrap_klines(
    client: &Client,
    rest_base_url: &str,
    symbol: &str,
    timeframe: &Timeframe,
    limit: u32,
) -> Result<Vec<Candle>> {
    match fetch_klines(client, rest_base_url, symbol, timeframe, limit).await {
        Ok(candles) => return Ok(candles),
        Err(primary) => {
            warn!(symbol = %symbol, error = %primary, "primary kline bootstrap failed");
        }
    }

    for (base, path) in [
        ("https://data-api.binance.vision", "/api/v3/klines"),
        ("https://testnet.binancefuture.com", "/fapi/v1/klines"),
    ] {
        match fetch_klines_from(client, base, path, symbol, timeframe, limit)
            .await
            .and_then(candles_from_raw)
        {
            Ok(candles) => {
                info!(symbol = %symbol, base = %base, "kline bootstrap fallback succeeded");
                return Ok(candles);
            }
            Err(e) => {
                warn!(symbol = %symbol, base = %base, error = %e, "kline bootstrap fallback failed");
            }
        }
    }

    anyhow::bail!("all kline bootstrap sources failed")
}

/// Pre-seed all SymbolState indicators from historical klines.
///
/// Called once at startup before the WebSocket agent is spawned.
/// After this returns, all indicators that need ≤ 220 candles are warm.
pub async fn bootstrap_states(
    states: &Arc<Mutex<HashMap<String, SymbolState>>>,
    rest_base_url: &str,
    timeframe: &Timeframe,
) {
    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .expect("http client");

    let symbols: Vec<String> = states.lock().await.keys().cloned().collect();

    for symbol in &symbols {
        info!(symbol = %symbol, "bootstrap start");

        match fetch_bootstrap_klines(&client, rest_base_url, symbol, timeframe, BOOTSTRAP_LIMIT)
            .await
        {
            Ok(candles) if candles.is_empty() => {
                warn!(symbol = %symbol, "bootstrap returned 0 candles — indicators will warm up live");
            }
            Ok(candles) => {
                let n = candles.len();
                let mut states = states.lock().await;
                if let Some(state) = states.get_mut(symbol) {
                    for c in candles {
                        state.on_closed(c);
                    }
                    info!(symbol = %symbol, seeded = n, "bootstrap ok");
                }
            }
            Err(e) => {
                warn!(symbol = %symbol, error = %e, "bootstrap failed");
            }
        }
    }
}
